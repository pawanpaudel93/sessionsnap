use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::domain::agents::{Resume, problem, resume_status, unknown_session};
use crate::domain::edit::{find_ws, locate, parse_tab_ref};
use crate::domain::model::{SessionRef, Tab, Workspace, has_control};
use crate::error::{Exit, failure};
use crate::ports::{AgentCatalog, OpenState, OpenWorkspace};

/// Which workspaces (whole) and tabs (`<ws>#<n>` / `<ws>:"title"`) to restore; empty = all.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub tabs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum PlanSource {
    Snapshot { snapshot: String, version: u32, selection: Selection },
    Sessions { sessions: Vec<SessionRef> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Action {
    CreateWorkspace { name: String, cwd: String, tabs: Vec<Tab> },
    AddTabs { workspace_ref: String, workspace_name: String, tabs: Vec<Tab> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub plan_id: String,
    pub created_at: String,
    pub source: PlanSource,
    pub actions: Vec<Action>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn tab_count(&self) -> usize {
        self.actions
            .iter()
            .map(|a| match a {
                Action::CreateWorkspace { tabs, .. } | Action::AddTabs { tabs, .. } => tabs.len(),
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct Built {
    pub actions: Vec<Action>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyStep {
    pub description: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Keeps the selected workspaces (whole) and tabs, in their saved order.
pub fn select(workspaces: Vec<Workspace>, sel: &Selection) -> Result<Vec<Workspace>> {
    if sel.workspaces.is_empty() && sel.tabs.is_empty() {
        return Ok(workspaces);
    }
    let mut chosen: BTreeMap<usize, Option<BTreeSet<usize>>> = BTreeMap::new();
    for w in &sel.workspaces {
        chosen.insert(find_ws(&workspaces, w)?, None);
    }
    for t in &sel.tabs {
        let (wi, ti) = locate(&workspaces, &parse_tab_ref(t)?)?;
        if let Some(set) = chosen.entry(wi).or_insert_with(|| Some(BTreeSet::new())) {
            set.insert(ti);
        }
    }
    Ok(workspaces
        .into_iter()
        .enumerate()
        .filter_map(|(i, mut w)| {
            let choice = chosen.get(&i)?;
            if let Some(set) = choice {
                w.tabs = w.tabs.into_iter().enumerate().filter(|(ti, _)| set.contains(ti)).map(|(_, t)| t).collect();
            }
            Some(w)
        })
        .collect())
}

/// Recovered sessions as one "Recovered sessions" workspace, titled from their histories.
pub fn recovered_workspace(sessions: &[SessionRef], agents: &dyn AgentCatalog) -> Result<Vec<Workspace>> {
    let mut tabs = vec![];
    for s in sessions {
        let status = resume_status(agents, s);
        if matches!(status, Resume::UnknownAgent | Resume::InvalidId) {
            return Err(failure(Exit::Error, problem(agents, s, status)));
        }
        let info = agents
            .history(&s.agent)
            .ok_or_else(|| failure(Exit::Error, format!("sessionsnap cannot read {}'s sessions", s.agent)))?
            .find(&s.session_id)?
            .ok_or_else(|| unknown_session(&s.session_id))?;
        tabs.push(Tab::Agent {
            title: info.label(),
            cwd: info.cwd.clone().unwrap_or_default(),
            agent: s.agent.clone(),
            session_id: s.session_id.clone(),
            args: vec![],
        });
    }
    let cwd = tabs.first().map(|t| t.cwd().to_string()).unwrap_or_default();
    Ok(vec![Workspace {
        name: "Recovered sessions".into(),
        color: None,
        cwd,
        git_branch: None,
        cmux_workspace_id: None,
        tabs,
    }])
}

/// What to open so that `saved` is open, given what is open now.
/// - Agent tabs whose session (agent and ID) is open anywhere are skipped; a session listed twice
///   opens once.
/// - Agent tabs that cannot be resumed (unknown agent, invalid ID, session gone from the agent's
///   history) become plain shells in their folder (with a warning); their ID is never run.
/// - A tab with a control character in a value that would be typed or used as its folder
///   (`Tab::control_field`) is never run either: it becomes a plain shell, in its folder when that
///   is clean, else in the workspace's (else in "", which the caller replaces with home).
/// - A saved workspace matching an open one gets only its missing agent tabs added; shells are
///   not added to open workspaces (they would duplicate). Unmatched workspaces are created.
pub fn build_actions(saved: &[Workspace], open: &OpenState, agents: &dyn AgentCatalog) -> Built {
    let open_sessions = open.open_sessions();
    let mut planned: HashSet<SessionRef> = HashSet::new();
    let mut b = Built::default();
    for ws in saved {
        let live = match_workspace(ws, open);
        let mut pending = vec![];
        for tab in &ws.tabs {
            let label = format!("{} › {}", ws.name, tab.title());
            let control = tab.control_field();
            let shell = || Tab::Terminal { title: tab.title().to_string(), cwd: clean_cwd(tab, ws), command: None };
            let Some(s) = tab.session() else {
                match control {
                    Some(field) => {
                        b.warnings.push(format!("{label}: its {field} has control characters, opening a plain shell"));
                        pending.push(shell());
                    }
                    None => pending.push(tab.clone()),
                }
                continue;
            };
            if open_sessions.contains(&s) {
                b.skipped.push(format!("{label}: already open"));
                continue;
            }
            if planned.contains(&s) {
                b.skipped.push(format!("{label}: same session listed earlier"));
                continue;
            }
            let lost = match tab {
                Tab::Agent { agent, args, .. } => agents.check_flags(agent, args).lost_reason(),
                Tab::Terminal { .. } => None,
            };
            let why = match (control, lost, resume_status(agents, &s)) {
                (Some(field), _, _) => Some(format!("its {field} has control characters")),
                (None, Some(lost), _) => Some(lost),
                (None, None, Resume::Ready) => None,
                (None, None, status) => Some(problem(agents, &s, status)),
            };
            match why {
                None => {
                    planned.insert(s);
                    pending.push(tab.clone());
                }
                Some(why) if live.is_some() => {
                    b.skipped.push(format!("{label}: {why}; not added to an open workspace"));
                }
                Some(why) => {
                    b.warnings.push(format!("{label}: {why}, opening a plain shell"));
                    pending.push(shell());
                }
            }
        }
        if pending.is_empty() {
            continue;
        }
        match live {
            Some(live) => {
                let (agent_tabs, shells): (Vec<Tab>, Vec<Tab>) =
                    pending.into_iter().partition(|t| t.session().is_some());
                for s in shells {
                    b.skipped.push(format!(
                        "{} › {}: shell tabs are not added to an open workspace",
                        ws.name,
                        s.title()
                    ));
                }
                if !agent_tabs.is_empty() {
                    b.actions.push(Action::AddTabs {
                        workspace_ref: live.reference.clone().unwrap_or_default(),
                        workspace_name: live.name.clone(),
                        tabs: agent_tabs,
                    });
                }
            }
            None => {
                b.actions.push(Action::CreateWorkspace { name: ws.name.clone(), cwd: ws.cwd.clone(), tabs: pending })
            }
        }
    }
    b
}

/// The tab's folder if it has no control characters, else the workspace's if that has none, else "".
fn clean_cwd(tab: &Tab, ws: &Workspace) -> String {
    [tab.cwd(), ws.cwd.as_str()].into_iter().find(|c| !has_control(c)).unwrap_or("").to_string()
}

/// The open workspace a saved one corresponds to: same terminal ID, else the one holding most of
/// its agent sessions, else the same name. Only live workspaces (with a ref) count.
fn match_workspace<'a>(ws: &Workspace, open: &'a OpenState) -> Option<&'a OpenWorkspace> {
    let live: Vec<&OpenWorkspace> = open.workspaces.iter().filter(|w| w.reference.is_some()).collect();
    if let Some(id) = &ws.cmux_workspace_id
        && let Some(w) = live.iter().copied().find(|w| &w.id == id)
    {
        return Some(w);
    }
    let sessions: HashSet<SessionRef> = ws.tabs.iter().filter_map(Tab::session).collect();
    let best = live
        .iter()
        .copied()
        .map(|w| {
            let shared = w.tabs.iter().filter(|t| t.session().is_some_and(|s| sessions.contains(&s))).count();
            (w, shared)
        })
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n);
    if let Some((w, _)) = best {
        return Some(w);
    }
    live.into_iter().find(|w| w.name.eq_ignore_ascii_case(&ws.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;
    use crate::testing::{A, B, C, FakeHistory, TestAgents, agent_tab, agents, open_ws};

    const D: &str = "44444444-4444-4444-8444-444444444444";

    fn history() -> TestAgents {
        agents(vec![FakeHistory::with(&[(A, "/w/a", "one"), (B, "/w/a", "two"), (C, "/w/c", "three")])])
    }

    fn claude_tab(title: &str, cwd: &str, id: &str) -> Tab {
        agent_tab("claude", title, cwd, id)
    }

    fn workspace(name: &str, cwd: &str, id: Option<&str>, tabs: Vec<Tab>) -> Workspace {
        Workspace {
            name: name.into(),
            color: None,
            cwd: cwd.into(),
            git_branch: None,
            cmux_workspace_id: id.map(str::to_string),
            tabs,
        }
    }

    fn saved() -> Vec<Workspace> {
        vec![
            workspace(
                "Alpha",
                "/w/a",
                Some("WA"),
                vec![
                    claude_tab("one", "/w/a", A),
                    claude_tab("two", "/w/a", B),
                    Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None },
                ],
            ),
            workspace("Gamma", "/w/c", None, vec![claude_tab("three", "/w/c", C)]),
            workspace("Delta", "/w/d", None, vec![claude_tab("lost", "/w/d", D)]),
        ]
    }

    fn alpha_with_one_open() -> OpenState {
        OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("WA", "Alpha", "/w/a", &[("one", "/w/a", Some(A))])],
        }
    }

    #[test]
    fn plans_only_missing_tabs() {
        let history = history();
        let b = build_actions(&saved(), &alpha_with_one_open(), &history);
        assert_eq!(
            b.actions,
            vec![
                Action::AddTabs {
                    workspace_ref: "workspace:WA".into(),
                    workspace_name: "Alpha".into(),
                    tabs: vec![claude_tab("two", "/w/a", B)]
                },
                Action::CreateWorkspace {
                    name: "Gamma".into(),
                    cwd: "/w/c".into(),
                    tabs: vec![claude_tab("three", "/w/c", C)]
                },
                Action::CreateWorkspace {
                    name: "Delta".into(),
                    cwd: "/w/d".into(),
                    tabs: vec![Tab::Terminal { title: "lost".into(), cwd: "/w/d".into(), command: None }],
                },
            ]
        );
        assert_eq!(b.skipped.len(), 2, "{:?}", b.skipped);
        assert_eq!(b.warnings.len(), 1, "{:?}", b.warnings);
    }

    #[test]
    fn missing_transcript_in_matched_workspace_is_skipped_without_warning() {
        let history = history();
        let ws =
            workspace("Alpha", "/w/a", Some("WA"), vec![claude_tab("one", "/w/a", A), claude_tab("lost", "/w/a", D)]);
        let b = build_actions(&[ws], &alpha_with_one_open(), &history);
        assert!(b.actions.is_empty(), "{:?}", b.actions);
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        let want = "Alpha › lost: transcript not found; not added to an open workspace";
        assert_eq!(b.skipped.iter().filter(|s| s.as_str() == want).count(), 1, "{:?}", b.skipped);
    }

    #[test]
    fn sessions_source_rejects_invalid_ids_and_unknown_agents() {
        let err = recovered_workspace(&[SessionRef::new("claude", "../x")], &history()).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Error);
        let err = recovered_workspace(&[SessionRef::new("aider", A)], &history()).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Error);
        let ws = recovered_workspace(&[SessionRef::new("claude", B)], &history()).unwrap();
        assert_eq!(ws[0].tabs, [claude_tab("two", "/w/a", B)]);
    }

    #[test]
    fn matches_open_sessions_by_agent_and_id() {
        let both = agents(vec![FakeHistory::with(&[(A, "/w/a", "one")]), FakeHistory::codex(&[(A, "/w/a", "uno")])]);
        let saved = vec![workspace(
            "Alpha",
            "/w/a",
            Some("WA"),
            vec![claude_tab("one", "/w/a", A), agent_tab("codex", "uno", "/w/a", A)],
        )];
        // The Claude session is open; the Codex session with the same ID is not.
        let b = build_actions(&saved, &alpha_with_one_open(), &both);
        assert_eq!(
            b.actions,
            vec![Action::AddTabs {
                workspace_ref: "workspace:WA".into(),
                workspace_name: "Alpha".into(),
                tabs: vec![agent_tab("codex", "uno", "/w/a", A)],
            }]
        );
        assert_eq!(b.skipped, ["Alpha › one: already open"]);
    }

    #[test]
    fn tabs_with_control_characters_never_run() {
        let agent = |title: &str, cwd: &str, id: &str, args: &[&str]| Tab::Agent {
            title: title.into(),
            cwd: cwd.into(),
            agent: "opencode".into(),
            session_id: id.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        };
        let saved = vec![workspace(
            "Gamma",
            "/w/c",
            None,
            vec![
                agent("ctrl-c", "/w/c", "s1", &["\u{3}touch /tmp/pwned\r"]),
                agent("newline id", "/w/c", "s2\nrm -rf ~", &[]),
                agent("bad folder", "/w/c\u{1b}x", "s3", &[]),
                Tab::Terminal { title: "cmd".into(), cwd: "/w/c/sub".into(), command: Some("make\nrm -rf ~".into()) },
                Tab::Terminal { title: "del".into(), cwd: "/w/c\u{7f}".into(), command: None },
            ],
        )];
        let b = build_actions(&saved, &OpenState::default(), &history());
        let shell = |title: &str, cwd: &str| Tab::Terminal { title: title.into(), cwd: cwd.into(), command: None };
        assert_eq!(
            b.actions,
            vec![Action::CreateWorkspace {
                name: "Gamma".into(),
                cwd: "/w/c".into(),
                tabs: vec![
                    shell("ctrl-c", "/w/c"),
                    shell("newline id", "/w/c"),
                    shell("bad folder", "/w/c"),
                    shell("cmd", "/w/c/sub"),
                    shell("del", "/w/c"),
                ],
            }]
        );
        assert_eq!(
            b.warnings,
            [
                "Gamma › ctrl-c: its flags has control characters, opening a plain shell",
                "Gamma › newline id: its session ID has control characters, opening a plain shell",
                "Gamma › bad folder: its folder has control characters, opening a plain shell",
                "Gamma › cmd: its command has control characters, opening a plain shell",
                "Gamma › del: its folder has control characters, opening a plain shell",
            ]
        );
        // In an open workspace such a tab is skipped, never added.
        let mut alpha = saved[0].clone();
        alpha.name = "Alpha".into();
        alpha.cmux_workspace_id = Some("WA".into());
        let b = build_actions(&[alpha], &alpha_with_one_open(), &history());
        assert!(b.actions.is_empty(), "{:?}", b.actions);
    }

    #[test]
    fn unresumable_tabs_open_plain_shells_with_a_warning() {
        let saved = vec![workspace(
            "Gamma",
            "/w/c",
            None,
            vec![
                agent_tab("aider", "unknown agent", "/w/c", A),
                agent_tab("claude", "bad id", "/w/c", "x; rm -rf ~"),
                agent_tab("opencode", "resume only", "/w/c", "ses_1"),
            ],
        )];
        let b = build_actions(&saved, &OpenState::default(), &history());
        let shell = |title: &str| Tab::Terminal { title: title.into(), cwd: "/w/c".into(), command: None };
        assert_eq!(
            b.actions,
            vec![Action::CreateWorkspace {
                name: "Gamma".into(),
                cwd: "/w/c".into(),
                tabs: vec![
                    shell("unknown agent"),
                    shell("bad id"),
                    agent_tab("opencode", "resume only", "/w/c", "ses_1")
                ],
            }]
        );
        assert_eq!(
            b.warnings,
            [
                "Gamma › unknown agent: sessionsnap doesn't know how to resume aider yet, opening a plain shell",
                "Gamma › bad id: 'x; rm -rf ~' is not a Claude Code session ID, opening a plain shell",
            ]
        );
    }

    /// An old snapshot may hold a deny rule that can no longer be kept, or a launch that is not a
    /// session: restoring opens a plain shell instead of resuming without the rule.
    #[test]
    fn tabs_whose_saved_flags_lose_the_session_open_plain_shells() {
        // Built at runtime so secret scanners don't flag the source.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let with_args = |title: &str, id: &str, args: Vec<String>| Tab::Agent {
            title: title.into(),
            cwd: "/w/c".into(),
            agent: "claude".into(),
            session_id: id.into(),
            args,
        };
        let deny = with_args("deny", A, vec!["--disallowedTools".into(), format!("Bash({ghp})")]);
        let print = with_args("print", B, vec!["-p".into()]);
        let saved = vec![workspace("Gamma", "/w/c", None, vec![deny, print])];
        let b = build_actions(&saved, &OpenState::default(), &history());
        let shell = |title: &str| Tab::Terminal { title: title.into(), cwd: "/w/c".into(), command: None };
        assert_eq!(
            b.actions,
            vec![Action::CreateWorkspace {
                name: "Gamma".into(),
                cwd: "/w/c".into(),
                tabs: vec![shell("deny"), shell("print")]
            }]
        );
        assert_eq!(
            b.warnings,
            [
                "Gamma › deny: a deny rule could not be saved; resume it by hand, opening a plain shell",
                "Gamma › print: a launch with -p is not a session that can be resumed, opening a plain shell",
            ]
        );
    }

    #[test]
    fn matches_renamed_workspace_by_sessions() {
        let history = history();
        let mut ws = saved();
        ws[0].cmux_workspace_id = None;
        ws[0].name = "Old name".into();
        let open = OpenState {
            live: true,
            state_version: None,
            workspaces: vec![open_ws("X9", "New name", "/w/a", &[("one", "/w/a", Some(A))])],
        };
        let b = build_actions(&ws[..1], &open, &history);
        assert_eq!(
            b.actions,
            vec![Action::AddTabs {
                workspace_ref: "workspace:X9".into(),
                workspace_name: "New name".into(),
                tabs: vec![claude_tab("two", "/w/a", B)],
            }]
        );
    }

    #[test]
    fn duplicate_sessions_open_once() {
        let history = history();
        let mut ws = saved();
        ws[1].tabs.push(claude_tab("three again", "/w/c", C));
        let b = build_actions(&ws[1..2], &OpenState::default(), &history);
        assert_eq!(
            b.actions,
            vec![Action::CreateWorkspace {
                name: "Gamma".into(),
                cwd: "/w/c".into(),
                tabs: vec![claude_tab("three", "/w/c", C)],
            }]
        );
    }

    #[test]
    fn selection_limits_workspaces_and_tabs() {
        let ws =
            select(saved(), &Selection { workspaces: vec!["gamma".into()], tabs: vec!["Alpha#2".into()] }).unwrap();
        assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Gamma"]);
        assert_eq!(ws[0].tabs, vec![claude_tab("two", "/w/a", B)]);
    }
}
