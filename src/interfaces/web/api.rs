//! The UI server's routes. Every API call runs one sessionsnap command through `cli::run_json`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use super::http::{Request, Response};
use crate::interfaces::cli::{Cli, Ctx, run_json};
use crate::ports::Shown;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

/// Commands the page may run (not `ui` nor the installers).
pub const ALLOWED: &[&str] = &[
    "list", "open", "show", "history", "diff", "save", "ws", "tab", "edit", "restore", "apply", "sessions", "recent",
    "delete", "prune", "agents",
];

/// Builds the context for one API call; the `Option<String>` is the call's `input`.
/// `Sync`: the server's workers share it.
pub type MakeCtx<'a> = dyn Fn(&Cli, Option<String>) -> Result<Ctx> + Send + Sync + 'a;

/// Shows a URL in cmux; returns where ("reused", "dock" or "tab") and whether its key got there.
pub type ShowFn<'a> = dyn Fn(&str) -> Result<Shown> + Send + Sync + 'a;

pub struct Server<'a> {
    pub port: u16,
    /// Random per start; the page gets it from the link fragment `#t=…` and sends it as X-Sessionsnap-Token.
    pub token: String,
    /// Called by `POST /api/show` with the keyed link; `sessionsnap ui` passes the cmux backend's.
    pub show: Box<ShowFn<'a>>,
    shows: ShowGate,
    /// Commands run one at a time, as before the server had workers: the store has no lock.
    runs: Mutex<()>,
}

/// Shortest time between the starts of two shows.
pub const SHOW_GAP: Duration = Duration::from_secs(1);

/// One show at a time, at most one per `SHOW_GAP`: each runs cmux, and `/api/show` needs no key.
#[derive(Default)]
struct ShowGate {
    /// Whether a show runs, and when the last one started.
    state: Mutex<(bool, Option<Instant>)>,
}

impl ShowGate {
    /// `None` when a show runs or one started too recently.
    fn enter(&self) -> Option<ShowTurn<'_>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.0 || state.1.is_some_and(|t| t.elapsed() < SHOW_GAP) {
            return None;
        }
        *state = (true, Some(Instant::now()));
        Some(ShowTurn(self))
    }
}

/// Ends the show when dropped.
struct ShowTurn<'a>(&'a ShowGate);

impl Drop for ShowTurn<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).0 = false;
    }
}

#[derive(Deserialize)]
struct RunBody {
    args: Vec<String>,
    #[serde(default)]
    input: Option<String>,
}

impl<'a> Server<'a> {
    pub fn new(port: u16, token: String, show: Box<ShowFn<'a>>) -> Self {
        Server { port, token, show, shows: ShowGate::default(), runs: Mutex::new(()) }
    }

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
                Response::json(200, &json!({"app": "sessionsnap", "version": env!("CARGO_PKG_VERSION")}))
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
    /// one (or a second `sessionsnap ui`) gets one, and the key only ever goes to cmux, never on a
    /// command line.
    fn show(&self, req: &Request) -> Response {
        if self.foreign_origin(req) {
            return Response::text(403, "wrong origin");
        }
        if !Self::is_json(req) {
            return Response::text(415, "send JSON");
        }
        self.show_page()
    }

    /// Shows the keyed link in cmux: `200 {"shown": …, "keyed": …}`, `502` with the reason, or
    /// `429` while another show runs or one started under `SHOW_GAP` ago. When cmux showed the
    /// page but could not be handed its key, the keyed link is printed to this server's own
    /// terminal instead, for the user to open.
    pub fn show_page(&self) -> Response {
        let Some(_turn) = self.shows.enter() else {
            return Response::text(429, "The sessionsnap UI is being shown already; try again in a moment.");
        };
        let link = format!("http://127.0.0.1:{}/#t={}", self.port, self.token);
        match (self.show)(&link) {
            Ok(shown) => {
                if let Some(why) = &shown.fragment_error {
                    eprintln!(
                        "sessionsnap: showed the UI in cmux but could not hand it its key ({}).\n\
                         Open this link in cmux's browser, or click Reconnect on the page to try again:\n{link}",
                        why.replace(&self.token, "…")
                    );
                }
                Response::json(200, &json!({"shown": shown.place, "keyed": shown.fragment_error.is_none()}))
            }
            // cmux's message could quote the link; the key stays out of the answer.
            Err(e) => Response::text(502, &format!("could not show the UI in cmux: {e:#}").replace(&self.token, "…")),
        }
    }

    fn run(&self, req: &Request, make_ctx: &MakeCtx<'_>) -> Response {
        if self.foreign_origin(req) {
            return Response::text(403, "wrong origin");
        }
        if !req.header("x-sessionsnap-token").is_some_and(|t| same(t, &self.token)) {
            return Response::json(
                401,
                &json!({"error": {"code": "stale-token", "message": "The sessionsnap UI was restarted, so this page's key is old. Click Reconnect, or run `sessionsnap ui`."}}),
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
        // The page puts a `--` before positionals that start with `-` (a snapshot named `-x`).
        // clap never takes `--` as an option's value, so all after it are positionals; refusing
        // anything long-option shaped there keeps the checks below (and `run_json`'s `--json`)
        // meaning what they say.
        if let Some(at) = body.args.iter().position(|a| a == "--")
            && body.args[at + 1..].iter().any(|a| a.starts_with("--"))
        {
            return Response::text(403, "only values may follow --");
        }
        // Batches come in `input`, never from a file the server would read.
        let file_batch = body.args.windows(2).any(|w| w[0] == "--batch" && w[1] != "-")
            || body.args.iter().any(|a| a.starts_with("--batch="));
        if file_batch {
            return Response::text(403, "send batches as input");
        }
        let input = body.input.unwrap_or_default();
        let _one = self.runs.lock().unwrap_or_else(|e| e.into_inner());
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
    use crate::testing::FakeTerminal;
    use serde_json::json;

    const PORT: u16 = 47317;

    fn server() -> Server<'static> {
        Server::new(PORT, "t0k".into(), Box::new(|_| Ok(Shown::whole("dock"))))
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
            &[("x-sessionsnap-token", "t0k"), ("content-type", "application/json"), ("origin", origin.as_str())],
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
            Ok(crate::testing::ctx(dir.path(), FakeTerminal::default(), cli.json, cli.yes, input))
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
            assert!(!html.contains(r#"sessionsnap-token" content"#));
            assert!(html.contains(r#"src="/app.js""#) && html.contains(r#"href="/app.css""#));
            assert_eq!(s.handle(&req("GET", "/app.js", &[], ""), mk).content_type, "text/javascript; charset=utf-8");
            assert_eq!(s.handle(&req("GET", "/app.css", &[], ""), mk).status, 200);
            assert_eq!(s.handle(&req("GET", "/nope", &[], ""), mk).status, 404);
            assert_eq!(s.handle(&req("DELETE", "/", &[], ""), mk).status, 405);
            assert_eq!(body(&s.handle(&req("GET", "/api/hello", &[], ""), mk))["app"], "sessionsnap");
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
                &[
                    ("x-sessionsnap-token", "t0k"),
                    ("content-type", "application/json"),
                    ("origin", "http://evil.example"),
                ],
                r#"{"args":["list"]}"#,
            );
            assert_eq!(s.handle(&r, mk).status, 403, "wrong origin");
            let r = req(
                "POST",
                "/api/run",
                &[("x-sessionsnap-token", "old"), ("content-type", "application/json")],
                r#"{"args":["list"]}"#,
            );
            let out = s.handle(&r, mk);
            assert_eq!(out.status, 401);
            assert_eq!(body(&out)["error"]["code"], "stale-token");
            let r = req("POST", "/api/run", &[("x-sessionsnap-token", "t0k"), ("content-type", "text/plain")], "{}");
            assert_eq!(s.handle(&r, mk).status, 415);
            for bad in [
                r#"{"args":["ui"]}"#,
                r#"{"args":["install-cmux"]}"#,
                r#"{"args":["update"]}"#,
                r#"{"args":["uninstall","--yes"]}"#,
                r#"{"args":["--yes","delete","x"]}"#,
                r#"{"args":[]}"#,
                r#"{"args":["edit","work","--batch","/etc/hosts"]}"#,
                r#"{"args":["edit","work","--batch=/etc/hosts"]}"#,
                r#"{"args":["restore","work","--","--pick"]}"#,
            ] {
                assert_eq!(s.handle(&api(bad), mk).status, 403, "{bad}");
            }
            assert_eq!(s.handle(&api("not json"), mk).status, 400);
        });
    }

    /// The page sends values that start with `-` as `--opt=value`, and such positionals after `--`.
    #[test]
    fn values_that_start_with_a_dash_get_through() {
        with_store(|mk| {
            let s = server();
            let r = body(&s.handle(&api(r#"{"args":["save","--message=- fixed","--","-x"]}"#), mk));
            assert_eq!(r["exit"], 0, "{r}");
            let r = body(&s.handle(&api(r#"{"args":["history","--","-x"]}"#), mk));
            assert_eq!(r["exit"], 0, "{r}");
            assert_eq!(r["result"]["versions"][0]["message"], "- fixed", "{r}");
            let r = body(&s.handle(&api(r#"{"args":["show","--","-x"]}"#), mk));
            assert_eq!(r["exit"], 0, "{r}");
            let r = body(&s.handle(&api(r#"{"args":["sessions","--search=-foo","--days","365"]}"#), mk));
            assert_eq!(r["exit"], 0, "{r}");
            let r = body(&s.handle(&api(r#"{"args":["edit","--batch","-","--dry-run","--","-x"],"input":"[]"}"#), mk));
            assert_eq!(r["exit"], 0, "{r}");
        });
    }

    #[test]
    fn no_flag_slips_in_after_a_double_dash() {
        with_store(|mk| {
            let s = server();
            s.handle(&api(r#"{"args":["save","x"]}"#), mk);
            for bad in [
                r#"{"args":["delete","--","x","--yes"]}"#,
                r#"{"args":["delete","x","--","--yes"]}"#,
                r#"{"args":["edit","--","x","--batch","/etc/hosts"]}"#,
                r#"{"args":["edit","--","x","--batch=/etc/hosts"]}"#,
                r#"{"args":["list","--","--json"]}"#,
                r#"{"args":["show","--","--","x"]}"#,
            ] {
                assert_eq!(s.handle(&api(bad), mk).status, 403, "{bad}");
            }
            let r = body(&s.handle(&api(r#"{"args":["show","x"]}"#), mk));
            assert_eq!(r["exit"], 0, "still there: {r}");
        });
    }

    #[test]
    fn commands_run_one_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let make = |cli: &Cli, input: Option<String>| -> anyhow::Result<Ctx> {
            most.fetch_max(now.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            now.fetch_sub(1, Ordering::SeqCst);
            Ok(crate::testing::ctx(dir.path(), FakeTerminal::default(), cli.json, cli.yes, input))
        };
        let s = server();
        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| assert_eq!(s.handle(&api(r#"{"args":["list"]}"#), &make).status, 200));
            }
        });
        assert_eq!(most.into_inner(), 1);
    }

    fn show_req(headers: &[(&str, &str)]) -> Request {
        req("POST", "/api/show", headers, "{}")
    }

    #[test]
    fn show_needs_no_token_and_shows_the_keyed_link() {
        with_store(|mk| {
            let seen = std::sync::Mutex::new(vec![]);
            let s = Server::new(
                PORT,
                "t0k".into(),
                Box::new(|u: &str| {
                    seen.lock().unwrap().push(u.to_string());
                    Ok(Shown::whole("reused"))
                }),
            );
            let origin = format!("http://127.0.0.1:{PORT}");
            let r = s.handle(&show_req(&[("content-type", "application/json"), ("origin", origin.as_str())]), mk);
            assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
            assert_eq!(body(&r), json!({"shown": "reused", "keyed": true}));
            assert!(!String::from_utf8_lossy(&r.body).contains("t0k"), "never returns the key");
            assert_eq!(*seen.lock().unwrap(), vec![format!("http://127.0.0.1:{PORT}/#t=t0k")]);
            // Without an Origin (a local program such as `sessionsnap ui`) it works too.
            assert_eq!(server().handle(&show_req(&[("content-type", "application/json")]), mk).status, 200);
            assert_eq!(s.handle(&req("GET", "/api/show", &[], ""), mk).status, 405);
        });
    }

    #[test]
    fn shows_run_one_at_a_time_and_at_most_once_a_gap() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let finish_rx = std::sync::Mutex::new(finish_rx);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let s = Server::new(
            PORT,
            "t0k".into(),
            Box::new(|_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                started_tx.send(()).unwrap();
                let _ = finish_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
                Ok(Shown::whole("dock"))
            }),
        );
        let show = || s.handle(&show_req(&[("content-type", "application/json")]), &|_, _| anyhow::bail!("no run"));
        std::thread::scope(|scope| {
            let first = scope.spawn(show);
            started_rx.recv().unwrap();
            let busy = show();
            assert_eq!(busy.status, 429, "a second show while one runs");
            finish_tx.send(()).unwrap();
            assert_eq!(first.join().unwrap().status, 200);
        });
        finish_tx.send(()).unwrap();
        assert_eq!(show().status, 429, "too soon after the last");
        std::thread::sleep(SHOW_GAP + Duration::from_millis(100));
        finish_tx.send(()).unwrap();
        assert_eq!(show().status, 200, "once the gap has passed");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn show_says_when_the_key_did_not_reach_the_page() {
        with_store(|mk| {
            let s = Server::new(
                PORT,
                "t0k".into(),
                Box::new(|_| Ok(Shown { place: "dock", fragment_error: Some("cmux refused t0k".into()) })),
            );
            let r = s.handle(&show_req(&[("content-type", "application/json")]), mk);
            assert_eq!(r.status, 200);
            assert_eq!(body(&r), json!({"shown": "dock", "keyed": false}));
            assert!(!String::from_utf8_lossy(&r.body).contains("t0k"), "never returns the key");
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
            let s = Server::new(PORT, "t0k".into(), Box::new(|u| anyhow::bail!("cmux is not running ({u})")));
            let r = s.handle(&show_req(&[("content-type", "application/json")]), mk);
            assert_eq!(r.status, 502);
            let text = String::from_utf8(r.body).unwrap();
            assert!(text.contains("cmux is not running"), "{text}");
            assert!(!text.contains("t0k"));
        });
    }
}
