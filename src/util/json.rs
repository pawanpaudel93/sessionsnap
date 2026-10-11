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

/// Writes JSON to a temporary file next to `path`, then renames it into place.
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
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    file.write_all((serde_json::to_string_pretty(value)? + "\n").as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
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
