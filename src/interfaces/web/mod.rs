//! The local web UI: a small HTTP server on 127.0.0.1 that runs sessionsnap commands for its page.
//!
//! Layer rule: an interface. The page speaks the CLI's command protocol: the commands it sends
//! run through `cli::run_json`, exactly as `sessionsnap <args> --json` would.

pub mod api;
pub mod http;

use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::json;

use crate::domain::terminal::TerminalKind;
use crate::error::{Exit, failure};
use crate::interfaces::cli::{Ctx, Outcome};
use api::{MakeCtx, Server};

/// Fixed so the Dock tab's URL stays valid across restarts.
pub const DEFAULT_PORT: u16 = 47317;

/// Connections handled at once. A show takes one while cmux loads the page, which needs another.
pub const WORKERS: usize = 4;
/// Connections that may wait for a worker; one more gets 503. Enough for a page reload (the
/// page, its script and its style) behind commands that hold every worker.
pub const QUEUE: usize = 16;

/// Answers requests on `WORKERS` threads until no request has come for `idle`. With
/// `show_at_start`, shows the page once the workers are up.
pub fn serve(
    listener: TcpListener,
    server: &Server<'_>,
    make_ctx: &MakeCtx<'_>,
    idle: Duration,
    show_at_start: bool,
) -> Result<()> {
    serve_until(listener, server, make_ctx, idle, show_at_start, &AtomicBool::new(false))
}

/// `serve`, which also stops once `stop` is set.
fn serve_until(
    listener: TcpListener,
    server: &Server<'_>,
    make_ctx: &MakeCtx<'_>,
    idle: Duration,
    show_at_start: bool,
    stop: &AtomicBool,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    // Connections queued or being handled; the accept loop keeps it at most `WORKERS + QUEUE`.
    let busy = AtomicUsize::new(0);
    // When the UI was last used (`Server::is_use`): connections that send nothing, and requests
    // anyone could send, don't keep the server running.
    let last = Mutex::new(Instant::now());
    let (queue, inbox) = mpsc::channel::<TcpStream>();
    let inbox = Mutex::new(inbox);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                loop {
                    // The lock is held only while waiting; it ends when `queue` is dropped.
                    let next = inbox.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok(stream) = next else { return };
                    // A bug in a command must not take its worker, or the server, with it.
                    let handled = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        handle_connection(&stream, server, make_ctx, &last);
                    }));
                    if handled.is_err() {
                        let response = http::Response::text(500, "sessionsnap failed; see the terminal running it");
                        let _ = http::write_response(&mut &stream, &response);
                    }
                    busy.fetch_sub(1, Ordering::SeqCst);
                }
            });
        }
        if show_at_start {
            scope.spawn(|| report_shown(&server.show_page(None)));
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) if busy.load(Ordering::SeqCst) >= WORKERS + QUEUE => refuse(stream),
                Ok((stream, _)) => {
                    busy.fetch_add(1, Ordering::SeqCst);
                    if queue.send(stream).is_err() {
                        busy.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                Err(e) => match e.kind() {
                    io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    // A client gave up before we accepted it, or a signal arrived: try again.
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted => {}
                    // E.g. out of file descriptors: often passes, so keep serving.
                    _ => {
                        eprintln!("sessionsnap: accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(100));
                    }
                },
            }
            if stop.load(Ordering::SeqCst) || last.lock().unwrap_or_else(|e| e.into_inner()).elapsed() >= idle {
                // Workers finish what they have, then stop; the scope waits for them. New clients
                // are refused at once meanwhile.
                drop(queue);
                drop(listener);
                return Ok(());
            }
        }
    })
}

/// Answers 503 without holding up the accept thread.
fn refuse(stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = http::write_response(&mut &stream, &http::Response::text(503, "busy; try again"));
    let _ = stream.shutdown(std::net::Shutdown::Write);
    // Closing on request bytes still unread would reset the connection, and the client could lose
    // the answer: read until it closes, briefly, on a thread of its own. When the system has no
    // thread to spare, the stream is just closed (`spawn` would panic the accept thread).
    let _ = std::thread::Builder::new().name("sessionsnap-drain".into()).spawn(move || {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        let until = Instant::now() + Duration::from_secs(1);
        let mut buf = [0u8; 4096];
        while Instant::now() < until && matches!((&stream).read(&mut buf), Ok(n) if n > 0) {}
    });
}

/// Prints where `sessionsnap ui` showed its page, or why it could not.
fn report_shown(response: &http::Response) {
    let body = String::from_utf8_lossy(&response.body);
    if response.status == 200 {
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let place = v["shown"].as_str().unwrap_or("?");
        if v["keyed"] == false {
            // `show_page` printed the keyed link just above.
            eprintln!("Showed it ({place}) without its key: open the link above.");
        } else {
            eprintln!("Showed it ({place}).");
        }
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

fn handle_connection(stream: &TcpStream, server: &Server<'_>, make_ctx: &MakeCtx<'_>, last: &Mutex<Instant>) {
    let seen = || *last.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    // Accepted sockets inherit non-blocking mode on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let reader = Deadline { stream, until: Instant::now() + REQUEST_DEADLINE };
    match http::read_request(&mut BufReader::new(reader)) {
        Ok(req) => {
            let used = server.is_use(&req);
            if used {
                seen();
            }
            let response = server.handle(&req, make_ctx);
            let _ = http::write_response(&mut &*stream, &response);
            if used {
                // A command can run for long; idle counts from when it ended.
                seen();
            }
        }
        Err(response) => {
            let _ = http::write_response(&mut &*stream, &response);
        }
    }
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

/// Asks the UI server on `port` to show its page (with its key) where it shows pages, or in the
/// default browser; returns where, and whether the key reached the page (if not, that server
/// printed the keyed link in its terminal).
pub fn ask_to_show(port: u16, browser: bool) -> Result<(String, bool)> {
    let body = if browser { r#"{"in":"browser"}"# } else { "{}" };
    let req = format!(
        "POST /api/show HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // Showing runs cmux, which can take a few seconds.
    let out =
        request(port, &req, Duration::from_secs(15)).context("asking the running sessionsnap UI to show itself")?;
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((out.as_str(), ""));
    if !head.starts_with("HTTP/1.1 200 ") {
        anyhow::bail!("the running sessionsnap UI could not show itself: {}", body.trim());
    }
    let v: serde_json::Value = serde_json::from_str(body).context("reading the running sessionsnap UI's answer")?;
    let shown = v["shown"].as_str().map(str::to_string).context("the running sessionsnap UI gave no `shown`")?;
    Ok((shown, v["keyed"] != false))
}

/// 32 hex characters from the system's random source.
pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context("reading /dev/urandom")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A UI already answers on `port`: its key is only in that server's memory, so ask it to show
/// its own page.
fn already_running(port: u16, url: &str, open: bool, browser: bool) -> Result<Outcome> {
    if !open {
        return Ok(Outcome::ok(
            json!({"url": url, "started": false}),
            format!("The sessionsnap UI is already running at {url}."),
        ));
    }
    match ask_to_show(port, browser) {
        Ok((shown, true)) => Ok(Outcome::ok(
            json!({"url": url, "started": false, "shown": shown, "keyed": true}),
            format!("The sessionsnap UI is already running at {url}; showed it ({shown})."),
        )),
        Ok((shown, false)) => Ok(Outcome::ok(
            json!({"url": url, "started": false, "shown": shown, "keyed": false}),
            format!(
                "The sessionsnap UI is already running at {url}; showed it ({shown}) but could not hand it its key. \
                 Open the link printed in the terminal running `sessionsnap ui`, or open it again from the page."
            ),
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

/// `sessionsnap ui`: serves the page on 127.0.0.1 and shows it in cmux's Dock, or the default
/// browser from tmux or with `browser`; returns once idle.
pub fn ui(ctx: &Ctx, port: u16, idle_minutes: u64, open: bool, browser: bool) -> Result<Outcome> {
    let url = format!("http://127.0.0.1:{port}/");
    let idle = idle_minutes
        .checked_mul(60)
        .map(Duration::from_secs)
        .ok_or_else(|| failure(Exit::Error, "--idle-minutes is too large"))?;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if is_running(port) => return already_running(port, &url, open, browser),
        Err(e) => {
            return Err(failure(Exit::Error, format!("cannot listen on 127.0.0.1:{port} ({e}); pick another --port")));
        }
    };
    // Shows run on their own thread, so each opens its own terminal client: cmux shows the page in
    // its Dock, tmux (or `--browser`) the default browser. The key travels in the link's fragment,
    // which the browser keeps to itself.
    let terminal = ctx.terminal.kind();
    let show = move |u: &str| crate::app::show_url(terminal, u);
    let mut server = Server::new(port, new_token()?, Box::new(show)).for_terminal(terminal);
    if terminal == TerminalKind::Cmux {
        // The page asks for either; `--browser` makes the browser the default.
        server = server.with_browser(Box::new(crate::app::show_in_browser), browser);
    }
    eprintln!("sessionsnap UI at {url}. It stops after {idle_minutes} min without use; Ctrl-C stops it now.");
    if !open {
        eprintln!("Run `sessionsnap ui` again to show it.");
    }
    let make_ctx = |cli: &crate::interfaces::cli::Cli, input: Option<String>| -> Result<Ctx> {
        // The page's commands use the terminal the server reports (`/api/hello`), not one picked anew.
        let terminal = cli.terminal.as_deref().or(Some(terminal.name()));
        Ok(Ctx { input, interactive: false, ..Ctx::open_default(true, cli.yes, terminal)? })
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
    use crate::interfaces::cli::Cli;
    use crate::ports::Shown;
    use crate::testing::FakeTerminal;

    #[test]
    fn serves_requests_until_idle() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let client = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        });
        let dir = tempfile::tempdir().unwrap();
        let make = |cli: &Cli, input: Option<String>| -> Result<Ctx> {
            Ok(crate::testing::ctx(dir.path(), FakeTerminal::default(), cli.json, cli.yes, input))
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
            crate::interfaces::cli::Command::Ui { port: 5000, idle_minutes: 30, no_open: true, browser: false }
        ));
        let cli = Cli::try_parse_from(["sessionsnap", "--terminal", "cmux", "ui", "--browser"]).unwrap();
        assert!(matches!(cli.command, crate::interfaces::cli::Command::Ui { browser: true, .. }));
        // Port 0 would serve on a port nobody is told; idle 0 would stop before the page loads.
        assert!(Cli::try_parse_from(["sessionsnap", "ui", "--port", "0"]).is_err());
        assert!(Cli::try_parse_from(["sessionsnap", "ui", "--idle-minutes", "0"]).is_err());
    }

    fn test_ctx_maker(dir: &std::path::Path) -> impl Fn(&Cli, Option<String>) -> Result<Ctx> + Send + Sync + '_ {
        move |cli, input| Ok(crate::testing::ctx(dir, FakeTerminal::default(), cli.json, cli.yes, input))
    }

    /// Sets the flag when dropped, so a failed client still stops the server.
    struct StopOnDrop<'a>(&'a AtomicBool);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// Serves on `listener` while `client` runs, then stops: the idle timeout plays no part.
    fn serve_while<T: Send>(
        listener: TcpListener,
        server: &Server<'_>,
        make: &MakeCtx<'_>,
        show_at_start: bool,
        client: impl FnOnce() -> T + Send,
    ) -> T {
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let client = scope.spawn(|| {
                let _stop = StopOnDrop(&stop);
                client()
            });
            serve_until(listener, server, make, Duration::from_secs(60), show_at_start, &stop).unwrap();
            client.join().unwrap()
        })
    }

    fn listen() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    fn get(port: u16, path: &str) -> String {
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        request(port, &req, Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn a_silent_client_does_not_block_others() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            let _silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let started = Instant::now();
            (get(port, "/api/hello"), started.elapsed())
        });
        assert!(out.0.starts_with("HTTP/1.1 200 OK"), "{}", out.0);
        assert!(out.1 < Duration::from_secs(1), "waited {:?} behind the silent client", out.1);
    }

    /// Sends a byte every 200 ms until dropped or the server drops it.
    struct Drip(Option<std::sync::mpsc::Sender<()>>, Option<std::thread::JoinHandle<()>>);

    fn drip(port: u16) -> Drip {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            while stopped.recv_timeout(Duration::from_millis(200)) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                if s.write_all(b"G").is_err() {
                    break;
                }
            }
        });
        Drip(Some(stop), Some(thread))
    }

    impl Drop for Drip {
        fn drop(&mut self) {
            drop(self.0.take());
            let _ = self.1.take().map(|t| t.join());
        }
    }

    #[test]
    fn a_slow_client_does_not_hold_up_a_second_request() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let (out, took) = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            let _slow = drip(port);
            let started = Instant::now();
            (get(port, "/api/hello"), started.elapsed())
        });
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(took < Duration::from_secs(1), "waited {took:?} behind the slow client");
    }

    #[test]
    fn connections_beyond_the_workers_are_refused() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let (full, after) = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            // Connections are accepted in order, so these hold every worker and fill the queue
            // when the next comes.
            let slow: Vec<_> = (0..WORKERS + QUEUE).map(|_| drip(port)).collect();
            let full = get(port, "/api/hello");
            drop(slow);
            (full, get(port, "/api/hello"))
        });
        assert!(full.starts_with("HTTP/1.1 503 "), "{full}");
        assert!(after.starts_with("HTTP/1.1 200 OK"), "{after}");
    }

    /// Sends `POST /api/run` with these args and returns the connection to read the answer from.
    fn post_run(port: u16, token: &str, args: &str) -> TcpStream {
        let body = format!(r#"{{"args":{args}}}"#);
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(
            s,
            "POST /api/run HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
             X-Sessionsnap-Token: {token}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        s
    }

    fn send_get(port: u16, path: &str) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        s
    }

    fn answer(mut s: TcpStream) -> String {
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn the_page_loads_while_every_worker_waits_on_a_command() {
        let (listener, port) = listen();
        let server = Server::new(port, "k".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        let (started, release_rx) = (Mutex::new(started), Mutex::new(release_rx));
        let first = AtomicBool::new(true);
        // The first command blocks until released; the next ones wait for it.
        let make = |cli: &Cli, input: Option<String>| -> Result<Ctx> {
            if first.swap(false, Ordering::SeqCst) {
                started.lock().unwrap().send(()).unwrap();
                let _ = release_rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
            }
            Ok(crate::testing::ctx(dir.path(), FakeTerminal::default(), cli.json, cli.yes, input))
        };
        let (long, waiting, page) = serve_while(listener, &server, &make, false, move || {
            let long = post_run(port, "k", r#"["list"]"#);
            started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            // Accepted in order: these take the other workers before the page comes.
            let waiting: Vec<_> = (1..WORKERS).map(|_| post_run(port, "k", r#"["list"]"#)).collect();
            let page: Vec<_> = ["/", "/app.css", "/app.js"].iter().map(|p| send_get(port, p)).collect();
            let page: Vec<_> = page.into_iter().map(answer).collect();
            let waiting: Vec<_> = waiting.into_iter().map(answer).collect();
            release.send(()).unwrap();
            (answer(long), waiting, page)
        });
        for out in &page {
            assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        }
        assert!(long.starts_with("HTTP/1.1 200 OK") && long.contains(r#""exit":0"#), "{long}");
        for out in &waiting {
            assert!(out.starts_with("HTTP/1.1 409 "), "{out}");
            assert!(out.contains(r#""code":"busy""#), "{out}");
        }
    }

    #[test]
    fn a_panicking_command_answers_500_and_keeps_its_worker() {
        let (listener, port) = listen();
        let server = Server::new(port, "k".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let make = |_: &Cli, _: Option<String>| -> Result<Ctx> { panic!("a bug in a command") };
        let (failed, after) = serve_while(listener, &server, &make, false, move || {
            // More than there are workers: none may be lost.
            let failed: Vec<_> = (0..WORKERS + 2).map(|_| answer(post_run(port, "k", r#"["list"]"#))).collect();
            (failed, get(port, "/api/hello"))
        });
        for out in &failed {
            assert!(out.starts_with("HTTP/1.1 500 "), "{out}");
        }
        assert!(after.starts_with("HTTP/1.1 200 OK"), "{after}");
    }

    #[test]
    fn a_refused_client_reads_its_503_even_when_it_sends_late() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            let _full: Vec<_> = (0..WORKERS + QUEUE).map(|_| drip(port)).collect();
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            // The rest of the request comes after the server has answered.
            s.write_all(b"GET /api/hello HTTP/1.1\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(60));
            let _ = s.write_all(format!("Host: 127.0.0.1:{port}\r\n\r\n").as_bytes());
            std::thread::sleep(Duration::from_millis(20));
            let mut out = vec![];
            let read = s.read_to_end(&mut out).map_err(|e| e.kind());
            (read.map(drop), String::from_utf8_lossy(&out).into_owned())
        });
        assert_eq!(out.0, Ok(()), "{}", out.1);
        assert!(out.1.starts_with("HTTP/1.1 503 "), "{}", out.1);
    }

    fn post_show(port: u16) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "POST /api/show HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
             Content-Length: 2\r\n\r\n{{}}"
        )
        .unwrap();
        s
    }

    /// The answer, or the error when none came in time.
    fn try_answer(mut s: TcpStream) -> String {
        let mut out = String::new();
        s.read_to_string(&mut out).map_or_else(|e| format!("no answer: {e}"), |_| out)
    }

    #[test]
    fn hung_shows_never_take_more_than_two_workers() {
        let (listener, port) = listen();
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (started, released) = (Mutex::new(started), Mutex::new(released));
        // Like a cmux that never answers, until the test ends.
        let server = Server::new(
            port,
            "k".into(),
            Box::new(|_| {
                started.lock().unwrap().send(()).unwrap();
                let _ = released.lock().unwrap().recv_timeout(Duration::from_secs(30));
                Ok(Shown::whole("dock"))
            }),
        )
        .with_show_limits(Duration::ZERO, Duration::from_millis(100));
        let dir = tempfile::tempdir().unwrap();
        let (refused, page) = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, move || {
            let mut hung = vec![];
            let mut refused = vec![];
            for _ in 0..4 {
                let s = post_show(port);
                if started_rx.recv_timeout(Duration::from_millis(500)).is_ok() {
                    hung.push(s);
                } else {
                    refused.push(try_answer(s));
                }
                // Let the running ones count as hung.
                std::thread::sleep(Duration::from_millis(150));
            }
            let page = get(port, "/");
            drop(release);
            assert_eq!(hung.len(), 2, "{refused:?}");
            (refused, page)
        });
        for out in &refused {
            assert!(out.starts_with("HTTP/1.1 429 "), "{out}");
        }
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
    }

    #[test]
    fn once_stopping_it_refuses_new_clients_while_a_command_finishes() {
        let (listener, port) = listen();
        let server = Server::new(port, "k".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (started, released) = (Mutex::new(started), Mutex::new(released));
        let make = |cli: &Cli, input: Option<String>| -> Result<Ctx> {
            started.lock().unwrap().send(()).unwrap();
            let _ = released.lock().unwrap().recv_timeout(Duration::from_secs(20));
            Ok(crate::testing::ctx(dir.path(), FakeTerminal::default(), cli.json, cli.yes, input))
        };
        let stop = AtomicBool::new(false);
        let refused = std::thread::scope(|scope| {
            let stop_ref = &stop;
            let client = scope.spawn(move || {
                let _stop = StopOnDrop(stop_ref);
                let long = post_run(port, "k", r#"["list"]"#);
                started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                stop_ref.store(true, Ordering::SeqCst);
                // The accept loop checks the flag every 50 ms.
                let until = Instant::now() + Duration::from_secs(2);
                let mut refused = false;
                while Instant::now() < until {
                    if TcpStream::connect(("127.0.0.1", port)).is_err() {
                        refused = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                release.send(()).unwrap();
                let out = answer(long);
                assert!(out.starts_with("HTTP/1.1 200 OK"), "the running command finishes: {out}");
                refused
            });
            serve_until(listener, &server, &make, Duration::from_secs(60), false, &stop).unwrap();
            client.join().unwrap()
        });
        assert!(refused, "a client still connected after the server stopped accepting");
    }

    /// The one test that waits for the idle timeout.
    #[test]
    fn stops_once_idle_and_only_use_of_the_ui_counts() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        // What another site, or a stray client, can send: none of it is use of the UI.
        let noise = [
            String::new(),
            format!("GET /nope HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            format!("GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            "GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n".to_string(),
            format!("DELETE / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            format!(
                "POST /api/run HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
                 Content-Length: 17\r\n\r\n{{\"args\":[\"list\"]}}"
            ),
        ];
        let pinger = std::thread::spawn(move || {
            let started = Instant::now();
            for req in noise.iter().cycle() {
                if started.elapsed() > Duration::from_secs(4) {
                    break;
                }
                let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else { break };
                let _ = s.write_all(req.as_bytes());
                drop(s);
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let make = test_ctx_maker(dir.path());
        let started = Instant::now();
        serve(listener, &server, &make, Duration::from_millis(500), false).unwrap();
        let took = started.elapsed();
        pinger.join().unwrap();
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!(!is_running(port), "the server stopped");
    }

    #[test]
    fn a_slow_drip_client_is_cut_off_at_the_deadline() {
        let (listener, port) = listen();
        let server = Server::new(port, "t".into(), Box::new(|_| Ok(Shown::whole("dock"))));
        let dir = tempfile::tempdir().unwrap();
        let took = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(14))).unwrap();
            let mut w = s.try_clone().unwrap();
            let started = Instant::now();
            let dripper = std::thread::spawn(move || {
                for _ in 0..70 {
                    if w.write_all(b"G").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
            // The server answers and closes once the deadline has passed.
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            let took = started.elapsed();
            drop(s);
            dripper.join().unwrap();
            took
        });
        assert!(took >= REQUEST_DEADLINE - Duration::from_millis(500), "{took:?}");
        assert!(took < Duration::from_secs(8), "{took:?}");
    }

    #[test]
    fn a_second_ui_asks_the_running_server_to_show_itself() {
        let (listener, port) = listen();
        let seen = std::sync::Mutex::new(vec![]);
        let server = Server::new(
            port,
            "k3y".into(),
            Box::new(|u: &str| {
                seen.lock().unwrap().push(u.to_string());
                Ok(Shown::whole("reused"))
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            ask_to_show(port, false).map_err(|e| format!("{e:#}"))
        });
        assert_eq!(out, Ok(("reused".to_string(), true)));
        assert_eq!(*seen.lock().unwrap(), vec![format!("http://127.0.0.1:{port}/#t=k3y")]);
    }

    /// A second `ui --browser` asks a running cmux server to show its page in the browser.
    #[test]
    fn a_second_ui_for_the_browser_asks_for_the_browser() {
        let (listener, port) = listen();
        let seen = std::sync::Mutex::new(vec![]);
        let server = Server::new(
            port,
            "k3y".into(),
            Box::new(|_| {
                seen.lock().unwrap().push("cmux");
                Ok(Shown::whole("dock"))
            }),
        )
        .with_browser(
            Box::new(|_| {
                seen.lock().unwrap().push("browser");
                Ok(Shown::whole("browser"))
            }),
            false,
        )
        .with_show_limits(Duration::ZERO, Duration::from_secs(5));
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{port}/");
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            [true, false].map(|b| {
                already_running(port, &url, true, b).map(|o| o.json["shown"].clone()).map_err(|e| format!("{e:#}"))
            })
        });
        assert_eq!(out, [Ok(json!("browser")), Ok(json!("dock"))]);
        assert_eq!(*seen.lock().unwrap(), ["browser", "cmux"]);
    }

    /// A show function that, like `cmux browser goto`, returns only once the page has loaded.
    fn show_that_loads_the_page(port: u16) -> impl Fn(&str) -> Result<Shown> + Send + Sync {
        move |_| {
            let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            let out = request(port, &req, Duration::from_secs(2)).context("navigation_timeout")?;
            anyhow::ensure!(out.starts_with("HTTP/1.1 200 OK"), "page did not load");
            Ok(Shown::whole("reused"))
        }
    }

    #[test]
    fn the_page_loads_while_a_show_waits_for_it() {
        let (listener, port) = listen();
        let server = Server::new(port, "k3y".into(), Box::new(show_that_loads_the_page(port)));
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            ask_to_show(port, false).map_err(|e| format!("{e:#}"))
        });
        assert_eq!(out, Ok(("reused".to_string(), true)));
    }

    #[test]
    fn showing_at_start_does_not_block_the_page() {
        let (listener, port) = listen();
        let (done, shown) = std::sync::mpsc::channel();
        let done = std::sync::Mutex::new(done);
        let load = show_that_loads_the_page(port);
        let server = Server::new(
            port,
            "k3y".into(),
            Box::new(|u: &str| {
                let r = load(u);
                let _ =
                    done.lock().unwrap().send(r.as_ref().map(|s| s.place.to_string()).map_err(|e| format!("{e:#}")));
                r
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), true, move || {
            shown.recv_timeout(Duration::from_secs(10)).unwrap()
        });
        assert_eq!(out, Ok("reused".to_string()));
    }

    #[test]
    fn a_second_ui_hears_when_the_key_did_not_reach_the_page() {
        let (listener, port) = listen();
        let server = Server::new(
            port,
            "k3y".into(),
            Box::new(|_| Ok(Shown { place: "dock", fragment_error: Some("no socket".into()) })),
        );
        let dir = tempfile::tempdir().unwrap();
        let out = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            ask_to_show(port, false).map_err(|e| format!("{e:#}"))
        });
        assert_eq!(out, Ok(("dock".to_string(), false)));
    }

    #[test]
    fn asking_a_server_that_cannot_show_is_an_error() {
        let (listener, port) = listen();
        let server = Server::new(port, "k3y".into(), Box::new(|_| anyhow::bail!("no cmux")));
        let dir = tempfile::tempdir().unwrap();
        let err = serve_while(listener, &server, &test_ctx_maker(dir.path()), false, || {
            ask_to_show(port, false).map_err(|e| format!("{e:#}"))
        })
        .unwrap_err();
        assert!(err.contains("no cmux"), "{err}");
    }
}
