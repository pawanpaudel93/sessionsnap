use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::claude::Claude;
use crate::cmux::OpenState;
use crate::core::capture;
use crate::core::model::{Tab, Workspace, find_by_name, is_session_id};
use crate::error::{Exit, failure};
use crate::store::{Store, parse_ref};

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
    TabAddSession {
        ws: String,
        session_id: String,
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
    TabSetSession {
        tab: String,
        session_id: String,
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
    pub claude: &'a Claude,
    pub store: &'a Store,
    /// Needed only by `ws-add-live`.
    pub open: Option<&'a OpenState>,
}

pub fn apply_ops(workspaces: &mut Vec<Workspace>, ops: &[Op], ctx: &EditCtx) -> Result<()> {
    for op in ops {
        apply(workspaces, op, ctx)?;
    }
    Ok(())
}

fn tab_at(workspaces: &[Workspace], tab: &str) -> Result<(usize, usize)> {
    locate(workspaces, &parse_tab_ref(tab)?)
}

fn apply(ws: &mut Vec<Workspace>, op: &Op, ctx: &EditCtx) -> Result<()> {
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
            let mut captured = capture::capture(open, ctx.claude, std::slice::from_ref(w))?;
            add_workspace(ws, captured.remove(0))?;
        }
        Op::WsFrom { from, ws: w } => {
            let (name, version) = parse_ref(from)?;
            let src = ctx.store.load(&name, version)?;
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
        Op::TabAddSession { ws: w, session_id, title } => {
            let i = find_ws(ws, w)?;
            check_session_id(session_id)?;
            let info = ctx.claude.info(&ensure_known(ctx.claude, session_id)?)?;
            let cwd = info
                .cwd
                .clone()
                .ok_or_else(|| failure(Exit::Refused, format!("session {session_id} has no recorded folder")))?;
            ws[i].tabs.push(Tab::Claude {
                title: title.clone().unwrap_or_else(|| info.label()),
                cwd,
                session_id: session_id.clone(),
            });
        }
        Op::TabAddTerminal { ws: w, cwd, command, title } => {
            let i = find_ws(ws, w)?;
            ws[i].tabs.push(Tab::Terminal {
                title: title.clone().unwrap_or_else(|| folder_name(cwd)),
                cwd: cwd.clone(),
                command: command.clone(),
            });
        }
        Op::TabSetCwd { tab, cwd } => {
            // Resuming by ID searches every project (Claude Code ≥ 2.1.223), so any folder works.
            let (w, t) = tab_at(ws, tab)?;
            ws[w].tabs[t].set_cwd(cwd.clone());
        }
        Op::TabSetSession { tab, session_id } => {
            check_session_id(session_id)?;
            let (w, t) = tab_at(ws, tab)?;
            ensure_known(ctx.claude, session_id)?;
            let (title, cwd) = (ws[w].tabs[t].title().to_string(), ws[w].tabs[t].cwd().to_string());
            ws[w].tabs[t] = Tab::Claude { title, cwd, session_id: session_id.clone() };
        }
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

fn check_session_id(id: &str) -> Result<()> {
    if is_session_id(id) {
        Ok(())
    } else {
        Err(failure(Exit::Error, format!("'{id}' is not a Claude session ID (a UUID)")))
    }
}

/// The session's transcript, from any project; `claude --resume <id>` finds it from any folder.
fn ensure_known(claude: &Claude, id: &str) -> Result<std::path::PathBuf> {
    claude.find_transcript(id).ok_or_else(|| failure(Exit::NotFound, format!("no transcript for session {id}")))
}

fn folder_name(cwd: &str) -> String {
    cwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("terminal").to_string()
}

/// Problems worth showing with a snapshot: missing transcripts and duplicated sessions.
pub fn warnings(workspaces: &[Workspace], claude: &Claude) -> Vec<String> {
    let mut out = vec![];
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for w in workspaces {
        for (i, t) in w.tabs.iter().enumerate() {
            if let Some(id) = t.session_id() {
                *seen.entry(id).or_default() += 1;
                if claude.find_transcript(id).is_none() {
                    out.push(format!(
                        "{}#{} '{}': transcript not found, restores as a plain shell",
                        w.name,
                        i + 1,
                        t.title()
                    ));
                }
            }
        }
    }
    let mut dups: Vec<(&str, usize)> = seen.into_iter().filter(|(_, n)| *n > 1).collect();
    dups.sort();
    for (id, n) in dups {
        out.push(format!("session {id} is in {n} tabs; restore opens it once"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;
    use crate::testing::{A, B, C, open_ws, write_transcript};

    fn setup() -> (tempfile::TempDir, Store, Claude, Vec<Workspace>) {
        let d = tempfile::tempdir().unwrap();
        let croot = d.path().join("c");
        write_transcript(&croot, "/w/a", A, "one");
        write_transcript(&croot, "/w/b", B, "two");
        write_transcript(&croot, "/w/c", C, "three");
        let store = Store::new(d.path().join("s"));
        let claude = Claude::new(croot);
        let ws = vec![
            Workspace {
                name: "Alpha".into(),
                color: None,
                cwd: "/w/a".into(),
                git_branch: None,
                cmux_workspace_id: None,
                tabs: vec![
                    Tab::Claude { title: "one".into(), cwd: "/w/a".into(), session_id: A.into() },
                    Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None },
                ],
            },
            Workspace {
                name: "Beta".into(),
                color: None,
                cwd: "/w/b".into(),
                git_branch: None,
                cmux_workspace_id: None,
                tabs: vec![Tab::Claude { title: "two".into(), cwd: "/w/b".into(), session_id: B.into() }],
            },
        ];
        (d, store, claude, ws)
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
        let (_d, store, claude, mut ws) = setup();
        let ctx = EditCtx { claude: &claude, store: &store, open: None };
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
        let (_d, store, claude, mut ws) = setup();
        let ctx = EditCtx { claude: &claude, store: &store, open: None };
        let err = apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "beta".into() }], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
        let err = apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "  ".into() }], &ctx).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
        apply_ops(&mut ws, &[Op::WsRename { ws: "Alpha".into(), to: "ALPHA".into() }], &ctx).unwrap();
        assert_eq!(ws[0].name, "ALPHA");
    }

    #[test]
    fn session_edits_are_checked() {
        let (_d, store, claude, mut ws) = setup();
        let ctx = EditCtx { claude: &claude, store: &store, open: None };
        // Resume by ID works from any folder (Claude Code ≥ 2.1.223), so a new folder is fine.
        apply_ops(&mut ws, &[Op::TabSetCwd { tab: "Alpha#1".into(), cwd: "/w/b".into() }], &ctx).unwrap();
        assert_eq!(ws[0].tabs[0].cwd(), "/w/b");
        let err = apply_ops(&mut ws, &[Op::TabSetSession { tab: "Alpha#1".into(), session_id: "nope".into() }], &ctx)
            .unwrap_err();
        assert_eq!(exit_of(&err), Exit::Error);
        let unknown = "44444444-4444-4444-8444-444444444444";
        let err = apply_ops(&mut ws, &[Op::TabSetSession { tab: "Alpha#1".into(), session_id: unknown.into() }], &ctx)
            .unwrap_err();
        assert_eq!(exit_of(&err), Exit::NotFound);
        apply_ops(&mut ws, &[Op::TabAddSession { ws: "Beta".into(), session_id: C.into(), title: None }], &ctx)
            .unwrap();
        assert_eq!(ws[1].tabs[1], Tab::Claude { title: "three".into(), cwd: "/w/c".into(), session_id: C.into() });
        apply_ops(&mut ws, &[Op::TabSetSession { tab: "Alpha#2".into(), session_id: A.into() }], &ctx).unwrap();
        assert_eq!(ws[0].tabs[1].session_id(), Some(A));
        assert_eq!(ws[0].tabs[1].title(), "zsh");
        assert!(warnings(&ws, &claude).iter().any(|w| w.contains("in 2 tabs")));
    }

    #[test]
    fn workspace_copies_and_live_adds() {
        let (_d, store, claude, mut ws) = setup();
        store
            .save_new(
                "other",
                "m",
                Default::default(),
                vec![Workspace {
                    name: "Gamma".into(),
                    color: None,
                    cwd: "/w/c".into(),
                    git_branch: None,
                    cmux_workspace_id: None,
                    tabs: vec![],
                }],
            )
            .unwrap();
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("WD", "Delta", "/w/d", &[("zsh", "/w/d", None)])],
        };
        let ctx = EditCtx { claude: &claude, store: &store, open: Some(&open) };
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
        assert_eq!(ops[0], Op::TabAddSession { ws: "Beta".into(), session_id: C.into(), title: None });
        assert_eq!(ops[1], Op::WsColor { ws: "Beta".into(), color: Some("blue".into()) });
    }
}
