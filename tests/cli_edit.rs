mod common;

use ccsnap::error::{Exit, exit_of};
use ccsnap::testing::{C, write_transcript};
use common::setup;

#[test]
fn tab_and_ws_edits_make_versions() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let o = env.run(&["tab", "work", "Alpha#1", "--rename", "login"]).unwrap();
    assert_eq!(o.json["version"], 2);
    assert_eq!(o.json["message"], "1 tab renamed");
    let o = env.run(&["tab", "work", "Alpha:login", "--move", "Beta"]).unwrap();
    assert_eq!(o.json["message"], "1 tab moved");
    env.run(&["ws", "work", "Alpha", "--rename", "Main"]).unwrap();
    assert!(env.run(&["show", "work"]).unwrap().text.contains("Main"));
}

#[test]
fn folder_changes_allowed_unknown_sessions_refused() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    // claude --resume <id> finds a session from any folder (Claude Code >= 2.1.223).
    env.run(&["tab", "work", "Alpha#1", "--cwd", "/elsewhere"]).unwrap();
    let unknown = "44444444-4444-4444-8444-444444444444";
    let err = env.run(&["tab", "work", "Alpha#1", "--session", unknown]).unwrap_err();
    assert_eq!(exit_of(&err), Exit::NotFound);
}

#[test]
fn add_session_and_batch() {
    let env = setup();
    write_transcript(&env.claude_root(), "/w/c", C, "cargo disk");
    env.run(&["save", "work"]).unwrap();
    assert_eq!(env.run(&["tab", "work", "Beta", "--add-session", C]).unwrap().json["tabs"], 4);
    let batch = env.dir.path().join("ops.json");
    std::fs::write(&batch, r#"[{"op":"ws-rename","ws":"Beta","to":"Review"},{"op":"tab-remove","tab":"Alpha#2"}]"#)
        .unwrap();
    let o = env.run(&["edit", "work", "--batch", batch.to_str().unwrap()]).unwrap();
    assert_eq!(o.json["tabs"], 3);
}

#[test]
fn rejects_combined_positional_flags() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    assert!(env.run(&["tab", "work", "Alpha#1", "--remove", "--rename", "x"]).is_err());
    assert!(env.run(&["ws", "work"]).is_err());
}
