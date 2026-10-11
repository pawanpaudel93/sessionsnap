use anyhow::Result;

use crate::adapters::agents::claude::Claude;
use crate::adapters::terminal::cmux::{OpenState, OpenTab, OpenWorkspace};
use crate::core::model::{Tab, Workspace, find_by_name};

/// Turns open workspaces (all, or those named in `only`) into snapshot workspaces. Claude tabs
/// take their title from the transcript (session name, else automatic title).
pub fn capture(open: &OpenState, claude: &Claude, only: &[String]) -> Result<Vec<Workspace>> {
    let chosen: Vec<&OpenWorkspace> = if only.is_empty() {
        open.workspaces.iter().collect()
    } else {
        let names: Vec<&str> = open.workspaces.iter().map(|w| w.name.as_str()).collect();
        only.iter()
            .map(|q| find_by_name(&names, q, "open workspace").map(|i| &open.workspaces[i]))
            .collect::<Result<_>>()?
    };
    let mut used: Vec<String> = vec![];
    Ok(chosen
        .into_iter()
        .map(|w| Workspace {
            name: unique_name(&w.name, &mut used),
            color: None,
            cwd: w.cwd.clone(),
            git_branch: w.git_branch.clone(),
            cmux_workspace_id: Some(w.id.clone()),
            tabs: w.tabs.iter().map(|t| to_tab(t, claude)).collect(),
        })
        .collect())
}

/// Non-empty and unique (case-insensitive) within one capture: "workspace", "Alpha", "Alpha (2)", …
fn unique_name(name: &str, used: &mut Vec<String>) -> String {
    let base = if name.trim().is_empty() { "workspace" } else { name };
    let mut candidate = base.to_string();
    let mut n = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&candidate)) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    used.push(candidate.clone());
    candidate
}

fn to_tab(t: &OpenTab, claude: &Claude) -> Tab {
    match &t.session_id {
        Some(id) => Tab::Claude {
            title: claude.title_for(&t.cwd, id).unwrap_or_else(|| t.title.clone()),
            cwd: t.cwd.clone(),
            session_id: id.clone(),
        },
        None => Tab::Terminal { title: t.title.clone(), cwd: t.cwd.clone(), command: None },
    }
}

/// Replaces same-named workspaces of `base` with `captured` (keeping their color); appends new ones.
pub fn merge(base: &[Workspace], captured: Vec<Workspace>) -> Vec<Workspace> {
    let mut out = base.to_vec();
    for mut ws in captured {
        match out.iter().position(|b| b.name.eq_ignore_ascii_case(&ws.name)) {
            Some(i) => {
                ws.color = out[i].color.take().or(ws.color);
                out[i] = ws;
            }
            None => out.push(ws),
        }
    }
    out
}

/// For a full save: keeps what cmux cannot tell us from the previous version's same-named workspace
/// (its color, and the command of a terminal tab with the same folder and title).
pub fn carry_over(previous: &[Workspace], captured: Vec<Workspace>) -> Vec<Workspace> {
    captured
        .into_iter()
        .map(|mut ws| {
            let Some(prev) = previous.iter().find(|p| p.name.eq_ignore_ascii_case(&ws.name)) else {
                return ws;
            };
            if ws.color.is_none() {
                ws.color = prev.color.clone();
            }
            for tab in &mut ws.tabs {
                if let Tab::Terminal { title, cwd, command: command @ None } = tab {
                    *command = prev.tabs.iter().find_map(|p| match p {
                        Tab::Terminal { title: pt, cwd: pc, command } if pt == title && pc == cwd => command.clone(),
                        _ => None,
                    });
                }
            }
            ws
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{A, open_ws, write_transcript};

    fn open_state() -> OpenState {
        OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![
                open_ws("WA", "Alpha", "/w/a", &[("fix login", "/w/a", Some(A)), ("zsh", "/w/a", None)]),
                open_ws("WB", "Beta", "/w/b", &[]),
            ],
        }
    }

    #[test]
    fn captures_tabs_with_transcript_titles() {
        let d = tempfile::tempdir().unwrap();
        let claude = Claude::new(d.path());
        write_transcript(d.path(), "/w/a", A, "login fix");
        let ws = capture(&open_state(), &claude, &[]).unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].cmux_workspace_id.as_deref(), Some("WA"));
        assert_eq!(ws[0].tabs[0], Tab::Claude { title: "login fix".into(), cwd: "/w/a".into(), session_id: A.into() });
        assert_eq!(ws[0].tabs[1], Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None });
    }

    #[test]
    fn workspace_named_sessionsnap_is_captured() {
        let d = tempfile::tempdir().unwrap();
        let claude = Claude::new(d.path());
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("W1", "Alpha", "/w/a", &[]), open_ws("W2", "sessionsnap", "~", &[])],
        };
        let all = capture(&open, &claude, &[]).unwrap();
        assert_eq!(all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Alpha", "sessionsnap"]);
    }

    #[test]
    fn captured_names_are_unique() {
        let d = tempfile::tempdir().unwrap();
        let claude = Claude::new(d.path());
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![
                open_ws("W1", "Alpha", "/w/a", &[]),
                open_ws("W2", "Alpha", "/w/b", &[]),
                open_ws("W3", "", "/w/c", &[]),
            ],
        };
        let ws = capture(&open, &claude, &[]).unwrap();
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Alpha (2)", "workspace"]);
    }

    #[test]
    fn carry_over_keeps_color_and_terminal_commands() {
        let d = tempfile::tempdir().unwrap();
        let claude = Claude::new(d.path());
        let mut prev = capture(&open_state(), &claude, &[]).unwrap();
        prev[0].color = Some("blue".into());
        prev[0].tabs[1] = Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: Some("make dev".into()) };
        let fresh = capture(&open_state(), &claude, &[]).unwrap();
        let out = carry_over(&prev, fresh);
        assert_eq!(out[0].color.as_deref(), Some("blue"));
        assert_eq!(
            out[0].tabs[1],
            Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: Some("make dev".into()) }
        );
        assert_eq!(out[1].color, None);
    }

    #[test]
    fn capture_filters_and_merge_replaces_by_name() {
        let d = tempfile::tempdir().unwrap();
        let claude = Claude::new(d.path());
        let only_beta = capture(&open_state(), &claude, &["beta".to_string()]).unwrap();
        assert_eq!(only_beta.len(), 1);
        let mut base = capture(&open_state(), &claude, &[]).unwrap();
        base[1].color = Some("blue".into());
        let mut changed = only_beta;
        changed[0].cwd = "/w/b2".into();
        let merged = merge(&base, changed);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].cwd, "/w/b2");
        assert_eq!(merged[1].color.as_deref(), Some("blue"));
    }
}
