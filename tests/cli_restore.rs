mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::testing::{B, C, open_ws, write_transcript};

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

/// A shell tab is not added to its open workspace by a full restore, but is when named with --tab.
#[test]
fn a_shell_tab_named_with_tab_is_added_to_its_open_workspace() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    // Alpha is open with its Claude tab; its zsh tab was closed. Beta is open too.
    env.fake.state.borrow_mut().workspaces.retain(|w| w.name != "Alpha");
    env.fake
        .state
        .borrow_mut()
        .workspaces
        .insert(0, open_ws("WA", "Alpha", "/w/a", &[("t1", "/w/a", Some(sessionsnap::testing::A))]));
    let full = env.run(&["restore", "work", "--plan"]).unwrap();
    assert_eq!(full.exit, Exit::NothingToDo, "{}", full.json);
    let picked = env.run(&["restore", "work", "--tab", "Alpha#2", "--plan"]).unwrap();
    let actions = picked.json["plan"]["actions"].as_array().unwrap().clone();
    assert_eq!(actions.len(), 1, "{}", picked.json);
    assert_eq!(actions[0]["action"], "add-tabs");
    assert_eq!(actions[0]["workspaceRef"], "workspace:WA");
    assert_eq!(actions[0]["tabs"][0]["title"], "zsh");
    let id = picked.json["plan"]["planId"].as_str().unwrap().to_string();
    assert_eq!(env.run(&["apply", &id]).unwrap().json["failed"], 0);
    assert_eq!(*env.fake.log.borrow(), ["add workspace:WA zsh"]);
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
    assert_eq!(exit_of(&env.run(&["restore", "work", "--plan"]).unwrap_err()), Exit::TerminalUnreachable);
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

/// Nothing recent to reopen: the same empty plan as `restore --plan`, exit 3.
#[test]
fn an_empty_recent_plan_has_the_shape_of_an_empty_restore_plan() {
    let env = setup();
    for args in [&["recent", "--plan"][..], &["recent", "--restore"][..]] {
        let o = env.run(args).unwrap();
        assert_eq!(o.exit, Exit::NothingToDo);
        assert_eq!(o.json["applied"], false);
        assert_eq!(o.json["plan"]["actions"], serde_json::json!([]), "{}", o.json);
    }
    assert_eq!(env.run(&["recent"]).unwrap().json["sessions"], serde_json::json!([]));
    assert!(env.fake.log.borrow().is_empty());
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
    assert_eq!(exit_of(&env.run(&["recent", "--plan"]).unwrap_err()), Exit::TerminalUnreachable);
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

#[test]
fn a_tab_whose_folder_is_gone_opens_as_a_shell_with_a_warning() {
    use clap::Parser;
    use sessionsnap::interfaces::cli::{Cli, run::dispatch};
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces.clear();
    // Beta's folder (/w/b) was a git worktree, removed since.
    let cli = Cli::try_parse_from(["sessionsnap", "restore", "work", "--yes"]).unwrap();
    let mut ctx = env.ctx(false, true, None);
    ctx.folder_exists = |path| !path.starts_with("/w/b");
    let o = dispatch(&ctx, cli.command).unwrap();
    let warnings = o.json["warnings"].to_string();
    assert!(warnings.contains("its folder /w/b is gone"), "{}", o.json);
    let launched = env.fake.launched.borrow();
    let beta = launched.iter().find(|t| t.title == "review").unwrap();
    assert_eq!(beta.command, None, "never resumed in another folder");
    assert_ne!(beta.cwd, "/w/b");
    let alpha = launched.iter().find(|t| t.title == "login fix").unwrap();
    assert!(alpha.command.as_deref().is_some_and(|c| c.contains("--resume")), "{alpha:?}");
}

#[test]
fn a_plan_applies_once() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    env.fake.state.borrow_mut().workspaces.clear();
    let planned = env.run(&["restore", "work", "--plan"]).unwrap();
    let id = planned.json["plan"]["planId"].as_str().unwrap().to_string();
    env.run(&["apply", &id]).unwrap();
    let again = env.run(&["apply", &id]).unwrap_err();
    assert!(matches!(exit_of(&again), Exit::NotFound | Exit::NothingToDo), "{again:#}");
}

#[test]
fn restore_with_no_name_takes_the_most_recently_saved_snapshot() {
    let env = setup();
    env.run(&["save", "older"]).unwrap();
    env.run(&["save", "newer"]).unwrap();
    // Saved a day earlier: times are to the second, so make the order plain.
    let dir = env.dir.path().join("store/snapshots/older");
    let file = std::fs::read_dir(&dir).unwrap().flatten().next().unwrap().path();
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    v["createdAt"] = "2000-01-01T00:00:00+00:00".into();
    std::fs::write(&file, v.to_string()).unwrap();
    env.fake.state.borrow_mut().workspaces.clear();
    let o = env.run(&["restore", "--plan"]).unwrap();
    assert_eq!(o.json["snapshot"], "newer", "{}", o.json);
    assert_eq!(o.json["plan"]["source"]["snapshot"], "newer");
    assert!(o.text.starts_with("The most recently saved snapshot: newer"), "{}", o.text);
}

#[test]
fn restore_with_no_name_and_no_snapshots_says_how_to_save_one() {
    let env = setup();
    let e = env.run(&["restore", "--plan"]).unwrap_err();
    assert_eq!(exit_of(&e), Exit::NotFound);
    assert!(e.to_string().contains("sessionsnap save"), "{e}");
}
