mod common;

use ccsnap::error::{Exit, exit_of};
use ccsnap::testing::{B, C, open_ws, write_transcript};
use common::setup;

#[test]
fn restore_plans_then_applies_only_missing_tabs() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    // Beta is gone; Alpha is open but lost its Claude tab.
    env.fake.state.borrow_mut().workspaces = vec![open_ws("WA", "Alpha", "/w/a", &[("zsh", "/w/a", None)])];
    let planned = env.run(&["restore", "work", "--plan"]).unwrap();
    let actions = planned.json["plan"]["actions"].as_array().unwrap().clone();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["action"], "add-tabs");
    assert_eq!(actions[0]["workspaceRef"], "workspace:WA");
    assert_eq!(actions[1]["action"], "create-workspace");
    assert_eq!(actions[1]["name"], "Beta");
    let id = planned.json["plan"]["planId"].as_str().unwrap().to_string();
    let applied = env.run(&["apply", &id]).unwrap();
    assert_eq!(applied.json["failed"], 0);
    assert_eq!(*env.fake.log.borrow(), ["add workspace:WA login fix", "create Beta 1"]);
    assert_eq!(env.run(&["restore", "work", "--yes"]).unwrap().exit, Exit::NothingToDo);
}

#[test]
fn restore_without_yes_only_plans() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces.pop();
    let o = env.run(&["restore", "work"]).unwrap();
    assert_eq!(o.json["applied"], false);
    assert!(env.fake.log.borrow().is_empty());
    let id = o.json["plan"]["planId"].as_str().unwrap().to_string();
    env.fake.state.borrow_mut().workspaces.push(open_ws("WB", "Beta", "/w/b", &[("t2", "/w/b", Some(B))]));
    assert_eq!(exit_of(&env.run(&["apply", &id]).unwrap_err()), Exit::PlanStale);
}

#[test]
fn plan_only_needs_running_cmux() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.running.set(false);
    assert_eq!(exit_of(&env.run(&["restore", "work", "--plan"]).unwrap_err()), Exit::CmuxUnreachable);
}

#[test]
fn recent_lists_and_plans_sessions_not_open() {
    let env = setup();
    write_transcript(&env.claude_root(), "/w/c", C, "cargo disk");
    let o = env.run(&["recent"]).unwrap();
    let ids: Vec<&str> =
        o.json["sessions"].as_array().unwrap().iter().map(|s| s["sessionId"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![C]);
    let p = env.run(&["recent", "--plan"]).unwrap();
    assert_eq!(p.json["plan"]["actions"][0]["name"], "Recovered sessions");
}

#[test]
fn sessions_search_marks_open_ones() {
    let env = setup();
    let o = env.run(&["sessions", "--search", "review"]).unwrap();
    assert_eq!(o.json["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(o.json["sessions"][0]["open"], true);
}

#[test]
fn recent_plan_needs_running_cmux() {
    let env = setup();
    write_transcript(&env.claude_root(), "/w/c", C, "cargo disk");
    env.fake.running.set(false);
    assert_eq!(exit_of(&env.run(&["recent", "--plan"]).unwrap_err()), Exit::CmuxUnreachable);
    assert!(!env.fake.running.get());
}

#[test]
fn huge_time_windows_are_an_error_not_a_panic() {
    let env = setup();
    let err = env.run(&["sessions", "--days", "18446744073709551615"]).unwrap_err();
    assert_eq!(exit_of(&err), Exit::Error);
    let err = env.run(&["recent", "--hours", "18446744073709551615"]).unwrap_err();
    assert_eq!(exit_of(&err), Exit::Error);
}
