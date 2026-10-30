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
                pinned: w.pinned,
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

/// An agent tab keeps only the flags its agent allows (see `AgentCatalog::check_flags`; an agent
/// the catalog does not know keeps none), and `note` names the flags left out. When its flags lose
/// the session (a restricting flag that can't be kept, an unknown flag, a launch that is not a
/// session), the tab keeps its session ID but no flags, with the reason in `left_out` (as loading
/// does), so restore gives the reason and opens a plain shell. An agent record with a control
/// character in its kind, ID or flags is not saved at all (its ID can't be trusted): the tab is
/// saved as a plain terminal, and `note` says so.
fn to_tab(t: &OpenTab, agents: &dyn AgentCatalog, note: &mut dyn FnMut(&str)) -> Tab {
    let terminal = || Tab::Terminal { title: t.title.clone(), cwd: t.cwd.clone(), command: None };
    let Some(a) = t.agent.as_ref() else { return terminal() };
    if has_control(&a.kind) || has_control(&a.session_id) || a.args.iter().any(|x| has_control(x)) {
        note("left out its agent session (control characters in its kind, ID or flags)");
        return terminal();
    }
    let check = agents.check_flags(&a.kind, &a.args);
    let display = agents.display_name(&a.kind);
    if let Some(n) = check.note(&display) {
        note(&n);
    }
    let left_out = check.lost_reason(&display);
    Tab::Agent {
        title: agents
            .history(&a.kind)
            .and_then(|h| h.title_for(&t.cwd, &a.session_id))
            .unwrap_or_else(|| t.title.clone()),
        cwd: t.cwd.clone(),
        agent: agents.kind_of(&a.kind).map_or_else(|| a.kind.clone(), str::to_string),
        session_id: a.session_id.clone(),
        args: if left_out.is_some() { vec![] } else { check.kept },
        left_out,
        left_out_unsure: false,
    }
}

/// Why a session whose live launch flags hold a control character is left out.
pub const CONTROL_FLAGS: &str = "its launch flags have control characters; resume it by hand";

/// The launches `captured` (from `capture` of `open`) were read from, for the record of sessions:
/// its tabs, plus each agent session whose flags hold a control character, which `capture` saved
/// as a plain terminal, as left out (when its kind and ID are clean), so the record never falls
/// back to older flags for it.
pub fn seen(open: &OpenState, agents: &dyn AgentCatalog, captured: &[Workspace]) -> Vec<Workspace> {
    let ids: Vec<&str> = captured.iter().filter_map(|w| w.cmux_workspace_id.as_deref()).collect();
    let tabs: Vec<Tab> = open
        .workspaces
        .iter()
        .filter(|w| ids.contains(&w.id.as_str()))
        .flat_map(|w| &w.tabs)
        .filter_map(|t| t.agent.as_ref())
        .filter(|a| !has_control(&a.kind) && !has_control(&a.session_id) && a.args.iter().any(|x| has_control(x)))
        .map(|a| Tab::Agent {
            title: String::new(),
            cwd: String::new(),
            agent: agents.kind_of(&a.kind).map_or_else(|| a.kind.clone(), str::to_string),
            session_id: a.session_id.clone(),
            args: vec![],
            left_out: Some(CONTROL_FLAGS.to_string()),
            left_out_unsure: false,
        })
        .collect();
    let mut out = captured.to_vec();
    if !tabs.is_empty() {
        out.push(Workspace {
            name: String::new(),
            color: None,
            cwd: String::new(),
            git_branch: None,
            pinned: false,
            cmux_workspace_id: None,
            tabs,
        });
    }
    out
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
                alt_session_id: None,
            }),
        };
        open.workspaces[1].tabs = vec![tab("codex", B, &["-a", "on-request"]), tab("aider", C, &[])];
        let (ws, notes) = capture(&open, &history, &["Beta".into()]).unwrap();
        assert_eq!(notes, Vec::<String>::new());
        assert_eq!(
            ws[0].tabs,
            [
                Tab::Agent {
                    title: "codex title".into(),
                    cwd: "/w/a".into(),
                    agent: "codex".into(),
                    session_id: B.into(),
                    args: vec!["-a".into(), "on-request".into()],
                    left_out: None,
                    left_out_unsure: false,
                },
                // Unknown to sessionsnap: kept as cmux reported it, titled from the tab, without
                // flags (it has no allow-list).
                Tab::Agent {
                    title: "aider tab".into(),
                    cwd: "/w/a".into(),
                    agent: "aider".into(),
                    session_id: C.into(),
                    args: vec![],
                    left_out: None,
                    left_out_unsure: false,
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
            agent: Some(AgentRef {
                kind: "codex".into(),
                session_id: A.into(),
                args: vec!["\u{3}x\r".into()],
                alt_session_id: None,
            }),
        }];
        let (ws, notes) = capture(&open, &history, &[]).unwrap();
        assert_eq!(ws[1].tabs, [Tab::Terminal { title: "bad".into(), cwd: "/w/b".into(), command: None }]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("Beta › bad: left out its agent session"), "{notes:?}");
    }

    /// Captures one agent tab in workspace "Beta" and returns it with the notes.
    fn capture_one(kind: &str, id: &str, args: &[String]) -> (Tab, Vec<String>) {
        let history = agents(vec![]);
        let mut open = open_state();
        open.workspaces[1].tabs = vec![OpenTab {
            reference: None,
            title: "t".into(),
            cwd: "/w/b".into(),
            agent: Some(AgentRef {
                kind: kind.into(),
                session_id: id.into(),
                args: args.to_vec(),
                alt_session_id: None,
            }),
        }];
        let (ws, notes) = capture(&open, &history, &["Beta".into()]).unwrap();
        (ws[0].tabs[0].clone(), notes)
    }

    fn saved_args(tab: &Tab) -> Vec<String> {
        match tab {
            Tab::Agent { args, .. } => args.clone(),
            Tab::Terminal { .. } => panic!("not an agent tab: {tab:?}"),
        }
    }

    /// The session is kept, without its flags, with the reason they were left out.
    fn is_lost(tab: &Tab) -> bool {
        matches!(tab, Tab::Agent { args, left_out: Some(_), .. } if args.is_empty())
    }

    fn words(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keeps_the_flags_each_agent_allows() {
        let claude = words(&[
            "--model",
            "opus",
            "--permission-mode",
            "plan",
            "--allowedTools",
            "Bash(git:*)",
            "--disallowedTools",
            "Bash(gh auth:*)",
            "--add-dir",
            "/w/sk-learn-2024",
            "--settings",
            "~/s.json",
        ]);
        let (tab, notes) = capture_one("claude", A, &claude);
        assert_eq!((saved_args(&tab), notes), (claude, vec![]));
        let codex =
            words(&["-m", "o3", "-s", "workspace-write", "-a", "on-request", "-c", "model_reasoning_effort=high"]);
        let (tab, notes) = capture_one("codex", B, &codex);
        assert_eq!((saved_args(&tab), notes), (codex, vec![]));
        // `--flag=value` forms and long names too.
        let inline = words(&["--model=o3", "--sandbox=read-only", "--config=model=o3", "--cd", "/w"]);
        let (tab, notes) = capture_one("codex", B, &inline);
        assert_eq!((saved_args(&tab), notes), (inline, vec![]));
    }

    /// Everything else is left out, and the note names the flag but never its value.
    #[test]
    fn leaves_out_other_flags_with_a_note_naming_them() {
        // Built at runtime so secret scanners don't flag the source.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let cases: Vec<(&str, Vec<String>, &str, &str)> = vec![
            ("claude", words(&["--mcp-config", r#"{"env":{"X":"y"}}"#]), "--mcp-config", "env"),
            ("codex", vec!["-c".into(), format!("model_context_window={ghp}")], "-c", "ghp_"),
            ("claude", words(&["--append-system-prompt", "secret plan"]), "--append-system-prompt", "secret"),
            ("codex", words(&["--remote-auth-token-env", "MY_TOKEN"]), "--remote-auth-token-env", "MY_"),
            ("claude", vec!["--model".into(), format!("opus-{ghp}")], "--model", "ghp_"),
            ("codex", vec!["-m".into(), ghp.clone()], "-m", "ghp_"),
        ];
        for (kind, mut args, name, value) in cases {
            let id = if kind == "claude" { A } else { B };
            args.extend(words(&["--verbose"]).into_iter().filter(|_| kind == "claude"));
            let (tab, notes) = capture_one(kind, id, &args);
            let want: Vec<String> = if kind == "claude" { words(&["--verbose"]) } else { vec![] };
            assert_eq!(saved_args(&tab), want, "{args:?}");
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].contains(name), "{notes:?}");
            assert!(!notes[0].contains(value) || name.contains(value), "{notes:?} shows {value}");
        }
    }

    /// A flag the agent's table doesn't know leaves out the session: its arity is unknown, so what
    /// follows it can't be read for certain.
    #[test]
    fn unknown_flags_leave_out_the_session() {
        for (kind, id, args, name) in [
            ("claude", A, words(&["--header", "x-api-key: v"]), "--header"),
            ("codex", B, words(&["--api-key", "v"]), "--api-key"),
            ("claude", A, words(&["--foo", "bar", "--verbose"]), "--foo"),
            ("codex", B, words(&["-vs", "read-only"]), "-v"),
            ("codex", B, words(&["--sandb", "read-only"]), "--sandb"),
        ] {
            let (tab, notes) = capture_one(kind, id, &args);
            assert!(is_lost(&tab), "{args:?}");
            assert_eq!(
                notes,
                [format!("Beta › t: left out its agent session: an unknown flag {name}; resume it by hand")],
                "{args:?}"
            );
            assert!(!notes[0].contains("x-api-key") && !notes[0].contains("bar"), "{notes:?}");
        }
    }
    /// A Codex `-c` key on no list leaves out the session too; its value is never shown.
    #[test]
    fn unlisted_codex_config_keys_leave_out_the_session() {
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let (tab, notes) =
            capture_one("codex", B, &["-c".into(), format!(r#"mcp_servers.gh.env={{GITHUB_TOKEN="{ghp}"}}"#)]);
        assert!(is_lost(&tab));
        assert_eq!(
            notes,
            ["Beta › t: left out its agent session: an unlisted Codex -c key mcp_servers.*; resume it by hand"]
        );
    }

    /// A restricting flag that cannot be saved leaves out the whole agent session: resuming
    /// without it would allow more than the user did.
    #[test]
    fn a_restricting_flag_that_cannot_be_saved_leaves_out_the_session() {
        // Built at runtime so secret scanners don't flag the source.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        for flag in ["--disallowedTools", "--disallowed-tools"] {
            let args = vec![flag.to_string(), "Edit".into(), format!("Bash(curl -H {ghp}:*)")];
            let (tab, notes) = capture_one("claude", A, &args);
            assert!(is_lost(&tab), "{tab:?}");
            assert!(matches!(&tab, Tab::Agent { session_id, left_out: Some(why), .. }
                if session_id == A && why == &format!("a restricting flag ({flag}) could not be saved; resume it by hand")));
            assert_eq!(
                notes,
                [format!(
                    "Beta › t: left out its agent session: a restricting flag ({flag}) could not be saved; resume it by hand"
                )]
            );
            let (tab, notes) = capture_one("claude", A, &[format!("{flag}=Bash(x {ghp})")]);
            assert!(is_lost(&tab) && notes.len() == 1, "{notes:?}");
        }
        // Codex: a sandbox or approval value, or a restricting `-c` key, of an unexpected shape.
        for (args, name) in [
            (words(&["--sandbox", "read only"]), "--sandbox"),
            (words(&["-s", "read-only", "-a"]), "-a"),
            (words(&["-c", "sandbox_mode=[\"read-only\"]"]), "-c"),
            (words(&["-c", "sandbox_workspace_write.writable_roots=[\"/a\"]"]), "-c"),
            (words(&["--profile", "{x}"]), "--profile"),
        ] {
            let (tab, notes) = capture_one("codex", B, &args);
            assert!(is_lost(&tab), "{args:?}");
            assert!(notes[0].contains(&format!("a restricting flag ({name})")), "{notes:?}");
        }
        let fine = words(&["-s", "read-only", "-c", "sandbox_mode=read-only", "-c", "approval_policy=\"never\""]);
        assert_eq!(saved_args(&capture_one("codex", B, &fine).0), fine);
        // A neutral flag that cannot be saved is only noted: the session still resumes.
        let (tab, notes) = capture_one("codex", B, &words(&["--model", "a b", "-s", "read-only"]));
        assert_eq!(
            (saved_args(&tab), notes),
            (words(&["-s", "read-only"]), vec!["Beta › t: left out flags it does not restore: --model".to_string()])
        );
        // An allow rule that cannot be saved only makes the resumed session stricter.
        let args = vec!["--allowedTools".into(), "Edit".into(), format!("Bash(curl -H {ghp}:*)")];
        let (tab, notes) = capture_one("claude", A, &args);
        assert_eq!(saved_args(&tab), words(&["--allowedTools", "Edit"]));
        assert_eq!(notes, ["Beta › t: left out flags it does not restore: --allowedTools"]);
    }

    /// A Claude Code launch that is a subcommand or a one-shot run is not a session to resume.
    #[test]
    fn launches_that_are_not_sessions_are_left_out() {
        for (args, what) in [
            (words(&["-p", "--model", "opus"]), "a launch with -p"),
            (words(&["--print"]), "a launch with --print"),
            (words(&["--no-session-persistence"]), "a launch with --no-session-persistence"),
            (words(&["--model", "opus", "mcp", "list"]), "the `mcp` command"),
            (words(&["setup-token"]), "the `setup-token` command"),
        ] {
            let (tab, notes) = capture_one("claude", A, &args);
            assert!(is_lost(&tab), "{args:?}");
            assert_eq!(
                notes,
                [format!("Beta › t: left out its agent session: {what} is not a session that can be resumed")]
            );
        }
        // A prompt that happens to be a command's name, after a flag's value, is a prompt.
        let (tab, _) = capture_one("claude", A, &words(&["--model", "opus", "fix the mcp config"]));
        assert_eq!(saved_args(&tab), words(&["--model", "opus"]));
    }

    /// Agents whose flags are untested resume only a launch without flags.
    #[test]
    fn resume_only_and_unknown_agents_with_flags_are_left_out() {
        for (kind, id, name) in
            [("gemini", "g-1", "Gemini CLI"), ("opencode", "s1", "OpenCode"), ("aider", "x1", "aider")]
        {
            let (tab, notes) = capture_one(kind, id, &words(&["--sandbox"]));
            assert!(is_lost(&tab), "{kind}");
            assert_eq!(
                notes,
                [format!(
                    "Beta › t: left out its agent session: sessionsnap can't restore {name}'s launch flags yet; resume it by hand"
                )],
                "{kind}"
            );
            let (tab, notes) = capture_one(kind, id, &[]);
            assert_eq!((saved_args(&tab), notes), (vec![], vec![]), "{kind}");
        }
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

    #[test]
    fn capture_keeps_pinned_and_saves_it_only_when_set() {
        let mut open = open_state();
        open.workspaces[0].pinned = true;
        let ws = capture(&open, &agents(vec![]), &[]).unwrap().0;
        assert!(ws[0].pinned && !ws[1].pinned);
        assert_eq!(serde_json::to_value(&ws[0]).unwrap()["pinned"], true);
        assert!(serde_json::to_value(&ws[1]).unwrap().get("pinned").is_none());
    }
}
