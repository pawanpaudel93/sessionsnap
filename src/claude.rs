use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::model::is_session_id;
use crate::store::home_dir;

/// Claude Code's data directory (`~/.claude`); transcripts are `projects/<project>/<id>.jsonl`.
pub struct Claude {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: Option<String>,
    /// The session's name (`/rename`, `claude --name`), else Claude's automatic title.
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    /// "cli" for interactive sessions; other values for `claude -p` and SDK runs.
    pub entrypoint: Option<String>,
    /// Last change, RFC 3339 local time.
    pub modified: String,
    #[serde(skip)]
    pub modified_at: SystemTime,
    #[serde(skip)]
    pub path: PathBuf,
}

impl SessionInfo {
    /// Best label: title, then first prompt, then the ID.
    pub fn label(&self) -> String {
        self.title.clone().or_else(|| self.first_prompt.clone()).unwrap_or_else(|| self.session_id.clone())
    }
}

impl Claude {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Claude { root: root.into() }
    }

    /// `$CCSNAP_CLAUDE_DIR`, else `~/.claude`.
    pub fn open_default() -> Result<Self> {
        if let Some(p) = std::env::var_os("CCSNAP_CLAUDE_DIR") {
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

    pub fn title_for(&self, cwd: &str, session_id: &str) -> Option<String> {
        let direct = self.transcript_path(cwd, session_id);
        let path = if direct.is_file() { direct } else { self.find_transcript(session_id)? };
        self.info(&path).ok().and_then(|i| i.title.or(i.first_prompt))
    }

    /// Reads a transcript's folder, name, first real prompt and entrypoint.
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
        Ok(SessionInfo {
            session_id,
            cwd,
            title: custom.or(ai),
            first_prompt: first,
            entrypoint,
            modified,
            modified_at,
            path: path.to_path_buf(),
        })
    }

    /// Sessions whose transcript changed within `within` (all when None), newest first.
    pub fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>> {
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
        assert_eq!(i.entrypoint.as_deref(), Some("cli"));
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
}
