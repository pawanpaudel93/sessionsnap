//! Claude Code: its session history (`~/.claude/projects`) and the sessionsnap skill installed
//! for it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::home_dir;
use crate::domain::model::is_session_id;
use crate::ports::{SessionHistory, SessionInfo};
use crate::util::json::{make_private_dir, read_json, write_private_json_atomic};

/// The registry kind of Claude Code.
pub const KIND: &str = "claude";

/// Claude Code's data directory (`~/.claude`); transcripts are `projects/<project>/<id>.jsonl`.
pub struct Claude {
    root: PathBuf,
    /// What earlier runs read of each transcript (see `info`); None reads every one in full.
    cache: Option<Cache>,
}

/// What a transcript said up to `upto` bytes: the fields `info` collects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Read {
    /// The file it was read from (device, inode): a transcript replaced since is read afresh.
    file: (u64, u64),
    upto: u64,
    cwd: Option<String>,
    custom: Option<String>,
    ai: Option<String>,
    first: Option<String>,
    entrypoint: Option<String>,
}

impl Read {
    /// Takes in one transcript line.
    fn take(&mut self, line: &str) {
        let interesting = line.contains("\"custom-title\"")
            || line.contains("\"ai-title\"")
            || ((self.cwd.is_none() || self.first.is_none() || self.entrypoint.is_none())
                && line.contains("\"type\":\"user\""));
        if !interesting {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim_end()) else { return };
        match v.get("type").and_then(Value::as_str) {
            Some("custom-title") => self.custom = str_field(&v, "customTitle").or(self.custom.take()),
            Some("ai-title") => self.ai = str_field(&v, "aiTitle").or(self.ai.take()),
            Some("user") => {
                if self.cwd.is_none() {
                    self.cwd = str_field(&v, "cwd");
                }
                if self.entrypoint.is_none() {
                    self.entrypoint = str_field(&v, "entrypoint");
                }
                let meta = v.get("isMeta").and_then(Value::as_bool) == Some(true);
                if self.first.is_none()
                    && !meta
                    && let Some(text) = v.pointer("/message/content").and_then(Value::as_str)
                {
                    let text = text.trim();
                    if !text.is_empty() && !text.starts_with('<') {
                        self.first = Some(one_line(text, 120));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The reads kept in a private file between runs, written back when the reader is dropped.
struct Cache {
    path: PathBuf,
    reads: RefCell<Option<HashMap<String, Read>>>,
    changed: Cell<bool>,
}

impl Cache {
    fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, Read>) -> T) -> T {
        let mut reads = self.reads.borrow_mut();
        f(reads.get_or_insert_with(|| read_json(&self.path).unwrap_or_default()))
    }

    fn get(&self, key: &str) -> Option<Read> {
        self.with(|r| r.get(key).cloned())
    }

    fn put(&self, key: String, read: Read) {
        if self.with(|r| r.insert(key, read.clone()).as_ref() != Some(&read)) {
            self.changed.set(true);
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if !self.changed.get() {
            return;
        }
        let Some(mut reads) = self.reads.borrow_mut().take() else { return };
        // Transcripts deleted since are forgotten.
        reads.retain(|path, _| Path::new(path).is_file());
        if let Some(dir) = self.path.parent() {
            let _ = make_private_dir(dir);
        }
        let _ = write_private_json_atomic(&self.path, &reads);
    }
}

/// A file's device and inode (zero where there are none).
fn file_id(meta: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (0, 0)
    }
}

impl Claude {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Claude { root: root.into(), cache: None }
    }

    /// Keeps what it has read of each transcript in the private file `path`, between runs.
    pub fn cached(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.cache = Some(Cache { path, reads: RefCell::default(), changed: Cell::new(false) });
        self
    }

    /// `$SESSIONSNAP_CLAUDE_DIR`, else `~/.claude`.
    pub fn open_default() -> Result<Self> {
        if let Some(p) = std::env::var_os("SESSIONSNAP_CLAUDE_DIR") {
            return Ok(Claude::new(p));
        }
        Ok(Claude::new(home_dir()?.join(".claude")))
    }

    fn projects(&self) -> PathBuf {
        self.root.join("projects")
    }

    /// Claude Code's project folder name for `cwd`: every non-alphanumeric character becomes '-'.
    pub fn project_dir_name(cwd: &str) -> String {
        cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    }

    pub fn transcript_path(&self, cwd: &str, session_id: &str) -> PathBuf {
        self.projects().join(Self::project_dir_name(cwd)).join(format!("{session_id}.jsonl"))
    }

    /// Finds a session's transcript in any project; none for an ID that isn't one (a path).
    pub fn find_transcript(&self, session_id: &str) -> Option<PathBuf> {
        if !is_session_id(session_id) {
            return None;
        }
        let name = format!("{session_id}.jsonl");
        fs::read_dir(self.projects()).ok()?.flatten().map(|e| e.path().join(&name)).find(|p| p.is_file())
    }

    /// Reads a transcript's folder, name and first real prompt, and whether it is a session a person
    /// ran: interactive (entrypoint "cli", not `claude -p` or an SDK run) and outside `~/.claude*`.
    /// Transcripts only grow, so with a cache only what was appended since the last read is parsed
    /// (a save does this for every open tab, every interval).
    pub fn info(&self, path: &Path) -> Result<SessionInfo> {
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let meta = fs::metadata(path)?;
        let modified_at = meta.modified()?;
        let key = path.to_string_lossy().into_owned();
        let file = file_id(&meta);
        // A cached read of this same file that it has only grown past; else from the start.
        let mut read = self
            .cache
            .as_ref()
            .and_then(|c| c.get(&key))
            .filter(|r| r.file == file && r.upto <= meta.len())
            .unwrap_or(Read { file, ..Read::default() });
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::Start(read.upto))?;
        let mut reader = BufReader::new(file);
        let mut buf = Vec::new();
        let mut partial = None;
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            if buf.last() != Some(&b'\n') {
                // Still being written: used now, read again once it is whole.
                let mut now = read.clone();
                now.take(&line);
                partial = Some(now);
                break;
            }
            read.upto += n as u64;
            read.take(&line);
        }
        if let Some(cache) = &self.cache {
            cache.put(key, read.clone());
        }
        let Read { cwd, custom, ai, first, entrypoint, .. } = partial.unwrap_or(read);
        let modified =
            chrono::DateTime::<chrono::Local>::from(modified_at).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let interactive = entrypoint.as_deref().is_none_or(|e| e == "cli");
        let internal = match (home_dir(), &cwd) {
            (Ok(home), Some(c)) => c.starts_with(&format!("{}/.claude", home.to_string_lossy())),
            _ => false,
        };
        Ok(SessionInfo {
            agent: KIND.to_string(),
            session_id,
            cwd,
            title: custom.or(ai),
            first_prompt: first,
            modified,
            modified_at,
            created_at: meta.created().ok(),
            recoverable: interactive && !internal,
        })
    }
}

impl SessionHistory for Claude {
    fn agent(&self) -> &'static str {
        KIND
    }

    fn contains(&self, session_id: &str) -> bool {
        self.find_transcript(session_id).is_some()
    }

    fn find(&self, session_id: &str) -> Result<Option<SessionInfo>> {
        match self.find_transcript(session_id) {
            Some(path) => self.info(&path).map(Some),
            None => Ok(None),
        }
    }

    fn title_for(&self, cwd: &str, session_id: &str) -> Option<String> {
        if !is_session_id(session_id) {
            return None;
        }
        let direct = self.transcript_path(cwd, session_id);
        let path = if direct.is_file() { direct } else { self.find_transcript(session_id)? };
        self.info(&path).ok().and_then(|i| i.title.or(i.first_prompt))
    }

    /// Sessions whose transcript changed within `within` (all when None), newest first.
    fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>> {
        let cutoff = within.and_then(|d| SystemTime::now().checked_sub(d));
        let mut out = vec![];
        let Ok(projects) = fs::read_dir(self.projects()) else { return Ok(out) };
        for project in projects.flatten() {
            let Ok(files) = fs::read_dir(project.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if !path.file_stem().and_then(|s| s.to_str()).is_some_and(is_session_id) {
                    continue;
                }
                let Ok(meta) = file.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                if let (Some(cutoff), Ok(modified)) = (cutoff, meta.modified())
                    && modified < cutoff
                {
                    continue;
                }
                if let Ok(info) = self.info(&path) {
                    out.push(info);
                }
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.modified_at));
        Ok(out)
    }
}

pub const SKILL: &str = include_str!("../../../skill/SKILL.md");

/// `$SESSIONSNAP_SKILLS_DIR`, else `~/.claude/skills`.
pub fn skills_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SESSIONSNAP_SKILLS_DIR") {
        return Ok(p.into());
    }
    Ok(home_dir()?.join(".claude/skills"))
}

/// `$SESSIONSNAP_AGENT_SKILLS_DIR`, else `~/.agents/skills`: the shared Agent Skills folder, which
/// Pi (and other agents that follow the Agent Skills layout) read. The same skill goes there.
pub fn agent_skills_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SESSIONSNAP_AGENT_SKILLS_DIR") {
        return Ok(p.into());
    }
    Ok(home_dir()?.join(".agents/skills"))
}

pub fn install_skill(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("sessionsnap").join("SKILL.md");
    std::fs::create_dir_all(dir.join("sessionsnap"))?;
    std::fs::write(&path, SKILL)?;
    Ok(path)
}

/// What `uninstall_skill` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillRemoval {
    Removed,
    Missing,
    /// The folder holds something sessionsnap did not put there; it is left alone.
    NotOurs,
}

/// Removes `dir/sessionsnap` when it is the skill `install_skill` wrote: only a SKILL.md naming
/// itself sessionsnap.
pub fn uninstall_skill(dir: &Path) -> Result<SkillRemoval> {
    let folder = dir.join("sessionsnap");
    let Ok(meta) = std::fs::symlink_metadata(&folder) else {
        return Ok(SkillRemoval::Missing);
    };
    let entries: Vec<_> = if meta.is_dir() {
        std::fs::read_dir(&folder)?.filter_map(|e| e.ok()).map(|e| e.file_name()).collect()
    } else {
        vec![]
    };
    let ours = entries.len() == 1
        && entries[0] == "SKILL.md"
        && std::fs::read_to_string(folder.join("SKILL.md"))
            .is_ok_and(|t| t.starts_with("---\n") && t.lines().any(|l| l.trim() == "name: sessionsnap"));
    if !ours {
        return Ok(SkillRemoval::NotOurs);
    }
    std::fs::remove_dir_all(&folder)?;
    Ok(SkillRemoval::Removed)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Collapses whitespace and cuts to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).collect::<String>() + "…" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{A, B, write_transcript};
    use serde_json::json;

    fn write_raw(root: &Path, cwd: &str, id: &str, lines: &[Value]) -> PathBuf {
        let dir = root.join("projects").join(Claude::project_dir_name(cwd));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{id}.jsonl"));
        fs::write(&p, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        p
    }

    #[test]
    fn project_dir_name_matches_claude_code() {
        assert_eq!(
            Claude::project_dir_name("/Users/me/cascadia/.claude/worktrees/x"),
            "-Users-me-cascadia--claude-worktrees-x"
        );
    }

    #[test]
    fn info_prefers_custom_title_and_skips_meta_prompts() {
        let d = tempfile::tempdir().unwrap();
        let c = Claude::new(d.path());
        let p = write_raw(
            d.path(),
            "/w/a",
            A,
            &[
                json!({"type": "user", "isMeta": true, "cwd": "/w/a", "entrypoint": "cli",
                   "message": {"content": "<command-name>/clear</command-name>"}}),
                json!({"type": "user", "cwd": "/w/a", "entrypoint": "cli",
                   "message": {"content": "fix the login bug\nplease"}}),
                json!({"type": "ai-title", "aiTitle": "Fix login bug", "sessionId": A}),
                json!({"type": "custom-title", "customTitle": "login fix", "sessionId": A}),
            ],
        );
        let i = c.info(&p).unwrap();
        assert_eq!(i.session_id, A);
        assert_eq!(i.cwd.as_deref(), Some("/w/a"));
        assert_eq!(i.title.as_deref(), Some("login fix"));
        assert_eq!(i.first_prompt.as_deref(), Some("fix the login bug please"));
        assert!(i.recoverable);
        assert_eq!(c.find_transcript(A), Some(p));
        assert_eq!(c.find_transcript(B), None);
        assert_eq!(c.title_for("/w/a", A).as_deref(), Some("login fix"));
        // The session is found even when the tab's folder is not the transcript's project.
        assert_eq!(c.title_for("/elsewhere", A).as_deref(), Some("login fix"));
    }

    /// A session ID from a snapshot file is never a path: `../x` finds nothing outside the projects.
    #[test]
    fn a_hostile_session_id_finds_no_transcript() {
        let d = tempfile::tempdir().unwrap();
        let c = Claude::new(d.path().join("claude"));
        write_raw(&d.path().join("claude"), "/w/a", A, &[json!({"type": "custom-title", "customTitle": "t"})]);
        fs::write(d.path().join("evil.jsonl"), format!("{}\n", json!({"type": "custom-title", "customTitle": "evil"})))
            .unwrap();
        let hostile = "../../../evil";
        assert!(c.find_transcript(&format!("../-w-a/{A}")).is_none());
        assert!(c.find_transcript(hostile).is_none());
        assert!(!c.contains(hostile));
        assert!(c.find(hostile).unwrap().is_none());
        assert_eq!(c.title_for("/w/a", hostile), None);
        assert_eq!(c.title_for("/w/a", A).as_deref(), Some("t"));
    }

    #[test]
    fn scan_ignores_files_that_are_not_session_ids() {
        let d = tempfile::tempdir().unwrap();
        let c = Claude::new(d.path());
        write_transcript(d.path(), "/w/a", A, "real");
        write_transcript(d.path(), "/w/a", "agent-x", "sub");
        let found = c.scan(None).unwrap();
        assert_eq!(found.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), vec![A]);
    }

    #[test]
    fn ai_title_is_the_fallback() {
        let d = tempfile::tempdir().unwrap();
        let c = Claude::new(d.path());
        write_raw(
            d.path(),
            "/w/a",
            A,
            &[
                json!({"type": "user", "cwd": "/w/a", "message": {"content": "hello"}}),
                json!({"type": "ai-title", "aiTitle": "Greeting", "sessionId": A}),
            ],
        );
        assert_eq!(c.title_for("/w/a", A).as_deref(), Some("Greeting"));
    }

    #[test]
    fn scan_filters_by_age_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let c = Claude::new(d.path());
        let old = write_transcript(d.path(), "/w/a", A, "old");
        write_transcript(d.path(), "/w/b", B, "new");
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(48 * 3600))
            .unwrap();
        let recent = c.scan(Some(Duration::from_secs(24 * 3600))).unwrap();
        assert_eq!(recent.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), vec![B]);
        let all = c.scan(None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].session_id, B);
    }

    #[test]
    fn meets_the_session_history_contract() {
        let d = tempfile::tempdir().unwrap();
        write_transcript(d.path(), "/w/a", A, "login fix");
        let old = write_transcript(d.path(), "/w/b", B, "review");
        crate::adapters::agents::contract::age(&old, Duration::from_secs(48 * 3600));
        let fixture = crate::adapters::agents::contract::Fixture {
            agent: "claude",
            recent: A,
            old: B,
            unknown: "44444444-4444-4444-8444-444444444444",
            cwd: "/w/a",
            title: "login fix",
        };
        crate::adapters::agents::contract::check(&Claude::new(d.path()), &fixture);
    }

    #[test]
    fn installs_skill() {
        let d = tempfile::tempdir().unwrap();
        let p = install_skill(d.path()).unwrap();
        assert!(std::fs::read_to_string(p).unwrap().contains("name: sessionsnap"));
    }

    #[test]
    fn uninstall_skill_removes_only_our_skill() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(uninstall_skill(d.path()).unwrap(), SkillRemoval::Missing);
        install_skill(d.path()).unwrap();
        assert_eq!(uninstall_skill(d.path()).unwrap(), SkillRemoval::Removed);
        assert!(!d.path().join("sessionsnap").exists());

        install_skill(d.path()).unwrap();
        fs::write(d.path().join("sessionsnap/notes.md"), "mine").unwrap();
        assert_eq!(uninstall_skill(d.path()).unwrap(), SkillRemoval::NotOurs);
        assert!(d.path().join("sessionsnap/notes.md").exists());

        let other = d.path().join("other");
        fs::create_dir_all(other.join("sessionsnap")).unwrap();
        fs::write(other.join("sessionsnap/SKILL.md"), "---\nname: something-else\n---\n").unwrap();
        assert_eq!(uninstall_skill(&other).unwrap(), SkillRemoval::NotOurs);
    }

    fn append(path: &Path, line: &str) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(f, "{line}").unwrap();
    }

    /// With a cache, a later read parses only what was appended: a title added since is seen, and
    /// the part read before is not read again.
    #[test]
    fn a_cached_reader_reads_only_what_was_appended() {
        let d = tempfile::tempdir().unwrap();
        let path = crate::testing::write_transcript(d.path(), "/w/a", A, "login fix");
        let cache = d.path().join("cache/claude.json");
        let first = Claude::new(d.path()).cached(&cache).info(&path).unwrap();
        assert_eq!(
            (first.title.as_deref(), first.first_prompt.as_deref()),
            (Some("login fix"), Some("start login fix"))
        );
        // Rewrite the part already read (same length): a full read would see the change.
        let text = fs::read_to_string(&path).unwrap().replace("start login fix", "START LOGIN FIX");
        fs::write(&path, &text).unwrap();
        append(&path, &format!(r#"{{"type":"custom-title","customTitle":"renamed","sessionId":"{A}"}}"#));
        let again = Claude::new(d.path()).cached(&cache).info(&path).unwrap();
        assert_eq!(again.title.as_deref(), Some("renamed"), "the appended title");
        assert_eq!(again.first_prompt.as_deref(), Some("start login fix"), "the cached part, not read again");
        assert_eq!(Claude::new(d.path()).info(&path).unwrap().first_prompt.as_deref(), Some("START LOGIN FIX"));
    }

    /// A transcript that got shorter (replaced, rewritten) is read from the start again.
    #[test]
    fn a_shorter_transcript_is_read_afresh() {
        let d = tempfile::tempdir().unwrap();
        let path = crate::testing::write_transcript(d.path(), "/w/a", A, "login fix");
        let cache = d.path().join("cache/claude.json");
        Claude::new(d.path()).cached(&cache).info(&path).unwrap();
        let line = format!(
            r#"{{"type":"user","cwd":"/w/b","entrypoint":"cli","sessionId":"{A}","message":{{"content":"new"}}}}"#
        );
        fs::write(&path, format!("{line}\n")).unwrap();
        let info = Claude::new(d.path()).cached(&cache).info(&path).unwrap();
        assert_eq!((info.cwd.as_deref(), info.first_prompt.as_deref(), info.title), (Some("/w/b"), Some("new"), None));
    }

    /// A last line still being written is used now and read again once it is whole.
    #[test]
    fn a_line_still_being_written_is_read_again() {
        let d = tempfile::tempdir().unwrap();
        let path = crate::testing::write_transcript(d.path(), "/w/a", A, "login fix");
        let cache = d.path().join("cache/claude.json");
        let partial = format!(r#"{{"type":"custom-title","customTitle":"half","sessionId":"{A}"}}"#);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut f, partial.as_bytes()).unwrap();
        assert_eq!(Claude::new(d.path()).cached(&cache).info(&path).unwrap().title.as_deref(), Some("half"));
        // The writer finishes the line and adds another title.
        append(&path, "");
        append(&path, &format!(r#"{{"type":"custom-title","customTitle":"whole","sessionId":"{A}"}}"#));
        assert_eq!(Claude::new(d.path()).cached(&cache).info(&path).unwrap().title.as_deref(), Some("whole"));
        // The cache file is private.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&cache).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
