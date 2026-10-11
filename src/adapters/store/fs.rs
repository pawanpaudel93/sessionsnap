//! Snapshot versions and restore plans as JSON files under `~/.sessionsnap`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::domain::model::{FORMAT, Source, Version, Workspace, validate_name};
use crate::domain::plan::Plan;
use crate::error::{Exit, failure};
use crate::ports::Snapshots;
use crate::util::json::{read_json, write_json_atomic};

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
        Ok(Store::new(crate::adapters::home_dir()?.join(".sessionsnap")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn plans_dir(&self) -> PathBuf {
        self.root.join("plans")
    }

    /// Keeps a plan for `sessionsnap apply`, deleting plans older than an hour.
    pub fn save_plan(&self, plan: &Plan) -> Result<()> {
        let dir = self.plans_dir();
        fs::create_dir_all(&dir)?;
        for entry in fs::read_dir(&dir)?.flatten() {
            let expired = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|m| m.elapsed().unwrap_or_default() > Duration::from_secs(3600))
                .unwrap_or(false);
            if expired {
                let _ = fs::remove_file(entry.path());
            }
        }
        write_json_atomic(&dir.join(format!("{}.json", plan.plan_id)), plan)
    }

    pub fn load_plan(&self, id: &str) -> Result<Plan> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(failure(Exit::Error, format!("invalid plan id '{id}'")));
        }
        let path = self.plans_dir().join(format!("{id}.json"));
        if !path.is_file() {
            return Err(failure(Exit::NotFound, format!("plan {id} not found (plans expire after an hour)")));
        }
        read_json(&path)
    }

    /// Forgets an applied plan; a plan that is already gone is fine.
    pub fn remove_plan(&self, id: &str) {
        let _ = fs::remove_file(self.plans_dir().join(format!("{id}.json")));
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
        let path = self.version_path(name, v)?;
        let raw: serde_json::Value = read_json(&path)?;
        let format = raw.get("sessionsnap").and_then(serde_json::Value::as_u64);
        if format != Some(u64::from(FORMAT)) {
            let found = format.map_or("an unknown format".to_string(), |f| format!("format {f}"));
            return Err(failure(
                Exit::UnknownFormat,
                format!(
                    "snapshot '{name}' v{v} is in sessionsnap {found}; this sessionsnap reads format {FORMAT} only ({})",
                    path.display()
                ),
            ));
        }
        serde_json::from_value(raw).with_context(|| format!("parsing {}", path.display()))
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

fn parse_version_file(fname: &str) -> Option<u32> {
    let stem = fname.strip_suffix(".json")?;
    let (num, _) = stem.split_once('-')?;
    num.parse().ok()
}

impl Snapshots for Store {
    fn load(&self, name: &str, version: Option<u32>) -> Result<Version> {
        Store::load(self, name, version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Tab;
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
    fn format_1_files_are_refused_with_their_version() {
        let (_d, s) = store();
        s.save_new("work", "m", Source::default(), vec![ws("A")]).unwrap();
        let path = s.version_path("work", 1).unwrap();
        let v1 = r#"{"sessionsnap": 1, "snapshot": "work", "version": 1, "createdAt": "", "message": "m",
                     "source": {"kind": "live"}, "workspaces": [{"name": "A", "cwd": "/w", "tabs": [
                       {"type": "claude", "title": "t", "cwd": "/w", "sessionId": "11111111-1111-4111-8111-111111111111"}]}]}"#;
        fs::write(&path, v1).unwrap();
        let err = s.load("work", None).unwrap_err();
        assert_eq!(exit_of(&err), Exit::UnknownFormat);
        assert!(err.to_string().contains("format 1") && err.to_string().contains("format 2"), "{err}");
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
