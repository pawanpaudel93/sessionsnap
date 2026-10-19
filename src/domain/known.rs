//! What sessionsnap knows of every agent session it has saved, in any snapshot, kept even when
//! those versions are pruned or deleted: the flags it was last saved with, or why it was left out.
//! A session put in a tab or a plan without flags of its own takes them from here, so it never
//! comes back with fewer restrictions than it ran with.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::model::{SessionRef, Tab, Workspace, has_control};
use crate::ports::{AgentCatalog, OpenState};

/// Every saved session, keyed by agent and lowercase session ID.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KnownSessions {
    pub sessions: BTreeMap<String, Known>,
}

/// One saved session: the flags it was last saved with (already filtered by the agent's
/// allow-list), or why it was left out. Once left out, it stays left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Known {
    pub agent: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_out: Option<String>,
}

/// How a session is launched when an edit or a plan puts it in a tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Launch {
    pub args: Vec<String>,
    pub left_out: Option<String>,
    /// False when nothing is known of the session's flags (it resumes with the agent's defaults).
    pub known: bool,
}

/// What a plan or an edit says about a session whose flags are unknown.
pub const UNKNOWN_FLAGS: &str = "launch flags unknown; resuming with defaults";

fn key(s: &SessionRef) -> String {
    format!("{}:{}", s.agent, s.session_id.to_lowercase())
}

impl KnownSessions {
    pub fn get(&self, s: &SessionRef) -> Option<&Known> {
        self.sessions.get(&key(s))
    }

    /// Records the agent tabs of `workspaces` (normalized), as the newest saved; returns whether
    /// anything changed.
    pub fn record(&mut self, workspaces: &[Workspace]) -> bool {
        let before = self.sessions.clone();
        for t in workspaces.iter().flat_map(|w| &w.tabs) {
            let Tab::Agent { agent, session_id, args, left_out, .. } = t else { continue };
            let entry = self.sessions.entry(key(&SessionRef::new(agent, session_id))).or_insert_with(|| Known {
                agent: agent.clone(),
                session_id: session_id.to_lowercase(),
                args: vec![],
                left_out: None,
            });
            if entry.left_out.is_some() {
                continue;
            }
            match left_out {
                Some(why) => {
                    entry.args.clear();
                    entry.left_out = Some(why.clone());
                }
                None => entry.args = args.clone(),
            }
        }
        self.sessions != before
    }

    /// How to launch session `s`: left out when it ever was; else with the flags it runs with in
    /// the terminal now (checked against the allow-list; a launch that loses the session leaves it
    /// out); else with the flags it was last saved with; else, unknown, with none.
    pub fn launch(&self, s: &SessionRef, open: Option<&OpenState>, agents: &dyn AgentCatalog) -> Launch {
        let saved = self.get(s);
        if let Some(why) = saved.and_then(|k| k.left_out.clone()) {
            return Launch { args: vec![], left_out: Some(why), known: true };
        }
        let live = open.into_iter().flat_map(|o| &o.workspaces).flat_map(|w| &w.tabs).find_map(|t| {
            t.agent.as_ref().filter(|a| a.kind == s.agent && a.session_id.eq_ignore_ascii_case(&s.session_id))
        });
        if let Some(a) = live {
            if a.args.iter().any(|x| has_control(x)) {
                let why = "its flags have control characters; resume it by hand".to_string();
                return Launch { args: vec![], left_out: Some(why), known: true };
            }
            let check = agents.check_flags(&a.kind, &a.args);
            return match check.lost_reason(&agents.display_name(&a.kind)) {
                Some(why) => Launch { args: vec![], left_out: Some(why), known: true },
                None => Launch { args: check.kept, left_out: None, known: true },
            };
        }
        match saved {
            Some(k) => Launch { args: k.args.clone(), left_out: None, known: true },
            None => Launch::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{A, agent_tab, agents, open_ws};

    fn ws(tabs: Vec<Tab>) -> Vec<Workspace> {
        vec![Workspace {
            name: "W".into(),
            color: None,
            cwd: "/w".into(),
            git_branch: None,
            cmux_workspace_id: None,
            tabs,
        }]
    }

    fn with(args: &[&str], left_out: Option<&str>) -> Tab {
        let mut t = agent_tab("claude", "t", "/w", A);
        if let Tab::Agent { args: a, left_out: l, .. } = &mut t {
            *a = args.iter().map(|s| s.to_string()).collect();
            *l = left_out.map(str::to_string);
        }
        t
    }

    #[test]
    fn newest_flags_win_and_left_out_sticks() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        assert_eq!(known.launch(&s, None, &agents), Launch::default());
        assert!(known.record(&ws(vec![with(&["--model", "opus"], None)])));
        assert!(!known.record(&ws(vec![with(&["--model", "opus"], None)])));
        known.record(&ws(vec![with(&["--model", "sonnet"], None)]));
        assert_eq!(known.launch(&s, None, &agents).args, ["--model", "sonnet"]);
        known.record(&ws(vec![with(&[], Some("lost"))]));
        known.record(&ws(vec![with(&["--model", "opus"], None)]));
        let l = known.launch(&s, None, &agents);
        assert_eq!((l.args.len(), l.left_out.as_deref()), (0, Some("lost")));
        // An uppercase ID is the same session.
        assert!(known.get(&SessionRef::new("claude", A.to_uppercase())).is_some());
    }

    #[test]
    fn live_flags_are_checked_and_win() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        known.record(&ws(vec![with(&["--model", "opus"], None)]));
        let mut open = OpenState {
            live: true,
            state_version: None,
            workspaces: vec![open_ws("W", "W", "/w", &[("t", "/w", Some(A))])],
        };
        let set = |open: &mut OpenState, args: &[&str]| {
            open.workspaces[0].tabs[0].agent.as_mut().unwrap().args = args.iter().map(|a| a.to_string()).collect();
        };
        set(&mut open, &["--permission-mode", "plan"]);
        assert_eq!(known.launch(&s, Some(&open), &agents).args, ["--permission-mode", "plan"]);
        set(&mut open, &["--frobnicate"]);
        assert!(known.launch(&s, Some(&open), &agents).left_out.is_some());
        set(&mut open, &["--model", "a\nb"]);
        assert!(known.launch(&s, Some(&open), &agents).left_out.is_some());
    }
}
