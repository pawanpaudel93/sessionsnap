//! The UI server's routes. Every API call runs one ccsnap command through `cli::run_json`.

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use super::http::{Request, Response};
use crate::cli::{Cli, Ctx, run_json};

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

/// Commands the page may run (not `ui` nor the installers).
pub const ALLOWED: &[&str] = &[
    "list", "open", "show", "history", "diff", "save", "ws", "tab", "edit", "restore", "apply", "sessions", "recent",
    "delete", "prune",
];

/// Builds the context for one API call; the `Option<String>` is the call's `input`.
pub type MakeCtx<'a> = dyn Fn(&Cli, Option<String>) -> Result<Ctx> + 'a;

/// Shows a URL in cmux; returns where ("reused", "dock" or "tab").
pub type ShowFn<'a> = dyn Fn(&str) -> Result<&'static str> + Send + Sync + 'a;

pub struct Server<'a> {
    pub port: u16,
    /// Random per start; the page gets it from the link fragment `#t=…` and sends it as X-Ccsnap-Token.
    pub token: String,
    /// Called by `POST /api/show` with the keyed link; `ccsnap ui` passes the cmux backend's.
    pub show: Box<ShowFn<'a>>,
}

#[derive(Deserialize)]
struct RunBody {
    args: Vec<String>,
    #[serde(default)]
    input: Option<String>,
}

impl Server<'_> {
    pub fn handle(&self, req: &Request, make_ctx: &MakeCtx<'_>) -> Response {
        // A page on another site that resolves its name to 127.0.0.1 still sends its own Host.
        if !self.own_host(req.header("host")) {
            return Response::text(403, "wrong host");
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => Response { status: 200, content_type: "text/html; charset=utf-8", body: INDEX_HTML.into() },
            ("GET", "/app.js") => {
                Response { status: 200, content_type: "text/javascript; charset=utf-8", body: APP_JS.into() }
            }
            ("GET", "/app.css") => {
                Response { status: 200, content_type: "text/css; charset=utf-8", body: APP_CSS.into() }
            }
            ("GET", "/api/hello") => {
                Response::json(200, &json!({"app": "ccsnap", "version": env!("CARGO_PKG_VERSION")}))
            }
            ("POST", "/api/run") => self.run(req, make_ctx),
            ("POST", "/api/show") => self.show(req),
            (_, "/" | "/app.js" | "/app.css" | "/api/hello" | "/api/run" | "/api/show") => {
                Response::text(405, "method not allowed")
            }
            _ => Response::text(404, "not found"),
        }
    }

    fn own_host(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port))
    }

    /// A present Origin must be this server's own.
    fn foreign_origin(&self, req: &Request) -> bool {
        req.header("origin").is_some_and(|o| !self.own_host(o.strip_prefix("http://")))
    }

    /// JSON forces a CORS preflight, which this server never answers, for any other origin.
    fn is_json(req: &Request) -> bool {
        req.header("content-type").is_some_and(|c| c.starts_with("application/json"))
    }

    /// Shows this server's page, with its key, in cmux. Needs no key: it is how a page without
    /// one (or a second `ccsnap ui`) gets one, and the key only ever goes to cmux.
    fn show(&self, req: &Request) -> Response {
        if self.foreign_origin(req) {
            return Response::text(403, "wrong origin");
        }
        if !Self::is_json(req) {
            return Response::text(415, "send JSON");
        }
        self.show_page()
    }

    /// Shows the keyed link in cmux: `200 {"shown": …}`, or `502` with the reason.
    pub fn show_page(&self) -> Response {
        match (self.show)(&format!("http://127.0.0.1:{}/#t={}", self.port, self.token)) {
            Ok(shown) => Response::json(200, &json!({"shown": shown})),
            // cmux's message could quote the link; the key stays out of the answer.
            Err(e) => Response::text(502, &format!("could not show the UI in cmux: {e:#}").replace(&self.token, "…")),
        }
    }

    fn run(&self, req: &Request, make_ctx: &MakeCtx<'_>) -> Response {
        if self.foreign_origin(req) {
            return Response::text(403, "wrong origin");
        }
        if !req.header("x-ccsnap-token").is_some_and(|t| same(t, &self.token)) {
            return Response::json(
                401,
                &json!({"error": {"code": "stale-token", "message": "The ccsnap UI was restarted, so this page's key is old. Click Reconnect, or run `ccsnap ui`."}}),
            );
        }
        if !Self::is_json(req) {
            return Response::text(415, "send JSON");
        }
        let body: RunBody = match serde_json::from_slice(&req.body) {
            Ok(b) => b,
            Err(e) => return Response::text(400, &format!("bad body: {e}")),
        };
        if !body.args.first().is_some_and(|c| ALLOWED.contains(&c.as_str())) {
            return Response::text(403, "command not allowed from the UI");
        }
        if body.args.iter().any(|a| a == "--") {
            return Response::text(403, "arguments may not contain --");
        }
        // Batches come in `input`, never from a file the server would read.
        let file_batch = body.args.windows(2).any(|w| w[0] == "--batch" && w[1] != "-")
            || body.args.iter().any(|a| a.starts_with("--batch="));
        if file_batch {
            return Response::text(403, "send batches as input");
        }
        let input = body.input.unwrap_or_default();
        let (exit, result) = run_json(&body.args, |cli| make_ctx(cli, Some(input)));
        Response::json(200, &json!({"exit": exit.code(), "code": exit.name(), "result": result}))
    }
}

/// Compares without stopping at the first different byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::Claude;
    use crate::store::Store;
    use crate::testing::FakeBackend;
    use serde_json::json;

    const PORT: u16 = 47317;

    fn server() -> Server<'static> {
        Server { port: PORT, token: "t0k".into(), show: Box::new(|_| Ok("dock")) }
    }

    fn req(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
        let mut all = vec![("host".to_string(), format!("127.0.0.1:{PORT}"))];
        all.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Request { method: method.into(), path: path.into(), headers: all, body: body.as_bytes().to_vec() }
    }

    fn api(body: &str) -> Request {
        let origin = format!("http://127.0.0.1:{PORT}");
        req(
            "POST",
            "/api/run",
            &[("x-ccsnap-token", "t0k"), ("content-type", "application/json"), ("origin", origin.as_str())],
            body,
        )
    }

    fn body(r: &Response) -> serde_json::Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    /// Runs `f` with contexts on one temporary store and an empty fake cmux.
    fn with_store<T>(f: impl FnOnce(&MakeCtx<'_>) -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let make = |cli: &Cli, input: Option<String>| -> anyhow::Result<Ctx> {
            Ok(Ctx {
                store: Store::new(dir.path().join("store")),
                claude: Claude::new(dir.path().join("claude")),
                backend: Box::new(FakeBackend::default()),
                json: cli.json,
                yes: cli.yes,
                interactive: false,
                input,
            })
        };
        f(&make)
    }

    #[test]
    fn serves_page_without_token_and_assets() {
        with_store(|mk| {
            let s = server();
            let r = s.handle(&req("GET", "/", &[], ""), mk);
            assert_eq!(r.status, 200);
            let html = String::from_utf8(r.body).unwrap();
            assert!(!html.contains("t0k"));
            assert!(!html.contains("{{TOKEN}}"));
            assert!(!html.contains(r#"ccsnap-token" content"#));
            assert!(html.contains(r#"src="/app.js""#) && html.contains(r#"href="/app.css""#));
            assert_eq!(s.handle(&req("GET", "/app.js", &[], ""), mk).content_type, "text/javascript; charset=utf-8");
            assert_eq!(s.handle(&req("GET", "/app.css", &[], ""), mk).status, 200);
            assert_eq!(s.handle(&req("GET", "/nope", &[], ""), mk).status, 404);
            assert_eq!(s.handle(&req("DELETE", "/", &[], ""), mk).status, 405);
            assert_eq!(body(&s.handle(&req("GET", "/api/hello", &[], ""), mk))["app"], "ccsnap");
        });
    }

    #[test]
    fn runs_allowed_commands() {
        with_store(|mk| {
            let s = server();
            let r = s.handle(&api(r#"{"args":["list"]}"#), mk);
            assert_eq!(r.status, 200);
            assert_eq!(body(&r), json!({"exit": 0, "code": "ok", "result": {"snapshots": []}}));
            let r = body(&s.handle(&api(r#"{"args":["show","nope"]}"#), mk));
            assert_eq!(r["exit"], 7);
            assert_eq!(r["result"]["error"]["code"], "not-found");
        });
    }

    #[test]
    fn passes_input_to_batch_edits() {
        with_store(|mk| {
            let s = server();
            s.handle(&api(r#"{"args":["save","work"]}"#), mk);
            let r = body(&s.handle(&api(r#"{"args":["edit","work","--batch","-","--dry-run"],"input":"[]"}"#), mk));
            assert_eq!(r["result"]["dryRun"], true);
            // No input: an empty batch, never the server's stdin.
            let r = body(&s.handle(&api(r#"{"args":["edit","work","--batch","-"]}"#), mk));
            assert_eq!(r["exit"], 1);
        });
    }

    #[test]
    fn refuses_foreign_or_unauthenticated_calls() {
        with_store(|mk| {
            let s = server();
            let mut r = api(r#"{"args":["list"]}"#);
            r.headers[0].1 = "evil.example:47317".into();
            assert_eq!(s.handle(&r, mk).status, 403, "wrong host");
            let r = req(
                "POST",
                "/api/run",
                &[("x-ccsnap-token", "t0k"), ("content-type", "application/json"), ("origin", "http://evil.example")],
                r#"{"args":["list"]}"#,
            );
            assert_eq!(s.handle(&r, mk).status, 403, "wrong origin");
            let r = req(
                "POST",
                "/api/run",
                &[("x-ccsnap-token", "old"), ("content-type", "application/json")],
                r#"{"args":["list"]}"#,
            );
            let out = s.handle(&r, mk);
            assert_eq!(out.status, 401);
            assert_eq!(body(&out)["error"]["code"], "stale-token");
            let r = req("POST", "/api/run", &[("x-ccsnap-token", "t0k"), ("content-type", "text/plain")], "{}");
            assert_eq!(s.handle(&r, mk).status, 415);
            for bad in [
                r#"{"args":["ui"]}"#,
                r#"{"args":["install-cmux"]}"#,
                r#"{"args":["--yes","delete","x"]}"#,
                r#"{"args":[]}"#,
                r#"{"args":["edit","work","--batch","/etc/hosts"]}"#,
                r#"{"args":["edit","work","--batch=/etc/hosts"]}"#,
                r#"{"args":["restore","work","--pick","--","x"]}"#,
            ] {
                assert_eq!(s.handle(&api(bad), mk).status, 403, "{bad}");
            }
            assert_eq!(s.handle(&api("not json"), mk).status, 400);
        });
    }

    fn show_req(headers: &[(&str, &str)]) -> Request {
        req("POST", "/api/show", headers, "{}")
    }

    #[test]
    fn show_needs_no_token_and_shows_the_keyed_link() {
        with_store(|mk| {
            let seen = std::sync::Mutex::new(vec![]);
            let s = Server {
                port: PORT,
                token: "t0k".into(),
                show: Box::new(|u: &str| {
                    seen.lock().unwrap().push(u.to_string());
                    Ok("reused")
                }),
            };
            let origin = format!("http://127.0.0.1:{PORT}");
            let r = s.handle(&show_req(&[("content-type", "application/json"), ("origin", origin.as_str())]), mk);
            assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
            assert_eq!(body(&r), json!({"shown": "reused"}));
            assert!(!String::from_utf8_lossy(&r.body).contains("t0k"), "never returns the key");
            assert_eq!(*seen.lock().unwrap(), vec![format!("http://127.0.0.1:{PORT}/#t=t0k")]);
            // Without an Origin (a local program such as `ccsnap ui`) it works too.
            assert_eq!(s.handle(&show_req(&[("content-type", "application/json")]), mk).status, 200);
            assert_eq!(s.handle(&req("GET", "/api/show", &[], ""), mk).status, 405);
        });
    }

    #[test]
    fn show_refuses_foreign_or_non_json_calls() {
        with_store(|mk| {
            let s = server();
            assert_eq!(s.handle(&show_req(&[("content-type", "text/plain")]), mk).status, 415);
            assert_eq!(s.handle(&show_req(&[]), mk).status, 415);
            let r = show_req(&[("content-type", "application/json"), ("origin", "http://evil.example")]);
            assert_eq!(s.handle(&r, mk).status, 403, "wrong origin");
            let mut r = show_req(&[("content-type", "application/json")]);
            r.headers[0].1 = "evil.example:47317".into();
            assert_eq!(s.handle(&r, mk).status, 403, "wrong host");
        });
    }

    #[test]
    fn show_reports_a_failed_show() {
        with_store(|mk| {
            let s = Server {
                port: PORT,
                token: "t0k".into(),
                show: Box::new(|u| anyhow::bail!("cmux is not running ({u})")),
            };
            let r = s.handle(&show_req(&[("content-type", "application/json")]), mk);
            assert_eq!(r.status, 502);
            let text = String::from_utf8(r.body).unwrap();
            assert!(text.contains("cmux is not running"), "{text}");
            assert!(!text.contains("t0k"));
        });
    }
}
