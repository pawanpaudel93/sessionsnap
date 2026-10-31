//! Which agent session runs in a tab, for a terminal that only reports the tab's processes (tmux).
//! The agent is the process nearest the shell whose program is a known agent (or whose script is,
//! under `node` and the like); its session is the one its arguments name (`--resume <id>`), else,
//! from the agent's history, the session in the tab's folder changed most recently since the agent
//! started. When several tabs run the same agent in one folder with no session in their
//! arguments, none is guessed: they stay plain tabs, with a warning.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ports::{AgentCatalog, AgentRef, OpenState, OpenWarning, TabProcess};

/// Launchers whose first argument is the agent's script (`node …/codex`).
const LAUNCHERS: &[&str] = &["node", "bun", "deno", "python", "python3"];
/// How long before an agent started its session may have last changed (a resumed one).
const SLACK: Duration = Duration::from_secs(60);

/// An agent found in a tab, before its session is known.
struct Found {
    kind: &'static str,
    args: Vec<String>,
    started: SystemTime,
    session: Option<String>,
}

/// Names the agent session of every tab that has processes and no agent yet; adds a warning to
/// `open` for each agent whose session can't be told.
pub fn identify(open: &mut OpenState, agents: &dyn AgentCatalog, now: SystemTime) {
    // (workspace, tab) → what runs there.
    let mut found: BTreeMap<(usize, usize), Found> = BTreeMap::new();
    for (wi, w) in open.workspaces.iter().enumerate() {
        for (ti, t) in w.tabs.iter().enumerate().filter(|(_, t)| t.agent.is_none()) {
            if let Some(f) = t.processes.iter().find_map(|p| agent_of(p, agents)) {
                found.insert((wi, ti), f);
            }
        }
    }
    let taken: HashSet<String> = found.values().filter_map(|f| f.session.clone()).collect();
    let mut groups: BTreeMap<(&'static str, String), Vec<(usize, usize)>> = BTreeMap::new();
    for (&at, f) in found.iter().filter(|(_, f)| f.session.is_none()) {
        groups.entry((f.kind, open.workspaces[at.0].tabs[at.1].cwd.clone())).or_default().push(at);
    }
    for ((kind, cwd), tabs) in groups {
        let session = match tabs[..] {
            [at] => recent_session(agents, kind, &cwd, found[&at].started, now, &taken),
            _ => Err(""),
        };
        match session {
            Ok(id) => found.get_mut(&tabs[0]).expect("grouped above").session = Some(id),
            Err(reason) => {
                let why = if tabs.len() > 1 {
                    format!(
                        "{} tabs run {} in {cwd} with no session ID in their command",
                        tabs.len(),
                        agents.display_name(kind)
                    )
                } else {
                    format!("{} runs in {cwd}, but {reason}", agents.display_name(kind))
                };
                // One note per workspace, so a save of some workspaces notes only its own.
                let mut by_workspace: BTreeMap<usize, Vec<String>> = BTreeMap::new();
                for &(wi, ti) in &tabs {
                    let w = &open.workspaces[wi];
                    by_workspace.entry(wi).or_default().push(format!("{} › {}", w.name, w.tabs[ti].title));
                }
                for (wi, names) in by_workspace {
                    let workspace = Some(open.workspaces[wi].name.clone());
                    let message = format!("{why}; saved as plain tabs: {}", names.join(", "));
                    open.warnings.push(OpenWarning { workspace, message });
                }
            }
        }
    }
    for ((wi, ti), f) in found {
        if let Some(session_id) = f.session {
            open.workspaces[wi].tabs[ti].agent =
                Some(AgentRef { kind: f.kind.into(), session_id, args: f.args, alt_session_id: None });
        }
    }
}

/// The agent process `p` runs, with its arguments after the program (and the launcher's script).
fn agent_of(p: &TabProcess, agents: &dyn AgentCatalog) -> Option<Found> {
    let base = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
    let program = base(p.argv.first()?);
    let (kind, args) = match agents.kind_of(&program) {
        Some(kind) => (kind, &p.argv[1..]),
        None if LAUNCHERS.contains(&program.as_str()) => {
            let script = base(p.argv.get(1)?);
            let script = script.strip_suffix(".js").or_else(|| script.strip_suffix(".mjs")).unwrap_or(&script);
            (agents.kind_of(script)?, &p.argv[2..])
        }
        None => return None,
    };
    let session = agents.session_in_args(kind, args);
    Some(Found { kind, args: args.to_vec(), started: p.started, session })
}

/// The one session of agent `kind` in `cwd` changed since `started` (less `SLACK`) that no other tab
/// has; else why there is none to take. Two or more is not guessed: one may be another terminal's.
fn recent_session(
    agents: &dyn AgentCatalog,
    kind: &str,
    cwd: &str,
    started: SystemTime,
    now: SystemTime,
    taken: &HashSet<String>,
) -> Result<String, &'static str> {
    const NOT_FOUND: &str = "its session was not found";
    let since = started.checked_sub(SLACK).ok_or(NOT_FOUND)?;
    let within = now.duration_since(since).map_err(|_| NOT_FOUND)?;
    let sessions = agents.history(kind).ok_or(NOT_FOUND)?.scan(Some(within)).map_err(|_| NOT_FOUND)?;
    let mut candidates = sessions
        .into_iter()
        .filter(|s| s.cwd.as_deref() == Some(cwd) && s.modified_at >= since && !taken.contains(&s.session_id));
    match (candidates.next(), candidates.next()) {
        (Some(s), None) => Ok(s.session_id),
        (Some(_), Some(_)) => Err("several of its sessions there changed since it started, so none is guessed"),
        _ => Err(NOT_FOUND),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{OpenTab, OpenWorkspace};
    use crate::testing::{A, B, FakeHistory, agents};

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn mins_ago(m: u64) -> SystemTime {
        now() - Duration::from_secs(m * 60)
    }

    /// A tab in `cwd` whose processes are (minutes since start, argv), nearest the shell first.
    fn tab(title: &str, cwd: &str, procs: &[(u64, &[&str])]) -> OpenTab {
        OpenTab {
            reference: None,
            title: title.into(),
            cwd: cwd.into(),
            agent: None,
            processes: procs
                .iter()
                .map(|&(m, argv)| TabProcess {
                    argv: argv.iter().map(|a| a.to_string()).collect(),
                    started: mins_ago(m),
                })
                .collect(),
        }
    }

    fn state(tabs: Vec<OpenTab>) -> OpenState {
        OpenState {
            live: true,
            workspaces: vec![OpenWorkspace {
                id: "$1".into(),
                reference: None,
                name: "w".into(),
                cwd: "/w".into(),
                git_branch: None,
                pinned: false,
                tabs,
            }],
            ..Default::default()
        }
    }

    /// Claude sessions (id, cwd) last changed `mins` minutes ago.
    fn history(sessions: &[(&str, &str, u64)]) -> FakeHistory {
        let mut h = FakeHistory::with(&sessions.iter().map(|&(id, cwd, _)| (id, cwd, "t")).collect::<Vec<_>>());
        for (s, &(_, _, m)) in h.sessions.iter_mut().zip(sessions) {
            s.modified_at = mins_ago(m);
        }
        h
    }

    fn messages(open: &OpenState) -> Vec<&str> {
        open.warnings.iter().map(|w| w.message.as_str()).collect()
    }

    fn agent(open: &OpenState, i: usize) -> Option<(String, String, Vec<String>)> {
        open.workspaces[0].tabs[i].agent.clone().map(|a| (a.kind, a.session_id, a.args))
    }

    #[test]
    fn the_agent_nearest_the_shell_with_the_session_its_args_name() {
        let mut open = state(vec![
            tab(
                "a",
                "/w",
                &[(20, &["/usr/local/bin/claude", "--add-dir", "/a b", "--resume", A]), (5, &["codex", "resume", B])],
            ),
            tab("b", "/w/b", &[(10, &["node", "/opt/lib/codex.js", "-m", "o3", "resume", B])]),
            tab("c", "/w", &[(10, &["vim", "notes"])]),
        ]);
        identify(&mut open, &agents(vec![]), now());
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(agent(&open, 0), Some(("claude".into(), A.into(), s(&["--add-dir", "/a b", "--resume", A]))));
        assert_eq!(agent(&open, 1), Some(("codex".into(), B.into(), s(&["-m", "o3", "resume", B]))));
        assert_eq!(agent(&open, 2), None);
        assert!(open.warnings.is_empty());
    }

    #[test]
    fn a_session_not_in_the_args_is_the_newest_in_its_folder_since_the_start() {
        let mut open = state(vec![tab("a", "/w", &[(20, &["claude", "--model", "opus"])])]);
        // A changed since the start in /w; B before it; the third elsewhere.
        let h = history(&[(A, "/w", 2), (B, "/w", 600), ("33333333-3333-4333-8333-333333333333", "/x", 1)]);
        identify(&mut open, &agents(vec![h]), now());
        assert_eq!(agent(&open, 0).map(|a| a.1), Some(A.to_string()));
    }

    #[test]
    fn two_sessions_changed_since_the_start_are_not_guessed_between() {
        // Another Claude Code in the same folder (in another terminal) also wrote since.
        let mut open = state(vec![tab("a", "/w", &[(20, &["claude"])])]);
        identify(&mut open, &agents(vec![history(&[(A, "/w", 2), (B, "/w", 5)])]), now());
        assert_eq!(agent(&open, 0), None);
        assert!(open.warnings[0].message.contains("several of its sessions there changed"), "{:?}", open.warnings);
    }

    #[test]
    fn a_session_another_tab_names_is_not_taken_twice() {
        let mut open = state(vec![tab("a", "/w", &[(20, &["claude", "-r", A])]), tab("b", "/x", &[(20, &["claude"])])]);
        identify(&mut open, &agents(vec![history(&[(A, "/x", 1)])]), now());
        assert_eq!(agent(&open, 1), None);
        assert_eq!(
            messages(&open),
            ["Claude Code runs in /x, but its session was not found; saved as plain tabs: w › b"]
        );
    }

    #[test]
    fn several_unnamed_sessions_in_one_folder_are_not_guessed() {
        let mut open = state(vec![tab("a", "/w", &[(20, &["claude"])]), tab("b", "/w", &[(20, &["claude"])])]);
        identify(&mut open, &agents(vec![history(&[(A, "/w", 2), (B, "/w", 3)])]), now());
        assert_eq!((agent(&open, 0), agent(&open, 1)), (None, None));
        assert_eq!(
            messages(&open),
            ["2 tabs run Claude Code in /w with no session ID in their command; saved as plain tabs: w › a, w › b"]
        );
    }

    #[test]
    fn an_agent_with_no_history_and_no_session_in_its_args_is_a_plain_tab() {
        let mut open = state(vec![tab("a", "/w", &[(20, &["gemini"])])]);
        identify(&mut open, &agents(vec![]), now());
        assert_eq!(agent(&open, 0), None);
        assert_eq!(open.warnings.len(), 1);
    }
}
