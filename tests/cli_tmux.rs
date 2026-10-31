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
