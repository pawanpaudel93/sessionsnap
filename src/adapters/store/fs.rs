//! Snapshot versions and restore plans as JSON files under `~/.sessionsnap`.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};

use super::lock::{Lock, STALE};
use crate::domain::known::KnownSessions;
use crate::domain::model::{FORMAT, Source, Version, Workspace, parse_version, validate_name, validate_new_name};
use crate::domain::plan::Plan;
use crate::domain::schedule::Schedules;
use crate::error::{Exit, failure};
use crate::ports::Snapshots;
use crate::util::json::{make_private_dir, read_json, write_private_json_atomic};

/// Snapshot versions live in `<root>/snapshots/<name>/<NNNN>-<timestamp>.json`; restore plans in
/// `<root>/plans/<id>.json`; automatic snapshots' schedules in `<root>/schedules.json`.
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

    /// True when snapshot `name` has a version: an empty folder left behind is not a snapshot.
    pub fn has_versions(&self, name: &str) -> bool {
        self.versions(name).is_ok_and(|v| !v.is_empty())
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
        // A renamed snapshot's older versions still hold the old name: the folder's name wins.
        let stored = self.stored_name(name);
        Ok(parse_version(&text).map(|v| Version { snapshot: stored, ..v }))
    }

    /// The lock of snapshot `name` (any case): saves, edits, prunes and deletes of one snapshot
    /// take turns. Held from reading the latest version to publishing the next, so nothing lands
    /// in between. Lives in `<root>/locks`, so taking it never creates the snapshot's folder.
    pub fn lock(&self, name: &str) -> Result<Lock> {
        validate_name(name)?;
        self.lock_file(&format!("snapshot-{}.lock", name.to_ascii_lowercase()))
    }

    /// A lock file `file` in `<root>/locks`.
    pub(crate) fn lock_file(&self, file: &str) -> Result<Lock> {
        make_private_dir(&self.root)?;
        let dir = self.root.join("locks");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Lock::acquire(&dir.join(file))
    }

    /// The lock of `sessions.json`, the record of every saved session.
    pub fn lock_sessions(&self) -> Result<Lock> {
        self.lock_file("sessions.lock")
    }

    /// The record of every session seen (`<root>/sessions.json`), as it reads.
    pub fn read_sessions(&self) -> SessionsFile {
        match fs::read_to_string(self.root.join("sessions.json")) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(known) => SessionsFile::Read(known),
                Err(e) => SessionsFile::Bad(e.to_string()),
            },
            Err(e) if e.kind() == ErrorKind::NotFound => SessionsFile::Missing,
            Err(e) => SessionsFile::Bad(e.to_string()),
        }
    }

    /// Replaces the record of every session seen, while holding its lock; readable by the owner
    /// only, synced to disk.
    pub fn write_sessions(&self, lock: &Lock, known: &KnownSessions) -> Result<()> {
        still_held(lock, "sessions.json")?;
        make_private_dir(&self.root)?;
        write_private_json_atomic(&self.root.join("sessions.json"), known)
    }

    /// Moves a record that can't be read aside (`sessions.json.bad-<time>`), so it is never
    /// overwritten; returns where it went.
    pub fn set_aside_sessions(&self, _lock: &Lock) -> Result<PathBuf> {
        let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S%3f");
        let aside = self.root.join(format!("sessions.json.bad-{stamp}"));
        fs::rename(self.root.join("sessions.json"), &aside)
            .with_context(|| format!("moving the unreadable sessions.json to {}", aside.display()))?;
        Ok(aside)
    }

    fn pending_dir(&self) -> PathBuf {
        self.root.join("sessions.pending")
    }

    /// Marks the record as about to learn of launches read from cmux at `at`, before the version
    /// holding them is written: `<root>/sessions.pending/<at>.<pid>-<n>`. Removed once the record
    /// is updated (`end_pending`), or kept as failed (`fail_pending`).
    pub fn begin_pending(&self, _lock: &Lock, at: &str) -> Result<PathBuf> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        make_private_dir(&self.root)?;
        let dir = self.pending_dir();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{at}.{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        file.sync_all().with_context(|| format!("writing {}", path.display()))?;
        sync_dir(&dir);
        Ok(path)
    }

    /// Keeps a mark whose update failed, so the next read completes the record.
    pub fn fail_pending(&self, mark: &Path) -> Result<()> {
        let failed = PathBuf::from(format!("{}.failed", mark.display()));
        fs::rename(mark, &failed).with_context(|| format!("renaming {}", mark.display()))
    }

    /// Removes a mark: its update is done, or the record has been completed for it.
    pub fn end_pending(&self, mark: &Path) {
        let _ = fs::remove_file(mark);
    }

    /// Every mark: (path, the read time it names, failed, age). A `sessions.dirty` file from an
    /// older sessionsnap counts as a failed mark with the time it holds.
    pub fn pending(&self) -> Vec<Pending> {
        let mut out = vec![];
        let age = |m: &fs::Metadata| m.modified().ok().and_then(|t| t.elapsed().ok()).unwrap_or_default();
        if let Ok(entries) = fs::read_dir(self.pending_dir()) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let at = name.split_once("Z.").map(|(t, _)| format!("{t}Z")).unwrap_or(name.clone());
                let age = e.metadata().map(|m| age(&m)).unwrap_or_default();
                out.push(Pending { path: e.path(), at, failed: name.ends_with(".failed"), age });
            }
        }
        let legacy = self.root.join("sessions.dirty");
        if let Ok(at) = fs::read_to_string(&legacy) {
            out.push(Pending { path: legacy, at: at.trim().to_string(), failed: true, age: Duration::MAX });
        }
        out
    }

    /// Writes the next version of `name` and returns it (takes the snapshot's lock).
    pub fn save_new(&self, name: &str, message: &str, source: Source, workspaces: Vec<Workspace>) -> Result<Version> {
        let lock = self.lock(name)?;
        let latest = if self.exists(name) { self.versions(name)?.last().copied() } else { None };
        self.save_locked(&lock, name, latest, message, source, workspaces)
    }

    /// Writes the next version of `name` while holding its `lock`, when the latest version is still
    /// `latest` (None: no version yet), the one the caller read; else refuses with exit 6. Each
    /// version is published under a number no other file has (see `publish`), so saves at the
    /// same moment never overwrite each other or share a number.
    pub fn save_locked(
        &self,
        lock: &Lock,
        name: &str,
        latest: Option<u32>,
        message: &str,
        source: Source,
        workspaces: Vec<Workspace>,
    ) -> Result<Version> {
        validate_name(name)?;
        // Names ignore case: "Main" adds to an existing "main".
        let name = &self.stored_name(name);
        let now_latest = if self.exists(name) { self.versions(name)?.last().copied() } else { None };
        if now_latest != latest {
            let shown = |v: Option<u32>| v.map_or("no version".to_string(), |v| format!("v{v}"));
            return Err(failure(
                Exit::PlanStale,
                format!(
                    "snapshot '{name}' changed meanwhile: its latest is {}, not {}; try again",
                    shown(now_latest),
                    shown(latest)
                ),
            ));
        }
        make_private_dir(&self.root)?;
        let dir = self.snapshot_dir(name);
        in_new_dir(&dir, || {
            remove_leftovers(&dir, STALE);
            let now = chrono::Local::now();
            let mut version = Version {
                sessionsnap: FORMAT,
                snapshot: name.to_string(),
                version: latest.unwrap_or(0) + 1,
                created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                message: message.to_string(),
                source,
                workspaces,
            };
            // A lock broken while this save took too long may be another save's now, built on
            // the same version: publish nothing.
            if !lock.still_held() {
                return Err(failure(
                    Exit::PlanStale,
                    format!("snapshot '{name}' was locked by another save meanwhile; try again"),
                ));
            }
            publish(&dir, &mut version, &now.format("%Y%m%dT%H%M%S").to_string(), |a, b| fs::hard_link(a, b))?;
            Ok(version)
        })
    }

    /// The lock of `schedules.json`. Taken after a snapshot's lock, never before.
    pub fn lock_schedules(&self) -> Result<Lock> {
        self.lock_file("schedules.lock")
    }

    /// Every automatic snapshot's schedule; none when there is no `schedules.json`.
    pub fn read_schedules(&self) -> Result<Schedules> {
        let path = self.root.join("schedules.json");
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Schedules::new()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
        }
    }

    /// Replaces every schedule, while holding their lock.
    pub fn write_schedules(&self, lock: &Lock, schedules: &Schedules) -> Result<()> {
        still_held(lock, "schedules.json")?;
        make_private_dir(&self.root)?;
        write_private_json_atomic(&self.root.join("schedules.json"), schedules)
    }

    /// Renames snapshot `old` to `new` (a new name, or `old` in another case), under both locks;
    /// returns the name it was stored under. Refuses (exit 4) a `new` that is another snapshot.
    pub fn rename(&self, old: &str, new: &str) -> Result<String> {
        validate_new_name(new)?;
        let same = old.eq_ignore_ascii_case(new);
        // In name order, so two renames between the same names never wait on each other.
        let (first, second) =
            if old.to_ascii_lowercase() <= new.to_ascii_lowercase() { (old, new) } else { (new, old) };
        let _first = self.lock(first)?;
        let _second = if same { None } else { Some(self.lock(second)?) };
        self.versions(old)?;
        let from = self.stored_name(old);
        if !same && self.has_versions(new) {
            return Err(failure(Exit::Refused, format!("snapshot '{new}' already exists")));
        }
        let to = self.snapshots_dir().join(new);
        if !same && to.is_dir() {
            // An empty folder left behind by a failed save is not a snapshot. Its lock is held, so
            // what a crashed save left in it is gone for good, however recent.
            remove_leftovers(&to, Duration::ZERO);
            fs::remove_dir(&to).with_context(|| format!("removing {}", to.display()))?;
        }
        fs::rename(self.snapshot_dir(&from), &to).with_context(|| format!("renaming snapshot '{from}' to '{new}'"))?;
        Ok(from)
    }

    /// Deletes snapshot `name` and every version, under its lock.
    pub fn delete(&self, name: &str) -> Result<()> {
        let _lock = self.lock(name)?;
        self.versions(name)?;
        fs::remove_dir_all(self.snapshot_dir(name))?;
        Ok(())
    }

    /// Deletes all but the newest `keep` versions, under the snapshot's lock; returns the deleted
    /// version numbers.
    pub fn prune(&self, name: &str, keep: usize) -> Result<Vec<u32>> {
        if keep == 0 {
            return Err(failure(Exit::Error, "--keep must be at least 1"));
        }
        let _lock = self.lock(name)?;
        let versions = self.versions(name)?;
        let doomed = versions[..versions.len().saturating_sub(keep)].to_vec();
        for v in &doomed {
            fs::remove_file(self.version_path(name, *v)?)?;
        }
        Ok(doomed)
    }
}

/// A mark that the record of sessions is about to change (see `Store::begin_pending`).
#[derive(Debug, Clone)]
pub struct Pending {
    pub path: PathBuf,
    /// The read time it names (as written; may not be a valid time).
    pub at: String,
    /// Its update failed.
    pub failed: bool,
    pub age: Duration,
}

/// What `sessions.json` holds.
#[derive(Debug)]
pub enum SessionsFile {
    Missing,
    Read(KnownSessions),
    /// There, but not readable as a record (why).
    Bad(String),
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

/// Refuses (exit 6) to write `what` under a lock broken while its holder took too long: another
/// may hold it now, and would lose what it writes.
fn still_held(lock: &Lock, what: &str) -> Result<()> {
    if lock.still_held() {
        return Ok(());
    }
    Err(failure(Exit::PlanStale, format!("{what} was locked by another sessionsnap meanwhile; try again")))
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

/// Publishes `version` in `dir` as `<NNNN>-<stamp>.json`, from its number up. A number is first
/// claimed with `<NNNN>.claim`, a name without the time, so two saves (even ones whose lock was
/// broken) never both get it; a number any version file already has is skipped, whatever its
/// time. The file is written and synced to disk under a temporary name, then linked into place
/// with `link`, which fails rather than replace a file; where the file system has no links, it is
/// created with an exclusive create instead. A crash never leaves an empty latest version. Sets
/// the number it got.
fn publish(
    dir: &Path,
    version: &mut Version,
    stamp: &str,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
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
        let claim = dir.join(format!("{n:04}.claim"));
        match fs::OpenOptions::new().write(true).create_new(true).open(&claim) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                version.version += 1;
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("writing {}", claim.display())),
        }
        // Not a version file (no `.json` end), and unique to this process and call.
        let tmp =
            dir.join(format!(".{n:04}-{stamp}.{}-{}.tmp", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
        let file = dir.join(format!("{n:04}-{stamp}.json"));
        let placed = write_synced(&tmp, version).map(|()| match link(&tmp, &file) {
            Err(e) if matches!(e.kind(), ErrorKind::Unsupported | ErrorKind::PermissionDenied) => {
                write_synced(&file, version).map_err(|err| match err.downcast_ref::<std::io::Error>() {
                    Some(io) if io.kind() == ErrorKind::AlreadyExists => std::io::Error::from(ErrorKind::AlreadyExists),
                    _ => std::io::Error::other(format!("{err:#}")),
                })
            }
            other => other,
        });
        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(&claim);
        match placed? {
            Ok(()) => {
                sync_dir(dir);
                return Ok(());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => version.version += 1,
            Err(e) => return Err(e).with_context(|| format!("writing {}", file.display())),
        }
    }
}

/// Removes what a save that crashed may have left in `dir`: temporary files and number claims,
/// at least `age` old (a stale lock's age while a live save may hold the lock: it has none that old).
fn remove_leftovers(dir: &Path, age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let leftover = (name.starts_with('.') && name.ends_with(".tmp")) || name.ends_with(".claim");
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m.elapsed().unwrap_or_default() >= age);
        if leftover && old {
            let _ = fs::remove_file(e.path());
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
            pinned: false,
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

    /// A lock left by a save that crashed doesn't block saves for good. Locks live outside the
    /// snapshot's folder.
    #[test]
    fn a_stale_lock_is_broken() {
        let (_d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let lock = s.root().join("locks/snapshot-work.lock");
        let f = fs::File::create(&lock).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
        assert_eq!(s.save_new("Work", "b", Source::default(), vec![]).unwrap().version, 2);
        assert!(!lock.exists());
    }

    /// A number someone else has claimed is skipped; a claim left by a crash long ago is removed.
    #[test]
    fn claimed_numbers_are_skipped_and_old_claims_cleaned() {
        let (_d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let dir = s.snapshot_dir("work");
        fs::write(dir.join("0002.claim"), "").unwrap();
        assert_eq!(s.save_new("work", "b", Source::default(), vec![]).unwrap().version, 3);
        let old = |name: &str| {
            let f = fs::File::create(dir.join(name)).unwrap();
            f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
        };
        old("0004.claim");
        old(".0004-20000101T000000.1-1.tmp");
        assert_eq!(s.save_new("work", "c", Source::default(), vec![]).unwrap().version, 4);
        let left: Vec<String> =
            fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        // Only the versions, and the fresh claim (a save in progress) that was kept.
        assert!(left.iter().all(|f| f.ends_with(".json") || f == "0002.claim"), "{left:?}");
        assert!(dir.join("0002.claim").exists());
    }

    /// A file system without hard links still saves: the version is created in place exclusively.
    #[test]
    fn saves_without_hard_links() {
        let (_d, s) = store();
        let v1 = s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let dir = s.snapshot_dir("work");
        let mut v = Version { version: 2, ..v1 };
        let no_links = |_: &Path, _: &Path| Err(std::io::Error::from(ErrorKind::Unsupported));
        publish(&dir, &mut v, "20991231T235959", no_links).unwrap();
        assert_eq!(s.load("work", Some(2)).unwrap().version, 2);
        let mut v = Version { version: 2, ..v };
        let denied = |_: &Path, _: &Path| Err(std::io::Error::from(ErrorKind::PermissionDenied));
        publish(&dir, &mut v, "20991231T235959", denied).unwrap();
        assert_eq!(v.version, 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = s.version_path("work", 3).unwrap();
            assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    /// Saving onto a version that is no longer the latest is refused (exit 6), and writes nothing.
    #[test]
    fn a_save_onto_an_old_latest_is_refused() {
        let (_d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let lock = s.lock("work").unwrap();
        let err = s.save_locked(&lock, "work", None, "b", Source::default(), vec![]).unwrap_err();
        assert_eq!(exit_of(&err), Exit::PlanStale);
        s.save_locked(&lock, "work", Some(1), "b", Source::default(), vec![]).unwrap();
        let err = s.save_locked(&lock, "work", Some(1), "c", Source::default(), vec![]).unwrap_err();
        assert_eq!(exit_of(&err), Exit::PlanStale);
        assert_eq!(s.versions("work").unwrap(), vec![1, 2]);
    }

    /// An empty folder left behind (by hand, or a crash) is not a snapshot: it has no versions.
    #[test]
    fn an_empty_folder_has_no_versions() {
        let (_d, s) = store();
        fs::create_dir_all(s.snapshots_dir().join("--y")).unwrap();
        assert!(s.exists("--y"));
        assert!(!s.has_versions("--y"));
        assert!(!s.has_versions("missing"));
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        assert!(s.has_versions("WORK"));
    }

    /// A holder whose lock was broken (it took too long) and taken by another save publishes
    /// nothing: the other save may be building on the same version.
    #[test]
    fn a_save_whose_lock_was_broken_is_refused() {
        let (_d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        let slow = s.lock("work").unwrap();
        let f = fs::OpenOptions::new().write(true).open(s.root().join("locks/snapshot-work.lock")).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
        let other = s.lock("work").unwrap();
        let err = s.save_locked(&slow, "work", Some(1), "b", Source::default(), vec![]).unwrap_err();
        assert_eq!(exit_of(&err), Exit::PlanStale, "{err}");
        assert_eq!(s.versions("work").unwrap(), vec![1]);
        s.save_locked(&other, "work", Some(1), "c", Source::default(), vec![]).unwrap();
    }

    /// Delete waits for a save in progress, and never leaves the folder behind.
    #[test]
    fn delete_and_prune_take_the_lock() {
        let (d, s) = store();
        s.save_new("work", "a", Source::default(), vec![]).unwrap();
        s.save_new("work", "b", Source::default(), vec![]).unwrap();
        let lock = s.lock("work").unwrap();
        let root = d.path().to_path_buf();
        let (deleted, waiter) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            Store::new(root).delete("work").unwrap();
            deleted.send(()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(waiter.try_recv().is_err(), "delete waits for the lock");
        s.save_locked(&lock, "work", Some(2), "c", Source::default(), vec![]).unwrap();
        drop(lock);
        t.join().unwrap();
        assert!(!s.snapshot_dir("work").exists());
        assert!(s.list().unwrap().is_empty());
        // Prune too.
        for m in ["a", "b"] {
            s.save_new("work", m, Source::default(), vec![]).unwrap();
        }
        let lock = s.lock("work").unwrap();
        let root = d.path().to_path_buf();
        let t = std::thread::spawn(move || Store::new(root).prune("work", 1).unwrap());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!t.is_finished(), "prune waits for the lock");
        drop(lock);
        assert_eq!(t.join().unwrap(), vec![1]);
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
        publish(&dir, &mut v, "20991231T235959", |a, b| fs::hard_link(a, b)).unwrap();
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

    #[test]
    fn rename_moves_every_version_and_names_them_after_the_folder() {
        let (_d, s) = store();
        s.save_new("work", "m", Source::default(), vec![ws("A")]).unwrap();
        s.save_new("work", "m", Source::default(), vec![ws("B")]).unwrap();
        assert_eq!(s.rename("Work", "job").unwrap(), "work");
        assert_eq!(s.list().unwrap(), vec!["job".to_string()]);
        assert_eq!(s.versions("job").unwrap(), vec![1, 2]);
        assert_eq!(s.load("job", Some(1)).unwrap().snapshot, "job");
        assert_eq!(exit_of(&s.versions("work").unwrap_err()), Exit::NotFound);
        // Only the case changes.
        s.rename("job", "Job").unwrap();
        assert_eq!(s.list().unwrap(), vec!["Job".to_string()]);
    }

    #[test]
    fn rename_refuses_an_existing_name_and_a_missing_snapshot() {
        let (_d, s) = store();
        s.save_new("a", "m", Source::default(), vec![ws("A")]).unwrap();
        s.save_new("b", "m", Source::default(), vec![ws("B")]).unwrap();
        assert_eq!(exit_of(&s.rename("a", "B").unwrap_err()), Exit::Refused);
        assert_eq!(exit_of(&s.rename("zzz", "c").unwrap_err()), Exit::NotFound);
        assert!(s.rename("a", "--c").is_err());
        assert_eq!(s.list().unwrap(), vec!["a".to_string(), "b".to_string()]);
        // An empty folder left behind is no snapshot.
        fs::create_dir_all(s.snapshots_dir().join("c")).unwrap();
        s.rename("a", "c").unwrap();
        assert_eq!(s.list().unwrap(), vec!["b".to_string(), "c".to_string()]);
        // Nor is one holding only what a crashed save left (a claim, a temporary file).
        let d = s.snapshots_dir().join("d");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("0001.claim"), "").unwrap();
        fs::write(d.join(".0001-20260101T000000.1-0.tmp"), "{").unwrap();
        s.rename("c", "d").unwrap();
        assert_eq!(s.list().unwrap(), vec!["b".to_string(), "d".to_string()]);
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1, "only c's version moved in");
    }

    #[test]
    fn schedules_read_empty_then_as_written() {
        use crate::domain::schedule::Schedule;
        let (_d, s) = store();
        assert!(s.read_schedules().unwrap().is_empty());
        let lock = s.lock_schedules().unwrap();
        let mut all = Schedules::new();
        all.insert(
            "work".into(),
            Schedule { interval: 600, created_at: "t".into(), stopped: false, last_run: None, terminal: None },
        );
        s.write_schedules(&lock, &all).unwrap();
        assert_eq!(s.read_schedules().unwrap(), all);
    }

    /// A write under a lock broken meanwhile (its holder took too long) is refused.
    #[test]
    fn writes_under_a_broken_lock_are_refused() {
        let (_d, s) = store();
        let break_lock = |file: &str| {
            let f = fs::OpenOptions::new().write(true).open(s.root().join("locks").join(file)).unwrap();
            f.set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
        };
        let slow = s.lock_schedules().unwrap();
        break_lock("schedules.lock");
        let _other = s.lock_schedules().unwrap();
        let err = s.write_schedules(&slow, &Schedules::new()).unwrap_err();
        assert_eq!(exit_of(&err), Exit::PlanStale, "{err}");
        assert!(!s.root().join("schedules.json").exists());
        let slow = s.lock_sessions().unwrap();
        break_lock("sessions.lock");
        let _other = s.lock_sessions().unwrap();
        let err = s.write_sessions(&slow, &KnownSessions::default()).unwrap_err();
        assert_eq!(exit_of(&err), Exit::PlanStale, "{err}");
        assert!(!s.root().join("sessions.json").exists());
    }
}
