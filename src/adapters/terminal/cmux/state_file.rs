use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::clean_title;
use crate::adapters::home_dir;
use crate::error::{Exit, failure};
use crate::ports::AgentRef;

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
    pub agent: Option<AgentRef>,
}

/// `$SESSIONSNAP_CMUX_STATE`, else cmux's state file for this OS.
pub fn default_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("SESSIONSNAP_CMUX_STATE") {
        return Ok(PathBuf::from(p));
    }
    let xdg_state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    Ok(path_for(std::env::consts::OS, &home_dir()?, xdg_state.as_deref()))
}

/// Where cmux keeps its state file on `os`. macOS: Application Support (checked against cmux
/// 0.64). Elsewhere: the XDG state folder; cmux's Linux app has not been checked, so this is a
/// guess that `SESSIONSNAP_CMUX_STATE` overrides.
pub fn path_for(os: &str, home: &Path, xdg_state: Option<&Path>) -> PathBuf {
    const FILE: &str = "session-com.cmuxterm.app.json";
    if os == "macos" {
        return home.join("Library/Application Support/cmux").join(FILE);
    }
    xdg_state.map_or_else(|| home.join(".local/state"), Path::to_path_buf).join("cmux").join(FILE)
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

/// Terminal panels only. The agent session comes from `terminal.agent` (any kind; its launch flags
/// are `launchCommand.arguments` after the program), else from a `resumeBinding`. Agent kinds and
/// session IDs are passed on as cmux wrote them; sessionsnap checks them before resuming.
fn parse_panel(p: &Value) -> Option<FilePanel> {
    if p.get("type").and_then(Value::as_str) != Some("terminal") {
        return None;
    }
    let term = p.get("terminal");
    let agent = term.and_then(|t| t.get("agent")).filter(|a| s(a, "kind").is_some() && s(a, "sessionId").is_some());
    let binding = term.and_then(|t| t.get("resumeBinding")).and_then(|b| {
        Some(AgentRef {
            kind: s(b, "kind")?,
            session_id: s(b, "checkpointId").or_else(|| s(b, "sessionId"))?,
            args: vec![],
            alt_session_id: None,
        })
    });
    let from_agent = agent.map(|a| {
        let kind = s(a, "kind").unwrap_or_default();
        let session_id = s(a, "sessionId").unwrap_or_default();
        // The binding's ID for the same agent, in case the agent record's ID turns out invalid.
        let alt_session_id = binding
            .as_ref()
            .filter(|b| b.kind.eq_ignore_ascii_case(&kind) && b.session_id != session_id)
            .map(|b| b.session_id.clone());
        AgentRef { kind, session_id, args: launch_args(a), alt_session_id }
    });
    let cwd = agent
        .and_then(|a| s(a, "workingDirectory"))
        .or_else(|| s(p, "directory"))
        .or_else(|| term.and_then(|t| s(t, "workingDirectory")))
        .unwrap_or_default();
    Some(FilePanel {
        tty: s(p, "ttyName"),
        title: s(p, "title").unwrap_or_default(),
        cwd,
        agent: from_agent.or(binding),
    })
}

/// `launchCommand.arguments` after the program; none at all when one of them is not a string
/// (dropping just that one would shift flags and their values).
fn launch_args(agent: &Value) -> Vec<String> {
    let Some(args) = agent.pointer("/launchCommand/arguments").and_then(Value::as_array) else { return vec![] };
    let strings: Option<Vec<String>> = args.iter().skip(1).map(|a| a.as_str().map(str::to_string)).collect();
    strings.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;
    use crate::testing::{A, B};

    const CODEX: &str = "01a12bb7-d379-7541-a81a-7895725dc6dd";
    const FIXTURE: &str = include_str!("../../../../tests/fixtures/cmux-state.json");

    #[test]
    fn parses_fixture_in_tab_order() {
        let (version, ws) = parse(FIXTURE).unwrap();
        assert_eq!(version, 1);
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "beta-proc"]);
        let alpha = &ws[0];
        assert_eq!(alpha.id, "WS-ALPHA");
        assert_eq!(alpha.git_branch.as_deref(), Some("main"));
        assert_eq!(alpha.panels.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), vec!["zsh", "✳ fix login"]);
        let claude = alpha.panels[1].agent.as_ref().unwrap();
        assert_eq!((claude.kind.as_str(), claude.session_id.as_str()), ("claude", A));
        assert_eq!(claude.args, ["--model", "opus"]);
        assert_eq!(alpha.panels[1].cwd, "/w/alpha/sub");
        assert_eq!(alpha.panels[1].tty.as_deref(), Some("ttys001"));
        assert_eq!(alpha.panels[0].agent, None);
        // resumeBinding fallback, even with autoResume false (cmux #13021)
        let beta = &ws[1].panels;
        assert_eq!(
            beta[0].agent,
            Some(AgentRef { kind: "claude".into(), session_id: B.into(), args: vec![], alt_session_id: None })
        );
        // Any agent kind, with its launch flags (the program is left out).
        assert_eq!(
            beta[1].agent,
            Some(AgentRef {
                kind: "codex".into(),
                session_id: CODEX.into(),
                args: ["-a", "on-request", "-s", "workspace-write"].map(String::from).to_vec(),
                alt_session_id: None,
            })
        );
        assert_eq!(beta[1].cwd, "/w/beta/api");
        // A kind sessionsnap does not know is passed on as is.
        assert_eq!(beta[2].agent.as_ref().map(|a| (a.kind.as_str(), a.session_id.as_str())), Some(("aider", "chat-7")));
        // An agent record without a session ID is no agent.
        assert_eq!(beta[3].agent, None);
    }

    #[test]
    fn keeps_the_binding_id_and_ignores_odd_arguments() {
        let text = r#"{"version": 1, "windows": [{"tabManager": {"workspaces": [{"workspaceId": "W", "panels": [
            {"id": "P1", "type": "terminal", "terminal": {
              "agent": {"kind": "claude", "sessionId": "not-a-uuid", "launchCommand": {"arguments": ["claude", "--model", 5, "opus"]}},
              "resumeBinding": {"kind": "claude", "checkpointId": "11111111-1111-4111-8111-111111111111"}}},
            {"id": "P2", "type": "terminal", "terminal": {
              "resumeBinding": {"kind": "opencode", "sessionId": "ses_1", "source": "agent-hook"}}}
        ]}]}}]}"#;
        let (_, ws) = parse(text).unwrap();
        let a = ws[0].panels[0].agent.clone().unwrap();
        assert_eq!(a.session_id, "not-a-uuid");
        assert_eq!(a.alt_session_id.as_deref(), Some(A));
        assert!(a.args.is_empty(), "a non-string argument drops them all: {:?}", a.args);
        // A binding alone (any kind) is an agent too.
        let b = ws[0].panels[1].agent.clone().unwrap();
        assert_eq!((b.kind.as_str(), b.session_id.as_str()), ("opencode", "ses_1"));
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

    #[test]
    fn state_file_path_per_os() {
        let home = Path::new("/home/u");
        assert_eq!(
            path_for("macos", home, Some(Path::new("/x"))),
            Path::new("/home/u/Library/Application Support/cmux/session-com.cmuxterm.app.json")
        );
        assert_eq!(path_for("linux", home, None), Path::new("/home/u/.local/state/cmux/session-com.cmuxterm.app.json"));
        assert_eq!(path_for("linux", home, Some(Path::new("/s"))), Path::new("/s/cmux/session-com.cmuxterm.app.json"));
    }
}
