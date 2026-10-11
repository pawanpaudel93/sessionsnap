use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::clean_title;
use crate::core::model::is_session_id;
use crate::error::{Exit, failure};
use crate::store::home_dir;

/// cmux state-file versions sessionsnap understands.
pub const KNOWN_VERSIONS: &[u64] = &[1];

#[derive(Debug, Clone, PartialEq)]
pub struct FileWorkspace {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    /// Terminal panels in tab order.
    pub panels: Vec<FilePanel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilePanel {
    pub tty: Option<String>,
    pub title: String,
    pub cwd: String,
    pub session_id: Option<String>,
}

/// `$SESSIONSNAP_CMUX_STATE`, else cmux's state file in Application Support.
pub fn default_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SESSIONSNAP_CMUX_STATE") {
        return Ok(PathBuf::from(p));
    }
    Ok(home_dir()?.join("Library/Application Support/cmux/session-com.cmuxterm.app.json"))
}

pub fn read(path: &Path) -> Result<(u64, Vec<FileWorkspace>)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading cmux state {}", path.display()))?;
    parse(&text)
}

/// Parses cmux's state file. Only the fields sessionsnap needs are read; nothing else is kept.
pub fn parse(text: &str) -> Result<(u64, Vec<FileWorkspace>)> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| failure(Exit::UnknownFormat, format!("cmux state is not valid JSON: {e}")))?;
    let version = root
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| failure(Exit::UnknownFormat, "cmux state has no version"))?;
    if !KNOWN_VERSIONS.contains(&version) {
        return Err(failure(
            Exit::UnknownFormat,
            format!("cmux state version {version} is not supported (known: {KNOWN_VERSIONS:?}); update sessionsnap"),
        ));
    }
    let windows = root
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| failure(Exit::UnknownFormat, "cmux state has no windows"))?;
    let mut out = vec![];
    for window in windows {
        let Some(workspaces) = window.pointer("/tabManager/workspaces").and_then(Value::as_array) else { continue };
        for ws in workspaces {
            out.push(parse_workspace(ws)?);
        }
    }
    Ok((version, out))
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn parse_workspace(ws: &Value) -> Result<FileWorkspace> {
    let id = s(ws, "workspaceId").ok_or_else(|| failure(Exit::UnknownFormat, "workspace without workspaceId"))?;
    let cwd = s(ws, "currentDirectory").unwrap_or_default();
    let name = s(ws, "customTitle")
        .or_else(|| s(ws, "processTitle"))
        .map(|n| clean_title(&n))
        .unwrap_or_else(|| cwd.rsplit('/').next().unwrap_or("workspace").to_string());
    let git_branch = ws.pointer("/gitBranch/branch").and_then(Value::as_str).map(str::to_string);
    let panels_json = ws.get("panels").and_then(Value::as_array).cloned().unwrap_or_default();

    let mut order = vec![];
    if let Some(layout) = ws.get("layout") {
        collect_panel_ids(layout, &mut order);
    }
    let mut used = HashSet::new();
    let mut panels = vec![];
    for id in &order {
        let found = panels_json.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(id.as_str()));
        if let Some(p) = found
            && used.insert(id.clone())
        {
            panels.extend(parse_panel(p));
        }
    }
    for p in &panels_json {
        if used.insert(s(p, "id").unwrap_or_default()) {
            panels.extend(parse_panel(p));
        }
    }
    Ok(FileWorkspace { id, name, cwd, git_branch, panels })
}

/// Collects `panelIds` from a layout tree in order, whatever its split shape.
fn collect_panel_ids(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(ids) = map.get("panelIds").and_then(Value::as_array) {
                out.extend(ids.iter().filter_map(Value::as_str).map(str::to_string));
            }
            for (key, child) in map {
                if key != "panelIds" {
                    collect_panel_ids(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_panel_ids(i, out)),
        _ => {}
    }
}

/// Terminal panels only; the Claude session comes from `agent`, else from a claude `resumeBinding`.
fn parse_panel(p: &Value) -> Option<FilePanel> {
    if p.get("type").and_then(Value::as_str) != Some("terminal") {
        return None;
    }
    let term = p.get("terminal");
    let agent = term.and_then(|t| t.get("agent")).filter(|a| a.get("kind").and_then(Value::as_str) == Some("claude"));
    let binding =
        term.and_then(|t| t.get("resumeBinding")).filter(|b| b.get("kind").and_then(Value::as_str) == Some("claude"));
    let session_id = agent
        .and_then(|a| s(a, "sessionId"))
        .or_else(|| binding.and_then(|b| s(b, "checkpointId")))
        .filter(|id| is_session_id(id));
    let cwd = agent
        .and_then(|a| s(a, "workingDirectory"))
        .or_else(|| s(p, "directory"))
        .or_else(|| term.and_then(|t| s(t, "workingDirectory")))
        .unwrap_or_default();
    Some(FilePanel { tty: s(p, "ttyName"), title: s(p, "title").unwrap_or_default(), cwd, session_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;
    use crate::testing::{A, B};

    const FIXTURE: &str = include_str!("../../tests/fixtures/cmux-state.json");

    #[test]
    fn parses_fixture_in_tab_order() {
        let (version, ws) = parse(FIXTURE).unwrap();
        assert_eq!(version, 1);
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "beta-proc"]);
        let alpha = &ws[0];
        assert_eq!(alpha.id, "WS-ALPHA");
        assert_eq!(alpha.git_branch.as_deref(), Some("main"));
        assert_eq!(alpha.panels.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), vec!["zsh", "✳ fix login"]);
        assert_eq!(alpha.panels[1].session_id.as_deref(), Some(A));
        assert_eq!(alpha.panels[1].cwd, "/w/alpha/sub");
        assert_eq!(alpha.panels[1].tty.as_deref(), Some("ttys001"));
        assert_eq!(alpha.panels[0].session_id, None);
        // resumeBinding fallback, even with autoResume false (cmux #13021)
        assert_eq!(ws[1].panels[0].session_id.as_deref(), Some(B));
    }

    #[test]
    fn rejects_unknown_version() {
        let text = FIXTURE.replacen("\"version\": 1", "\"version\": 2", 1);
        assert_eq!(exit_of(&parse(&text).unwrap_err()), Exit::UnknownFormat);
    }

    #[test]
    fn never_keeps_environment() {
        let (_, ws) = parse(FIXTURE).unwrap();
        assert!(!format!("{ws:?}").contains("do-not-copy"));
    }
}
