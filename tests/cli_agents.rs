mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::ports::{AgentRef, OpenTab};
use sessionsnap::testing::{A, C, open_ws, write_rollout};

const X: &str = "01a12bb7-d379-7541-a81a-7895725dc6dd";

fn codex_tab(title: &str, id: &str, args: &[&str]) -> OpenTab {
    OpenTab {
        processes: vec![],
        reference: None,
        title: title.into(),
        cwd: "/w/a".into(),
        agent: Some(AgentRef {
            kind: "codex".into(),
            session_id: id.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            alt_session_id: None,
        }),
    }
}

#[test]
fn agents_lists_the_registry() {
    let env = setup();
    let o = env.run(&["agents", "--json"]).unwrap();
    let agents = o.json["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 17);
    assert_eq!(
        agents[1],
        serde_json::json!({"kind": "codex", "name": "Codex", "aliases": [], "program": "codex",
                           "resume": "codex resume <id>", "support": "full", "onPath": false})
    );
    let resume_only = agents.iter().filter(|a| a["support"] == "resume-only").count();
    assert_eq!(resume_only, 15);
    assert_eq!(agents[8]["aliases"], serde_json::json!(["cursor-agent"]));
    assert!(o.text.lines().any(|l| l.starts_with("rovodev") && l.contains("acli rovodev run --restore <id>")));
}

#[test]
fn sessions_and_recent_cover_codex() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/x", X, "port the parser");
    let o = env.run(&["sessions", "--json"]).unwrap();
    let rows = o.json["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let codex = rows.iter().find(|r| r["session"]["agent"] == "codex").unwrap();
    assert_eq!(
        codex["session"],
        serde_json::json!({"agent": "codex", "sessionId": X, "title": "port the parser",
                           "firstPrompt": "port the parser", "cwd": "/w/x", "modified": codex["session"]["modified"]})
    );
    assert_eq!(codex["open"], false);
    let only = env.run(&["sessions", "--agent", "codex"]).unwrap();
    assert_eq!(only.json["sessions"].as_array().unwrap().len(), 1);
    assert!(only.text.contains("codex") && only.text.contains(X), "{}", only.text);
    assert_eq!(exit_of(&env.run(&["sessions", "--agent", "opencode"]).unwrap_err()), Exit::Error);
    assert_eq!(exit_of(&env.run(&["sessions", "--agent", "nope"]).unwrap_err()), Exit::Error);

    let o = env.run(&["recent", "--json"]).unwrap();
    assert_eq!(o.json["sessions"][0]["agent"], "codex");
    assert_eq!(o.json["sessions"][0]["sessionId"], X);
    let p = env.run(&["recent", "--plan"]).unwrap();
    assert_eq!(
        p.json["plan"]["source"],
        serde_json::json!({"kind": "sessions", "sessions": [{"agent": "codex", "sessionId": X}]})
    );
    let tab = &p.json["plan"]["actions"][0]["tabs"][0];
    assert_eq!((tab["type"].as_str(), tab["agent"].as_str()), (Some("agent"), Some("codex")));
    // No program is on the test's search path.
    let warnings = p.json["plan"]["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w == "Codex (`codex`) is not on PATH; its tabs may not resume"), "{warnings:?}");
}

#[test]
fn codex_tabs_save_and_resume_with_their_flags() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/a", X, "port the parser");
    env.fake.state.borrow_mut().workspaces[0].tabs.push(codex_tab(
        "codex",
        X,
        &["-a", "on-request", "-s", "workspace-write"],
    ));
    env.run(&["save", "work"]).unwrap();
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    assert_eq!(
        shown.json["version"]["workspaces"][0]["tabs"][2],
        serde_json::json!({"type": "agent", "title": "port the parser", "cwd": "/w/a", "agent": "codex",
                           "sessionId": X, "args": ["-a", "on-request", "-s", "workspace-write"]})
    );
    assert_eq!(shown.json["version"]["sessionsnap"], 2);
    let text = env.run(&["show", "work"]).unwrap().text;
    assert!(text.contains("#3   codex   port the parser"), "{text}");
    assert!(text.contains("#2   shell   zsh"), "{text}");

    // The Codex tab closes; the Claude session with the same workspace stays open.
    env.fake.state.borrow_mut().workspaces[0].tabs.pop();
    let planned = env.run(&["restore", "work", "--plan"]).unwrap();
    let actions = planned.json["plan"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["tabs"][0]["agent"], "codex");
    let id = planned.json["plan"]["planId"].as_str().unwrap().to_string();
    env.run(&["apply", &id]).unwrap();
    let launched = env.fake.launched.borrow().clone();
    assert_eq!(
        launched[0].command.as_deref(),
        Some(format!("codex resume {X} -a on-request -s workspace-write").as_str())
    );
    assert_eq!(env.run(&["restore", "work", "--yes"]).unwrap().exit, Exit::NothingToDo);
}

#[test]
fn a_claude_session_does_not_match_a_codex_tab_with_the_same_id() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/a", A, "same id");
    env.run(&["save", "work"]).unwrap();
    // Alpha's Claude tab (A) is now a Codex tab with the same ID.
    env.fake.state.borrow_mut().workspaces[0].tabs[0] = codex_tab("t1", A, &[]);
    let planned = env.run(&["restore", "work", "--plan"]).unwrap();
    let tab = &planned.json["plan"]["actions"][0]["tabs"][0];
    assert_eq!((tab["agent"].as_str(), tab["sessionId"].as_str()), (Some("claude"), Some(A)));
}

#[test]
fn unknown_agents_are_kept_and_restore_as_shells() {
    let env = setup();
    env.fake.state.borrow_mut().workspaces.push(open_ws(
        "WG",
        "Gamma",
        "/w/g",
        &[("aider", "/w/g", Some("aider:chat-7"))],
    ));
    env.run(&["save", "work"]).unwrap();
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    let tab = &shown.json["version"]["workspaces"][2]["tabs"][0];
    assert_eq!((tab["agent"].as_str(), tab["sessionId"].as_str()), (Some("aider"), Some("chat-7")));
    assert!(shown.json["warnings"][0].as_str().unwrap().contains("doesn't know how to resume aider yet"));
    env.fake.state.borrow_mut().workspaces.pop();
    let o = env.run(&["restore", "work", "--yes"]).unwrap();
    assert_eq!(o.json["failed"], 0);
    let launched = env.fake.launched.borrow().clone();
    assert_eq!(launched.len(), 1);
    assert_eq!(
        (launched[0].title.as_str(), launched[0].cwd.as_str(), launched[0].command.as_deref()),
        ("aider", "/w/g", None)
    );
}

#[test]
fn tabs_take_sessions_of_any_agent() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/x", X, "port the parser");
    env.run(&["save", "work"]).unwrap();
    env.run(&["tab", "work", "Beta", "--add-session", X]).unwrap();
    let ws = env.run(&["show", "work", "--json"]).unwrap().json["version"]["workspaces"][1].clone();
    assert_eq!(ws["tabs"][1]["agent"], "codex");
    assert_eq!(ws["tabs"][1]["cwd"], "/w/x");
    env.run(&["tab", "work", "Beta", "--add-session", "ses_1", "--agent", "opencode", "--title", "oc"]).unwrap();
    let ws = env.run(&["show", "work", "--json"]).unwrap().json["version"]["workspaces"][1].clone();
    assert_eq!(
        ws["tabs"][2],
        serde_json::json!({"type": "agent", "title": "oc", "cwd": "/w/b", "agent": "opencode", "sessionId": "ses_1"})
    );
    // Not in any history.
    assert_eq!(exit_of(&env.run(&["tab", "work", "Beta", "--add-session", C]).unwrap_err()), Exit::NotFound);
    env.run(&["tab", "work", "Beta#1", "--session", X]).unwrap();
    let batch = format!(r#"[{{"op":"tab-add-session","ws":"Alpha","sessionId":"{X}","agent":"codex"}}]"#);
    let o = env.run_with_input(&["edit", "work", "--batch", "-"], &batch).unwrap();
    assert_eq!(o.json["tabs"], 6);
}

#[test]
fn format_1_snapshots_are_refused() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let text = std::fs::read_to_string(&file).unwrap().replace("\"sessionsnap\": 2", "\"sessionsnap\": 1");
    std::fs::write(&file, text).unwrap();
    let err = env.run(&["show", "work"]).unwrap_err();
    assert_eq!(exit_of(&err), Exit::UnknownFormat);
    assert!(err.to_string().contains("unreadable: format 1, expected 2"), "{err}");
    // list and history mark it instead of failing.
    env.run(&["save", "other"]).unwrap();
    let o = env.run(&["list"]).unwrap();
    assert_eq!(
        o.json["snapshots"][1],
        serde_json::json!({"name": "work", "latestVersion": 1, "error": "unreadable: format 1, expected 2"})
    );
    assert_eq!(o.json["snapshots"][0]["name"], "other");
    assert!(o.text.contains("unreadable: format 1, expected 2"), "{}", o.text);
    let h = env.run(&["history", "work"]).unwrap();
    assert_eq!(h.json["versions"][0], serde_json::json!({"version": 1, "error": "unreadable: format 1, expected 2"}));
}

#[test]
fn open_shows_each_tabs_agent() {
    let env = setup();
    env.fake.state.borrow_mut().workspaces[0].tabs.push(codex_tab("codex", X, &[]));
    let o = env.run(&["open"]).unwrap();
    let tabs = &o.json["workspaces"][0]["tabs"];
    assert_eq!(tabs[0]["agent"], "claude");
    assert_eq!(tabs[1]["agent"], serde_json::Value::Null);
    assert_eq!((tabs[2]["agent"].as_str(), tabs[2]["sessionId"].as_str()), (Some("codex"), Some(X)));
}

#[test]
fn control_characters_never_reach_the_terminal() {
    let env = setup();
    // Captured: an agent record with ^C in a flag is left out, with a warning.
    env.fake.state.borrow_mut().workspaces[0].tabs.push(codex_tab("bad", X, &["\u{3}touch /tmp/pwned\r"]));
    let o = env.run(&["save", "work"]).unwrap();
    assert!(o.json["warnings"][0].as_str().unwrap().contains("left out its agent session"), "{}", o.json);
    env.fake.state.borrow_mut().workspaces[0].tabs.pop();
    // Hand-edited: control characters in a flag, an ID, a folder and a command.
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    v["workspaces"].as_array_mut().unwrap().push(serde_json::json!({"name": "Evil", "cwd": "/w/e", "tabs": [
        {"type": "agent", "title": "a", "cwd": "/w/e", "agent": "codex", "sessionId": X, "args": ["\u{3}touch /tmp/pwned\r"]},
        {"type": "agent", "title": "b", "cwd": "/w/e", "agent": "opencode", "sessionId": "s1\nrm -rf ~"},
        {"type": "agent", "title": "c", "cwd": "/w/e\u{1b}[2J", "agent": "opencode", "sessionId": "s2"},
        {"type": "terminal", "title": "d", "cwd": "/w/e", "command": "make\rrm -rf ~"}
    ]}));
    std::fs::write(&file, serde_json::to_string(&v).unwrap()).unwrap();
    let o = env.run(&["restore", "work", "--yes", "-w", "Evil"]).unwrap();
    assert_eq!(o.json["failed"], 0, "{}", o.json);
    let launched = env.fake.launched.borrow().clone();
    assert_eq!(launched.len(), 4);
    for t in &launched {
        assert_eq!(t.command, None, "{t:?}");
        assert!(!t.cwd.chars().any(|c| c.is_control()), "{t:?}");
    }
    assert_eq!(launched[2].cwd, "/w/e");
}

#[test]
fn agent_names_are_normalized_on_load() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/a", X, "port the parser");
    env.fake.state.borrow_mut().workspaces[0].tabs.push(codex_tab("codex", X, &[]));
    env.run(&["save", "work"]).unwrap();
    // Hand-edited to another spelling of the same agent.
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let text = std::fs::read_to_string(&file).unwrap().replace("\"agent\": \"codex\"", "\"agent\": \"Codex\"");
    assert!(text.contains("\"Codex\""));
    std::fs::write(&file, text).unwrap();
    // Still the open session: nothing to restore, and the shown tab says "codex".
    assert_eq!(env.run(&["restore", "work", "--plan"]).unwrap().exit, Exit::NothingToDo);
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    assert_eq!(shown.json["version"]["workspaces"][0]["tabs"][2]["agent"], "codex");
    // A save of the same state writes nothing new.
    assert_eq!(env.run(&["save", "work"]).unwrap().exit, Exit::NothingToDo);
}

/// `save` keeps only allow-listed flags and names the others, never their values; restore of a
/// hand-edited snapshot drops a flag that is not allowed.
#[test]
fn save_names_the_flags_it_leaves_out() {
    let env = setup();
    let value = "do-not-show-me";
    env.fake.state.borrow_mut().workspaces[0].tabs.push(codex_tab(
        "codex",
        X,
        &["-m", "o3", "--remote-auth-token-env", value, "-c", "hide_agent_reasoning=true"],
    ));
    let o = env.run(&["save", "work"]).unwrap();
    let warnings = o.json["warnings"].to_string();
    assert!(warnings.contains("left out flags it does not restore: --remote-auth-token-env, -c"), "{warnings}");
    assert!(!warnings.contains(value) && !warnings.contains("hide_agent"), "{warnings}");
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    let tabs = shown.json["version"]["workspaces"][0]["tabs"].as_array().unwrap().clone();
    assert_eq!(tabs.last().unwrap()["args"], serde_json::json!(["-m", "o3"]));
    assert!(!shown.json.to_string().contains(value));
}

/// A snapshot saved before the allow-list (or hand-edited) is filtered when it is loaded: `show`
/// never displays a flag that is not allowed, and a later save never carries it forward.
#[test]
fn old_snapshots_are_filtered_on_load() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // Built at runtime so secret scanners don't flag the source.
    let ghp = format!("ghp_{}", "a1".repeat(18));
    let inline = format!(r#"{{"env":{{"T":"{ghp}"}}}}"#);
    v["workspaces"].as_array_mut().unwrap().push(serde_json::json!({"name": "Old", "cwd": "/w/o", "tabs": [
        {"type": "agent", "title": "a", "cwd": "/w/o", "agent": "claude", "sessionId": A,
         "args": ["--mcp-config", inline, "--model", "opus"]},
        {"type": "agent", "title": "b", "cwd": "/w/o", "agent": "claude", "sessionId": C,
         "args": ["--disallowedTools", format!("Bash(x {ghp})")]},
        {"type": "agent", "title": "c", "cwd": "/w/o", "agent": "claude", "sessionId": A,
         "args": ["--disallowedTools", format!("Bash(x {ghp})")]},
    ]}));
    v["workspaces"].as_array_mut().unwrap().push(serde_json::json!({"name": "Fresh", "cwd": "/w/f", "tabs": [
        {"type": "agent", "title": "f", "cwd": "/w/f", "agent": "claude", "sessionId": "44444444-4444-4444-8444-444444444444",
         "args": ["--disallowedTools", format!("Bash(x {ghp})")]},
    ]}));
    std::fs::write(&file, serde_json::to_string(&v).unwrap()).unwrap();
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    assert!(!shown.json.to_string().contains(&ghp), "{}", shown.json);
    let old = &shown.json["version"]["workspaces"][2]["tabs"];
    assert_eq!(old[0]["args"], serde_json::json!(["--model", "opus"]));
    // The deny rule can't be kept: the session stays, without its flags, with the reason.
    let reason = "a restricting flag (--disallowedTools) could not be saved; resume it by hand";
    assert_eq!((old[1]["type"].as_str(), old[1]["sessionId"].as_str()), (Some("agent"), Some(C)), "{old}");
    assert!(old[1].get("args").is_none(), "{old}");
    assert_eq!(old[1]["leftOut"], reason, "{old}");
    let warnings = shown.json["warnings"].to_string();
    assert!(warnings.contains(&format!("Old#2 'b': {reason}, restores as a plain shell")), "{warnings}");
    // Restore gives the reason. "Old" matches the open "Alpha" by its sessions: the lost tab is
    // skipped there, and a tab whose session is open is still "already open".
    let planned = env.run(&["restore", "work", "--plan"]).unwrap();
    let plan = planned.json["plan"].to_string();
    assert!(plan.contains(&format!("Old › b: {reason}; not added to an open workspace")), "{plan}");
    assert!(plan.contains("Old › c: already open"), "{plan}");
    // In a new workspace it opens as a plain shell, with a warning.
    assert!(plan.contains(&format!("Fresh › f: {reason}, opening a plain shell")), "{plan}");
    assert!(!plan.contains(&ghp));
    let tabs = &planned.json["plan"]["actions"][0]["tabs"];
    assert_eq!(tabs[0]["type"], "terminal", "{tabs}");
    // An edit keeps the session and the reason, and never writes the flags back.
    let ops = r#"[{"op":"ws-rename","ws":"Old","to":"Older"}]"#;
    env.run_with_input(&["edit", "work", "--batch", "-"], ops).unwrap();
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    let older = &shown.json["version"]["workspaces"][2]["tabs"];
    assert_eq!((older[1]["sessionId"].as_str(), older[1]["leftOut"].as_str()), (Some(C), Some(reason)), "{older}");
    let newest = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).max().unwrap();
    let text = std::fs::read_to_string(newest).unwrap();
    assert!(!text.contains(&ghp) && text.contains(C), "{text}");
}

/// A control character anywhere in a tab (a title is enough) never stops its flags being filtered:
/// no command that reads a snapshot shows flags a save would leave out.
#[test]
fn every_read_of_a_snapshot_filters_its_flags() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // Built at runtime so secret scanners don't flag the source.
    let ghp = format!("ghp_{}", "a1".repeat(18));
    let inline = format!(r#"{{"env":{{"T":"{ghp}"}}}}"#);
    v["workspaces"].as_array_mut().unwrap().push(serde_json::json!({"name": "Old", "cwd": "/w/o", "tabs": [
        {"type": "agent", "title": "bell\u{7}", "cwd": "/w/o", "agent": "claude", "sessionId": C,
         "args": ["--mcp-config", inline.clone(), "--model", "opus"]},
        {"type": "agent", "title": "t", "cwd": "/w/o", "agent": "claude", "sessionId": A,
         "args": ["--mcp-config", inline, "\u{3}x"]},
    ]}));
    std::fs::write(&file, serde_json::to_string(&v).unwrap()).unwrap();
    // A second version, so there is a diff and a history.
    env.fake.state.borrow_mut().workspaces[0].tabs.pop();
    env.run(&["save", "work"]).unwrap();
    let ops = r#"[{"op":"ws-from","from":"work@1","ws":"Old"}]"#;
    env.run(&["save", "other"]).unwrap();
    let outputs = [
        env.run(&["show", "work", "--json"]).unwrap(),
        env.run(&["show", "work"]).unwrap(),
        env.run(&["show", "work@1", "--json"]).unwrap(),
        env.run(&["diff", "work@1", "work@2", "--json"]).unwrap(),
        env.run(&["diff", "work@1", "work@2"]).unwrap(),
        env.run(&["history", "work", "--json"]).unwrap(),
        env.run(&["list", "--json"]).unwrap(),
        env.run_with_input(&["edit", "other", "--batch", "-", "--dry-run", "--json"], ops).unwrap(),
    ];
    for o in &outputs {
        let all = format!("{} {}", o.json, o.text);
        assert!(!all.contains(&ghp), "{all}");
    }
    let shown = &outputs[2].json["version"]["workspaces"][2]["tabs"];
    assert_eq!(shown[0]["args"], serde_json::json!(["--model", "opus"]), "{shown}");
    // Flags with a control character: the tab can't be resumed; its flags go, the session stays.
    assert_eq!((shown[1]["type"].as_str(), shown[1]["sessionId"].as_str()), (Some("agent"), Some(A)), "{shown}");
    assert!(shown[1].get("args").is_none());
    assert_eq!(shown[1]["leftOut"], "its flags have control characters; resume it by hand");
}

/// A save keeps a lost session the way a load does: its ID, no flags, and the reason. Restore then
/// warns and opens a shell, or skips the tab while its session is open.
#[test]
fn save_keeps_a_lost_session_with_its_reason() {
    let env = setup();
    // Built at runtime so secret scanners don't flag the source.
    let ghp = format!("ghp_{}", "a1".repeat(18));
    let deny = format!("Bash(x {ghp})");
    let mut tab = codex_tab("t", X, &[]);
    if let Some(a) = tab.agent.as_mut() {
        a.kind = "claude".into();
        a.session_id = C.into();
        a.args = vec!["--disallowedTools".into(), deny];
    }
    env.fake.state.borrow_mut().workspaces.push(open_ws("WG", "Gamma", "/w/g", &[]));
    env.fake.state.borrow_mut().workspaces[2].tabs.push(tab);
    let o = env.run(&["save", "work"]).unwrap();
    let reason = "a restricting flag (--disallowedTools) could not be saved; resume it by hand";
    assert!(o.json["warnings"].to_string().contains(reason), "{}", o.json);
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    assert!(!shown.json.to_string().contains(&ghp));
    let t = &shown.json["version"]["workspaces"][2]["tabs"][0];
    assert_eq!(
        (t["type"].as_str(), t["sessionId"].as_str(), t["leftOut"].as_str()),
        (Some("agent"), Some(C), Some(reason))
    );
    assert!(t.get("args").is_none());
    // Still open in cmux: skipped.
    let plan = env.run(&["restore", "work", "--plan"]).unwrap();
    assert_eq!(plan.exit, Exit::NothingToDo, "{}", plan.json);
    // Closed: a new workspace with a plain shell, and the reason.
    env.fake.state.borrow_mut().workspaces.pop();
    let plan = env.run(&["restore", "work", "--plan"]).unwrap().json["plan"].clone();
    assert!(plan["warnings"].to_string().contains(&format!("Gamma › t: {reason}, opening a plain shell")), "{plan}");
    assert_eq!(plan["actions"][0]["tabs"][0]["type"], "terminal", "{plan}");
}

/// A `leftOut` reason edited by hand is shown only as one short plain line.
#[test]
fn a_hand_edited_reason_is_sanitized() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let dir = env.dir.path().join("store/snapshots/work");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let ghp = format!("ghp_{}", "a1".repeat(18));
    for odd in ["\u{1b}[2Jgone".to_string(), "x".repeat(500), format!("token {ghp}")] {
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        v["workspaces"][1]["tabs"][0]["leftOut"] = serde_json::json!(odd);
        std::fs::write(&file, serde_json::to_string(&v).unwrap()).unwrap();
        let shown = env.run(&["show", "work", "--json"]).unwrap();
        let t = &shown.json["version"]["workspaces"][1]["tabs"][0];
        assert_eq!(t["leftOut"], "its flags were left out; resume it by hand", "{odd:?}");
        assert!(!shown.json.to_string().contains(&ghp));
    }
}
