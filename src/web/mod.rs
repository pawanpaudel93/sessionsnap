//! The local web UI: a small HTTP server on 127.0.0.1 that runs ccsnap commands for its page.

pub mod api;
pub mod http;

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::json;

use crate::cli::{Ctx, Outcome};
use crate::error::{Exit, failure};
use api::{MakeCtx, Server};

/// Fixed so the Dock tab's URL stays valid across restarts.
pub const DEFAULT_PORT: u16 = 47317;

/// Answers requests one at a time until none has come for `idle`.
pub fn serve(listener: TcpListener, server: &Server, make_ctx: &MakeCtx<'_>, idle: Duration) -> Result<()> {
    listener.set_nonblocking(true)?;
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                handle_connection(&stream, server, make_ctx);
                last = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if last.elapsed() >= idle {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn handle_connection(stream: &TcpStream, server: &Server, make_ctx: &MakeCtx<'_>) {
    // Accepted sockets inherit non-blocking mode on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let response = match http::read_request(&mut BufReader::new(stream)) {
        Ok(req) => server.handle(&req, make_ctx),
        Err(resp) => resp,
    };
    let mut out = stream;
    let _ = http::write_response(&mut out, &response);
}

/// True when a ccsnap UI answers on `port`.
pub fn is_running(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    let mut out = String::new();
    s.write_all(request.as_bytes()).is_ok() && s.read_to_string(&mut out).is_ok() && out.contains(r#""app":"ccsnap""#)
}

/// 32 hex characters from the system's random source.
pub fn new_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context("reading /dev/urandom")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn show(ctx: &Ctx, url: &str, open: bool) -> &'static str {
    if !open {
        return "not shown";
    }
    ctx.backend.show_url(url).unwrap_or_else(|e| {
        eprintln!("ccsnap: could not show the UI in cmux: {e:#}");
        "not shown"
    })
}

/// `ccsnap ui`: serves the page on 127.0.0.1 and shows it in cmux; returns once idle.
pub fn ui(ctx: &Ctx, port: u16, idle_minutes: u64, open: bool) -> Result<Outcome> {
    let url = format!("http://127.0.0.1:{port}/");
    let idle = idle_minutes
        .checked_mul(60)
        .map(Duration::from_secs)
        .ok_or_else(|| failure(Exit::Error, "--idle-minutes is too large"))?;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(_) if is_running(port) => {
            // Its key is only in that server's memory; a fresh key needs a fresh start.
            return Ok(Outcome::ok(
                json!({"url": url, "started": false}),
                format!(
                    "The ccsnap UI is already running at {url}. Use its tab, or stop it and run `ccsnap ui` again."
                ),
            ));
        }
        Err(e) => {
            return Err(failure(Exit::Error, format!("cannot listen on 127.0.0.1:{port} ({e}); pick another --port")));
        }
    };
    let server = Server { port, token: new_token()? };
    // The key travels in the fragment, which the browser keeps to itself.
    let shown = show(ctx, &format!("{url}#t={}", server.token), open);
    eprintln!("ccsnap UI at {url} ({shown}). It stops after {idle_minutes} min without use; Ctrl-C stops it now.");
    let make_ctx = |cli: &crate::cli::Cli, input: Option<String>| -> Result<Ctx> {
        Ok(Ctx { input, interactive: false, ..Ctx::open_default(true, cli.yes)? })
    };
    serve(listener, &server, &make_ctx, idle)?;
    Ok(Outcome::ok(json!({"url": url, "started": true, "stopped": "idle"}), "The ccsnap UI stopped after being idle."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::Claude;
    use crate::cli::Cli;
    use crate::store::Store;
    use crate::testing::FakeBackend;

    #[test]
    fn serves_requests_until_idle() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server { port, token: "t".into() };
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
        serve(listener, &server, &make, Duration::from_millis(300)).unwrap();
        let out = client.join().unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(out.contains(r#""app":"ccsnap""#));
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
        let cli = Cli::try_parse_from(["ccsnap", "ui", "--port", "5000", "--no-open"]).unwrap();
        assert!(matches!(cli.command, crate::cli::Command::Ui { port: 5000, idle_minutes: 30, no_open: true }));
    }
}
