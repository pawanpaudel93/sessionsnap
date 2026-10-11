use anyhow::Result;

use crate::domain::model::{Tab, Workspace, find_by_name, has_control};
use crate::ports::{AgentCatalog, OpenState, OpenTab, OpenWorkspace};

/// Turns open workspaces (all, or those named in `only`) into snapshot workspaces, with notes on
/// what was left out. Agent tabs take their title from the agent's session history when it has
/// one, else the tab's title.
pub fn capture(open: &OpenState, agents: &dyn AgentCatalog, only: &[String]) -> Result<(Vec<Workspace>, Vec<String>)> {
    let chosen: Vec<&OpenWorkspace> = if only.is_empty() {
        open.workspaces.iter().collect()
    } else {
        let names: Vec<&str> = open.workspaces.iter().map(|w| w.name.as_str()).collect();
        only.iter()
            .map(|q| find_by_name(&names, q, "open workspace").map(|i| &open.workspaces[i]))
            .collect::<Result<_>>()?
    };
    let mut used: Vec<String> = vec![];
    let mut notes = vec![];
    let workspaces = chosen
        .into_iter()
        .map(|w| {
            let name = unique_name(&w.name, &mut used);
            let tabs = w
                .tabs
                .iter()
                .map(|t| to_tab(t, agents, &mut |n| notes.push(format!("{name} › {}: {n}", t.title))))
                .collect();
            Workspace {
                name,
                color: None,
                cwd: w.cwd.clone(),
                git_branch: w.git_branch.clone(),
                cmux_workspace_id: Some(w.id.clone()),
                tabs,
            }
        })
        .collect();
    Ok((workspaces, notes))
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

/// An agent the catalog does not know is kept as it is, so a later sessionsnap can resume it. An
/// agent record with a control character in its kind, ID or flags is left out (`note` says so):
/// the tab is saved as a plain terminal.
fn to_tab(t: &OpenTab, agents: &dyn AgentCatalog, note: &mut dyn FnMut(&str)) -> Tab {
    let agent = t.agent.as_ref().filter(|a| {
        let bad = has_control(&a.kind) || has_control(&a.session_id) || a.args.iter().any(|x| has_control(x));
        if bad {
            note("left out its agent session (control characters in its kind, ID or flags)");
        }
        !bad
    });
    match agent {
        Some(a) => Tab::Agent {
            title: agents
                .history(&a.kind)
                .and_then(|h| h.title_for(&t.cwd, &a.session_id))
                .unwrap_or_else(|| t.title.clone()),
            cwd: t.cwd.clone(),
            agent: agents.kind_of(&a.kind).map_or_else(|| a.kind.clone(), str::to_string),
            session_id: a.session_id.clone(),
            args: a.args.clone(),
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
    use crate::ports::AgentRef;
    use crate::testing::{A, B, C, FakeHistory, agent_tab, agents, open_ws};

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
        let history = agents(vec![FakeHistory::with(&[(A, "/w/a", "login fix")])]);
        let ws = capture(&open_state(), &history, &[]).unwrap().0;
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].cmux_workspace_id.as_deref(), Some("WA"));
        assert_eq!(ws[0].tabs[0], agent_tab("claude", "login fix", "/w/a", A));
        assert_eq!(ws[0].tabs[1], Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None });
    }

    #[test]
    fn captures_any_agent_with_its_flags() {
        let history = agents(vec![FakeHistory::codex(&[(B, "/w/a", "codex title")])]);
        let mut open = open_state();
        let tab = |kind: &str, id: &str, args: &[&str]| OpenTab {
            reference: None,
            title: format!("{kind} tab"),
            cwd: "/w/a".into(),
            agent: Some(AgentRef {
                kind: kind.into(),
                session_id: id.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
            }),
        };
        open.workspaces[1].tabs = vec![tab("codex", B, &["-a", "on-request"]), tab("aider", C, &["--x"])];
        let ws = capture(&open, &history, &["Beta".into()]).unwrap().0;
        assert_eq!(
            ws[0].tabs,
            [
                Tab::Agent {
                    title: "codex title".into(),
                    cwd: "/w/a".into(),
                    agent: "codex".into(),
                    session_id: B.into(),
                    args: vec!["-a".into(), "on-request".into()],
                },
                // Unknown to sessionsnap: kept as cmux reported it, titled from the tab.
                Tab::Agent {
                    title: "aider tab".into(),
                    cwd: "/w/a".into(),
                    agent: "aider".into(),
                    session_id: C.into(),
                    args: vec!["--x".into()],
                },
            ]
        );
    }

    #[test]
    fn leaves_out_agent_records_with_control_characters() {
        let history = agents(vec![]);
        let mut open = open_state();
        open.workspaces[1].tabs = vec![OpenTab {
            reference: None,
            title: "bad".into(),
            cwd: "/w/b".into(),
            agent: Some(AgentRef { kind: "codex".into(), session_id: A.into(), args: vec!["\u{3}x\r".into()] }),
        }];
        let (ws, notes) = capture(&open, &history, &[]).unwrap();
        assert_eq!(ws[1].tabs, [Tab::Terminal { title: "bad".into(), cwd: "/w/b".into(), command: None }]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("Beta › bad: left out its agent session"), "{notes:?}");
    }

    #[test]
    fn workspace_named_sessionsnap_is_captured() {
        let history = agents(vec![]);
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("W1", "Alpha", "/w/a", &[]), open_ws("W2", "sessionsnap", "~", &[])],
        };
        let all = capture(&open, &history, &[]).unwrap().0;
        assert_eq!(all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Alpha", "sessionsnap"]);
    }

    #[test]
    fn captured_names_are_unique() {
        let history = agents(vec![]);
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![
                open_ws("W1", "Alpha", "/w/a", &[]),
                open_ws("W2", "Alpha", "/w/b", &[]),
                open_ws("W3", "", "/w/c", &[]),
            ],
        };
        let ws = capture(&open, &history, &[]).unwrap().0;
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Alpha (2)", "workspace"]);
    }

    #[test]
    fn carry_over_keeps_color_and_terminal_commands() {
        let history = agents(vec![]);
        let mut prev = capture(&open_state(), &history, &[]).unwrap().0;
        prev[0].color = Some("blue".into());
        prev[0].tabs[1] = Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: Some("make dev".into()) };
        let fresh = capture(&open_state(), &history, &[]).unwrap().0;
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
        let history = agents(vec![]);
        let only_beta = capture(&open_state(), &history, &["beta".to_string()]).unwrap().0;
        assert_eq!(only_beta.len(), 1);
        let mut base = capture(&open_state(), &history, &[]).unwrap().0;
        base[1].color = Some("blue".into());
        let mut changed = only_beta;
        changed[0].cwd = "/w/b2".into();
        let merged = merge(&base, changed);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].cwd, "/w/b2");
        assert_eq!(merged[1].color.as_deref(), Some("blue"));
    }
}
