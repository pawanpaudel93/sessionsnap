//! A lock file: one holder at a time, across processes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::error::{Exit, failure};

/// Held while the file at `path` is the one this lock made (same inode); removed when dropped,
/// only if it still is.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
    id: FileId,
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
                Ok(file) => {
                    let meta = file.metadata().with_context(|| format!("locking {}", path.display()))?;
                    return Ok(Lock { path: path.to_path_buf(), id: FileId::of(&meta) });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let Ok(meta) = fs::metadata(path) else { continue };
                    let old = meta.modified().is_ok_and(|m| m.elapsed().unwrap_or_default() > stale);
                    if old {
                        break_stale(path, FileId::of(&meta));
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

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // A lock broken as stale may have a new holder by now: leave theirs alone.
        if fs::metadata(&self.path).is_ok_and(|m| FileId::of(&m) == self.id) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Breaks the stale lock `seen` at `path`: moves it aside under a name of its own, and removes it
/// only when it is the one that was seen. When another waiter broke it first and a new holder
/// took the path meanwhile, the new lock is moved back.
fn break_stale(path: &Path, seen: FileId) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let aside = path.with_extension(format!("stale-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
    if fs::rename(path, &aside).is_err() {
        return;
    }
    if fs::metadata(&aside).is_ok_and(|m| FileId::of(&m) != seen) {
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
        break_stale(&path, FileId(0, 0));
        assert!(fs::metadata(&path).is_ok_and(|m| FileId::of(&m) == holder.id));
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
}
