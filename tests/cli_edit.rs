mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::testing::{C, write_transcript};

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

#[test]
fn batch_reads_input_instead_of_stdin() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let o = env
        .run_with_input(&["edit", "work", "--batch", "-", "-m", "tidy"], r#"[{"op":"tab-remove","tab":"Alpha#2"}]"#)
        .unwrap();
    assert_eq!(o.json["version"], 2);
    assert_eq!(o.json["message"], "tidy");
}

#[test]
fn batch_dry_run_shows_result_without_saving() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let ops = r#"[{"op":"ws-rename","ws":"Beta","to":"Review"},{"op":"tab-remove","tab":"Alpha#2"}]"#;
    let o = env.run_with_input(&["edit", "work", "--batch", "-", "--dry-run"], ops).unwrap();
    assert_eq!(o.json["dryRun"], true);
    assert_eq!(o.json["baseVersion"], 1);
    assert_eq!(o.json["workspaces"][1]["name"], "Review");
    assert_eq!(o.json["workspaces"][0]["tabs"].as_array().unwrap().len(), 1);
    assert!(!o.json["changes"].as_array().unwrap().is_empty());
    assert!(o.text.contains("not saved"));
    let history = env.run(&["history", "work"]).unwrap();
    assert_eq!(history.json["versions"].as_array().unwrap().len(), 1);
}

#[test]
fn dry_run_reports_invalid_ops() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let err = env
        .run_with_input(&["edit", "work", "--batch", "-", "--dry-run"], r#"[{"op":"tab-remove","tab":"Nope#1"}]"#)
        .unwrap_err();
    assert_eq!(exit_of(&err), Exit::NotFound);
}

/// `--base` saves only onto the version the dry run previewed.
#[test]
fn base_must_be_the_latest_version() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    let ops = r#"[{"op":"tab-remove","tab":"Alpha#2"}]"#;
    let base = env.run_with_input(&["edit", "work", "--batch", "-", "--dry-run"], ops).unwrap().json["baseVersion"]
        .to_string();
    // The snapshot changes after the preview.
    env.run(&["ws", "work", "Beta", "--rename", "Review"]).unwrap();
    let err = env.run_with_input(&["edit", "work", "--batch", "-", "--base", &base], ops).unwrap_err();
    assert_eq!(exit_of(&err), Exit::PlanStale);
    assert!(err.to_string().contains("is now at v2, not v1"), "{err}");
    assert_eq!(env.run(&["history", "work"]).unwrap().json["versions"].as_array().unwrap().len(), 2);
    // Onto the version it previewed, it saves.
    let o = env.run_with_input(&["edit", "work", "--batch", "-", "--base", "2"], ops).unwrap();
    assert_eq!(o.json["version"], 3);
}

fn left_out_of(env: &common::Env, name: &str) -> Vec<serde_json::Value> {
    let shown = env.run(&["show", name]).unwrap();
    let ws = shown.json["version"]["workspaces"].as_array().unwrap().clone();
    ws.iter().flat_map(|w| w["tabs"].as_array().unwrap().clone()).map(|t| t["leftOut"].clone()).collect()
}

/// A session that was left out (its restricting flags could not be saved) never comes back
/// through an edit without that reason: re-adding it, or pointing a tab at it, keeps it left out.
#[test]
fn readding_a_left_out_session_keeps_it_left_out() {
    let env = setup();
    set_args_on(&env, &["--frobnicate"]);
    env.run(&["save", "work"]).unwrap();
    let reason = "an unknown flag --frobnicate; resume it by hand";
    assert_eq!(left_out_of(&env, "work")[0], reason);
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    // cmux no longer shows the flag: the snapshot's history still does.
    set_args_on(&env, &[]);
    env.run(&["tab", "work", "Alpha", "--add-session", sessionsnap::testing::A]).unwrap();
    assert_eq!(left_out_of(&env, "work")[1], reason);
    // Pointing the plain terminal at it, too.
    env.run(&["tab", "work", "Alpha#1", "--session", sessionsnap::testing::A]).unwrap();
    assert_eq!(left_out_of(&env, "work")[0], reason);
    // Restore opens a plain shell for it, never `claude --resume` without the flag.
    env.fake.state.borrow_mut().workspaces.clear();
    let plan = env.run(&["restore", "work", "--plan"]).unwrap().json;
    env.run(&["apply", plan["plan"]["planId"].as_str().unwrap()]).unwrap();
    let launched = env.fake.launched.borrow();
    assert!(launched.iter().any(|t| t.command.as_deref().is_some_and(|c| c.contains(sessionsnap::testing::B))));
    assert!(!launched.iter().any(|t| t.command.as_deref().is_some_and(|c| c.contains(sessionsnap::testing::A))));
    drop(launched);

    // Never left out in the snapshot, but open in cmux with flags that lose it.
    let env = setup();
    env.run(&["save", "other"]).unwrap();
    env.run(&["tab", "other", "Alpha#1", "--remove"]).unwrap();
    set_args_on(&env, &["--frobnicate"]);
    env.run(&["tab", "other", "Alpha", "--add-session", sessionsnap::testing::A]).unwrap();
    assert_eq!(left_out_of(&env, "other")[1], reason);
}

fn set_args_on(env: &common::Env, args: &[&str]) {
    let mut state = env.fake.state.borrow_mut();
    let agent = state.workspaces[0].tabs[0].agent.as_mut().unwrap();
    agent.args = args.iter().map(|a| a.to_string()).collect();
}
