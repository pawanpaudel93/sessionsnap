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
    let me = getuid();
    trusted(path, me)?;
    let stream =
        UnixStream::connect(path).with_context(|| format!("connecting to cmux's socket {}", path.display()))?;
    // The path could have changed since it was checked: what counts is who is at the other end.
    let peer = peer_uid(&stream).context("asking who runs cmux's socket")?;
    if let Some(why) = peer_refusal(peer, me) {
        bail!("not using cmux's socket {}: {why}", path.display());
    }
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = json!({"id": "sessionsnap", "method": method, "params": params}).to_string();
    line.push('\n');
    (&stream).write_all(line.as_bytes()).context("writing to cmux's socket")?;
    let mut answer = String::new();
    BufReader::new(&stream).read_line(&mut answer).context("reading cmux's answer")?;
    // cmux refuses some callers in plain text, e.g. "ERROR: Access denied - only processes started
    // inside cmux can connect" for a process that is not cmux's descendant.
    if let Some(why) = answer.trim().strip_prefix("ERROR:") {
        bail!("cmux refused {method}: {}", why.trim());
    }
    let v: Value = serde_json::from_str(answer.trim()).with_context(|| {
        if answer.trim().is_empty() {
            "cmux's socket closed without answering".to_string()
        } else {
            "cmux's socket answered something other than JSON".to_string()
        }
    })?;
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
    /// Owner of the folder holding it.
    pub parent_owner: u32,
}

/// Why `facts` make a socket unsafe to send a key to, for the user `me`; None when it is safe:
/// a socket itself (not a symlink), owned by `me`, in a folder others cannot swap it in (not
/// writable by group or others, or sticky like /tmp, where only the owner may rename a file) that
/// belongs to `me` or root.
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
    if facts.parent_owner != me && facts.parent_owner != 0 {
        return Some("its folder belongs to another user");
    }
    if facts.parent_mode & 0o022 != 0 && facts.parent_mode & 0o1000 == 0 {
        return Some("its folder is writable by other users");
    }
    None
}

/// Why the process at the other end of a connected socket, run by `peer`, must not get a key.
pub fn peer_refusal(peer: u32, me: u32) -> Option<&'static str> {
    (peer != me).then_some("another user is at the other end")
}

mod sys {
    unsafe extern "C" {
        pub fn getuid() -> u32;
        #[cfg(target_os = "macos")]
        pub fn getpeereid(fd: i32, euid: *mut u32, egid: *mut u32) -> i32;
        #[cfg(target_os = "linux")]
        pub fn getsockopt(fd: i32, level: i32, name: i32, value: *mut std::ffi::c_void, len: *mut u32) -> i32;
    }
}

/// This process's user ID.
fn getuid() -> u32 {
    // SAFETY: getuid takes nothing and cannot fail.
    unsafe { sys::getuid() }
}

/// The user ID of the process at the other end of `stream`.
#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (u32::MAX, u32::MAX);
    // SAFETY: a valid socket fd and two writable u32s, as getpeereid(3) takes.
    let rc = unsafe { sys::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if rc == 0 { Ok(uid) } else { Err(std::io::Error::last_os_error()) }
}

/// The user ID of the process at the other end of `stream`.
#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct Ucred {
        pid: i32,
        uid: u32,
        gid: u32,
    }
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;
    let mut cred = Ucred { pid: 0, uid: u32::MAX, gid: u32::MAX };
    let mut len = std::mem::size_of::<Ucred>() as u32;
    // SAFETY: a valid socket fd, and a ucred-sized buffer with its length, as SO_PEERCRED takes.
    let rc = unsafe {
        sys::getsockopt(stream.as_raw_fd(), SOL_SOCKET, SO_PEERCRED, (&mut cred as *mut Ucred).cast(), &mut len)
    };
    if rc == 0 && len as usize == std::mem::size_of::<Ucred>() {
        Ok(cred.uid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Elsewhere the peer cannot be checked, so no key is sent.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peer_uid(_stream: &UnixStream) -> std::io::Result<u32> {
    Err(std::io::Error::other("cannot check a socket's peer on this system"))
}

/// Fails when the socket at `path` is not safe to send a key to (see `refusal`).
fn trusted(path: &Path, me: u32) -> Result<()> {
    let shown = path.display();
    let meta = std::fs::symlink_metadata(path).with_context(|| format!("cmux's socket {shown} is not there"))?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent_meta = std::fs::metadata(parent).with_context(|| format!("reading the folder of {shown}"))?;
    let facts = SocketFacts {
        symlink: meta.file_type().is_symlink(),
        socket: meta.file_type().is_socket(),
        owner: meta.uid(),
        parent_mode: parent_meta.mode(),
        parent_owner: parent_meta.uid(),
    };
    match refusal(facts, me) {
        None => Ok(()),
        Some(why) => bail!("not using cmux's socket {shown}: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 501;
    const OK: SocketFacts =
        SocketFacts { symlink: false, socket: true, owner: ME, parent_mode: 0o40700, parent_owner: ME };

    #[test]
    fn only_our_own_socket_in_a_safe_folder_is_trusted() {
        assert_eq!(refusal(OK, ME), None);
        // /tmp: writable by everyone but sticky.
        assert_eq!(refusal(SocketFacts { parent_mode: 0o41777, ..OK }, ME), None);
        assert_eq!(refusal(SocketFacts { parent_mode: 0o40755, ..OK }, ME), None);
        assert!(refusal(SocketFacts { owner: 502, ..OK }, ME).unwrap().contains("another user"));
        assert!(refusal(SocketFacts { symlink: true, ..OK }, ME).unwrap().contains("symlink"));
        assert!(refusal(SocketFacts { socket: false, ..OK }, ME).unwrap().contains("not a socket"));
        // A folder of root's (such as /tmp) is fine; one of another user's is not.
        assert_eq!(refusal(SocketFacts { parent_owner: 0, parent_mode: 0o41777, ..OK }, ME), None);
        let why = refusal(SocketFacts { parent_owner: 502, parent_mode: 0o40755, ..OK }, ME).unwrap();
        assert!(why.contains("folder belongs to another user"), "{why}");
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
        trusted(&sock, getuid()).unwrap();
        let link = dir.path().join("l");
        std::os::unix::fs::symlink(&sock, &link).unwrap();
        assert!(format!("{:#}", trusted(&link, getuid()).unwrap_err()).contains("symlink"));
        let file = dir.path().join("f");
        std::fs::write(&file, "").unwrap();
        assert!(format!("{:#}", trusted(&file, getuid()).unwrap_err()).contains("not a socket"));
        assert!(trusted(&dir.path().join("missing"), getuid()).is_err());
        // A refused path is never connected to.
        let err = format!("{:#}", call(&link, "browser.navigate", json!({})).unwrap_err());
        assert!(err.contains("not using cmux's socket"), "{err}");
    }

    /// cmux's answers: a result, a JSON error, a plain-text refusal, or none.
    #[test]
    fn reads_cmuxs_answers() {
        use std::io::Read;
        let dir = tempfile::Builder::new().prefix("ss").tempdir_in("/tmp").unwrap();
        let sock = dir.path().join("s");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let answers = [
            "{\"ok\":true,\"result\":{\"pong\":true}}\n",
            "{\"ok\":false,\"error\":{\"code\":\"invalid_params\",\"message\":\"Browser operation failed\"}}\n",
            "ERROR: Access denied - only processes started inside cmux can connect\n",
            "",
        ];
        let server = std::thread::spawn(move || {
            for answer in answers {
                let (mut s, _) = listener.accept().unwrap();
                let mut byte = [0u8; 1];
                while s.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {}
                s.write_all(answer.as_bytes()).unwrap();
            }
        });
        let ask = || {
            call(&sock, "browser.navigate", json!({"url": "http://127.0.0.1:1/#t=k3y"})).map_err(|e| format!("{e:#}"))
        };
        assert_eq!(ask(), Ok(json!({"pong": true})));
        assert_eq!(ask(), Err("cmux refused browser.navigate: invalid_params: Browser operation failed".into()));
        assert_eq!(
            ask(),
            Err("cmux refused browser.navigate: Access denied - only processes started inside cmux can connect".into())
        );
        let none = ask().unwrap_err();
        assert!(none.contains("closed without answering") && !none.contains("k3y"), "{none}");
        server.join().unwrap();
    }

    /// The process at the other end of a connected socket is checked by its user ID.
    #[test]
    fn checks_who_is_at_the_other_end() {
        assert_eq!(peer_refusal(ME, ME), None);
        assert!(peer_refusal(502, ME).unwrap().contains("another user"));
        let (a, _b) = UnixStream::pair().unwrap();
        assert_eq!(peer_uid(&a).unwrap(), getuid());
        // getuid agrees with the owner of a file this process creates.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        std::fs::write(&f, "").unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().uid(), getuid());
    }
}
