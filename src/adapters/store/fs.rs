//! Snapshot versions and restore plans as JSON files under `~/.sessionsnap`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::domain::model::{FORMAT, Source, Version, Workspace, validate_name};
use crate::domain::plan::Plan;
use crate::error::{Exit, failure};
use crate::ports::Snapshots;
use crate::util::json::{make_private_dir, read_json, write_private_json_atomic};

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
        make_private_dir(&self.root)?;
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
        write_private_json_atomic(&dir.join(format!("{}.json", plan.plan_id)), plan)
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

    /// The folder of snapshot `name`, under the name it is stored as (see `stored_name`).
    fn snapshot_dir(&self, name: &str) -> PathBuf {
        self.snapshots_dir().join(self.stored_name(name))
    }

    /// The name snapshot `name` is stored under. Names ignore case: "Main" is an existing "main".
    /// Found in the folder listing, so it is the same whether or not the file system ignores case
    /// (macOS by default, but not case-sensitive APFS or Linux); every lookup goes through it.
    pub fn stored_name(&self, name: &str) -> String {
        let Ok(entries) = fs::read_dir(self.snapshots_dir()) else { return name.to_string() };
        let folders: Vec<(String, bool)> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.eq_ignore_ascii_case(name))
            .map(|n| {
                let has_versions = fs::read_dir(self.snapshots_dir().join(&n))
                    .is_ok_and(|d| d.flatten().any(|f| parse_version_file(&f.file_name().to_string_lossy()).is_some()));
                (n, has_versions)
            })
            .collect();
        resolve_name(&folders, name).to_string()
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
        match self.read_version(name, v)? {
            Ok(version) => Ok(version),
            Err(why) => {
                let path = self.version_path(name, v)?;
                let exit = if why.starts_with("format") { Exit::UnknownFormat } else { Exit::Error };
                Err(failure(exit, format!("snapshot '{name}' v{v} is unreadable: {why} ({})", path.display())))
            }
        }
    }

    /// Reads version `v` of `name`. The inner error says why a file that is there cannot be read as
    /// a version of this format ("format 1, expected 2", "not valid JSON: …"); the outer one is for
    /// a missing snapshot or version, or a file that cannot be opened.
    pub fn read_version(&self, name: &str, v: u32) -> Result<std::result::Result<Version, String>> {
        let path = self.version_path(name, v)?;
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(raw) => raw,
            Err(e) => return Ok(Err(format!("not valid JSON: {e}"))),
        };
        let format = raw.get("sessionsnap").and_then(serde_json::Value::as_u64);
        if format != Some(u64::from(FORMAT)) {
            let found = format.map_or("an unknown format".to_string(), |f| format!("format {f}"));
            return Ok(Err(format!("{found}, expected {FORMAT}")));
        }
        Ok(serde_json::from_value(raw).map_err(|e| format!("not a sessionsnap version: {e}")))
    }

    /// Writes the next version of `name` and returns it. Saves of one snapshot take turns (a lock
    /// file in its folder), and each publishes its file with an exclusive create under a number no
    /// other file has, so saves at the same moment never overwrite each other or share a number.
    pub fn save_new(&self, name: &str, message: &str, source: Source, workspaces: Vec<Workspace>) -> Result<Version> {
        validate_name(name)?;
        // Names ignore case: "Main" adds to an existing "main".
        let name = &self.stored_name(name);
        make_private_dir(&self.root)?;
        let dir = self.snapshot_dir(name);
        in_new_dir(&dir, || {
            let _lock = SaveLock::acquire(&dir)?;
            let now = chrono::Local::now();
            let mut version = Version {
                sessionsnap: FORMAT,
                snapshot: name.to_string(),
                version: self.versions(name)?.last().copied().unwrap_or(0) + 1,
                created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                message: message.to_string(),
                source,
                workspaces,
            };
            publish(&dir, &mut version, &now.format("%Y%m%dT%H%M%S").to_string())?;
            Ok(version)
        })
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

/// `name` as one of the `stored` folders (name, holds versions) that differ from it only in case:
/// one with versions first, then `name` itself, then the first in sorted order, so the answer does
/// not depend on listing order; `name` when there is none (a new snapshot).
fn resolve_name<'a>(stored: &'a [(String, bool)], name: &'a str) -> &'a str {
    stored
        .iter()
        .filter(|(s, _)| s.eq_ignore_ascii_case(name))
        .min_by_key(|(s, has_versions)| (!has_versions, s != name, s.as_str()))
        .map_or(name, |(s, _)| s.as_str())
}

/// Runs `f` with `dir` created; removes `dir` again when this made it and `f` failed, so a failed
/// save leaves no empty folder behind.
fn in_new_dir<T>(dir: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let made = !dir.exists();
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = f();
    if made && result.is_err() {
        let _ = fs::remove_dir(dir);
    }
    result
}

/// Publishes `version` in `dir` as `<NNNN>-<stamp>.json`, from its number up: a number any file
/// already has (whatever its time suffix) is skipped, and the file is linked into place, which
/// fails rather than replace one that appeared meanwhile; it is synced to disk first, so a crash
/// never leaves an empty latest version. Sets the number it got.
fn publish(dir: &Path, version: &mut Version, stamp: &str) -> Result<()> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let taken = |n: u32| -> Result<bool> {
        Ok(fs::read_dir(dir)?.flatten().any(|e| parse_version_file(&e.file_name().to_string_lossy()) == Some(n)))
    };
    loop {
        let n = version.version;
        if taken(n)? {
            version.version += 1;
            continue;
        }
        // Not a version file (no `.json` end), and unique to this process and call.
        let tmp =
            dir.join(format!(".{n:04}-{stamp}.{}-{}.tmp", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
        let file = dir.join(format!("{n:04}-{stamp}.json"));
        let linked = write_synced(&tmp, version).map(|()| fs::hard_link(&tmp, &file));
        let _ = fs::remove_file(&tmp);
        match linked? {
            Ok(()) => {
                sync_dir(dir);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => version.version += 1,
            Err(e) => return Err(e).with_context(|| format!("writing {}", file.display())),
        }
    }
}

/// Writes `value` as JSON to a new file readable by its owner only, and syncs it to disk.
fn write_synced(path: &Path, value: &Version) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("writing {}", path.display()))?;
    file.write_all((serde_json::to_string_pretty(value)? + "\n").as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", path.display()))
}

/// Syncs a folder's entries to disk where the system allows it (a new file's name is in its
/// folder, not in the file).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// One save at a time per snapshot folder: `.save.lock`, made with an exclusive create and removed
/// when dropped. A lock older than `STALE` was left by a save that crashed, and is broken.
struct SaveLock(PathBuf);

impl SaveLock {
    const STALE: Duration = Duration::from_secs(30);

    fn acquire(dir: &Path) -> Result<SaveLock> {
        let path = dir.join(".save.lock");
        let started = std::time::Instant::now();
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(SaveLock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .is_ok_and(|m| m.elapsed().unwrap_or_default() > Self::STALE);
                    if stale || started.elapsed() > Self::STALE {
                        let _ = fs::remove_file(&path);
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("locking {}", path.display())),
            }
        }
    }
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
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

    /// macOS folders ignore case, so "Main" would land in "main"'s folder: keep the existing name.
    #[test]
    fn saving_under_another_case_keeps_the_existing_name() {
        let (_d, s) = store();
        s.save_new("main", "m", Source::default(), vec![ws("A")]).unwrap();
        let v = s.save_new("Main", "m", Source::default(), vec![ws("A")]).unwrap();
        assert_eq!((v.snapshot.as_str(), v.version), ("main", 2));
        assert_eq!(s.list().unwrap(), vec!["main".to_string()]);
    }

    /// Lookups go by the folder listing, not by the file system's case rules, so a case-sensitive
    /// volume (Linux, case-sensitive APFS) finds "main" for "Main" too.
    #[test]
    fn names_resolve_ignoring_case_by_the_listing() {
        let stored: Vec<(String, bool)> = ["main", "Work", "work"].iter().map(|n| (n.to_string(), true)).collect();
        assert_eq!(resolve_name(&stored, "main"), "main");
        assert_eq!(resolve_name(&stored, "Main"), "main");
        assert_eq!(resolve_name(&stored, "MAIN"), "main");
        // Two folders that differ only in case (possible on a case-sensitive volume): exact wins.
        assert_eq!(resolve_name(&stored, "work"), "work");
        assert_eq!(resolve_name(&stored, "Work"), "Work");
        assert_eq!(resolve_name(&stored, "new"), "new");

        let (_d, s) = store();
        s.save_new("main", "m", Source::default(), vec![ws("A")]).unwrap();
        assert_eq!(s.stored_name("MAIN"), "main");
        assert!(s.exists("Main"));
        assert_eq!(s.versions("mAiN").unwrap(), vec![1]);
        assert_eq!(s.load("Main", None).unwrap().snapshot, "main");
        assert_eq!(s.prune("MAIN", 1).unwrap(), Vec::<u32>::new());
        s.delete("Main").unwrap();
        assert!(s.list().unwrap().is_empty());
    }

    /// Folders that differ only in case resolve the same whatever the listing order: the one with
    /// versions first, then an exact match, then the first in sorted order.
    #[test]
    fn colliding_names_resolve_deterministically() {
        let both = |a: (&str, bool), b: (&str, bool)| {
            let one = vec![(a.0.to_string(), a.1), (b.0.to_string(), b.1)];
            let other: Vec<(String, bool)> = one.iter().rev().cloned().collect();
            (one, other)
        };
        // An empty "Work" (left by a failed save) loses to "work", which has versions.
        let (one, other) = both(("Work", false), ("work", true));
        for stored in [&one, &other] {
            assert_eq!(resolve_name(stored, "Work"), "work");
            assert_eq!(resolve_name(stored, "WORK"), "work");
        }
        // Both with versions: the exact name, else the first in sorted order.
        let (one, other) = both(("work", true), ("Work", true));
        for stored in [&one, &other] {
            assert_eq!(resolve_name(stored, "work"), "work");
            assert_eq!(resolve_name(stored, "WORK"), "Work");
        }
    }

    /// A save that fails leaves no empty folder behind to shadow the name later.
    #[test]
    fn a_failed_write_removes_the_folder_it_made() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("snapshots").join("new");
        let r: Result<()> = in_new_dir(&dir, || anyhow::bail!("disk full"));
        assert!(r.is_err());
        assert!(!dir.exists());
        // A folder that was already there is left alone.
        fs::create_dir_all(&dir).unwrap();
        let r: Result<()> = in_new_dir(&dir, || anyhow::bail!("disk full"));
        assert!(r.is_err());
        assert!(dir.is_dir());
        assert_eq!(in_new_dir(&d.path().join("ok"), || Ok(7)).unwrap(), 7);
    }

    /// Snapshots hold launch flags and session IDs: only the owner may read them.
    #[cfg(unix)]
    #[test]
    fn snapshot_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store();
        fs::create_dir_all(s.root()).unwrap();
        fs::set_permissions(s.root(), fs::Permissions::from_mode(0o755)).unwrap();
        s.save_new("work", "m", Source::default(), vec![ws("A")]).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(s.root()), 0o700, "an existing store folder is made private");
        let file = fs::read_dir(s.snapshot_dir("work")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(mode(&file), 0o600);
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

    /// Saves of the same snapshot at the same moment each get their own number: none is lost and
    /// no two files share one.
    #[test]
    fn concurrent_saves_keep_every_version() {
        let (d, _) = store();
        let (threads, each) = (4, 10);
        std::thread::scope(|scope| {
            for t in 0..threads {
                let root = d.path().to_path_buf();
                scope.spawn(move || {
                    let s = Store::new(root);
                    for i in 0..each {
                        s.save_new("work", &format!("{t}-{i}"), Source::default(), vec![ws("A")]).unwrap();
                    }
                });
            }
        });
        let s = Store::new(d.path());
        let total = threads * each;
        assert_eq!(s.versions("work").unwrap(), (1..=total as u32).collect::<Vec<_>>());
        let files: Vec<String> = fs::read_dir(s.snapshot_dir("work"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.iter().filter(|f| parse_version_file(f).is_some()).count(), total, "{files:?}");
        assert!(files.iter().all(|f| f.ends_with(".json")), "temporary files left: {files:?}");
        let mut messages: Vec<String> = (1..=total as u32).map(|v| s.load("work", Some(v)).unwrap().message).collect();
        messages.sort();
        messages.dedup();
        assert_eq!(messages.len(), total);
        for v in 1..=total as u32 {
            assert_eq!(s.load("work", Some(v)).unwrap().version, v);
        }
    }

    /// A lock left by a save that crashed doesn't block saves for good.
    #[test]
    fn a_stale_lock_is_broken() {
        let (_d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let lock = s.snapshot_dir("work").join(".save.lock");
        let f = fs::File::create(&lock).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(60)).unwrap();
        assert_eq!(s.save_new("work", "b", Source::default(), vec![]).unwrap().version, 2);
        assert!(!lock.exists());
    }

    /// A number is taken by any file that has it, whatever its time suffix; publishing skips it.
    #[test]
    fn numbers_ignore_the_time_suffix() {
        let (_d, s) = store();
        let v1 = s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let dir = s.snapshot_dir("work");
        // Another save published 2 and 3 after this one counted the versions.
        fs::write(dir.join("0002-20000101T000000.json"), "{}").unwrap();
        fs::write(dir.join("0003-20000101T000001.json"), "{}").unwrap();
        let mut v = Version { version: 2, ..v1 };
        publish(&dir, &mut v, "20991231T235959").unwrap();
        assert_eq!(v.version, 4);
        assert_eq!(s.versions("work").unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(s.load("work", Some(4)).unwrap().version, 4);
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
        assert!(err.to_string().contains("unreadable: format 1, expected 2"), "{err}");
        assert_eq!(s.read_version("work", 1).unwrap(), Err("format 1, expected 2".to_string()));
        fs::write(&path, "{").unwrap();
        assert!(s.read_version("work", 1).unwrap().unwrap_err().starts_with("not valid JSON"));
        assert_eq!(exit_of(&s.load("work", Some(1)).unwrap_err()), Exit::Error);
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
