mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};

#[test]
fn save_creates_versions_and_skips_unchanged() {
    let env = setup();
    let o = env.run(&["save", "work"]).unwrap();
    assert_eq!(o.json["version"], 1);
    assert_eq!(o.json["tabs"], 3);
    assert_eq!(env.run(&["save", "work"]).unwrap().exit, Exit::NothingToDo);
    env.fake.state.borrow_mut().workspaces.pop();
    let o = env.run(&["save", "work"]).unwrap();
    assert_eq!(o.json["version"], 2);
    assert_eq!(o.json["message"], "-1 workspace, -1 tab");
}

#[test]
fn save_one_workspace_keeps_the_rest() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces[1].tabs.clear();
    let o = env.run(&["save", "work", "--workspace", "beta"]).unwrap();
    assert_eq!(o.json["workspaces"], 2);
    assert_eq!(o.json["tabs"], 2);
}

#[test]
fn list_show_history_diff() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces.pop();
    env.run(&["save", "work"]).unwrap();
    assert_eq!(env.run(&["list", "--json"]).unwrap().json["snapshots"][0]["latestVersion"], 2);
    assert!(env.run(&["show", "work@1"]).unwrap().text.contains("login fix"));
    assert_eq!(env.run(&["history", "work"]).unwrap().json["versions"].as_array().unwrap().len(), 2);
    assert_eq!(env.run(&["diff", "work@1", "work@2"]).unwrap().json["summary"], "-1 workspace, -1 tab");
}

#[test]
fn delete_and_prune_need_yes() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces.pop();
    env.run(&["save", "work"]).unwrap();
    assert_eq!(exit_of(&env.run(&["delete", "work"]).unwrap_err()), Exit::Refused);
    assert_eq!(env.run(&["prune", "work", "--keep", "1", "--yes"]).unwrap().json["deleted"], serde_json::json!([1]));
    env.run(&["delete", "work", "-y"]).unwrap();
    assert_eq!(exit_of(&env.run(&["show", "work"]).unwrap_err()), Exit::NotFound);
}

#[test]
fn copy_from_another_snapshot() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let o = env.run(&["save", "backup", "--from", "work@1"]).unwrap();
    assert_eq!(o.json["tabs"], 3);
}

#[test]
fn full_save_keeps_colors_and_terminal_commands() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.run(&["ws", "work", "Alpha", "--color", "blue"]).unwrap();
    let o = env.run(&["save", "work"]).unwrap();
    assert_eq!(o.exit, Exit::NothingToDo);
    let shown = env.run(&["show", "work", "--json"]).unwrap();
    assert_eq!(shown.json["version"]["workspaces"][0]["color"], "blue");
}

#[test]
fn open_lists_live_workspaces() {
    let env = setup();
    let o = env.run(&["open"]).unwrap();
    assert_eq!(o.json["live"], true);
    assert_eq!(o.json["workspaces"][0]["name"], "Alpha");
    assert_eq!(o.json["workspaces"][0]["tabs"][0]["sessionId"], sessionsnap::testing::A);
    assert_eq!(o.json["workspaces"][0]["tabs"][1]["sessionId"], serde_json::Value::Null);
    assert!(o.text.contains("Beta (1 tab"), "{}", o.text);
}

/// Names ignore case on every command, also where the file system does not (Linux, case-sensitive
/// APFS): `save Main` after `save main` adds to "main", so it compares with and merges into it.
#[test]
fn names_ignore_case_on_every_command() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    assert_eq!(env.run(&["save", "Work"]).unwrap().exit, Exit::NothingToDo, "nothing changed");
    env.fake.state.borrow_mut().workspaces[1].tabs.clear();
    let o = env.run(&["save", "WORK", "--workspace", "beta"]).unwrap();
    assert_eq!((o.json["snapshot"].as_str(), o.json["version"].as_u64()), (Some("work"), Some(2)));
    assert_eq!(o.json["workspaces"], 2, "Alpha is kept: {}", o.json);
    assert_eq!(env.run(&["show", "Work"]).unwrap().json["version"]["workspaces"].as_array().unwrap().len(), 2);
    assert_eq!(env.run(&["history", "WORK"]).unwrap().json["versions"].as_array().unwrap().len(), 2);
    assert_eq!(env.run(&["list"]).unwrap().json["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(env.run(&["prune", "Work", "--keep", "1", "--yes"]).unwrap().json["deleted"], serde_json::json!([1]));
    env.run(&["delete", "WORK", "-y"]).unwrap();
    assert_eq!(exit_of(&env.run(&["show", "work"]).unwrap_err()), Exit::NotFound);
}
