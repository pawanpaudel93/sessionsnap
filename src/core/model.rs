use serde::{Deserialize, Serialize};

use crate::error::{Exit, failure};

/// Version of sessionsnap's own file format.
pub const FORMAT: u32 = 1;

/// One immutable version of a named snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub sessionsnap: u32,
    pub snapshot: String,
    pub version: u32,
    pub created_at: String,
    pub message: String,
    pub source: Source,
    pub workspaces: Vec<Workspace>,
}

/// Where a version came from: "live" (captured from cmux), "edit", or "copy".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmux_state_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmux_workspace_id: Option<String>,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Tab {
    Claude {
        title: String,
        cwd: String,
        session_id: String,
    },
    Terminal {
        title: String,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
}

impl Tab {
    pub fn title(&self) -> &str {
        match self {
            Tab::Claude { title, .. } | Tab::Terminal { title, .. } => title,
        }
    }

    pub fn cwd(&self) -> &str {
        match self {
            Tab::Claude { cwd, .. } | Tab::Terminal { cwd, .. } => cwd,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Tab::Claude { session_id, .. } => Some(session_id),
            Tab::Terminal { .. } => None,
        }
    }

    pub fn set_title(&mut self, new: String) {
        match self {
            Tab::Claude { title, .. } | Tab::Terminal { title, .. } => *title = new,
        }
    }

    pub fn set_cwd(&mut self, new: String) {
        match self {
            Tab::Claude { cwd, .. } | Tab::Terminal { cwd, .. } => *cwd = new,
        }
    }

    /// Identity used by diffs: the session for Claude tabs, folder + command for terminals.
    pub fn key(&self) -> String {
        match self {
            Tab::Claude { session_id, .. } => format!("claude:{session_id}"),
            Tab::Terminal { cwd, command, .. } => format!("terminal:{cwd}|{}", command.as_deref().unwrap_or("")),
        }
    }
}

/// True for Claude Code session IDs (UUIDs), the only values sessionsnap passes to `claude --resume`.
pub fn is_session_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(
            |(i, c)| {
                if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() }
            },
        )
}

pub fn tab_count(workspaces: &[Workspace]) -> usize {
    workspaces.iter().map(|w| w.tabs.len()).sum()
}

/// Index of the name matching `query`: an exact case-insensitive match wins, otherwise a unique
/// case-insensitive substring. `noun` names the items in error messages.
pub fn find_by_name(names: &[&str], query: &str, noun: &str) -> anyhow::Result<usize> {
    let q = query.to_lowercase();
    if let Some(i) = names.iter().position(|n| n.to_lowercase() == q) {
        return Ok(i);
    }
    let hits: Vec<usize> =
        names.iter().enumerate().filter(|(_, n)| n.to_lowercase().contains(&q)).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => Err(failure(Exit::NotFound, format!("no {noun} matches '{query}'"))),
        many => Err(failure(
            Exit::Error,
            format!(
                "'{query}' matches several {noun}s: {}",
                many.iter().map(|i| names[*i]).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Exit, exit_of};

    const ID: &str = "97aeb422-20c1-41f7-a9c6-0f7c5a658991";

    #[test]
    fn claude_tab_json_shape() {
        let t = Tab::Claude { title: "fix".into(), cwd: "/w".into(), session_id: ID.into() };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v, serde_json::json!({"type": "claude", "title": "fix", "cwd": "/w", "sessionId": ID}));
        assert_eq!(serde_json::from_value::<Tab>(v).unwrap(), t);
    }

    #[test]
    fn terminal_tab_omits_missing_command() {
        let t = Tab::Terminal { title: "sh".into(), cwd: "/w".into(), command: None };
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!({"type": "terminal", "title": "sh", "cwd": "/w"})
        );
    }

    #[test]
    fn session_id_validation() {
        assert!(is_session_id(ID));
        assert!(!is_session_id("work"));
        assert!(!is_session_id("97aeb422-20c1-41f7-a9c6-0f7c5a65899z"));
        assert!(!is_session_id("97aeb422;rm -rf ~;20c1-41f7-a9c6-0f7c"));
    }

    #[test]
    fn find_by_name_prefers_exact_then_unique_substring() {
        let names = ["Cascadia", "Cascadia Community", "skills", "my-skills"];
        assert_eq!(find_by_name(&names, "cascadia", "workspace").unwrap(), 0);
        assert_eq!(find_by_name(&names, "community", "workspace").unwrap(), 1);
        assert_eq!(find_by_name(&names, "skills", "workspace").unwrap(), 2);
        assert!(find_by_name(&names, "skill", "workspace").unwrap_err().to_string().contains("several"));
        assert_eq!(exit_of(&find_by_name(&names, "nope", "workspace").unwrap_err()), Exit::NotFound);
    }
}
