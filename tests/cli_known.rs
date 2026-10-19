//! The store's record of every saved session (`sessions.json`): a session put in a tab or a plan
//! without its own flags gets the flags it was last saved with, or stays left out.

mod common;

use common::{Env, setup};
use serde_json::Value;
use sessionsnap::adapters::store::Store;
use sessionsnap::domain::model::{Source, Tab, Workspace};
use sessionsnap::testing::{A, C, write_rollout, write_transcript};

const CODEX: &str = "01a12bb7-d379-7541-a81a-7895725dc6dd";
const PLAN: [&str; 2] = ["--permission-mode", "plan"];
const LOST: &str = "an unknown flag --frobnicate; resume it by hand";

/// Sets the flags session A (Alpha's first tab) runs with in cmux.
fn run_a_with(env: &Env, args: &[&str]) {
    let mut state = env.fake.state.borrow_mut();
    state.workspaces[0].tabs[0].agent.as_mut().unwrap().args = args.iter().map(|a| a.to_string()).collect();
}

/// Closes session A's tab in cmux.
fn close_a(env: &Env) {
    env.fake.state.borrow_mut().workspaces[0].tabs.remove(0);
}

/// The tab holding session `id` in the latest version of `name`.
fn tab_of(env: &Env, name: &str, id: &str) -> Value {
    let shown = env.run(&["show", name]).unwrap();
    let tabs: Vec<Value> = shown.json["version"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|w| w["tabs"].as_array().unwrap().clone())
        .collect();
    tabs.into_iter().find(|t| t["sessionId"] == id).unwrap_or_else(|| panic!("no tab with {id} in {name}"))
}

fn args(t: &Value) -> Vec<String> {
    t["args"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

/// `work` saved with session A left out (a flag that loses it), then A's tab closed.
fn left_out_in_work() -> Env {
    let env = setup();
    run_a_with(&env, &["--frobnicate"]);
    env.run(&["save", "work"]).unwrap();
    assert_eq!(tab_of(&env, "work", A)["leftOut"], LOST);
    close_a(&env);
    env
}

#[test]
fn removed_and_readded_keeps_its_saved_flags() {
    let env = setup();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    let o = env.run(&["tab", "work", "Alpha", "--add-session", A]).unwrap();
    assert_eq!(args(&tab_of(&env, "work", A)), PLAN);
    assert!(!o.json["warnings"].to_string().contains("launch flags unknown"), "{}", o.json);
}

#[test]
fn switching_to_another_agent_and_back_keeps_the_flags() {
    let env = setup();
    write_rollout(&env.codex_root(), "/w/x", CODEX, "codex work");
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    env.run(&["tab", "work", "Alpha#1", "--session", CODEX]).unwrap();
    assert!(args(&tab_of(&env, "work", CODEX)).is_empty());
    env.run(&["tab", "work", "Alpha#1", "--session", A]).unwrap();
    assert_eq!(args(&tab_of(&env, "work", A)), PLAN);
}

#[test]
fn another_snapshot_knows_a_left_out_session() {
    let env = left_out_in_work();
    env.run(&["save", "other"]).unwrap();
    env.run(&["tab", "other", "Beta", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "other", A)["leftOut"], LOST);
    assert!(args(&tab_of(&env, "other", A)).is_empty());
}

#[test]
fn pruning_forgets_no_session() {
    let env = left_out_in_work();
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    env.run(&["prune", "work", "--keep", "1", "--yes"]).unwrap();
    env.run(&["tab", "work", "Alpha", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "work", A)["leftOut"], LOST);
}

#[test]
fn deleting_and_recreating_forgets_no_session() {
    let env = left_out_in_work();
    env.run(&["delete", "work", "--yes"]).unwrap();
    env.run(&["save", "work"]).unwrap();
    env.run(&["tab", "work", "Alpha", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "work", A)["leftOut"], LOST);
}

/// A workspace copied from a snapshot whose tab has no flags gets the session's known ones.
#[test]
fn copied_workspaces_get_known_flags() {
    let env = setup();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    // Written without sessionsnap's record (e.g. by hand): A with no flags.
    let store = Store::new(env.dir.path().join("store"));
    let tab = Tab::Agent {
        title: "a".into(),
        cwd: "/w/a".into(),
        agent: "claude".into(),
        session_id: A.into(),
        args: vec![],
        left_out: None,
    };
    let ws = Workspace {
        name: "Old".into(),
        color: None,
        cwd: "/w/a".into(),
        git_branch: None,
        cmux_workspace_id: None,
        tabs: vec![tab],
    };
    store.save_new("src", "m", Source::default(), vec![ws]).unwrap();
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    env.run(&["ws", "work", "--from", "src:Old"]).unwrap();
    assert_eq!(args(&tab_of(&env, "work", A)), PLAN);
}

/// Live flags win over saved ones, and a live launch that loses the session leaves it out.
#[test]
fn live_flags_win() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["tab", "work", "Alpha", "--add-session", A]).unwrap();
    assert_eq!(args(&tab_of(&env, "work", A)), PLAN);
}

#[test]
fn recent_restores_left_out_sessions_as_shells_and_warns_about_unknown_flags() {
    let env = left_out_in_work();
    write_transcript(&env.claude_root(), "/w/c", C, "cargo disk");
    let p = env.run(&["recent", "--plan"]).unwrap();
    let warnings = p.json["plan"]["warnings"].to_string();
    assert!(warnings.contains(LOST), "{warnings}");
    assert!(warnings.contains("launch flags unknown; resuming with defaults"), "{warnings}");
    let tabs = p.json["plan"]["actions"][0]["tabs"].as_array().unwrap().clone();
    let a = tabs.iter().find(|t| t["title"] == "login fix").unwrap();
    assert_eq!(a["type"], "terminal", "{a}");
}

/// The record is built from every snapshot on first use, so data saved before it counts.
#[test]
fn the_record_is_seeded_from_existing_snapshots() {
    let env = left_out_in_work();
    let index = env.dir.path().join("store/sessions.json");
    assert!(index.is_file());
    std::fs::remove_file(&index).unwrap();
    env.run(&["save", "other"]).unwrap();
    env.run(&["tab", "other", "Beta", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "other", A)["leftOut"], LOST);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&index).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
