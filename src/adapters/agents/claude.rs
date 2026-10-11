//! Claude Code: its session history (`~/.claude/projects`), how a saved tab resumes a session,
//! and the sessionsnap skill installed for it.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde_json::Value;

use crate::adapters::home_dir;
use crate::domain::model::{Tab, is_session_id};
use crate::ports::{SessionHistory, SessionInfo};

/// The registry kind of Claude Code.
pub const KIND: &str = "claude";

/// Claude Code's data directory (`~/.claude`); transcripts are `projects/<project>/<id>.jsonl`.
pub struct Claude {
    root: PathBuf,
}

impl Claude {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Claude { root: root.into() }
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

    /// Finds a session's transcript in any project.
    pub fn find_transcript(&self, session_id: &str) -> Option<PathBuf> {
        let name = format!("{session_id}.jsonl");
        fs::read_dir(self.projects()).ok()?.flatten().map(|e| e.path().join(&name)).find(|p| p.is_file())
    }

    /// Reads a transcript's folder, name and first real prompt, and whether it is a session a person
    /// ran: interactive (entrypoint "cli", not `claude -p` or an SDK run) and outside `~/.claude*`.
    pub fn info(&self, path: &Path) -> Result<SessionInfo> {
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let modified_at = fs::metadata(path)?.modified()?;
        let (mut cwd, mut custom, mut ai, mut first, mut entrypoint) = (None, None, None, None, None);
        for line in BufReader::new(fs::File::open(path)?).lines() {
            let Ok(line) = line else { break };
            let interesting = line.contains("\"custom-title\"")
                || line.contains("\"ai-title\"")
                || ((cwd.is_none() || first.is_none() || entrypoint.is_none()) && line.contains("\"type\":\"user\""));
            if !interesting {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v.get("type").and_then(Value::as_str) {
                Some("custom-title") => custom = str_field(&v, "customTitle").or(custom),
                Some("ai-title") => ai = str_field(&v, "aiTitle").or(ai),
                Some("user") => {
                    if cwd.is_none() {
                        cwd = str_field(&v, "cwd");
                    }
                    if entrypoint.is_none() {
                        entrypoint = str_field(&v, "entrypoint");
                    }
                    let meta = v.get("isMeta").and_then(Value::as_bool) == Some(true);
                    if first.is_none()
                        && !meta
                        && let Some(text) = v.pointer("/message/content").and_then(Value::as_str)
                    {
                        let text = text.trim();
                        if !text.is_empty() && !text.starts_with('<') {
                            first = Some(one_line(text, 120));
                        }
                    }
                }
                _ => {}
            }
        }
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
            recoverable: interactive && !internal,
        })
    }
}

impl SessionHistory for Claude {
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

/// The command a restored tab runs. Claude tabs only with a valid session ID.
pub fn tab_command(tab: &Tab) -> Option<String> {
    match tab {
        Tab::Claude { session_id, .. } if is_session_id(session_id) => Some(format!("claude --resume {session_id}")),
        Tab::Claude { .. } => None,
        Tab::Terminal { command, .. } => command.clone(),
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

pub fn install_skill(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("sessionsnap").join("SKILL.md");
    std::fs::create_dir_all(dir.join("sessionsnap"))?;
    std::fs::write(&path, SKILL)?;
    Ok(path)
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
    fn tab_command_only_for_valid_sessions() {
        let ok = Tab::Claude { title: "t".into(), cwd: "/w".into(), session_id: A.into() };
        assert_eq!(tab_command(&ok).unwrap(), format!("claude --resume {A}"));
        let bad = Tab::Claude { title: "t".into(), cwd: "/w".into(), session_id: "x; rm -rf ~".into() };
        assert_eq!(tab_command(&bad), None);
        let term = Tab::Terminal { title: "t".into(), cwd: "/w".into(), command: Some("npm run dev".into()) };
        assert_eq!(tab_command(&term).as_deref(), Some("npm run dev"));
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
}
