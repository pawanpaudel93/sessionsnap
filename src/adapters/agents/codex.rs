//! Codex: its session history (`~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`).
//! Archived sessions (`~/.codex/archived_sessions`) are ignored.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde_json::Value;

use crate::adapters::home_dir;
use crate::domain::model::is_session_id;
use crate::ports::{SessionHistory, SessionInfo};

/// The registry kind of Codex.
pub const KIND: &str = "codex";

/// Codex's data directory (`~/.codex`).
pub struct Codex {
    root: PathBuf,
}

impl Codex {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Codex { root: root.into() }
    }

    /// `$SESSIONSNAP_CODEX_DIR`, else `~/.codex`.
    pub fn open_default() -> Result<Self> {
        if let Some(p) = std::env::var_os("SESSIONSNAP_CODEX_DIR") {
            return Ok(Codex::new(p));
        }
        Ok(Codex::new(home_dir()?.join(".codex")))
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Every rollout file under `sessions/`, with its session ID (from the file name).
    fn rollouts(&self) -> Vec<(PathBuf, String)> {
        let mut out = vec![];
        walk(&self.sessions_dir(), 4, &mut |path| {
            if let Some(id) = rollout_id(path) {
                out.push((path.to_path_buf(), id.to_string()));
            }
        });
        out
    }

    fn find_rollout(&self, session_id: &str) -> Option<PathBuf> {
        if !is_session_id(session_id) {
            return None;
        }
        self.rollouts().into_iter().find(|(_, id)| id.eq_ignore_ascii_case(session_id)).map(|(p, _)| p)
    }

    /// Reads a rollout's session_meta (ID, folder, how it was started) and its first real prompt.
    pub fn info(&self, path: &Path) -> Result<SessionInfo> {
        let modified_at = fs::metadata(path)?.modified()?;
        let mut session_id = rollout_id(path).unwrap_or_default().to_string();
        let (mut cwd, mut first, mut scripted) = (None, None, false);
        for line in BufReader::new(fs::File::open(path)?).lines() {
            let Ok(line) = line else { break };
            let wanted =
                line.contains("\"session_meta\"") || (line.contains("\"response_item\"") && line.contains("\"user\""));
            if !wanted {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let payload = &v["payload"];
            match v.get("type").and_then(Value::as_str) {
                Some("session_meta") => {
                    if let Some(id) = payload.get("id").and_then(Value::as_str).filter(|id| is_session_id(id)) {
                        session_id = id.to_string();
                    }
                    cwd = payload.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
                    scripted = is_scripted(payload);
                }
                Some("response_item") if is_user_message(payload) => {
                    if let Some(text) = user_text(payload) {
                        first = Some(text);
                        break;
                    }
                }
                _ => {}
            }
        }
        let internal = match (home_dir(), &cwd) {
            (Ok(home), Some(c)) => c.starts_with(&format!("{}/.codex", home.to_string_lossy())),
            _ => false,
        };
        let modified =
            chrono::DateTime::<chrono::Local>::from(modified_at).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        Ok(SessionInfo {
            agent: KIND.to_string(),
            session_id,
            cwd,
            title: first.as_deref().map(|t| one_line(t, 80)),
            first_prompt: first.as_deref().map(|t| one_line(t, 120)),
            modified,
            modified_at,
            recoverable: !scripted && !internal,
        })
    }
}

impl SessionHistory for Codex {
    fn agent(&self) -> &'static str {
        KIND
    }

    fn contains(&self, session_id: &str) -> bool {
        self.find_rollout(session_id).is_some()
    }

    fn find(&self, session_id: &str) -> Result<Option<SessionInfo>> {
        match self.find_rollout(session_id) {
            Some(path) => self.info(&path).map(Some),
            None => Ok(None),
        }
    }

    /// Codex sessions resume from any folder, so `cwd` does not matter.
    fn title_for(&self, _cwd: &str, session_id: &str) -> Option<String> {
        self.find(session_id).ok().flatten().and_then(|i| i.title.or(i.first_prompt))
    }

    fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>> {
        let cutoff = within.and_then(|d| SystemTime::now().checked_sub(d));
        let mut out = vec![];
        for (path, _) in self.rollouts() {
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

/// Calls `f` for every file under `dir`, at most `depth` directories down.
fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && depth > 0 {
            walk(&entry.path(), depth - 1, f);
        } else if kind.is_file() {
            f(&entry.path());
        }
    }
}

/// The session ID in `rollout-<timestamp>-<id>.jsonl`.
fn rollout_id(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    is_session_id(id).then_some(id)
}

/// Started by `codex exec`, an MCP client or as a sub-agent, not by a person in a terminal.
fn is_scripted(meta: &Value) -> bool {
    let source = meta.get("source");
    let scripted_source = match source.and_then(Value::as_str) {
        Some(s) => matches!(s.to_ascii_lowercase().as_str(), "exec" | "mcp"),
        // A sub-agent's source is an object.
        None => source.is_some_and(|s| !s.is_null()),
    };
    let exec_originator =
        meta.get("originator").and_then(Value::as_str).is_some_and(|o| o.to_ascii_lowercase().contains("exec"));
    scripted_source || exec_originator
}

fn is_user_message(payload: &Value) -> bool {
    payload.get("type").and_then(Value::as_str) == Some("message")
        && payload.get("role").and_then(Value::as_str) == Some("user")
}

/// The message's text, unless it is context Codex injects (`<environment_context>`, AGENTS.md
/// instructions and the like, which start with '<').
fn user_text(payload: &Value) -> Option<String> {
    let text: String = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
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
    use crate::testing::{A, B, write_rollout};
    use serde_json::json;

    const C7: &str = "01a12bb7-d379-7541-a81a-7895725dc6dd";

    #[test]
    fn meets_the_session_history_contract() {
        let d = tempfile::tempdir().unwrap();
        write_rollout(d.path(), "/w/a", A, "fix the login bug");
        let old = write_rollout(d.path(), "/w/b", B, "review");
        contract::age(&old, Duration::from_secs(48 * 3600));
        let fixture = contract::Fixture {
            agent: "codex",
            recent: A,
            old: B,
            unknown: "44444444-4444-4444-8444-444444444444",
            cwd: "/w/a",
            title: "fix the login bug",
        };
        contract::check(&Codex::new(d.path()), &fixture);
    }

    fn write(root: &Path, id: &str, lines: &[Value]) -> PathBuf {
        let dir = root.join("sessions/2026/10/11");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-2026-10-11T09-30-00-{id}.jsonl"));
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        path
    }

    fn user(text: &str) -> Value {
        json!({"type": "response_item", "payload": {"type": "message", "role": "user",
               "content": [{"type": "input_text", "text": text}]}})
    }

    #[test]
    fn title_skips_injected_context_and_is_one_short_line() {
        let d = tempfile::tempdir().unwrap();
        let long = format!("  please   refactor\n the {} module  ", "x".repeat(100));
        write(
            d.path(),
            C7,
            &[
                json!({"type": "session_meta", "payload": {"id": C7, "cwd": "/w/x", "timestamp": "2026-10-11T09:30:00Z",
                       "source": "cli", "originator": "codex_cli_rs", "cli_version": "0.50.0"}}),
                user("<user_instructions>\n# AGENTS.md instructions\n</user_instructions>"),
                user("<environment_context>\n  <cwd>/w/x</cwd>\n</environment_context>"),
                json!({"type": "response_item", "payload": {"type": "message", "role": "assistant",
                       "content": [{"type": "output_text", "text": "hello"}]}}),
                user(&long),
                user("second prompt"),
            ],
        );
        let c = Codex::new(d.path());
        let info = c.find(C7).unwrap().unwrap();
        assert_eq!(info.agent, "codex");
        assert_eq!(info.cwd.as_deref(), Some("/w/x"));
        let title = info.title.unwrap();
        assert!(title.starts_with("please refactor the xxx"), "{title}");
        assert_eq!(title.chars().count(), 81, "80 characters and an ellipsis");
        assert!(info.recoverable);
    }

    #[test]
    fn scripted_and_internal_sessions_are_not_recoverable() {
        let d = tempfile::tempdir().unwrap();
        let c = Codex::new(d.path());
        let meta = |id: &str, extra: Value| {
            let mut p = json!({"id": id, "cwd": "/w/x"});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            json!({"type": "session_meta", "payload": p})
        };
        let ids = [
            "11111111-1111-4111-8111-11111111111a",
            "11111111-1111-4111-8111-11111111111b",
            "11111111-1111-4111-8111-11111111111c",
            "11111111-1111-4111-8111-11111111111d",
        ];
        write(d.path(), ids[0], &[meta(ids[0], json!({"source": "exec", "originator": "codex_exec"})), user("x")]);
        write(d.path(), ids[1], &[meta(ids[1], json!({"source": {"subagent": "review"}})), user("x")]);
        write(d.path(), ids[2], &[meta(ids[2], json!({"source": "vscode", "originator": "codex_vscode"})), user("x")]);
        let home = home_dir().unwrap().to_string_lossy().into_owned();
        write(
            d.path(),
            ids[3],
            &[
                json!({"type": "session_meta", "payload": {"id": ids[3], "cwd": format!("{home}/.codex/x"), "source": "cli"}}),
                user("x"),
            ],
        );
        let recoverable: Vec<bool> = ids.iter().map(|id| c.find(id).unwrap().unwrap().recoverable).collect();
        assert_eq!(recoverable, [false, false, true, false]);
    }

    #[test]
    fn ignores_other_files_and_archived_sessions() {
        let d = tempfile::tempdir().unwrap();
        write_rollout(d.path(), "/w/a", A, "real");
        fs::write(d.path().join("sessions/notes.jsonl"), "{}\n").unwrap();
        fs::create_dir_all(d.path().join("archived_sessions")).unwrap();
        fs::write(d.path().join(format!("archived_sessions/rollout-2026-01-01T00-00-00-{B}.jsonl")), "{}\n").unwrap();
        let c = Codex::new(d.path());
        assert_eq!(c.scan(None).unwrap().iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), [A]);
        assert!(!c.contains(B));
        assert!(!c.contains("../../etc"));
        assert_eq!(rollout_id(Path::new(&format!("rollout-2026-10-11T09-30-00-{C7}.jsonl"))), Some(C7));
        assert_eq!(rollout_id(Path::new("rollout-x.jsonl")), None);
    }
}
