//! cmux's control socket, for what must not go on a command line: any local user can read a
//! process's argv with `ps`, so a link that carries a key is handed to cmux here instead.
//!
//! Protocol (cmux v2, checked against 0.64.22): one JSON request and "\n" per line, one JSON line
//! back: `{"ok":true,"result":…}` or `{"ok":false,"error":{"code":…,"message":…}}`. A process
//! running inside cmux may send a plain request.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
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
    // A request may carry a key: send it only to a socket another user cannot have put there.
    trusted(path)?;
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

/// What decides whether a socket path can be trusted with a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SocketFacts {
    /// The path itself is a symlink (`lstat`).
    pub symlink: bool,
    pub socket: bool,
    pub owner: u32,
    /// Mode bits of the folder holding it.
    pub parent_mode: u32,
}

/// Why `facts` make a socket unsafe to send a key to, for the user `me`; None when it is safe:
/// a socket itself (not a symlink), owned by `me`, in a folder others cannot swap it in (not
/// writable by group or others, or sticky like /tmp, where only the owner may rename a file).
pub fn refusal(facts: SocketFacts, me: u32) -> Option<&'static str> {
    if facts.symlink {
        return Some("it is a symlink");
    }
    if !facts.socket {
        return Some("it is not a socket");
    }
    if facts.owner != me {
        return Some("another user owns it");
    }
    if facts.parent_mode & 0o022 != 0 && facts.parent_mode & 0o1000 == 0 {
        return Some("its folder is writable by other users");
    }
    None
}

/// Fails when the socket at `path` is not safe to send a key to (see `refusal`).
fn trusted(path: &Path) -> Result<()> {
    let shown = path.display();
    let meta = std::fs::symlink_metadata(path).with_context(|| format!("cmux's socket {shown} is not there"))?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent_meta = std::fs::metadata(parent).with_context(|| format!("reading the folder of {shown}"))?;
    let facts = SocketFacts {
        symlink: meta.file_type().is_symlink(),
        socket: meta.file_type().is_socket(),
        owner: meta.uid(),
        parent_mode: parent_meta.mode(),
    };
    match refusal(facts, my_uid()?) {
        None => Ok(()),
        Some(why) => bail!("not using cmux's socket {shown}: {why}"),
    }
}

/// This process's user ID, read from a file it creates (std has no getuid).
fn my_uid() -> Result<u32> {
    let path =
        std::env::temp_dir().join(format!("sessionsnap-uid-{}-{:?}", std::process::id(), std::thread::current().id()));
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path);
    let uid = file.and_then(|f| f.metadata()).map(|m| m.uid());
    let _ = std::fs::remove_file(&path);
    uid.context("finding this process's user ID")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 501;
    const OK: SocketFacts = SocketFacts { symlink: false, socket: true, owner: ME, parent_mode: 0o40700 };

    #[test]
    fn only_our_own_socket_in_a_safe_folder_is_trusted() {
        assert_eq!(refusal(OK, ME), None);
        // /tmp: writable by everyone but sticky.
        assert_eq!(refusal(SocketFacts { parent_mode: 0o41777, ..OK }, ME), None);
        assert_eq!(refusal(SocketFacts { parent_mode: 0o40755, ..OK }, ME), None);
        assert!(refusal(SocketFacts { owner: 502, ..OK }, ME).unwrap().contains("another user"));
        assert!(refusal(SocketFacts { symlink: true, ..OK }, ME).unwrap().contains("symlink"));
        assert!(refusal(SocketFacts { socket: false, ..OK }, ME).unwrap().contains("not a socket"));
        for mode in [0o40777, 0o40775, 0o40757] {
            let why = refusal(SocketFacts { parent_mode: mode, ..OK }, ME).unwrap();
            assert!(why.contains("writable by other users"), "{mode:o}");
        }
    }

    #[test]
    fn checks_the_real_path() {
        let dir = tempfile::Builder::new().prefix("ss").tempdir_in("/tmp").unwrap();
        let sock = dir.path().join("s");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        trusted(&sock).unwrap();
        let link = dir.path().join("l");
        std::os::unix::fs::symlink(&sock, &link).unwrap();
        assert!(format!("{:#}", trusted(&link).unwrap_err()).contains("symlink"));
        let file = dir.path().join("f");
        std::fs::write(&file, "").unwrap();
        assert!(format!("{:#}", trusted(&file).unwrap_err()).contains("not a socket"));
        assert!(trusted(&dir.path().join("missing")).is_err());
        // A refused path is never connected to.
        let err = format!("{:#}", call(&link, "browser.navigate", json!({})).unwrap_err());
        assert!(err.contains("not using cmux's socket"), "{err}");
    }
}
