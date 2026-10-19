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
    /// The kind of `left_out`: true when a rebuild from saved versions couldn't decide the flags
    /// (`DIFFER`), so the next live reading decides them; false (and for older records, which
    /// don't have it) when a launch's flags couldn't be kept, which is for good.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsure: bool,
    /// For an undecided entry: when the newest version it disagrees with was written (UTC, to the
    /// second). Only a reading taken after it decides the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsure_since: Option<String>,
}

/// How a session is launched when an edit or a plan puts it in a tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Launch {
    pub args: Vec<String>,
    pub left_out: Option<String>,
    /// False when nothing is known of the session's flags (it resumes with the agent's defaults).
    pub known: bool,
    /// `left_out` only says the flags are undecided (see `Known::unsure`).
    pub unsure: bool,
}

/// What a plan or an edit says about a session whose flags are unknown.
pub const UNKNOWN_FLAGS: &str = "launch flags unknown; resuming with defaults";
/// Why a session rebuilt from saved versions is left out when they disagree on its flags.
pub const DIFFER: &str = "its flags differ between saved versions; resume it by hand";
/// What a plan says when a tab's saved flags are not the session's newest.
pub const NEWEST_FLAGS: &str = "using the session's newest known flags";

/// The time of a version whose time is not known: later than any other, so it is always compared.
pub const ANY_TIME: &str = "~";

/// A record time (`…T12:00:05.123Z`) to the second, as version times are (`…T12:00:05Z`).
fn to_second(at: &str) -> String {
    if at.len() >= 19 { format!("{}Z", &at[..19]) } else { String::new() }
}

/// Kept flags as a set of launches can be compared: each flag with the values after it, sorted.
fn canonical(kept: &[String]) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = vec![];
    for word in kept {
        match groups.last_mut() {
            Some(group) if !word.starts_with('-') => group.push(word.clone()),
            _ => groups.push(vec![word.clone()]),
        }
    }
    groups.sort();
    groups
}

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
            unsure: false,
            unsure_since: None,
        })
    }

    /// Records the agent tabs of live launches captured from the terminal at `at` (UTC, RFC 3339)
    /// and normalized: a left-out tab leaves its session out for good; otherwise its flags replace
    /// older ones, and decide a session a rebuild left undecided. Returns whether anything changed.
    pub fn record_live(&mut self, workspaces: &[Workspace], at: &str) -> bool {
        let before = self.sessions.clone();
        for t in workspaces.iter().flat_map(|w| &w.tabs) {
            let Tab::Agent { agent, session_id, args, left_out, .. } = t else { continue };
            let entry = self.entry(agent, session_id);
            if entry.left_out.is_some() && !entry.unsure {
                continue;
            }
            if entry.unsure {
                // Undecided by a rebuild: decided by a reading taken after the version that made it
                // so (to the second, so a reading in that same second doesn't count).
                if entry.unsure_since.as_deref().is_some_and(|since| to_second(at).as_str() <= since) {
                    continue;
                }
                entry.left_out = None;
                entry.unsure = false;
                entry.unsure_since = None;
                entry.observed_at = None;
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

    /// Adds what saved versions (normalized, in any order: their times are not trusted) tell.
    /// Versions are compared on every flag they keep (in a canonical order), since a flag that
    /// widens what the agent may do matters as much as one that limits it, and which version came
    /// last can't be told: failing closed, a session they disagree on is left out
    /// (`DIFFER`).
    /// - A session the record lacks takes the flags its versions agree on.
    /// - An entry the record has stays as it is when every version agrees with it; a version with
    ///   other flags (it may hold a newer launch an update missed) leaves it out, and so does a
    ///   version where the session is left out.
    /// - Left out because the versions disagree, a session is only undecided (`unsure`): the next
    ///   live reading decides it. Left out because a version's launch was, it is left out for good.
    ///
    /// Flags taken from versions count as seen before any live read.
    ///
    /// Here every version may be newer than any entry; see `fill_from_dated`.
    pub fn fill_from<'a>(&mut self, versions: impl IntoIterator<Item = &'a [Workspace]>, agents: &dyn AgentCatalog) {
        self.fill_from_dated(versions.into_iter().map(|w| (ANY_TIME, w)), "", "", agents);
    }

    /// `fill_from`, with each version's time (UTC, `YYYY-MM-DDTHH:MM:SSZ`; `ANY_TIME` when not
    /// known). An existing entry is compared only with versions written no earlier than when it
    /// was seen and than `since` (the first failed update): an older version can't hold a newer
    /// launch. Times are to the second, so a version from the same second counts (failing
    /// closed). A session the record lacks is compared across all versions.
    ///
    /// A session made undecided remembers the newest such version's time (`now`, the time of this
    /// rebuild, for a version whose time is not known).
    pub fn fill_from_dated<'a>(
        &mut self,
        versions: impl IntoIterator<Item = (&'a str, &'a [Workspace])>,
        since: &str,
        now: &str,
        agents: &dyn AgentCatalog,
    ) {
        let time = |at: &str| if at == ANY_TIME { to_second(now) } else { at.to_string() };
        let mut seen: BTreeMap<String, Vec<(&str, &Tab)>> = BTreeMap::new();
        for (at, workspaces) in versions {
            for t in workspaces.iter().flat_map(|w| &w.tabs) {
                if let Some(s) = t.session() {
                    seen.entry(key(&s)).or_default().push((at, t));
                }
            }
        }
        for (k, dated) in seen {
            let tabs: Vec<&Tab> = dated.iter().map(|(_, t)| *t).collect();
            let Some(Tab::Agent { agent, session_id, args, .. }) = tabs.first().copied() else { continue };
            // A tab left out for good is a lost launch; one only undecided (copied from this
            // record by an edit) is a launch nothing is known of, which matches no other.
            let left_out = tabs.iter().find_map(|t| match t {
                Tab::Agent { left_out: Some(why), left_out_unsure: false, .. } => Some(why.clone()),
                _ => None,
            });
            let launch = |agent: &str, args: &[String]| canonical(&agents.check_flags(agent, args).kept);
            let launch_of = |t: &Tab| match t {
                Tab::Agent { left_out: Some(_), left_out_unsure: true, .. } => Some(None),
                Tab::Agent { agent, args, .. } => Some(Some(launch(agent, args))),
                Tab::Terminal { .. } => None,
            };
            let flags: Vec<Option<Vec<Vec<String>>>> = tabs.iter().filter_map(|t| launch_of(t)).collect();
            match self.sessions.get_mut(&k) {
                // Left out already, for good or until a live reading: never turned back into flags.
                Some(entry) if entry.left_out.is_some() => {
                    if let (Some(why), true) = (left_out, entry.unsure) {
                        entry.left_out = Some(why);
                        entry.unsure = false;
                        entry.unsure_since = None;
                    }
                }
                Some(entry) => {
                    let ours = Some(launch(&entry.agent, &entry.args));
                    let from = to_second(entry.observed_at.as_deref().unwrap_or("")).max(to_second(since));
                    let newest_differing = dated
                        .iter()
                        .filter(|(at, _)| *at >= from.as_str())
                        .filter(|(_, t)| launch_of(t).is_some_and(|f| f != ours))
                        .map(|(at, _)| time(at))
                        .max();
                    let differs = newest_differing.is_some();
                    if let Some(why) = left_out {
                        *entry = Known {
                            args: vec![],
                            left_out: Some(why),
                            observed_at: None,
                            unsure: false,
                            unsure_since: None,
                            ..entry.clone()
                        };
                    } else if differs {
                        *entry = Known {
                            args: vec![],
                            left_out: Some(DIFFER.to_string()),
                            observed_at: None,
                            unsure: true,
                            unsure_since: newest_differing,
                            ..entry.clone()
                        };
                    }
                }
                None => {
                    let differ = left_out.is_none() && flags.iter().any(|f| f.is_none() || *f != flags[0]);
                    let left_out = left_out.or_else(|| differ.then(|| DIFFER.to_string()));
                    self.sessions.insert(
                        k,
                        Known {
                            agent: agent.clone(),
                            session_id: session_id.to_lowercase(),
                            args: if left_out.is_some() { vec![] } else { args.clone() },
                            observed_at: left_out.is_none().then(String::new),
                            left_out,
                            unsure: differ,
                            unsure_since: differ.then(|| dated.iter().map(|(at, _)| time(at)).max()).flatten(),
                        },
                    );
                }
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
                let Tab::Agent { title, agent, session_id, args, left_out, left_out_unsure, .. } = t else { continue };
                let Some(k) = self.get(&SessionRef::new(agent.as_str(), session_id.as_str())) else { continue };
                let lost = left_out.is_some() && !*left_out_unsure;
                if let Some(why) = &k.left_out {
                    // A tab's own lost launch stays lost.
                    if !lost {
                        args.clear();
                        *left_out = Some(why.clone());
                        *left_out_unsure = k.unsure;
                    }
                } else if *left_out_unsure && k.observed_at.is_some() {
                    // Undecided when the tab was written; the record has decided since.
                    warnings.push(format!("{} › {title}: {NEWEST_FLAGS}", w.name));
                    *args = k.args.clone();
                    *left_out = None;
                    *left_out_unsure = false;
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
        // Left out for good; undecided only until the session is seen live (just below).
        let undecided = saved.filter(|k| k.unsure).and_then(|k| k.left_out.clone());
        if let Some(k) = saved.filter(|k| !k.unsure)
            && let Some(why) = &k.left_out
        {
            return Launch { args: vec![], left_out: Some(why.clone()), known: true, unsure: false };
        }
        let live = open.into_iter().flat_map(|o| &o.workspaces).flat_map(|w| &w.tabs).find_map(|t| {
            t.agent.as_ref().filter(|a| a.kind == s.agent && a.session_id.eq_ignore_ascii_case(&s.session_id))
        });
        if let Some(a) = live {
            if a.args.iter().any(|x| has_control(x)) {
                let why = "its flags have control characters; resume it by hand".to_string();
                return Launch { args: vec![], left_out: Some(why), known: true, unsure: false };
            }
            let check = agents.check_flags(&a.kind, &a.args);
            return match check.lost_reason(&agents.display_name(&a.kind)) {
                Some(why) => Launch { args: vec![], left_out: Some(why), known: true, unsure: false },
                None => Launch { args: check.kept, left_out: None, known: true, unsure: false },
            };
        }
        match saved {
            _ if undecided.is_some() => Launch { args: vec![], left_out: undecided, known: true, unsure: true },
            Some(k) if k.observed_at.is_some() => {
                Launch { args: k.args.clone(), left_out: None, known: true, unsure: false }
            }
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

    /// Versions that differ in any kept flag, one that widens what the agent may do included,
    /// leave the session out: which came last can't be told.
    #[test]
    fn versions_differing_in_any_flag_leave_the_session_out() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let rule = format!("Bash({}:*)", "git");
        for (one, other) in [
            (vec!["--dangerously-skip-permissions"], vec![]),
            (vec!["--allowedTools", rule.as_str()], vec![]),
            (vec!["--add-dir", "/w/x"], vec!["--add-dir", "/w/y"]),
        ] {
            let a = ws(vec![with(&one, None)]);
            let b = ws(vec![with(&other, None)]);
            for order in [[a.as_slice(), b.as_slice()], [b.as_slice(), a.as_slice()]] {
                let mut known = KnownSessions::default();
                known.fill_from(order, &agents);
                assert_eq!(known.get(&s).unwrap().left_out.as_deref(), Some(DIFFER), "{one:?} / {other:?}");
            }
        }
        // The same flags in another order are the same launch.
        let a = ws(vec![with(&["--model", "opus", "--add-dir", "/w/x"], None)]);
        let b = ws(vec![with(&["--add-dir", "/w/x", "--model", "opus"], None)]);
        let mut known = KnownSessions::default();
        known.fill_from([a.as_slice(), b.as_slice()], &agents);
        assert_eq!(known.get(&s).unwrap().args.len(), 4);
        assert_eq!(known.get(&s).unwrap().left_out, None);
    }

    /// Completing an entry after a failed update: a saved version with other flags than the entry
    /// (it may be the newer, stricter launch the update missed) leaves the session out.
    #[test]
    fn completing_an_entry_that_a_version_contradicts_leaves_it_out() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        known.record_live(&ws(vec![with(&[], None)]), T1);
        known.fill_from([ws(vec![with(&[], None)]).as_slice()], &agents);
        assert_eq!(known.get(&s).unwrap().left_out, None, "versions that agree change nothing");
        known.fill_from([ws(vec![with(&["--permission-mode", "plan"], None)]).as_slice()], &agents);
        assert_eq!(known.get(&s).unwrap().left_out.as_deref(), Some(DIFFER));
    }

    /// Undecided after a rebuild, a session is decided by the next live reading; a lost launch is
    /// never cleared, and a record written before the kind existed counts as lost.
    #[test]
    fn only_undecided_sessions_are_decided_by_a_live_reading() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let plan = ws(vec![with(&["--permission-mode", "plan"], None)]);
        let none = ws(vec![with(&[], None)]);
        let mut known = KnownSessions::default();
        known.fill_from([plan.as_slice(), none.as_slice()], &agents);
        assert!(known.get(&s).unwrap().unsure);
        known.record_live(&plan, T1);
        let k = known.get(&s).unwrap();
        assert_eq!((k.args.len(), k.left_out.as_deref(), k.unsure), (2, None, false));
        // Lost: stays.
        known.record_live(&ws(vec![with(&[], Some("lost"))]), T1);
        known.record_live(&plan, T2);
        assert_eq!(known.get(&s).unwrap().left_out.as_deref(), Some("lost"));
        // An older record's left-out entry has no kind: lost.
        let old: KnownSessions = serde_json::from_value(serde_json::json!({"sessions": {format!("claude:{A}"): {
            "agent": "claude", "sessionId": A, "leftOut": DIFFER}}}))
        .unwrap();
        let mut old = old;
        old.record_live(&plan, T2);
        assert_eq!(old.get(&s).unwrap().left_out.as_deref(), Some(DIFFER));
        assert!(!serde_json::to_string(&old).unwrap().contains("unsure"));
    }

    /// An existing entry is compared only with versions written since it was seen and since the
    /// first failed update.
    #[test]
    fn only_newer_versions_can_contradict_an_entry() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let plan = ws(vec![with(&["--permission-mode", "plan"], None)]);
        let none = ws(vec![with(&[], None)]);
        let entry = || {
            let mut known = KnownSessions::default();
            known.record_live(&plan, T2);
            known
        };
        let state = |k: &KnownSessions| k.get(&s).unwrap().unsure;
        // Older than the entry: no say.
        let mut known = entry();
        known.fill_from_dated([("2026-01-01T00:00:00Z", none.as_slice())], "", "", &agents);
        assert!(!state(&known));
        // Newer than the entry, but older than the failed update: no say either.
        let mut known = entry();
        known.fill_from_dated([("2026-01-02T00:00:01Z", none.as_slice())], "2026-01-03T00:00:00.000Z", "", &agents);
        assert!(!state(&known));
        // Newer than both (or the same second): it may hold the launch the update missed.
        for at in ["2026-01-03T00:00:00Z", "2026-01-04T00:00:00Z", ANY_TIME] {
            let mut known = entry();
            known.fill_from_dated([(at, none.as_slice())], "2026-01-03T00:00:00.500Z", "", &agents);
            assert!(state(&known), "{at}");
        }
    }

    /// A tab's undecided state is read as undecided; a left-out tab without a kind (written
    /// before kinds existed) is a lost launch.
    #[test]
    fn tabs_keep_their_kind_of_left_out() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut undecided = with(&[], Some(DIFFER));
        if let Tab::Agent { left_out_unsure, .. } = &mut undecided {
            *left_out_unsure = true;
        }
        let mut known = KnownSessions::default();
        known.fill_from([ws(vec![undecided.clone()]).as_slice()], &agents);
        assert!(known.get(&s).unwrap().unsure);
        let mut known = KnownSessions::default();
        known.fill_from([ws(vec![with(&[], Some(DIFFER))]).as_slice()], &agents);
        assert!(!known.get(&s).unwrap().unsure);
        // A decided entry overrides an undecided tab when restoring, with a warning.
        let mut known = KnownSessions::default();
        known.record_live(&ws(vec![with(&["--permission-mode", "plan"], None)]), T1);
        let mut saved = ws(vec![undecided]);
        assert_eq!(known.apply_to(&mut saved).len(), 1);
        assert_eq!(saved[0].tabs[0], with(&["--permission-mode", "plan"], None));
    }

    /// Probe 12: an undecided entry is decided only by a reading taken after the version that made
    /// it undecided was written; an older, slow reading has no say.
    #[test]
    fn only_a_later_reading_decides() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let mut known = KnownSessions::default();
        known.record_live(&ws(vec![with(&[], None)]), "2026-01-01T00:00:00.000Z");
        let plan = ws(vec![with(&["--permission-mode", "plan"], None)]);
        known.fill_from_dated([("2026-01-01T00:10:05Z", plan.as_slice())], "2026-01-01T00:10:00.000Z", "", &agents);
        assert!(known.get(&s).unwrap().unsure);
        known.record_live(&ws(vec![with(&[], None)]), "2026-01-01T00:05:00.000Z");
        assert!(known.get(&s).unwrap().unsure, "an older reading");
        known.record_live(&ws(vec![with(&[], None)]), "2026-01-01T00:10:05.500Z");
        assert!(known.get(&s).unwrap().unsure, "a reading in the same second");
        known.record_live(&plan, "2026-01-01T00:10:06.000Z");
        let k = known.get(&s).unwrap();
        assert_eq!((k.unsure, k.args.len()), (false, 2));
    }

    #[test]
    fn filling_adds_what_the_record_lacks() {
        let agents = agents(vec![]);
        let s = SessionRef::new("claude", A);
        let plan = ws(vec![with(&["--permission-mode", "plan"], None)]);
        let none = ws(vec![with(&[], None)]);
        // Versions that differ: left out.
        let mut known = KnownSessions::default();
        known.fill_from([plan.as_slice(), none.as_slice()], &agents);
        assert_eq!(known.get(&s).unwrap().left_out.as_deref(), Some(DIFFER));
        // Versions that agree: their flags, seen before any live read.
        let mut known = KnownSessions::default();
        known.fill_from([plan.as_slice(), plan.as_slice()], &agents);
        let k = known.get(&s).unwrap();
        assert_eq!((k.args.len(), k.left_out.as_deref(), k.observed_at.as_deref()), (2, None, Some("")));
        // A live launch replaces them.
        known.record_live(&none, T1);
        assert!(known.get(&s).unwrap().args.is_empty());
        // A session left out in some version is left out.
        known.fill_from([ws(vec![with(&[], Some("lost"))]).as_slice()], &agents);
        assert_eq!(known.get(&s).unwrap().left_out.as_deref(), Some("lost"));
        assert_eq!(
            agents.check_flags("claude", &words(&["--model", "opus", "--permission-mode", "plan"])).restricting,
            ["--permission-mode", "plan"]
        );
    }

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
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
        known.fill_from([ws(vec![with(&[], Some("lost"))]).as_slice()], &agents(vec![]));
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
