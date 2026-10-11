use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::agents::{Resume, find_session, problem, resume_status};
use crate::domain::capture;
use crate::domain::model::{SessionRef, Tab, Workspace, find_by_name, has_control, parse_ref};
use crate::error::{Exit, failure};
use crate::ports::{AgentCatalog, OpenState, Snapshots};

/// One change to a saved snapshot. Positions (`index`) are 1-based.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Op {
    WsRename {
        ws: String,
        to: String,
    },
    WsColor {
        ws: String,
        #[serde(default)]
        color: Option<String>,
    },
    WsIndex {
        ws: String,
        index: usize,
    },
    WsRemove {
        ws: String,
    },
    WsAddLive {
        ws: String,
    },
    WsFrom {
        from: String,
        ws: String,
    },
    TabRemove {
        tab: String,
    },
    TabRename {
        tab: String,
        to: String,
    },
    TabIndex {
        tab: String,
        index: usize,
    },
    TabMove {
        tab: String,
        to: String,
    },
    /// Adds an agent tab. Without `agent`, the session is looked up in every agent history.
    TabAddSession {
        ws: String,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    TabAddTerminal {
        ws: String,
        cwd: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    TabSetCwd {
        tab: String,
        cwd: String,
    },
    /// Points a tab at another agent session (found as for `TabAddSession`).
    TabSetSession {
        tab: String,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TabSel {
    Position(usize),
    Title(String),
}

/// `<workspace>#<position>` or `<workspace>:"<title text>"`.
#[derive(Debug, Clone, PartialEq)]
pub struct TabRef {
    pub ws: String,
    pub sel: TabSel,
}

pub fn parse_tab_ref(s: &str) -> Result<TabRef> {
    if let Some((ws, pos)) = s.rsplit_once('#')
        && let Ok(n) = pos.parse::<usize>()
        && n >= 1
    {
        return Ok(TabRef { ws: ws.to_string(), sel: TabSel::Position(n) });
    }
    if let Some((ws, text)) = s.split_once(':') {
        let text = text.trim().trim_matches('"');
        if !text.is_empty() {
            return Ok(TabRef { ws: ws.to_string(), sel: TabSel::Title(text.to_string()) });
        }
    }
    Err(failure(Exit::Error, format!("invalid tab '{s}': use <workspace>#<position> or <workspace>:\"<title text>\"")))
}

pub fn find_ws(workspaces: &[Workspace], name: &str) -> Result<usize> {
    find_by_name(&workspaces.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), name, "workspace")
}

/// (workspace index, tab index) of a tab ref.
pub fn locate(workspaces: &[Workspace], r: &TabRef) -> Result<(usize, usize)> {
    let wi = find_ws(workspaces, &r.ws)?;
    let tabs = &workspaces[wi].tabs;
    let ti = match &r.sel {
        TabSel::Position(n) if *n <= tabs.len() => n - 1,
        TabSel::Position(n) => {
            return Err(failure(
                Exit::NotFound,
                format!("workspace '{}' has {} tab(s), no #{n}", workspaces[wi].name, tabs.len()),
            ));
        }
        TabSel::Title(text) => find_by_name(&tabs.iter().map(|t| t.title()).collect::<Vec<_>>(), text, "tab")?,
    };
    Ok((wi, ti))
}

pub struct EditCtx<'a> {
    pub agents: &'a dyn AgentCatalog,
    /// Needed only by `ws-from`.
    pub snapshots: &'a dyn Snapshots,
    /// Needed only by `ws-add-live`.
    pub open: Option<&'a OpenState>,
}

/// Applies `ops` in order; returns notes on what was left out (of workspaces added from cmux).
pub fn apply_ops(workspaces: &mut Vec<Workspace>, ops: &[Op], ctx: &EditCtx) -> Result<Vec<String>> {
    let mut notes = vec![];
    for op in ops {
        notes.extend(apply(workspaces, op, ctx)?);
    }
    Ok(notes)
}

fn tab_at(workspaces: &[Workspace], tab: &str) -> Result<(usize, usize)> {
    locate(workspaces, &parse_tab_ref(tab)?)
}

fn apply(ws: &mut Vec<Workspace>, op: &Op, ctx: &EditCtx) -> Result<Vec<String>> {
    match op {
        Op::WsRename { ws: w, to } => {
            let i = find_ws(ws, w)?;
            let to = to.trim();
            if to.is_empty() {
                return Err(failure(Exit::Refused, "a workspace name cannot be empty"));
            }
            if ws.iter().enumerate().any(|(j, o)| j != i && o.name.eq_ignore_ascii_case(to)) {
                return Err(failure(Exit::Refused, format!("the snapshot already has a workspace named '{to}'")));
            }
            ws[i].name = to.to_string();
        }
        Op::WsColor { ws: w, color } => {
            let i = find_ws(ws, w)?;
            ws[i].color = color.clone();
        }
        Op::WsIndex { ws: w, index } => {
            let i = find_ws(ws, w)?;
            let item = ws.remove(i);
            let to = index.saturating_sub(1).min(ws.len());
            ws.insert(to, item);
        }
        Op::WsRemove { ws: w } => {
            let i = find_ws(ws, w)?;
            ws.remove(i);
        }
        Op::WsAddLive { ws: w } => {
            let open = ctx
                .open
                .ok_or_else(|| failure(Exit::CmuxUnreachable, "adding an open workspace needs cmux's state"))?;
            let (mut captured, notes) = capture::capture(open, ctx.agents, std::slice::from_ref(w))?;
            add_workspace(ws, captured.remove(0))?;
            return Ok(notes);
        }
        Op::WsFrom { from, ws: w } => {
            let (name, version) = parse_ref(from)?;
            let src = ctx.snapshots.load(&name, version)?;
            let i = find_ws(&src.workspaces, w)?;
            add_workspace(ws, src.workspaces[i].clone())?;
        }
        Op::TabRemove { tab } => {
            let (w, t) = tab_at(ws, tab)?;
            ws[w].tabs.remove(t);
        }
        Op::TabRename { tab, to } => {
            let (w, t) = tab_at(ws, tab)?;
            ws[w].tabs[t].set_title(to.clone());
        }
        Op::TabIndex { tab, index } => {
            let (w, t) = tab_at(ws, tab)?;
            let item = ws[w].tabs.remove(t);
            let to = index.saturating_sub(1).min(ws[w].tabs.len());
            ws[w].tabs.insert(to, item);
        }
        Op::TabMove { tab, to } => {
            let (w, t) = tab_at(ws, tab)?;
            let dest = find_ws(ws, to)?;
            let item = ws[w].tabs.remove(t);
            ws[dest].tabs.push(item);
        }
        Op::TabAddSession { ws: w, session_id, agent, title } => {
            clean("session ID", session_id)?;
            clean("agent", agent.as_deref().unwrap_or(""))?;
            let i = find_ws(ws, w)?;
            let (kind, info) = find_session(ctx.agents, session_id, agent.as_deref())?;
            // An agent without a history (resume only) starts in the workspace's folder.
            let (cwd, label) = match info {
                Some(info) => (
                    info.cwd.clone().ok_or_else(|| {
                        failure(Exit::Refused, format!("session {session_id} has no recorded folder"))
                    })?,
                    info.label(),
                ),
                None => (ws[i].cwd.clone(), format!("{} session", ctx.agents.display_name(&kind))),
            };
            ws[i].tabs.push(Tab::Agent {
                title: title.clone().unwrap_or(label),
                cwd,
                agent: kind,
                session_id: session_id.clone(),
                args: vec![],
            });
        }
        Op::TabAddTerminal { ws: w, cwd, command, title } => {
            clean("folder", cwd)?;
            clean("command", command.as_deref().unwrap_or(""))?;
            let i = find_ws(ws, w)?;
            ws[i].tabs.push(Tab::Terminal {
                title: title.clone().unwrap_or_else(|| folder_name(cwd)),
                cwd: cwd.clone(),
                command: command.clone(),
            });
        }
        Op::TabSetCwd { tab, cwd } => {
            // Claude Code (≥ 2.1.223) and Codex resume a session by ID from any folder.
            clean("folder", cwd)?;
            let (w, t) = tab_at(ws, tab)?;
            ws[w].tabs[t].set_cwd(cwd.clone());
        }
        Op::TabSetSession { tab, session_id, agent } => {
            clean("session ID", session_id)?;
            clean("agent", agent.as_deref().unwrap_or(""))?;
            let (w, t) = tab_at(ws, tab)?;
            let (kind, _) = find_session(ctx.agents, session_id, agent.as_deref())?;
            let old = &ws[w].tabs[t];
            // The launch flags still fit when the agent stays the same.
            let args = match old {
                Tab::Agent { agent, args, .. } if ctx.agents.kind_of(agent) == Some(kind.as_str()) => args.clone(),
                _ => vec![],
            };
            let (title, cwd) = (old.title().to_string(), old.cwd().to_string());
            ws[w].tabs[t] = Tab::Agent { title, cwd, agent: kind, session_id: session_id.clone(), args };
        }
    }
    Ok(vec![])
}

/// Refuses a value with control characters: it could act on the terminal when typed.
fn clean(what: &str, value: &str) -> Result<()> {
    if has_control(value) {
        return Err(failure(Exit::Error, format!("the {what} {value:?} contains control characters")));
    }
    Ok(())
}

fn add_workspace(workspaces: &mut Vec<Workspace>, ws: Workspace) -> Result<()> {
    if workspaces.iter().any(|w| w.name.eq_ignore_ascii_case(&ws.name)) {
        return Err(failure(
            Exit::Refused,
            format!("the snapshot already has a workspace named '{}'; rename or remove it first", ws.name),
        ));
    }
    workspaces.push(ws);
    Ok(())
}

fn folder_name(cwd: &str) -> String {
    cwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("terminal").to_string()
}

/// Problems worth showing with a snapshot: agent tabs that would not resume (missing transcripts,
/// unknown agents, invalid IDs) and duplicated sessions.
pub fn warnings(workspaces: &[Workspace], agents: &dyn AgentCatalog) -> Vec<String> {
    let mut out = vec![];
    let mut seen: HashMap<SessionRef, usize> = HashMap::new();
    for w in workspaces {
        for (i, t) in w.tabs.iter().enumerate() {
            if let Some(field) = t.control_field() {
                out.push(format!(
                    "{}#{} '{}': its {field} has control characters, restores as a plain shell",
                    w.name,
                    i + 1,
                    t.title()
                ));
            }
            if let Some(s) = t.session() {
                let status = resume_status(agents, &s);
                if status != Resume::Ready && t.control_field().is_none() {
                    out.push(format!(
                        "{}#{} '{}': {}, restores as a plain shell",
                        w.name,
                        i + 1,
                        t.title(),
                        problem(agents, &s, status)
                    ));
                }
                *seen.entry(s).or_default() += 1;
            }
        }
    }
    let mut dups: Vec<(SessionRef, usize)> = seen.into_iter().filter(|(_, n)| *n > 1).collect();
    dups.sort();
    for (s, n) in dups {
        out.push(format!("session {} is in {n} tabs; restore opens it once", s.session_id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Version;
    use crate::error::exit_of;
    use crate::testing::{A, B, C, FakeHistory, FakeSnapshots, TestAgents, agent_tab, agents, open_ws};

    fn setup() -> (FakeSnapshots, TestAgents, Vec<Workspace>) {
        let history = agents(vec![FakeHistory::with(&[(A, "/w/a", "one"), (B, "/w/b", "two"), (C, "/w/c", "three")])]);
        let ws = vec![
            Workspace {
                name: "Alpha".into(),
                color: None,
                cwd: "/w/a".into(),
                git_branch: None,
                cmux_workspace_id: None,
                tabs: vec![
                    agent_tab("claude", "one", "/w/a", A),
                    Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None },
                ],
            },
            Workspace {
                name: "Beta".into(),
                color: None,
                cwd: "/w/b".into(),
                git_branch: None,
                cmux_workspace_id: None,
                tabs: vec![agent_tab("claude", "two", "/w/b", B)],
            },
        ];
        (FakeSnapshots::default(), history, ws)
    }

    fn titles(w: &Workspace) -> Vec<&str> {
        w.tabs.iter().map(|t| t.title()).collect()
    }

    #[test]
    fn parses_tab_refs() {
        assert_eq!(parse_tab_ref("Alpha#2").unwrap(), TabRef { ws: "Alpha".into(), sel: TabSel::Position(2) });
        assert_eq!(
            parse_tab_ref("Alpha:\"zs\"").unwrap(),
            TabRef { ws: "Alpha".into(), sel: TabSel::Title("zs".into()) }
        );
        assert!(parse_tab_ref("Alpha").is_err());
        assert!(parse_tab_ref("Alpha#0").is_err());
    }

    #[test]
    fn rename_move_reorder_remove() {
        let (snapshots, history, mut ws) = setup();
        let ctx = EditCtx { agents: &history, snapshots: &snapshots, open: None };
        apply_ops(
            &mut ws,
            &[
                Op::TabRename { tab: "Alpha#2".into(), to: "shell".into() },
                Op::TabIndex { tab: "Alpha:shell".into(), index: 1 },
                Op::TabMove { tab: "Alpha#2".into(), to: "Beta".into() },
                Op::WsIndex { ws: "Beta".into(), index: 1 },
                Op::WsRename { ws: "Alpha".into(), to: "Main".into() },
            ],
            &ctx,
        )
        .unwrap();
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Beta", "Main"]);
        assert_eq!(titles(&ws[0]), ["two", "one"]);
        assert_eq!(titles(&ws[1]), ["shell"]);
        apply_ops(&mut ws, &[Op::TabRemove { tab: "Main#1".into() }, Op::WsRemove { ws: "Main".into() }], &ctx)
            .unwrap();
        assert_eq!(ws.len(), 1);
    }

    #[test]
    fn rename_refuses_duplicates_and_empty() {
        let (snapshots, history, mut ws) = setup();
        let ctx = EditCtx { agents: &history, snapshots: &snapshots, open: None };
        let err = apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "beta".into() }], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
        let err = apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "  ".into() }], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
        apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "ALPHA".into() }], &ctx).unwrap();
        assert_eq!(ws[0].name, "ALPHA");
    }

    #[test]
    fn session_edits_are_checked() {
        let (snapshots, history, mut ws) = setup();
        let ctx = EditCtx { agents: &history, snapshots: &snapshots, open: None };
        // Resume by ID works from any folder (Claude Code ≥ 2.1.223), so a new folder is fine.
        apply_ops(&mut ws, &[Op::TabSetCwd { tab: "Alpha#1".into(), cwd: "/w/b".into() }], &ctx).unwrap();
        assert_eq!(ws[0].tabs[0].cwd(), "/w/b");
        let set = |id: &str| Op::TabSetSession { tab: "Alpha#1".into(), session_id: id.into(), agent: None };
        let err = apply_ops(&mut ws, &[set("nope")], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Error);
        let unknown = "44444444-4444-4444-8444-444444444444";
        let err = apply_ops(&mut ws, &[set(unknown)], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::NotFound);
        let add = Op::TabAddSession { ws: "Beta".into(), session_id: C.into(), agent: None, title: None };
        apply_ops(&mut ws, &[add], &ctx).unwrap();
        assert_eq!(ws[1].tabs[1], agent_tab("claude", "three", "/w/c", C));
        let set = Op::TabSetSession { tab: "Alpha#2".into(), session_id: A.into(), agent: None };
        apply_ops(&mut ws, &[set], &ctx).unwrap();
        assert_eq!(ws[0].tabs[1].session(), Some(SessionRef::new("claude", A)));
        assert_eq!(ws[0].tabs[1].title(), "zsh");
        assert!(warnings(&ws, &history).iter().any(|w| w.contains("in 2 tabs")));
    }

    #[test]
    fn sessions_are_found_in_every_history_or_the_named_agent() {
        let both = agents(vec![
            FakeHistory::with(&[(A, "/w/a", "claude one"), (C, "/w/c", "claude three")]),
            FakeHistory::codex(&[(B, "/w/b", "codex two"), (C, "/w/c", "codex three")]),
        ]);
        let (snapshots, _, mut ws) = setup();
        let ctx = EditCtx { agents: &both, snapshots: &snapshots, open: None };
        let add = |id: &str, agent: Option<&str>| Op::TabAddSession {
            ws: "Beta".into(),
            session_id: id.into(),
            agent: agent.map(str::to_string),
            title: None,
        };
        apply_ops(&mut ws, &[add(B, None)], &ctx).unwrap();
        assert_eq!(ws[1].tabs.last().unwrap(), &agent_tab("codex", "codex two", "/w/b", B));
        // In both histories: ambiguous without --agent.
        let err = apply_ops(&mut ws, &[add(C, None)], &ctx).unwrap_err();
        assert!(err.to_string().contains("claude, codex"), "{err}");
        apply_ops(&mut ws, &[add(C, Some("codex"))], &ctx).unwrap();
        assert_eq!(ws[1].tabs.last().unwrap(), &agent_tab("codex", "codex three", "/w/c", C));
        // Not in the named agent's history.
        assert_eq!(exit_of(&apply_ops(&mut ws, &[add(A, Some("codex"))], &ctx).unwrap_err()), Exit::NotFound);
        // A resume-only agent takes any valid ID and starts in the workspace's folder.
        apply_ops(&mut ws, &[add("ses_9", Some("opencode"))], &ctx).unwrap();
        assert_eq!(ws[1].tabs.last().unwrap(), &agent_tab("opencode", "OpenCode session", "/w/b", "ses_9"));
        assert_eq!(exit_of(&apply_ops(&mut ws, &[add("a b", Some("opencode"))], &ctx).unwrap_err()), Exit::Error);
        assert_eq!(exit_of(&apply_ops(&mut ws, &[add(A, Some("aider"))], &ctx).unwrap_err()), Exit::Error);
        // Switching a tab to another agent's session drops its launch flags.
        ws[0].tabs[0] = Tab::Agent {
            title: "one".into(),
            cwd: "/w/a".into(),
            agent: "claude".into(),
            session_id: A.into(),
            args: vec!["--model".into(), "opus".into()],
        };
        let set = |id: &str| Op::TabSetSession { tab: "Alpha#1".into(), session_id: id.into(), agent: None };
        apply_ops(&mut ws, &[set(A)], &ctx).unwrap();
        assert!(matches!(&ws[0].tabs[0], Tab::Agent { args, .. } if args.len() == 2));
        apply_ops(&mut ws, &[set(B)], &ctx).unwrap();
        assert!(matches!(&ws[0].tabs[0], Tab::Agent { agent, args, .. } if agent == "codex" && args.is_empty()));
    }

    #[test]
    fn edits_refuse_control_characters() {
        let (snapshots, history, mut ws) = setup();
        let ctx = EditCtx { agents: &history, snapshots: &snapshots, open: None };
        let ops = [
            Op::TabAddSession {
                ws: "Beta".into(),
                session_id: "s\n1".into(),
                agent: Some("opencode".into()),
                title: None,
            },
            Op::TabSetSession { tab: "Alpha#1".into(), session_id: format!("{A}\r"), agent: None },
            Op::TabAddTerminal {
                ws: "Beta".into(),
                cwd: "/w".into(),
                command: Some("\u{3}rm -rf ~\r".into()),
                title: None,
            },
            Op::TabAddTerminal { ws: "Beta".into(), cwd: "/w\t".into(), command: None, title: None },
            Op::TabSetCwd { tab: "Alpha#1".into(), cwd: "/w\u{1b}[2J".into() },
        ];
        for op in ops {
            let err = apply_ops(&mut ws, std::slice::from_ref(&op), &ctx).unwrap_err();
            assert_eq!(exit_of(&err), Exit::Error, "{op:?}");
            assert!(err.to_string().contains("control characters"), "{err}");
        }
    }

    #[test]
    fn warns_about_tabs_that_will_not_resume() {
        let (_, history, mut ws) = setup();
        ws[0].tabs.push(agent_tab("aider", "x", "/w/a", A));
        ws[0].tabs.push(agent_tab("claude", "gone", "/w/a", "44444444-4444-4444-8444-444444444444"));
        assert_eq!(
            warnings(&ws, &history),
            [
                "Alpha#3 'x': sessionsnap doesn't know how to resume aider yet, restores as a plain shell",
                "Alpha#4 'gone': transcript not found, restores as a plain shell",
            ]
        );
    }

    #[test]
    fn workspace_copies_and_live_adds() {
        let (mut snapshots, history, mut ws) = setup();
        snapshots.versions.push(Version {
            sessionsnap: crate::domain::model::FORMAT,
            snapshot: "other".into(),
            version: 1,
            created_at: String::new(),
            message: "m".into(),
            source: Default::default(),
            workspaces: vec![Workspace {
                name: "Gamma".into(),
                color: None,
                cwd: "/w/c".into(),
                git_branch: None,
                cmux_workspace_id: None,
                tabs: vec![],
            }],
        });
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("WD", "Delta", "/w/d", &[("zsh", "/w/d", None)])],
        };
        let ctx = EditCtx { agents: &history, snapshots: &snapshots, open: Some(&open) };
        apply_ops(
            &mut ws,
            &[Op::WsFrom { from: "other@1".into(), ws: "gamma".into() }, Op::WsAddLive { ws: "Delta".into() }],
            &ctx,
        )
        .unwrap();
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Beta", "Gamma", "Delta"]);
        let err = apply_ops(&mut ws, &[Op::WsAddLive { ws: "Delta".into() }], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
    }

    #[test]
    fn batch_json_format() {
        let ops: Vec<Op> = serde_json::from_str(&format!(
            r#"[{{"op":"tab-add-session","ws":"Beta","sessionId":"{C}"}},{{"op":"ws-color","ws":"Beta","color":"blue"}}]"#
        )).unwrap();
        assert_eq!(ops[0], Op::TabAddSession { ws: "Beta".into(), session_id: C.into(), agent: None, title: None });
        let op: Op =
            serde_json::from_str(r#"{"op":"tab-add-session","ws":"Beta","sessionId":"s1","agent":"opencode"}"#)
                .unwrap();
        assert!(matches!(op, Op::TabAddSession { agent: Some(a), .. } if a == "opencode"));
        assert_eq!(ops[1], Op::WsColor { ws: "Beta".into(), color: Some("blue".into()) });
    }
}
