mod common;

use common::{Env, setup};
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::testing::A;

/// The version `export` writes, as JSON.
fn exported(env: &Env, snapshot: &str) -> serde_json::Value {
    env.run(&["export", snapshot]).unwrap().json
}

#[test]
fn an_export_imports_on_another_machine_with_its_folders_moved() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let file = env.dir.path().join("work.json");
    let o = env.run(&["export", "work@1", "-o", file.to_str().unwrap()]).unwrap();
    assert_eq!((o.json["snapshot"].as_str(), o.json["version"].as_u64()), (Some("work"), Some(1)));
    let again = env.run(&["export", "work", "-o", file.to_str().unwrap()]).unwrap_err();
    assert_eq!(exit_of(&again), Exit::Refused, "an existing file is never overwritten");

    // Another machine: nothing saved, nothing open, the code under /home/me.
    let other = Env::new();
    let o = other.run(&["import", file.to_str().unwrap(), "--map", "/w=/home/me/w"]).unwrap();
    assert_eq!((o.json["snapshot"].as_str(), o.json["version"].as_u64()), (Some("work"), Some(1)), "{}", o.json);
    assert_eq!(o.json["message"], "imported work@1");
    let v = &other.run(&["show", "work", "--json"]).unwrap().json["version"];
    assert_eq!(v["source"], serde_json::json!({"kind": "import", "from": "work@1"}));
    assert_eq!(v["workspaces"][0]["cwd"], "/home/me/w/a");
    assert_eq!(v["workspaces"][0]["tabs"][0]["sessionId"], A, "the session is kept, not its history");
    assert!(v["workspaces"].as_array().unwrap().iter().all(|w| w.get("cmuxWorkspaceId").is_none()), "{v}");
    assert!(!other.dir.path().join("claude").exists(), "no transcript is copied");
    // The same file again changes nothing.
    assert_eq!(
        other.run(&["import", file.to_str().unwrap(), "--map", "/w=/home/me/w"]).unwrap().exit,
        Exit::NothingToDo
    );
    // Under another name, from stdin.
    let text = std::fs::read_to_string(&file).unwrap();
    let o = other.run_with_input(&["import", "-", "--as", "copy"], &text).unwrap();
    assert_eq!(o.json["snapshot"], "copy");
}

#[test]
fn import_refuses_what_it_cannot_take() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let text = serde_json::to_string(&exported(&env, "work")).unwrap();
    let bad_map = env.run_with_input(&["import", "-", "--as", "x", "--map", "w=/home"], &text).unwrap_err();
    assert!(format!("{bad_map:#}").contains("two absolute folders"), "{bad_map:#}");
    let old = text.replacen("\"sessionsnap\":2", "\"sessionsnap\":1", 1);
    assert_ne!(old, text);
    assert_eq!(exit_of(&env.run_with_input(&["import", "-", "--as", "x"], &old).unwrap_err()), Exit::UnknownFormat);
    let e = env.run_with_input(&["import", "-", "--as", "x"], "{").unwrap_err();
    assert!(format!("{e:#}").contains("not an exported snapshot"), "{e:#}");
    assert_eq!(exit_of(&env.run_with_input(&["import", "-", "--as", "../x"], &text).unwrap_err()), Exit::Error);
    env.run(&["schedule", "add", "auto", "--interval", "1h"]).unwrap();
    let e = env.run_with_input(&["import", "-", "--as", "auto"], &text).unwrap_err();
    assert_eq!(exit_of(&e), Exit::Refused, "an automatic snapshot is saved only by its schedule");
    assert_eq!(exit_of(&env.run(&["import", "/nope/x.json"]).unwrap_err()), Exit::NotFound);
}

/// An imported file is as untrusted as a hand-edited one: restoring it never sends a control
/// character, an unknown flag or a bad session ID to the terminal.
#[test]
fn a_hostile_import_never_reaches_the_terminal() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let mut v = exported(&env, "work");
    v["workspaces"] = serde_json::json!([{"name": "Evil", "cwd": "/w/e", "cmuxWorkspaceId": "WA", "tabs": [
        {"type": "agent", "title": "a", "cwd": "/w/e", "agent": "claude", "sessionId": A, "args": ["--dangerously-skip-permissions", "\u{3}touch /tmp/pwned\r"]},
        {"type": "agent", "title": "b", "cwd": "/w/e", "agent": "opencode", "sessionId": "s1\nrm -rf ~"},
        {"type": "agent", "title": "c", "cwd": "/w/e\u{1b}[2J", "agent": "opencode", "sessionId": "s2"},
        {"type": "terminal", "title": "d", "cwd": "/w/e", "command": "make\rrm -rf ~"},
        {"type": "terminal", "title": "e", "cwd": "/w/e", "command": "echo $(id) `id` \\ ; rm -rf ~"}
    ]}]);
    let other = Env::new();
    other.run_with_input(&["import", "-", "--as", "evil", "--map", "/w=/x"], &v.to_string()).unwrap();
    // A terminal tab's command runs again, as saved: the plan people read shows it first.
    let plan = other.run(&["restore", "evil", "--plan"]).unwrap().text;
    assert!(plan.contains("+ e (runs: echo $(id) `id` \\\\ ; rm -rf ~)"), "{plan}");
    assert!(plan.contains("+ d\n") && plan.contains("d: its command has control characters"), "{plan}");
    let o = other.run(&["restore", "evil", "--yes"]).unwrap();
    assert_eq!(o.json["failed"], 0, "{}", o.json);
    let launched = other.fake.launched.borrow().clone();
    assert_eq!(launched.len(), 5, "{launched:?}");
    for t in &launched {
        assert!(!t.cwd.chars().any(|c| c.is_control()), "{t:?}");
        let command = t.command.as_deref().unwrap_or("");
        assert!(!command.chars().any(|c| c.is_control()), "{t:?}");
        assert!(!command.contains("pwned"), "{t:?}");
    }
    assert_eq!(launched[4].command.as_deref(), Some("echo $(id) `id` \\ ; rm -rf ~"));
    // A flag with a control character leaves its session out (a shell), as does a bad ID or folder.
    assert!(launched[..4].iter().all(|t| t.command.is_none()), "{launched:?}");
}
