//! Reading and atomically writing JSON files.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Writes JSON to a temporary file next to `path`, syncs it to disk, then renames it into place.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, value, false)
}

/// Like `write_json_atomic`, but the file is readable by its owner only (0600 on Unix).
pub fn write_private_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, value, true)
}

fn write_atomic<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    // One left by a crashed run with this pid would keep its permissions: start a new file.
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    let written = (|| -> Result<()> {
        file.write_all((serde_json::to_string_pretty(value)? + "\n").as_bytes())
            .and_then(|()| file.sync_all())
            .with_context(|| format!("writing {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    // The new name is in the folder: sync it too, where the system allows.
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Creates `dir` if needed and makes it accessible to its owner only (0700 on Unix).
pub fn make_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary file left by a crashed run with the same pid can't keep its wider permissions.
    #[cfg(unix)]
    #[test]
    fn a_private_write_is_private_even_over_a_stale_tmp() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.json");
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, "stale").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
        write_private_json_atomic(&path, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!tmp.exists());
        assert_eq!(read_json::<serde_json::Value>(&path).unwrap()["a"], 1);
    }

    /// A write that fails leaves no temporary file behind.
    #[test]
    fn a_failed_write_removes_its_tmp() {
        let d = tempfile::tempdir().unwrap();
        // A folder where the file goes: the rename into place fails.
        let path = d.path().join("a.json");
        fs::create_dir_all(path.join("x")).unwrap();
        assert!(write_json_atomic(&path, &1).is_err());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "only the folder is left");
    }
}
