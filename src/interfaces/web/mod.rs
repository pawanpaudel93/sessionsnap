//! The local web UI: a small HTTP server on 127.0.0.1 that runs sessionsnap commands for its page.
//!
//! Layer rule: an interface. The page speaks the CLI's command protocol: the commands it sends
//! run through `cli::run_json`, exactly as `sessionsnap <args> --json` would.

pub mod api;
pub mod http;

use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::json;

use crate::adapters::terminal::cmux::cli::CmuxCli;
use crate::error::{Exit, failure};
use crate::interfaces::cli::{Ctx, Outcome};
use crate::ports::Terminal;
use api::{MakeCtx, Server};

/// Fixed so the Dock tab's URL stays valid across restarts.
pub const DEFAULT_PORT: u16 = 47317;

/// Answers requests one at a time until none has come for `idle`. Showing the page runs on its own
/// thread: cmux waits for the page to load, and loading it needs this loop. With `show_at_start`,
/// shows the page once the loop is up.
pub fn serve(
    listener: TcpListener,
    server: &Server<'_>,
    make_ctx: &MakeCtx<'_>,
    idle: Duration,
    show_at_start: bool,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    std::thread::scope(|scope| {
        if show_at_start {
            scope.spawn(|| report_shown(&server.show_page()));
        }
        let mut last = Instant::now();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    handle_connection(scope, stream, server, make_ctx);
                    last = Instant::now();
                }
                Err(e) => {
                    match e.kind() {
                        io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                        // A client gave up before we accepted it, or a signal arrived: try again.
                        io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted => {}
                        // E.g. out of file descriptors: often passes, so keep serving.
                        _ => {
                            eprintln!("sessionsnap: accept failed: {e}");
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                    if last.elapsed() >= idle {
                        return Ok(());
                    }
                }
            }
        }
    })
}

/// Prints where `sessionsnap ui` showed its page, or why it could not.
fn report_shown(response: &http::Response) {
    let body = String::from_utf8_lossy(&response.body);
    if response.status == 200 {
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        eprintln!("Showed it in cmux ({}).", v["shown"].as_str().unwrap_or("?"));
    } else {
        eprintln!("sessionsnap: {}\nRun `sessionsnap ui` again to show it.", body.trim());
    }
}

/// Longest wait for one read; a client that sends nothing is dropped after this.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest wait for one write; a client that stops reading is dropped after this.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Total time allowed to receive one request, however slowly the bytes arrive.
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// A stream whose reads fail with `TimedOut` once `until` has passed.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.read(buf)
    }
}

fn handle_connection<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    stream: TcpStream,
    server: &'scope Server<'_>,
    make_ctx: &MakeCtx<'_>,
) {
    // Accepted sockets inherit non-blocking mode on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let reader = Deadline { stream: &stream, until: Instant::now() + REQUEST_DEADLINE };
    let response = match http::read_request(&mut BufReader::new(reader)) {
        Ok(req) if req.method == "POST" && req.path == "/api/show" => {
            scope.spawn(move || {
                let response = server.handle(&req, &|_, _| anyhow::bail!("/api/show runs no command"));
                let _ = http::write_response(&mut &stream, &response);
            });
            return;
        }
        Ok(req) => server.handle(&req, make_ctx),
        Err(resp) => resp,
    };
    let _ = http::write_response(&mut &stream, &response);
}

/// Sends one raw request to the server on `port` and returns the whole response.
fn request(port: u16, request: &str, wait: Duration) -> io::Result<String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500))?;
    s.set_read_timeout(Some(wait))?;
    s.set_write_timeout(Some(wait))?;
    s.write_all(request.as_bytes())?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

/// True when a sessionsnap UI answers on `port`.
pub fn is_running(port: u16) -> bool {
    let req = format!("GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    request(port, &req, Duration::from_secs(2)).is_ok_and(|out| out.contains(r#""app":"sessionsnap""#))
}

/// Asks the UI server on `port` to show its page (with its key) in cmux; returns where.
pub fn ask_to_show(port: u16) -> Result<String> {
    let req = format!(
        "POST /api/show HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    // Showing runs cmux, which can take a few seconds.
    let out =
        request(port, &req, Duration::from_secs(15)).context("asking the running sessionsnap UI to show itself")?;
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((out.as_str(), ""));
    if !head.starts_with("HTTP/1.1 200 ") {
        anyhow::bail!("the running sessionsnap UI could not show itself: {}", body.trim());
    }
    let v: serde_json::Value = serde_json::from_str(body).context("reading the running sessionsnap UI's answer")?;
    v["shown"].as_str().map(str::to_string).context("the running sessionsnap UI gave no `shown`")
}

/// 32 hex characters from the system's random source.
pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context("reading /dev/urandom")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A UI already answers on `port`: its key is only in that server's memory, so ask it to show
/// its own page.
fn already_running(port: u16, url: &str, open: bool) -> Result<Outcome> {
    if !open {
        return Ok(Outcome::ok(
            json!({"url": url, "started": false}),
            format!("The sessionsnap UI is already running at {url}."),
        ));
    }
    match ask_to_show(port) {
        Ok(shown) => Ok(Outcome::ok(
            json!({"url": url, "started": false, "shown": shown}),
            format!("The sessionsnap UI is already running at {url}; showed it ({shown})."),
        )),
        Err(e) => Err(failure(
            Exit::Error,
            format!(
                "The sessionsnap UI is already running at {url} but could not be shown: {e:#}. To stop it: Ctrl-C in \
                 the terminal running `sessionsnap ui`, or close the \"sessionsnap UI\" workspace."
            ),
        )),
    }
}

/// `sessionsnap ui`: serves the page on 127.0.0.1 and shows it in cmux; returns once idle.
pub fn ui(_ctx: &Ctx, port: u16, idle_minutes: u64, open: bool) -> Result<Outcome> {
    let url = format!("http://127.0.0.1:{port}/");
    let idle = idle_minutes
        .checked_mul(60)
        .map(Duration::from_secs)
        .ok_or_else(|| failure(Exit::Error, "--idle-minutes is too large"))?;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if is_running(port) => return already_running(port, &url, open),
        Err(e) => {
            return Err(failure(Exit::Error, format!("cannot listen on 127.0.0.1:{port} ({e}); pick another --port")));
        }
    };
    // Shows run on their own thread, so each opens its own cmux client. The key travels in the
    // link's fragment, which the browser keeps to itself.
    let server = Server { port, token: new_token()?, show: Box::new(|u| CmuxCli::open_default()?.show_url(u)) };
    eprintln!("sessionsnap UI at {url}. It stops after {idle_minutes} min without use; Ctrl-C stops it now.");
    if !open {
        eprintln!("Run `sessionsnap ui` again to show it.");
    }
    let make_ctx = |cli: &crate::interfaces::cli::Cli, input: Option<String>| -> Result<Ctx> {
        Ok(Ctx { input, interactive: false, ..Ctx::open_default(true, cli.yes)? })
    };
    serve(listener, &server, &make_ctx, idle, open)?;
    Ok(Outcome::ok(
        json!({"url": url, "started": true, "stopped": "idle"}),
        "The sessionsnap UI stopped after being idle.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agents::claude::Claude;
    use crate::adapters::store::fs::Store;
    use crate::interfaces::cli::Cli;
    use crate::testing::FakeBackend;

    #[test]
    fn serves_requests_until_idle() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "t".into(), show: Box::new(|_| Ok("dock")) };
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        });
        let dir = tempfile::tempdir().unwrap();
        let make = |cli: &Cli, input: Option<String>| -> Result<Ctx> {
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
        let started = Instant::now();
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        let out = client.join().unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(out.contains(r#""app":"sessionsnap""#));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!is_running(port), "the server stopped");
    }

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 32);
        // Lowercase: the page only accepts #t=[0-9a-f]+.
        assert!(a.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert_ne!(a, b);
    }

    #[test]
    fn ui_flags_parse() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["sessionsnap", "ui", "--port", "5000", "--no-open"]).unwrap();
        assert!(matches!(
            cli.command,
            crate::interfaces::cli::Command::Ui { port: 5000, idle_minutes: 30, no_open: true }
        ));
    }

    fn test_ctx_maker(dir: &std::path::Path) -> impl Fn(&Cli, Option<String>) -> Result<Ctx> + '_ {
        move |cli, input| {
            Ok(Ctx {
                store: Store::new(dir.join("store")),
                claude: Claude::new(dir.join("claude")),
                backend: Box::new(FakeBackend::default()),
                json: cli.json,
                yes: cli.yes,
                interactive: false,
                input,
            })
        }
    }

    #[test]
    fn a_silent_client_does_not_block_others() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "t".into(), show: Box::new(|_| Ok("dock")) };
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let silent = std::thread::spawn(move || {
            let _s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            tx.send(()).unwrap();
            let _ = done_rx.recv();
        });
        rx.recv().unwrap();
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        });
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        let started = Instant::now();
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        let out = client.join().unwrap();
        done_tx.send(()).unwrap();
        silent.join().unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_slow_drip_client_is_cut_off_at_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "t".into(), show: Box::new(|_| Ok("dock")) };
        let drip = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            for _ in 0..14 {
                if s.write_all(b"G").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        let started = Instant::now();
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        let took = started.elapsed();
        drip.join().unwrap();
        assert!(took < Duration::from_secs(8), "{took:?}");
    }

    #[test]
    fn a_second_ui_asks_the_running_server_to_show_itself() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Mutex::new(vec![]);
        let server = Server {
            port,
            token: "k3y".into(),
            show: Box::new(|u: &str| {
                seen.lock().unwrap().push(u.to_string());
                Ok("reused")
            }),
        };
        let client = std::thread::spawn(move || ask_to_show(port).map_err(|e| format!("{e:#}")));
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        assert_eq!(client.join().unwrap(), Ok("reused".to_string()));
        assert_eq!(*seen.lock().unwrap(), vec![format!("http://127.0.0.1:{port}/#t=k3y")]);
    }

    /// A show function that, like `cmux browser goto`, returns only once the page has loaded.
    fn show_that_loads_the_page(port: u16) -> impl Fn(&str) -> Result<&'static str> + Send + Sync {
        move |_| {
            let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            let out = request(port, &req, Duration::from_secs(2)).context("navigation_timeout")?;
            anyhow::ensure!(out.starts_with("HTTP/1.1 200 OK"), "page did not load");
            Ok("reused")
        }
    }

    #[test]
    fn the_page_loads_while_a_show_waits_for_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "k3y".into(), show: Box::new(show_that_loads_the_page(port)) };
        let client = std::thread::spawn(move || ask_to_show(port).map_err(|e| format!("{e:#}")));
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        assert_eq!(client.join().unwrap(), Ok("reused".to_string()));
    }

    #[test]
    fn showing_at_start_does_not_block_the_page() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let shown = std::sync::Mutex::new(None);
        let load = show_that_loads_the_page(port);
        let server = Server {
            port,
            token: "k3y".into(),
            show: Box::new(|u: &str| {
                let r = load(u);
                *shown.lock().unwrap() = Some(r.as_ref().map(|s| s.to_string()).map_err(|e| format!("{e:#}")));
                r
            }),
        };
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        serve(listener, &server, &make, Duration::from_millis(300), true).unwrap();
        drop(server);
        assert_eq!(shown.into_inner().unwrap(), Some(Ok("reused".to_string())));
    }

    #[test]
    fn asking_a_server_that_cannot_show_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "k3y".into(), show: Box::new(|_| anyhow::bail!("no cmux")) };
        let client = std::thread::spawn(move || ask_to_show(port).map_err(|e| format!("{e:#}")));
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        serve(listener, &server, &make, Duration::from_millis(300), false).unwrap();
        let err = client.join().unwrap().unwrap_err();
        assert!(err.contains("no cmux"), "{err}");
    }
}
