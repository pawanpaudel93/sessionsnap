mod common;

use ccsnap::error::{Exit, exit_of};
use common::setup;

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
