use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::core::edit::{find_ws, locate, parse_tab_ref};
use crate::core::model::{Tab, Workspace, is_session_id};
use crate::error::{Exit, failure};
use crate::ports::{OpenState, OpenWorkspace, SessionHistory};

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
    Sessions { session_ids: Vec<String> },
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

/// Recovered sessions as one "Recovered sessions" workspace, titled from the history.
pub fn recovered_workspace(session_ids: &[String], history: &dyn SessionHistory) -> Result<Vec<Workspace>> {
    let mut tabs = vec![];
    for id in session_ids {
        if !is_session_id(id) {
            return Err(failure(Exit::Error, format!("'{id}' is not a Claude session ID")));
        }
        let info =
            history.find(id)?.ok_or_else(|| failure(Exit::NotFound, format!("no transcript for session {id}")))?;
        tabs.push(Tab::Claude {
            title: info.label(),
            cwd: info.cwd.clone().unwrap_or_default(),
            session_id: id.clone(),
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
/// - Claude tabs whose session is open anywhere are skipped; a session listed twice opens once.
/// - Claude tabs without a resumable transcript become plain shells (with a warning).
/// - A saved workspace matching an open one gets only its missing Claude tabs added; shells are
///   not added to open workspaces (they would duplicate). Unmatched workspaces are created.
pub fn build_actions(saved: &[Workspace], open: &OpenState, history: &dyn SessionHistory) -> Built {
    let open_sessions = open.open_sessions();
    let mut planned: HashSet<String> = HashSet::new();
    let mut b = Built::default();
    for ws in saved {
        let live = match_workspace(ws, open);
        let mut pending = vec![];
        for tab in &ws.tabs {
            let label = format!("{} › {}", ws.name, tab.title());
            match tab.session_id() {
                Some(id) if open_sessions.contains(id) => b.skipped.push(format!("{label}: already open")),
                Some(id) if planned.contains(id) => b.skipped.push(format!("{label}: same session listed earlier")),
                Some(id) if (!is_session_id(id) || !history.contains(id)) && live.is_some() => {
                    b.skipped.push(format!("{label}: transcript not found; not added to an open workspace"));
                }
                Some(id) if !is_session_id(id) || !history.contains(id) => {
                    b.warnings.push(format!("{label}: transcript not found, opening a plain shell"));
                    pending.push(Tab::Terminal {
                        title: tab.title().to_string(),
                        cwd: tab.cwd().to_string(),
                        command: None,
                    });
                }
                Some(id) => {
                    planned.insert(id.to_string());
                    pending.push(tab.clone());
                }
                None => pending.push(tab.clone()),
            }
        }
        if pending.is_empty() {
            continue;
        }
        match live {
            Some(live) => {
                let (claude_tabs, shells): (Vec<Tab>, Vec<Tab>) =
                    pending.into_iter().partition(|t| t.session_id().is_some());
                for s in shells {
                    b.skipped.push(format!(
                        "{} › {}: shell tabs are not added to an open workspace",
                        ws.name,
                        s.title()
                    ));
                }
                if !claude_tabs.is_empty() {
                    b.actions.push(Action::AddTabs {
                        workspace_ref: live.reference.clone().unwrap_or_default(),
                        workspace_name: live.name.clone(),
                        tabs: claude_tabs,
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

/// The open workspace a saved one corresponds to: same cmux ID, else the one holding most of its
/// Claude sessions, else the same name. Only live workspaces (with a ref) count.
fn match_workspace<'a>(ws: &Workspace, open: &'a OpenState) -> Option<&'a OpenWorkspace> {
    let live: Vec<&OpenWorkspace> = open.workspaces.iter().filter(|w| w.reference.is_some()).collect();
    if let Some(id) = &ws.cmux_workspace_id
        && let Some(w) = live.iter().copied().find(|w| &w.id == id)
    {
        return Some(w);
    }
    let sessions: HashSet<&str> = ws.tabs.iter().filter_map(|t| t.session_id()).collect();
    let best = live
        .iter()
        .copied()
        .map(|w| {
            let shared =
                w.tabs.iter().filter(|t| t.session_id.as_deref().is_some_and(|s| sessions.contains(s))).count();
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
    use crate::testing::{A, B, C, FakeHistory, open_ws};

    const D: &str = "44444444-4444-4444-8444-444444444444";

    fn history() -> FakeHistory {
        FakeHistory::with(&[(A, "/w/a", "one"), (B, "/w/a", "two"), (C, "/w/c", "three")])
    }

    fn claude_tab(title: &str, cwd: &str, id: &str) -> Tab {
        Tab::Claude { title: title.into(), cwd: cwd.into(), session_id: id.into() }
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
    fn sessions_source_rejects_non_uuid_ids() {
        let err = recovered_workspace(&["../x".into()], &history()).unwrap_err();
        assert_eq!(exit_of(&err), Exit::Error);
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
