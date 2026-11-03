//! Pi (pi-coding-agent): its session history, one JSONL file per session in
//! `~/.pi/agent/sessions/--<folder>--/<timestamp>_<id>.jsonl` (format version 3). The first line is
//! the header (`{"type":"session","id","timestamp","cwd"}`); a `session_info` entry names the session
//! (the latest wins) and `message` entries hold the conversation.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde_json::Value;

use crate::adapters::home_dir;
use crate::domain::model::is_session_id;
use crate::ports::{SessionHistory, SessionInfo};

/// The registry kind of Pi.
pub const KIND: &str = "pi";

/// The custom entry the sessionsnap extension adds at each start of Pi: `{"args": [...]}`, the
/// flags it was started with (Pi renames its process, which hides them from cmux, tmux and `ps`).
pub const LAUNCH_ENTRY: &str = "sessionsnap.launch";

/// The extension `install-pi` writes, and the line that marks it as sessionsnap's.
pub const EXTENSION: &str = include_str!("pi-extension.ts");
const EXTENSION_MARK: &str = "// sessionsnap: records the flags this Pi was started with";
const EXTENSION_FILE: &str = "sessionsnap.ts";

/// Pi's global extensions folder: `$PI_CODING_AGENT_DIR/extensions`, else `~/.pi/agent/extensions`.
pub fn extensions_dir() -> Result<PathBuf> {
    match std::env::var_os("PI_CODING_AGENT_DIR").filter(|p| !p.is_empty()) {
        Some(p) => Ok(PathBuf::from(p).join("extensions")),
        None => Ok(home_dir()?.join(".pi/agent/extensions")),
    }
}

/// Writes the extension into `dir`; returns its path. A file of that name sessionsnap did not write
/// is left alone (exit 4).
pub fn install_extension(dir: &Path) -> Result<PathBuf> {
    let path = dir.join(EXTENSION_FILE);
    if path.exists() && !is_ours(&path) {
        return Err(crate::error::failure(
            crate::error::Exit::Refused,
            format!("{} exists and sessionsnap did not write it; move it away first", path.display()),
        ));
    }
    fs::create_dir_all(dir)?;
    fs::write(&path, EXTENSION)?;
    Ok(path)
}

/// Removes the extension from `dir` when sessionsnap wrote it.
pub fn uninstall_extension(dir: &Path) -> Result<super::claude::SkillRemoval> {
    use super::claude::SkillRemoval;
    let path = dir.join(EXTENSION_FILE);
    if fs::symlink_metadata(&path).is_err() {
        return Ok(SkillRemoval::Missing);
    }
    if !is_ours(&path) {
        return Ok(SkillRemoval::NotOurs);
    }
    fs::remove_file(&path)?;
    Ok(SkillRemoval::Removed)
}

fn is_ours(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && fs::read_to_string(path).is_ok_and(|t| t.starts_with(EXTENSION_MARK))
}

/// Pi's sessions folder.
pub struct Pi {
    root: PathBuf,
    /// Pi's extensions folder, where `install-pi` puts the extension that records launches.
    extensions: PathBuf,
    /// Session ID (lowercase) → session file, read once on first lookup.
    index: OnceCell<HashMap<String, PathBuf>>,
}

impl Pi {
    /// Sessions in `root`, with the extensions folder beside it (`<agent dir>/sessions` and
    /// `<agent dir>/extensions`, as Pi lays them out).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let extensions = root.parent().unwrap_or(&root).join("extensions");
        Pi { root, extensions, index: OnceCell::new() }
    }

    /// Where Pi keeps sessions, as Pi decides it: `$PI_CODING_AGENT_SESSION_DIR`, else
    /// `$PI_CODING_AGENT_DIR/sessions`, else `~/.pi/agent/sessions`. A `sessionDir` in Pi's settings
    /// is not read: that file may hold credentials.
    pub fn open_default() -> Result<Self> {
        let var = |name| std::env::var_os(name).filter(|p| !p.is_empty()).map(PathBuf::from);
        let root = match (var("PI_CODING_AGENT_SESSION_DIR"), var("PI_CODING_AGENT_DIR")) {
            (Some(p), _) => p,
            (None, Some(p)) => p.join("sessions"),
            (None, None) => home_dir()?.join(".pi/agent/sessions"),
        };
        Ok(Pi { extensions: extensions_dir()?, ..Pi::new(root) })
    }

    /// Every session file, with the session ID its name ends with.
    fn files(&self) -> Vec<(PathBuf, String)> {
        let mut out = vec![];
        let Ok(folders) = fs::read_dir(&self.root) else { return out };
        for folder in folders.flatten() {
            let Ok(files) = fs::read_dir(folder.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if let Some(id) = file_id(&path) {
                    out.push((path.clone(), id.to_string()));
                }
            }
        }
        out
    }

    /// A session's file, from an index built on the first lookup.
    fn find_file(&self, session_id: &str) -> Option<PathBuf> {
        if !is_session_id(session_id) {
            return None;
        }
        let index =
            self.index.get_or_init(|| self.files().into_iter().map(|(p, id)| (id.to_ascii_lowercase(), p)).collect());
        index.get(&session_id.to_ascii_lowercase()).cloned()
    }

    /// Reads a session file: its header (ID, folder), its latest name and its first prompt.
    pub fn info(&self, path: &Path) -> Result<SessionInfo> {
        let meta = fs::metadata(path)?;
        let (modified_at, created_at) = (meta.modified()?, meta.created().ok());
        let mut session_id = file_id(path).unwrap_or_default().to_string();
        let (mut cwd, mut name, mut first) = (None, None, None);
        for (n, line) in BufReader::new(fs::File::open(path)?).lines().enumerate() {
            let Ok(line) = line else { break };
            // Only the header, names and user messages are parsed.
            let wanted = n == 0 || line.contains("\"session_info\"") || (first.is_none() && line.contains("\"user\""));
            if !wanted {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v.get("type").and_then(Value::as_str) {
                Some("session") => {
                    if let Some(id) = v.get("id").and_then(Value::as_str).filter(|id| is_session_id(id)) {
                        session_id = id.to_string();
                    }
                    cwd = v.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
                }
                // The latest name wins; an empty one clears it.
                Some("session_info") => {
                    name = v
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                }
                Some("message") if first.is_none() => first = user_text(&v["message"]),
                _ => {}
            }
        }
        let modified =
            chrono::DateTime::<chrono::Local>::from(modified_at).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        Ok(SessionInfo {
            agent: KIND.to_string(),
            session_id,
            cwd,
            title: name.as_deref().map(|t| one_line(t, 80)),
            first_prompt: first.as_deref().map(|t| one_line(t, 120)),
            modified,
            modified_at,
            created_at,
            // Pi doesn't record whether a session was a `--print` run.
            recoverable: true,
        })
    }
}

impl SessionHistory for Pi {
    fn agent(&self) -> &'static str {
        KIND
    }

    fn contains(&self, session_id: &str) -> bool {
        self.find_file(session_id).is_some()
    }

    fn find(&self, session_id: &str) -> Result<Option<SessionInfo>> {
        match self.find_file(session_id) {
            Some(path) => self.info(&path).map(Some),
            None => Ok(None),
        }
    }

    /// The flags of the latest start the extension recorded; None without a record.
    fn launch_args(&self, session_id: &str) -> Option<Vec<String>> {
        let path = self.find_file(session_id)?;
        let mut found = None;
        for line in BufReader::new(fs::File::open(path).ok()?).lines() {
            let Ok(line) = line else { break };
            if !line.contains(LAUNCH_ENTRY) {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("type").and_then(Value::as_str) != Some("custom")
                || v.get("customType").and_then(Value::as_str) != Some(LAUNCH_ENTRY)
            {
                continue;
            }
            // Every value must be a string, else the record is not one to trust.
            found = v
                .pointer("/data/args")
                .and_then(Value::as_array)
                .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect::<Option<Vec<_>>>());
        }
        found.map(|args| without_cmux_extension(&args))
    }

    fn records_launches(&self) -> bool {
        true
    }

    fn recorder_installed(&self) -> bool {
        is_ours(&self.extensions.join(EXTENSION_FILE))
    }

    /// `pi --session <id>` finds the session from any folder, so `cwd` does not matter.
    fn title_for(&self, _cwd: &str, session_id: &str) -> Option<String> {
        self.find(session_id).ok().flatten().and_then(|i| i.title.or(i.first_prompt))
    }

    fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>> {
        let cutoff = within.and_then(|d| SystemTime::now().checked_sub(d));
        let mut out = vec![];
        for (path, _) in self.files() {
            let Ok(meta) = fs::metadata(&path) else { continue };
            if let (Some(cutoff), Ok(modified)) = (cutoff, meta.modified())
                && modified < cutoff
            {
                continue;
            }
            if let Ok(info) = self.info(&path) {
                out.push(info);
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.modified_at));
        Ok(out)
    }
}

/// `args` without the extension cmux's `pi` wrapper adds to every launch (`-e
/// /tmp/cmux-pi-extensions/…/cmux-session.ts`): cmux adds it again, and the file is temporary.
fn without_cmux_extension(args: &[String]) -> Vec<String> {
    let cmux = |v: &str| v.contains("/cmux-pi-extensions/");
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if matches!(a, "-e" | "--extension") && args.get(i + 1).is_some_and(|v| cmux(v)) {
            i += 2;
            continue;
        }
        if a.strip_prefix("--extension=").is_some_and(cmux) {
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// The session ID in `<timestamp>_<id>.jsonl`.
fn file_id(path: &Path) -> Option<&str> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let id = stem.rsplit_once('_')?.1;
    is_session_id(id).then_some(id)
}

/// A user message's text: `content` as a string, or its `text` blocks joined.
fn user_text(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let text = match message.get("content")? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Collapses whitespace and cuts to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { flat.chars().take(max).collect::<String>() + "…" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agents::contract;
    use crate::testing::{A, B, write_pi_session};
    use serde_json::json;

    #[test]
    fn meets_the_session_history_contract() {
        let d = tempfile::tempdir().unwrap();
        write_pi_session(d.path(), "/w/a", A, "fix the login bug");
        let old = write_pi_session(d.path(), "/w/b", B, "review");
        contract::age(&old, Duration::from_secs(48 * 3600));
        let fixture = contract::Fixture {
            agent: "pi",
            recent: A,
            old: B,
            unknown: "44444444-4444-4444-8444-444444444444",
            cwd: "/w/a",
            title: "fix the login bug",
        };
        contract::check(&Pi::new(d.path()), &fixture);
    }

    #[test]
    fn the_latest_name_is_the_title_and_block_content_is_read() {
        let d = tempfile::tempdir().unwrap();
        let path = write_pi_session(d.path(), "/w/a", A, "ignored");
        let lines = [
            json!({"type": "session", "version": 3, "id": A, "timestamp": "2026-10-11T09:30:00.000Z", "cwd": "/w/a"}),
            json!({"type": "message", "id": "a1", "message": {"role": "assistant", "content": "hi"}}),
            json!({"type": "message", "id": "a2", "message": {"role": "user",
                   "content": [{"type": "image", "data": "x"}, {"type": "text", "text": "  add\n  tests  "}]}}),
            json!({"type": "session_info", "id": "a3", "name": "First name"}),
            json!({"type": "session_info", "id": "a4", "name": "Auth refactor"}),
        ];
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let info = Pi::new(d.path()).find(A).unwrap().unwrap();
        assert_eq!((info.title.as_deref(), info.first_prompt.as_deref()), (Some("Auth refactor"), Some("add tests")));
        assert_eq!(info.cwd.as_deref(), Some("/w/a"));
    }

    /// The extension's record of each start: the latest wins, and one with a value that is not a
    /// string is not trusted (no record).
    #[test]
    fn launch_args_are_the_latest_record_of_the_extension() {
        let d = tempfile::tempdir().unwrap();
        let path = write_pi_session(d.path(), "/w/a", A, "hi");
        let pi = Pi::new(d.path());
        assert_eq!(pi.launch_args(A), None, "no record yet");
        let launch =
            |args: Value| json!({"type": "custom", "id": "c1", "customType": LAUNCH_ENTRY, "data": {"args": args}});
        let mut text = fs::read_to_string(&path).unwrap();
        for l in [launch(json!(["--tools", "read"])), launch(json!(["--model", "m", "--offline"]))] {
            text.push_str(&format!("{l}\n"));
        }
        fs::write(&path, &text).unwrap();
        assert_eq!(pi.launch_args(A), Some(vec!["--model".into(), "m".into(), "--offline".into()]));
        // The extension cmux's wrapper adds is not the user's: it is left out; the user's own stays.
        let cmux = "/tmp/cmux-pi-extensions/default/cmux-session.ts";
        fs::write(
            &path,
            format!("{text}{}\n", launch(json!(["-e", cmux, "-e", "/x/mine.ts", format!("--extension={cmux}")]))),
        )
        .unwrap();
        assert_eq!(Pi::new(d.path()).launch_args(A), Some(vec!["-e".into(), "/x/mine.ts".into()]));
        fs::write(&path, format!("{text}{}\n", launch(json!(["--tools", 5])))).unwrap();
        assert_eq!(Pi::new(d.path()).launch_args(A), None);
        assert_eq!(pi.launch_args(B), None);
    }

    #[test]
    fn the_extension_is_installed_and_removed_only_when_it_is_ours() {
        use crate::adapters::agents::claude::SkillRemoval;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("extensions");
        let path = install_extension(&dir).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains(LAUNCH_ENTRY));
        // The API key never reaches the record.
        assert!(EXTENSION.contains("\"--api-key\"") && EXTENSION.contains("\"--api-key=\""));
        install_extension(&dir).unwrap();
        assert_eq!(uninstall_extension(&dir).unwrap(), SkillRemoval::Removed);
        assert_eq!(uninstall_extension(&dir).unwrap(), SkillRemoval::Missing);
        fs::write(&path, "// mine\n").unwrap();
        assert_eq!(uninstall_extension(&dir).unwrap(), SkillRemoval::NotOurs);
        assert!(install_extension(&dir).is_err(), "someone else's file is not overwritten");
        assert_eq!(fs::read_to_string(&path).unwrap(), "// mine\n");
    }

    #[test]
    fn files_that_are_not_sessions_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        write_pi_session(d.path(), "/w/a", A, "real");
        let folder = d.path().join("--w-a--");
        fs::write(folder.join("2026-10-11T09-30-00-000Z_not-an-id.jsonl"), "{}\n").unwrap();
        fs::write(folder.join(format!("notes_{B}.txt")), "{}\n").unwrap();
        let ids: Vec<String> = Pi::new(d.path()).scan(None).unwrap().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, [A]);
    }
}
