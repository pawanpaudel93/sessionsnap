//! cmux's control socket, for what must not go on a command line: any local user can read a
//! process's argv with `ps`, so a link that carries a key is handed to cmux here instead.
//!
//! Protocol (cmux v2, checked against 0.64.22): one JSON request and "\n" per line, one JSON line
//! back: `{"ok":true,"result":…}` or `{"ok":false,"error":{"code":…,"message":…}}`. A process
//! running inside cmux may send a plain request.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Longest wait to connect, send or read the answer.
const TIMEOUT: Duration = Duration::from_secs(3);

/// The socket of the running cmux: `$SESSIONSNAP_CMUX_SOCKET`, else cmux's own override
/// `$CMUX_SOCKET_PATH` (or its older name `$CMUX_SOCKET`), else the path cmux last wrote to
/// `~/Library/Application Support/cmux/last-socket-path`. None when none is known.
pub fn default_path() -> Option<PathBuf> {
    for var in ["SESSIONSNAP_CMUX_SOCKET", "CMUX_SOCKET_PATH", "CMUX_SOCKET"] {
        if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
    }
    let file = crate::adapters::home_dir().ok()?.join("Library/Application Support/cmux/last-socket-path");
    let text = std::fs::read_to_string(file).ok()?;
    let path = text.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Calls the v2 method `method` with `params` on the socket at `path`; returns its `result`.
/// Errors carry cmux's error code and message, never the request.
pub fn call(path: &Path, method: &str, params: Value) -> Result<Value> {
    let stream =
        UnixStream::connect(path).with_context(|| format!("connecting to cmux's socket {}", path.display()))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = json!({"id": "sessionsnap", "method": method, "params": params}).to_string();
    line.push('\n');
    (&stream).write_all(line.as_bytes()).context("writing to cmux's socket")?;
    let mut answer = String::new();
    BufReader::new(&stream).read_line(&mut answer).context("reading cmux's answer")?;
    let v: Value = serde_json::from_str(answer.trim()).context("cmux's socket answered something other than JSON")?;
    if v["ok"].as_bool() == Some(true) {
        return Ok(v["result"].clone());
    }
    let code = v["error"]["code"].as_str().unwrap_or("error");
    let message = v["error"]["message"].as_str().unwrap_or("no message");
    bail!("cmux refused {method}: {code}: {message}")
}
