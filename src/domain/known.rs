//! What sessionsnap has seen of every agent session it saved, in any snapshot, kept even when
//! those versions are pruned or deleted: the flags it last ran with, as `save` read them from cmux,
//! or why it was left out. Only live launches write it: edits, copies and restores never do, so an
//! older copy of a session can never replace what was seen last. Everything that resumes a
//! session, or puts one in a tab, reads it, so a session never comes back with fewer restrictions
//! than it last ran with.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::model::{SessionRef, Tab, Workspace, has_control};
use crate::ports::{AgentCatalog, OpenState};

/// Every session seen, keyed by agent and lowercase session ID.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KnownSessions {
    pub sessions: BTreeMap<String, Known>,
}

/// One session: the flags of its newest live launch (already checked against the agent's
/// allow-list) and when that was seen, or why it was left out. Once left out, it stays left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Known {
    pub agent: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_out: Option<String>,
    /// When the launch these flags come from was seen (UTC, RFC 3339); None when it never was
    /// (an entry made only to keep a session left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
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
/// What a plan says when a tab's saved flags are not the session's newest.
pub const NEWEST_FLAGS: &str = "using the session's newest known flags";

fn key(s: &SessionRef) -> String {
    format!("{}:{}", s.agent, s.session_id.to_lowercase())
}

impl KnownSessions {
    pub fn get(&self, s: &SessionRef) -> Option<&Known> {
        self.sessions.get(&key(s))
    }

    fn entry(&mut self, agent: &str, session_id: &str) -> &mut Known {
        self.sessions.entry(key(&SessionRef::new(agent, session_id))).or_insert_with(|| Known {
            agent: agent.to_string(),
            session_id: session_id.to_lowercase(),
            args: vec![],
            left_out: None,
            observed_at: None,
        })
    }

    /// Records the agent tabs of live launches captured from the terminal at `at` (UTC, RFC 3339)
    /// and normalized: a left-out tab leaves its session out for good; otherwise its flags replace
    /// older ones. Returns whether anything changed.
    pub fn record_live(&mut self, workspaces: &[Workspace], at: &str) -> bool {
        let before = self.sessions.clone();
        for t in workspaces.iter().flat_map(|w| &w.tabs) {
            let Tab::Agent { agent, session_id, args, left_out, .. } = t else { continue };
            let entry = self.entry(agent, session_id);
            if entry.left_out.is_some() {
                continue;
            }
            match left_out {
                Some(why) => {
                    entry.args.clear();
                    entry.left_out = Some(why.clone());
                }
                None if entry.observed_at.as_deref().is_none_or(|seen| seen <= at) => {
                    entry.args = args.clone();
                    entry.observed_at = Some(at.to_string());
                }
                None => {}
            }
        }
        self.sessions != before
    }

    /// Keeps every session left out in `workspaces` left out (from any version, live or not).
    pub fn record_left_out(&mut self, workspaces: &[Workspace]) {
        for t in workspaces.iter().flat_map(|w| &w.tabs) {
            if let Tab::Agent { agent, session_id, left_out: Some(why), .. } = t {
                let entry = self.entry(agent, session_id);
                if entry.left_out.is_none() {
                    entry.args.clear();
                    entry.left_out = Some(why.clone());
                }
            }
        }
    }

    /// Adds what `other` knows: its left-out sessions stay left out, and its flags win where they
    /// were seen later.
    pub fn merge(&mut self, other: KnownSessions) {
        for (k, theirs) in other.sessions {
            let ours = self.sessions.entry(k).or_insert_with(|| theirs.clone());
            if ours.left_out.is_some() {
                continue;
            }
            if theirs.left_out.is_some() {
                ours.args.clear();
                ours.left_out = theirs.left_out;
            } else if theirs.observed_at > ours.observed_at {
                ours.args = theirs.args;
                ours.observed_at = theirs.observed_at;
            }
        }
    }

    /// For restoring `workspaces`: each agent tab whose session is left out here is left out with
    /// that reason; a tab that is not left out takes its session's newest seen flags, with a
    /// warning when they differ from its own. Returns the warnings.
    pub fn apply_to(&self, workspaces: &mut [Workspace]) -> Vec<String> {
        let mut warnings = vec![];
        for w in workspaces {
            for t in &mut w.tabs {
                let Tab::Agent { title, agent, session_id, args, left_out, .. } = t else { continue };
                let Some(k) = self.get(&SessionRef::new(agent.as_str(), session_id.as_str())) else { continue };
                if let Some(why) = &k.left_out {
                    args.clear();
                    *left_out = Some(why.clone());
                } else if left_out.is_none() && k.observed_at.is_some() && *args != k.args {
                    warnings.push(format!("{} › {title}: {NEWEST_FLAGS}", w.name));
                    *args = k.args.clone();
                }
            }
        }
        warnings
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
            Some(k) if k.observed_at.is_some() => Launch { args: k.args.clone(), left_out: None, known: true },
            _ => Launch::default(),
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

    const T1: &str = "2026-01-01T00:00:00.000Z";
    const T2: &str = "2026-01-02T00:00:00.000Z";

    #[test]
    fn newest_live_flags_win_and_left_out_sticks() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        assert_eq!(known.launch(&s, None, &agents), Launch::default());
        assert!(known.record_live(&ws(vec![with(&["--model", "opus"], None)]), T1));
        assert!(!known.record_live(&ws(vec![with(&["--model", "opus"], None)]), T1));
        known.record_live(&ws(vec![with(&["--model", "sonnet"], None)]), T2);
        // An older observation never replaces a newer one.
        known.record_live(&ws(vec![with(&[], None)]), T1);
        assert_eq!(known.launch(&s, None, &agents).args, ["--model", "sonnet"]);
        known.record_live(&ws(vec![with(&[], Some("lost"))]), T2);
        known.record_live(&ws(vec![with(&["--model", "opus"], None)]), T2);
        let l = known.launch(&s, None, &agents);
        assert_eq!((l.args.len(), l.left_out.as_deref()), (0, Some("lost")));
        // An uppercase ID is the same session.
        assert!(known.get(&SessionRef::new("claude", A.to_uppercase())).is_some());
    }

    #[test]
    fn merging_keeps_left_out_and_the_newest_flags() {
        let mut ours = KnownSessions::default();
        ours.record_live(&ws(vec![with(&["--model", "opus"], None)]), T2);
        let mut theirs = KnownSessions::default();
        theirs.record_live(&ws(vec![with(&["--model", "sonnet"], None)]), T1);
        ours.merge(theirs.clone());
        assert_eq!(ours.get(&SessionRef::new("claude", A)).unwrap().args, ["--model", "opus"]);
        theirs.record_left_out(&ws(vec![with(&[], Some("lost"))]));
        ours.merge(theirs);
        assert_eq!(ours.get(&SessionRef::new("claude", A)).unwrap().left_out.as_deref(), Some("lost"));
    }

    #[test]
    fn restores_take_left_out_and_the_newest_flags() {
        let mut known = KnownSessions::default();
        known.record_live(&ws(vec![with(&["--permission-mode", "plan"], None)]), T1);
        let mut saved = ws(vec![with(&[], None)]);
        assert_eq!(known.apply_to(&mut saved), [format!("W › t: {NEWEST_FLAGS}")]);
        assert_eq!(saved[0].tabs[0], with(&["--permission-mode", "plan"], None));
        // The same flags: no warning.
        assert!(known.apply_to(&mut saved).is_empty());
        // A tab left out on its own stays so.
        let mut saved = ws(vec![with(&[], Some("own"))]);
        known.apply_to(&mut saved);
        assert_eq!(saved[0].tabs[0], with(&[], Some("own")));
        known.record_left_out(&ws(vec![with(&[], Some("lost"))]));
        let mut saved = ws(vec![with(&["--permission-mode", "plan"], None)]);
        known.apply_to(&mut saved);
        assert_eq!(saved[0].tabs[0], with(&[], Some("lost")));
    }

    #[test]
    fn live_flags_are_checked_and_win() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        known.record_live(&ws(vec![with(&["--model", "opus"], None)]), T1);
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
