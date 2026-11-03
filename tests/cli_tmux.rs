mod common;

use common::setup;
use sessionsnap::error::Exit;
use sessionsnap::testing::FakeScheduler;

#[test]
fn a_save_from_tmux_says_so_and_a_cmux_one_does_not() {
    let env = setup();
    env.run(&["save", "cm"]).unwrap();
    let v = env.run(&["show", "cm", "--json"]).unwrap();
    assert!(v.json["version"]["source"].get("terminal").is_none(), "cmux is the default, not written");
    env.fake.name.set("tmux");
    env.run(&["save", "tm"]).unwrap();
    let v = env.run(&["show", "tm", "--json"]).unwrap();
    assert_eq!(v.json["version"]["source"]["terminal"], "tmux");
}

#[test]
fn a_schedule_keeps_saving_from_the_terminal_it_was_added_in() {
    let env = setup();
    env.fake.name.set("tmux");
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    env.run(&["schedule", "edit", "work", "--interval", "1h"]).unwrap();
    assert_eq!(FakeScheduler::log(env.dir.path()), ["install work 600 tmux", "install work 3600 tmux"]);
    env.fake.state.borrow_mut().workspaces.clear();
    let o = env.run(&["schedule", "run", "work"]).unwrap();
    assert_eq!(o.exit, Exit::NothingToDo);
    assert_eq!(o.json["reason"], "tmux shows no workspaces");
}

#[test]
fn the_terminal_option_takes_only_known_terminals() {
    use clap::Parser;
    use sessionsnap::interfaces::cli::Cli;
    let parse = |t: &str| Cli::try_parse_from(["sessionsnap", "--terminal", t, "list"]);
    assert_eq!(parse("tmux").unwrap().terminal.as_deref(), Some("tmux"));
    assert!(parse("iterm").is_err());
}

#[test]
fn an_agent_found_in_a_tabs_processes_is_saved_and_a_hostile_flag_never_reaches_a_command() {
    use sessionsnap::ports::TabProcess;
    use sessionsnap::testing::{A, open_ws};
    let env = setup();
    env.fake.name.set("tmux");
    let mut ws = open_ws("$1", "work", "/w/a", &[("fix", "/w/a", None)]);
    let argv = ["/usr/local/bin/claude", "--model", "x; rm -rf ~", "--resume", A];
    ws.tabs[0].processes =
        vec![TabProcess { argv: argv.map(String::from).to_vec(), started: std::time::SystemTime::now() }];
    *env.fake.state.borrow_mut() = Default::default();
    env.fake.state.borrow_mut().live = true;
    env.fake.state.borrow_mut().workspaces = vec![ws];
    env.run(&["save", "tm"]).unwrap();
    let shown = env.run(&["show", "tm", "--json"]).unwrap().json;
    let tab = &shown["version"]["workspaces"][0]["tabs"][0];
    assert_eq!((tab["agent"].as_str(), tab["sessionId"].as_str()), (Some("claude"), Some(A)));
    assert!(!shown.to_string().contains("rm -rf"), "{shown}");
    // Nothing open: the restore opens it, and its command never holds the hostile value.
    env.fake.state.borrow_mut().workspaces.clear();
    env.run(&["restore", "tm", "--yes"]).unwrap();
    let launched = env.fake.launched.borrow();
    assert_eq!(launched.len(), 1);
    let command = launched[0].command.as_deref().unwrap();
    assert!(command.contains(A) && !command.contains("rm -rf"), "{command}");
}

#[test]
fn a_save_with_nothing_running_writes_nothing() {
    let env = setup();
    env.fake.name.set("tmux");
    *env.fake.state.borrow_mut() = Default::default(); // not live, nothing open
    let e = env.run(&["save", "x"]).unwrap_err();
    assert_eq!(sessionsnap::error::exit_of(&e), Exit::NothingToDo);
    assert_eq!(e.to_string(), "tmux is not running");
    assert_eq!(env.run(&["list", "--json"]).unwrap().json["snapshots"].as_array().unwrap().len(), 0);
}

#[test]
fn a_schedule_never_saves_from_another_terminal() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap(); // added in cmux
    env.fake.name.set("tmux");
    let o = env.run(&["schedule", "run", "work"]).unwrap();
    assert_eq!(o.exit, Exit::NothingToDo);
    assert!(o.json["reason"].as_str().unwrap().contains("--terminal cmux schedule run work"), "{}", o.json);
    let listed = env.run(&["schedule", "list"]).unwrap().json;
    assert_eq!(listed["schedules"][0]["lastRun"]["result"], "skipped");
    assert_eq!(listed["schedules"][0]["latestVersion"], 1, "nothing saved from tmux");
}

#[test]
fn saving_some_workspaces_notes_only_their_own_tabs() {
    use sessionsnap::ports::TabProcess;
    use sessionsnap::testing::open_ws;
    let env = setup();
    env.fake.name.set("tmux");
    let claude = || vec![TabProcess { argv: vec!["claude".into()], started: std::time::SystemTime::now() }];
    let mut a = open_ws("$1", "one", "/w/x", &[("a", "/w/x", None)]);
    let mut b = open_ws("$2", "two", "/w/x", &[("b", "/w/x", None)]);
    a.tabs[0].processes = claude();
    b.tabs[0].processes = claude();
    env.fake.state.borrow_mut().workspaces = vec![a, b];
    let all = env.run(&["save", "all", "--json"]).unwrap().json;
    let notes = all["warnings"].to_string();
    assert!(notes.contains("one › a") && notes.contains("two › b"), "{notes}");
    let one = env.run(&["save", "only", "-w", "one", "--json"]).unwrap().json;
    let notes = one["warnings"].to_string();
    assert!(notes.contains("one › a") && !notes.contains("two › b"), "{notes}");
}

/// tmux split windows: saved with the workspace, recreated as panes when the whole workspace is
/// restored, unchanged by a resize, and dropped by an edit that moves its tabs.
#[test]
fn split_windows_are_saved_and_restored_as_panes() {
    use sessionsnap::domain::model::Window;
    let window = |tabs: usize, layout: &str| Window { tabs, layout: layout.into() };
    let env = setup();
    env.fake.name.set("tmux");
    env.fake.state.borrow_mut().workspaces[0].windows = vec![window(2, "aaaa,80x24,0,0{40x24,0,0,1,39x24,41,0,2}")];
    env.fake.state.borrow_mut().workspaces[1].windows = vec![window(1, "bbbb,80x24,0,0,3")];
    env.run(&["save", "s"]).unwrap();
    let shown = env.run(&["show", "s", "--json"]).unwrap().json;
    assert_eq!(shown["version"]["workspaces"][0]["windows"][0]["tabs"], 2);
    assert!(shown["version"]["workspaces"][1].get("windows").is_none(), "no split, nothing saved");

    // Dragging the divider changes only the sizes: not a new version.
    env.fake.state.borrow_mut().workspaces[0].windows = vec![window(2, "cccc,80x24,0,0{20x24,0,0,1,59x24,21,0,2}")];
    assert_eq!(env.run(&["save", "s"]).unwrap().exit, Exit::NothingToDo);

    env.fake.state.borrow_mut().workspaces.remove(0);
    let plan = env.run(&["restore", "s", "--plan"]).unwrap().json;
    assert_eq!(plan["plan"]["actions"][0]["windows"][0]["tabs"], 2, "{plan}");
    env.run(&["apply", plan["plan"]["planId"].as_str().unwrap()]).unwrap();
    let launched: Vec<(bool, Option<String>)> =
        env.fake.launched.borrow().iter().map(|t| (t.split, t.layout.clone())).collect();
    assert_eq!(launched, [(false, None), (true, Some("aaaa,80x24,0,0{40x24,0,0,1,39x24,41,0,2}".into()))]);

    // Moving a tab out leaves the window's panes unknown: one window per tab again.
    env.run(&["tab", "s", "Alpha#1", "--move", "Beta"]).unwrap();
    let shown = env.run(&["show", "s", "--json"]).unwrap().json;
    assert!(shown["version"]["workspaces"].as_array().unwrap().iter().all(|w| w.get("windows").is_none()));
}

/// A window whose panes changed places (side by side to stacked) is a change; a window count that
/// can't be right (an edited file) restores one window per tab instead of failing.
#[test]
fn a_rearranged_split_is_saved_and_a_bad_window_count_is_ignored() {
    use sessionsnap::domain::model::Window;
    let env = setup();
    env.fake.name.set("tmux");
    let side = r#"{"V":2,"L":{"t":"h","w":160,"h":48,"x":0,"y":0,"c":[{"t":"p","w":80,"h":48,"x":0,"y":0,"i":0},{"t":"p","w":79,"h":48,"x":81,"y":0,"i":1}]}}"#;
    let stacked = side.replace(r#""t":"h""#, r#""t":"v""#);
    env.fake.state.borrow_mut().workspaces[0].windows = vec![Window { tabs: 2, layout: side.into() }];
    env.run(&["save", "s"]).unwrap();
    env.fake.state.borrow_mut().workspaces[0].windows = vec![Window { tabs: 2, layout: stacked.clone() }];
    assert_eq!(env.run(&["save", "s"]).unwrap().json["version"], 2, "orientation changed");
    let shown = env.run(&["show", "s", "--json"]).unwrap().json;
    assert_eq!(shown["version"]["workspaces"][0]["windows"][0]["layout"], stacked.as_str());

    // A count that overflows when added up.
    env.fake.state.borrow_mut().workspaces[0].windows =
        vec![Window { tabs: usize::MAX, layout: side.into() }, Window { tabs: 3, layout: side.into() }];
    env.run(&["save", "s"]).unwrap();
    env.fake.state.borrow_mut().workspaces.remove(0);
    let plan = env.run(&["restore", "s", "--plan"]).unwrap().json;
    assert!(plan["plan"]["actions"][0].get("windows").is_none(), "{plan}");
    env.run(&["apply", plan["plan"]["planId"].as_str().unwrap()]).unwrap();
    assert!(env.fake.launched.borrow().iter().all(|t| !t.split && t.layout.is_none()));
}

/// Pi hides its flags from the terminal: they come from the record its sessionsnap extension adds
/// to the session, and without a record the session is saved without flags, with a warning.
#[test]
fn pi_flags_come_from_its_own_record() {
    use sessionsnap::ports::{AgentRef, OpenTab};
    use sessionsnap::testing::{A, B, write_pi_session};
    let env = setup();
    let recorded = write_pi_session(&env.dir.path().join("pi"), "/w/p", A, "first");
    write_pi_session(&env.dir.path().join("pi"), "/w/q", B, "second");
    let mut text = std::fs::read_to_string(&recorded).unwrap();
    text.push_str(r#"{"type":"custom","id":"c1","customType":"sessionsnap.launch","data":{"args":["--tools","read","--offline"]}}"#);
    text.push('\n');
    std::fs::write(&recorded, text).unwrap();
    let pi_tab = |id: &str| OpenTab {
        reference: None,
        title: "pi".into(),
        cwd: "/w/p".into(),
        // What a terminal reports for Pi: no flags (or the wrong ones).
        agent: Some(AgentRef { kind: "pi".into(), session_id: id.into(), args: vec![], alt_session_id: None }),
        processes: vec![],
    };
    env.fake.state.borrow_mut().workspaces[1].tabs = vec![pi_tab(A), pi_tab(B)];
    let saved = env.run(&["save", "s"]).unwrap();
    let warnings = saved.json["warnings"].to_string();
    assert!(warnings.contains("install-pi") && !warnings.contains(A), "{warnings}");
    let shown = env.run(&["show", "s", "--json"]).unwrap().json;
    let tabs = &shown["version"]["workspaces"][1]["tabs"];
    assert_eq!(tabs[0]["args"], serde_json::json!(["--tools", "read", "--offline"]));
    assert!(tabs[1].get("args").is_none(), "{}", tabs[1]);
    env.fake.state.borrow_mut().workspaces.remove(1);
    let plan = env.run(&["restore", "s", "--plan"]).unwrap().json;
    env.run(&["apply", plan["plan"]["planId"].as_str().unwrap()]).unwrap();
    let commands: Vec<Option<String>> = env.fake.launched.borrow().iter().map(|t| t.command.clone()).collect();
    assert!(commands.contains(&Some(format!("pi --session {A} --tools read --offline"))), "{commands:?}");
    assert!(commands.contains(&Some(format!("pi --session {B}"))), "{commands:?}");
}
