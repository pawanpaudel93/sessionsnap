mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::ports::{AgentRef, OpenTab};
use sessionsnap::testing::{A, C, open_ws, write_rollout};

const X: &str = "01a12bb7-d379-7541-a81a-7895725dc6dd";

fn codex_tab(title: &str, id: &str, args: &[&str]) -> OpenTab {
    OpenTab {
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
