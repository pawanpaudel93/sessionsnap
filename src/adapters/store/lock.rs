//! A lock file: one holder at a time, across processes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::error::{Exit, failure};

/// Held while the file at `path` is the one this lock made (same inode and same token inside);
/// removed when dropped, only if it still is. The token matters on Linux, which can give a new
/// lock file the inode of one just removed.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
    id: FileId,
    token: String,
}

/// A token no other lock file has: this process, the time, and a count.
fn new_token() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed))
}

/// The lock file at `path` as it is now: its identity and the token inside (None when gone).
fn current(path: &Path) -> Option<(FileId, String)> {
    let meta = fs::symlink_metadata(path).ok()?;
    Some((FileId::of(&meta), fs::read_to_string(path).unwrap_or_default()))
}

/// A lock this old was left by a holder that crashed, and is broken.
pub const STALE: Duration = Duration::from_secs(60);
/// How long to wait for a live lock before giving up.
const WAIT: Duration = Duration::from_secs(90);

impl Lock {
    /// Takes the lock at `path`, waiting while another holder has it.
    pub fn acquire(path: &Path) -> Result<Lock> {
        Self::acquire_within(path, STALE, WAIT)
    }

    fn acquire_within(path: &Path, stale: Duration, wait: Duration) -> Result<Lock> {
        let started = Instant::now();
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    use std::io::Write;
                    let token = new_token();
                    file.write_all(token.as_bytes()).with_context(|| format!("locking {}", path.display()))?;
                    let meta = file.metadata().with_context(|| format!("locking {}", path.display()))?;
                    return Ok(Lock { path: path.to_path_buf(), id: FileId::of(&meta), token });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // The path itself (a symlink is a lock too, never followed).
                    let meta = fs::symlink_metadata(path);
                    let old = meta
                        .as_ref()
                        .is_ok_and(|m| m.modified().is_ok_and(|m| m.elapsed().unwrap_or_default() > stale));
                    if let (true, Ok(meta)) = (old, &meta) {
                        break_stale(path, (FileId::of(meta), fs::read_to_string(path).unwrap_or_default()));
                    } else if started.elapsed() > wait {
                        return Err(failure(
                            Exit::Error,
                            format!("another sessionsnap holds {} (still busy); try again", path.display()),
                        ));
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("locking {}", path.display())),
            }
        }
    }

    /// True while the lock file at the path is still this one: a holder that ran past `STALE` may
    /// have had it broken and taken by another.
    pub fn still_held(&self) -> bool {
        current(&self.path).is_some_and(|(id, token)| id == self.id && token == self.token)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // A lock broken as stale may have a new holder by now: leave theirs alone.
        if self.still_held() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Breaks the stale lock `seen` at `path`: moves it aside under a name of its own, and removes it
/// only when it is the one that was seen. When another waiter broke it first and a new holder
/// took the path meanwhile, the new lock is moved back.
fn break_stale(path: &Path, seen: (FileId, String)) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let aside = path.with_extension(format!("stale-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    if fs::rename(path, &aside).is_err() {
        return;
    }
    if current(&aside).is_some_and(|now| now != seen) {
        // Not the stale one: give it back (unless yet another holder took the path).
        let _ = fs::hard_link(&aside, path);
    }
    let _ = fs::remove_file(&aside);
}

/// What tells two files apart: device and inode on Unix; elsewhere the modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId(u64, u64);

impl FileId {
    #[cfg(unix)]
    fn of(meta: &fs::Metadata) -> FileId {
        use std::os::unix::fs::MetadataExt;
        FileId(meta.dev(), meta.ino())
    }

    #[cfg(not(unix))]
    fn of(meta: &fs::Metadata) -> FileId {
        let nanos = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        FileId(nanos, meta.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, by: Duration) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(std::time::SystemTime::now() - by).unwrap();
    }

    #[test]
    fn one_holder_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        let lock = Lock::acquire(&path).unwrap();
        let err = Lock::acquire_within(&path, STALE, Duration::from_millis(30)).unwrap_err();
        assert!(err.to_string().contains("still busy"), "{err}");
        drop(lock);
        assert!(!path.exists());
        Lock::acquire(&path).unwrap();
    }

    /// A lock path that is a symlink to nothing is a foreign lock: waited for, then given up on
    /// (never a busy loop); once stale, it is broken.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_is_waited_for_then_broken() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &path).unwrap();
        let (done, wait) = std::sync::mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || done.send(Lock::acquire_within(&p, STALE, Duration::from_millis(50)).is_err()));
        assert_eq!(wait.recv_timeout(Duration::from_secs(5)), Ok(true), "gave up within the wait");
        // Stale (as a link, by its own age): broken and taken.
        let (done, wait) = std::sync::mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            done.send(Lock::acquire_within(&p, Duration::ZERO, Duration::from_millis(50)).map(drop).is_ok())
        });
        assert_eq!(wait.recv_timeout(Duration::from_secs(5)), Ok(true));
    }

    /// A crashed holder's lock is broken; a live one is not, however long the wait.
    #[test]
    fn only_a_stale_lock_is_broken() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        fs::write(&path, "").unwrap();
        age(&path, Duration::from_secs(120));
        let lock = Lock::acquire(&path).unwrap();
        assert!(path.exists());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "the stale lock was moved aside and removed");
        drop(lock);
        let _live = Lock::acquire(&path).unwrap();
        assert!(Lock::acquire_within(&path, STALE, Duration::from_millis(30)).is_err());
    }

    /// A holder whose lock was broken (it ran too long) never removes the next holder's lock.
    #[test]
    fn drop_leaves_another_holders_lock() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        let first = Lock::acquire(&path).unwrap();
        age(&path, Duration::from_secs(120));
        let second = Lock::acquire(&path).unwrap();
        drop(first);
        assert!(path.exists(), "the second holder's lock is still there");
        drop(second);
        assert!(!path.exists());
    }

    /// Breaking a lock that a new holder took meanwhile gives it back.
    #[test]
    fn a_fresh_lock_moved_aside_is_put_back() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        let holder = Lock::acquire(&path).unwrap();
        // A waiter saw some earlier lock as stale, but the path now holds `holder`'s.
        break_stale(&path, (FileId(0, 0), String::new()));
        assert!(holder.still_held(), "put back as it was");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }

    /// Linux can give a new lock file the inode of one just removed: the token inside still tells
    /// the old holder the file is no longer its own, so it never removes the new holder's lock.
    #[test]
    fn the_same_inode_with_another_token_is_not_ours() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.lock");
        let first = Lock::acquire(&path).unwrap();
        // What a new holder on a reused inode looks like: same file, another token.
        fs::write(&path, "another-holder").unwrap();
        assert!(!first.still_held());
        drop(first);
        assert_eq!(fs::read_to_string(&path).unwrap(), "another-holder", "left for its holder");
    }
}
