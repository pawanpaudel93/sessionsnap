//! The process table, for terminals that don't say which agent runs in a tab (tmux): each
//! process's parent, when it started, and its arguments exactly as they were passed (from
//! `/proc/<pid>/cmdline` on Linux, `sysctl kern.procargs2` on macOS; never from `ps`, which joins
//! them with spaces).

use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};

use super::cmux::runner::Runner;

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub started: SystemTime,
}

/// What a terminal adapter needs from the process table; faked in tests.
pub trait Processes {
    /// Every process.
    fn list(&self) -> Result<Vec<Proc>>;
    /// Process `pid`'s arguments, program first; None when they can't be read (gone, not ours).
    fn argv(&self, pid: u32) -> Option<Vec<String>>;
}

/// The real process table: `ps` for parents and ages, the OS for arguments.
pub struct System<R: Runner> {
    pub runner: R,
}

impl<R: Runner> Processes for System<R> {
    fn list(&self) -> Result<Vec<Proc>> {
        let args: Vec<String> = ["-A", "-o", "pid=", "-o", "ppid=", "-o", "etime="].map(String::from).to_vec();
        let out = self.runner.run_timeout("ps", &args, std::time::Duration::from_secs(15))?;
        if !out.success {
            bail!("`ps` failed: {}", out.stderr.trim());
        }
        Ok(parse_ps(&out.stdout, SystemTime::now()))
    }

    fn argv(&self, pid: u32) -> Option<Vec<String>> {
        argv_of(pid)
    }
}

/// `ps -o pid= -o ppid= -o etime=` lines; lines that don't parse are skipped.
pub fn parse_ps(text: &str, now: SystemTime) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let pid = f.next()?.parse().ok()?;
            let ppid = f.next()?.parse().ok()?;
            let age = parse_etime(f.next()?)?;
            Some(Proc { pid, ppid, started: now.checked_sub(Duration::from_secs(age))? })
        })
        .collect()
}

/// Seconds in `ps`'s elapsed time: `[[dd-]hh:]mm:ss`.
pub fn parse_etime(text: &str) -> Option<u64> {
    let (days, clock) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let parts: Vec<u64> = clock.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = match parts[..] {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

/// NUL-terminated words, as `/proc/<pid>/cmdline` holds them. An empty argument stays: dropping
/// it would shift the values after it onto other flags.
pub fn split_nul(bytes: &[u8]) -> Vec<String> {
    let Some(words) = bytes.strip_suffix(&[0]) else { return vec![] };
    words.split(|&b| b == 0).map(|w| String::from_utf8_lossy(w).into_owned()).collect()
}

#[cfg(target_os = "linux")]
fn argv_of(pid: u32) -> Option<Vec<String>> {
    let words = split_nul(&std::fs::read(format!("/proc/{pid}/cmdline")).ok()?);
    (!words.is_empty()).then_some(words)
}

#[cfg(target_os = "macos")]
fn argv_of(pid: u32) -> Option<Vec<String>> {
    procargs2(&macos::procargs2(pid)?)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn argv_of(_pid: u32) -> Option<Vec<String>> {
    None
}

/// The arguments in a `kern.procargs2` buffer: argc (a native-endian i32), the executable's path,
/// NUL padding, then argc NUL-terminated arguments (the environment follows; it is never read).
pub fn procargs2(buf: &[u8]) -> Option<Vec<String>> {
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let rest = &buf[4..];
    let path_end = rest.iter().position(|&b| b == 0)?;
    let start = path_end + rest[path_end..].iter().position(|&b| b != 0)?;
    let words: Vec<String> =
        rest[start..].split(|&b| b == 0).take(argc).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    (argc > 0 && words.len() == argc).then_some(words)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;

    const CTL_KERN: i32 = 1;
    const KERN_ARGMAX: i32 = 8;
    const KERN_PROCARGS2: i32 = 49;

    unsafe extern "C" {
        fn sysctl(
            name: *mut i32,
            namelen: u32,
            old: *mut c_void,
            oldlen: *mut usize,
            new: *mut c_void,
            newlen: usize,
        ) -> i32;
    }

    /// The raw `kern.procargs2` buffer of `pid`.
    pub fn procargs2(pid: u32) -> Option<Vec<u8>> {
        let mut argmax: i32 = 0;
        let mut len = std::mem::size_of::<i32>();
        let mut mib = [CTL_KERN, KERN_ARGMAX];
        // SAFETY: `mib` names an int, written into `argmax` whose size `len` holds.
        let ok = unsafe { sysctl(mib.as_mut_ptr(), 2, (&raw mut argmax).cast(), &mut len, std::ptr::null_mut(), 0) };
        if ok != 0 || argmax <= 0 {
            return None;
        }
        let mut buf = vec![0u8; usize::try_from(argmax).ok()?];
        let mut len = buf.len();
        let mut mib = [CTL_KERN, KERN_PROCARGS2, i32::try_from(pid).ok()?];
        // SAFETY: the kernel writes at most `len` bytes into `buf`, and sets `len` to how many.
        let ok = unsafe { sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
        (ok == 0).then(|| {
            buf.truncate(len);
            buf
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ps_ages_in_every_form() {
        assert_eq!(parse_etime("05:07"), Some(307));
        assert_eq!(parse_etime("02:05:07"), Some(7507));
        assert_eq!(parse_etime("3-02:05:07"), Some(3 * 86_400 + 7507));
        assert_eq!(parse_etime("x"), None);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let procs = parse_ps("  10     1   01:40\n bad line\n 11 10 00:05\n", now);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0], Proc { pid: 10, ppid: 1, started: now - Duration::from_secs(100) });
    }

    #[test]
    fn reads_exact_arguments() {
        assert_eq!(split_nul(b"claude\0--add-dir\0/a b\0"), ["claude", "--add-dir", "/a b"]);
        // `--model "$UNSET" --permission-mode plan`: the empty value keeps its place.
        let empty = split_nul(b"claude\0--model\0\0--permission-mode\0plan\0");
        assert_eq!(empty, ["claude", "--model", "", "--permission-mode", "plan"]);
        assert!(split_nul(b"").is_empty());
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/bin/claude\0\0\0claude\0--add-dir\0/a b\0HOME=/x\0");
        assert_eq!(procargs2(&buf).unwrap(), ["claude", "--add-dir", "/a b"]);
        assert_eq!(procargs2(&buf[..10]), None);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn reads_this_process_arguments() {
        let argv = argv_of(std::process::id()).unwrap();
        assert_eq!(argv, std::env::args().collect::<Vec<_>>());
    }
}
