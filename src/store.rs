use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

use crate::core::model::{FORMAT, Source, Version, Workspace};
use crate::error::{Exit, failure};

/// Snapshot versions live in `<root>/snapshots/<name>/<NNNN>-<timestamp>.json`; restore plans in
/// `<root>/plans/<id>.json`.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    /// `$SESSIONSNAP_HOME`, else `~/.sessionsnap`.
    pub fn open_default() -> Result<Self> {
        if let Some(p) = std::env::var_os("SESSIONSNAP_HOME") {
            return Ok(Store::new(p));
        }
        Ok(Store::new(home_dir()?.join(".sessionsnap")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn plans_dir(&self) -> PathBuf {
        self.root.join("plans")
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.root.join("snapshots")
    }

    fn snapshot_dir(&self, name: &str) -> PathBuf {
        self.snapshots_dir().join(name)
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let dir = self.snapshots_dir();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut names = vec![];
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if validate_name(&name).is_ok() && self.versions(&name).is_ok_and(|v| !v.is_empty()) {
                    names.push(name);
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn exists(&self, name: &str) -> bool {
        validate_name(name).is_ok() && self.snapshot_dir(name).is_dir()
    }

    /// Version numbers of `name`, ascending.
    pub fn versions(&self, name: &str) -> Result<Vec<u32>> {
        validate_name(name)?;
        let dir = self.snapshot_dir(name);
        if !dir.is_dir() {
            return Err(failure(Exit::NotFound, format!("snapshot '{name}' not found")));
        }
        let mut out = vec![];
        for entry in fs::read_dir(&dir)? {
            if let Some(v) = parse_version_file(&entry?.file_name().to_string_lossy()) {
                out.push(v);
            }
        }
        out.sort();
        Ok(out)
    }

    fn version_path(&self, name: &str, version: u32) -> Result<PathBuf> {
        let dir = self.snapshot_dir(name);
        for entry in fs::read_dir(&dir)? {
            let fname = entry?.file_name().to_string_lossy().into_owned();
            if parse_version_file(&fname) == Some(version) {
                return Ok(dir.join(fname));
            }
        }
        Err(failure(Exit::NotFound, format!("snapshot '{name}' has no version {version}")))
    }

    /// Loads `version` of `name`, or the latest when `version` is None.
    pub fn load(&self, name: &str, version: Option<u32>) -> Result<Version> {
        let versions = self.versions(name)?;
        let v = match version {
            Some(v) => v,
            None => {
                *versions.last().ok_or_else(|| failure(Exit::NotFound, format!("snapshot '{name}' has no versions")))?
            }
        };
        read_json(&self.version_path(name, v)?)
    }

    /// Writes the next version of `name` and returns it.
    pub fn save_new(&self, name: &str, message: &str, source: Source, workspaces: Vec<Workspace>) -> Result<Version> {
        validate_name(name)?;
        let dir = self.snapshot_dir(name);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let next = self.versions(name)?.last().copied().unwrap_or(0) + 1;
        let now = chrono::Local::now();
        let version = Version {
            sessionsnap: FORMAT,
            snapshot: name.to_string(),
            version: next,
            created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            message: message.to_string(),
            source,
            workspaces,
        };
        let file = dir.join(format!("{next:04}-{}.json", now.format("%Y%m%dT%H%M%S")));
        write_json_atomic(&file, &version)?;
        Ok(version)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        self.versions(name)?;
        fs::remove_dir_all(self.snapshot_dir(name))?;
        Ok(())
    }

    /// Deletes all but the newest `keep` versions; returns the deleted version numbers.
    pub fn prune(&self, name: &str, keep: usize) -> Result<Vec<u32>> {
        if keep == 0 {
            return Err(failure(Exit::Error, "--keep must be at least 1"));
        }
        let versions = self.versions(name)?;
        let doomed = versions[..versions.len().saturating_sub(keep)].to_vec();
        for v in &doomed {
            fs::remove_file(self.version_path(name, *v)?)?;
        }
        Ok(doomed)
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(failure(Exit::Error, format!("invalid snapshot name '{name}': use letters, digits, '.', '_' or '-'")))
    }
}

/// "work" → ("work", None); "work@3" → ("work", Some(3)); "work@latest" → ("work", None).
pub fn parse_ref(s: &str) -> Result<(String, Option<u32>)> {
    let (name, v) = match s.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (s, None),
    };
    validate_name(name)?;
    let version = match v {
        None | Some("latest") => None,
        Some(v) => Some(v.parse::<u32>().map_err(|_| failure(Exit::Error, format!("invalid version '{v}' in '{s}'")))?),
    };
    Ok((name.to_string(), version))
}

fn parse_version_file(fname: &str) -> Option<u32> {
    let stem = fname.strip_suffix(".json")?;
    let (num, _) = stem.split_once('-')?;
    num.parse().ok()
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| failure(Exit::Error, "HOME is not set"))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Writes JSON to a temporary file next to `path`, then renames it into place.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Tab;
    use crate::error::exit_of;

    fn ws(name: &str) -> Workspace {
        Workspace {
            name: name.into(),
            color: None,
            cwd: "/w".into(),
            git_branch: None,
            cmux_workspace_id: None,
            tabs: vec![Tab::Terminal { title: "sh".into(), cwd: "/w".into(), command: None }],
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        (d, s)
    }

    #[test]
    fn versions_increment_and_load() {
        let (_d, s) = store();
        let v1 = s.save_new("work", "first", Source::default(), vec![ws("A")]).unwrap();
        let v2 = s.save_new("work", "second", Source::default(), vec![ws("A"), ws("B")]).unwrap();
        assert_eq!((v1.version, v2.version), (1, 2));
        assert_eq!(s.versions("work").unwrap(), vec![1, 2]);
        assert_eq!(s.load("work", None).unwrap().workspaces.len(), 2);
        assert_eq!(s.load("work", Some(1)).unwrap().message, "first");
        assert_eq!(s.list().unwrap(), vec!["work".to_string()]);
        assert!(s.exists("work"));
    }

    #[test]
    fn list_ignores_empty_and_invalid_directories() {
        let (_d, s) = store();
        s.save_new("work", "m", Source::default(), vec![ws("A")]).unwrap();
        fs::create_dir_all(s.snapshots_dir().join("empty")).unwrap();
        fs::create_dir_all(s.snapshots_dir().join("bad name")).unwrap();
        fs::write(s.snapshots_dir().join("bad name").join("v1.json"), "{}").unwrap();
        assert_eq!(s.list().unwrap(), vec!["work".to_string()]);
    }

    #[test]
    fn missing_snapshot_and_version_are_not_found() {
        let (_d, s) = store();
        assert_eq!(exit_of(&s.load("nope", None).unwrap_err()), Exit::NotFound);
        s.save_new("work", "m", Source::default(), vec![]).unwrap();
        assert_eq!(exit_of(&s.load("work", Some(9)).unwrap_err()), Exit::NotFound);
    }

    #[test]
    fn refs_and_names() {
        assert_eq!(parse_ref("work").unwrap(), ("work".to_string(), None));
        assert_eq!(parse_ref("work@3").unwrap(), ("work".to_string(), Some(3)));
        assert_eq!(parse_ref("work@latest").unwrap(), ("work".to_string(), None));
        assert!(parse_ref("work@x").is_err());
        assert!(validate_name("../etc").is_err());
        assert!(validate_name("my-day_2.0").is_ok());
    }

    #[test]
    fn prune_keeps_newest_and_delete_removes_all() {
        let (_d, s) = store();
        for m in ["a", "b", "c"] {
            s.save_new("work", m, Source::default(), vec![]).unwrap();
        }
        assert_eq!(s.prune("work", 1).unwrap(), vec![1, 2]);
        assert_eq!(s.versions("work").unwrap(), vec![3]);
        s.delete("work").unwrap();
        assert!(s.list().unwrap().is_empty());
    }
}
