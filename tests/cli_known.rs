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
        left_out_unsure: false,
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

/// A record that can't be read is kept aside, not overwritten, and built again from the snapshots.
#[test]
fn a_corrupt_record_is_kept_aside_and_rebuilt() {
    let env = left_out_in_work();
    let index = env.dir.path().join("store/sessions.json");
    std::fs::write(&index, "{\"sessions\": {").unwrap();
    env.run(&["save", "other"]).unwrap();
    env.run(&["tab", "other", "Beta", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "other", A)["leftOut"], LOST);
    let aside: Vec<String> = std::fs::read_dir(env.dir.path().join("store"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("sessions.json.bad-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
}

/// A record that can't be updated is never removed: what only it knows (a left-out session whose
/// versions are gone) survives, and the next read adds what the snapshots tell.
#[test]
fn a_failed_update_keeps_the_record() {
    let env = left_out_in_work();
    let store = env.dir.path().join("store");
    // Only the record remembers A now.
    env.run(&["delete", "work", "--yes"]).unwrap();
    // The record's temporary file can't be written: the update fails.
    let blocker = store.join(format!("sessions.tmp-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    let o = env.run(&["save", "other"]).unwrap();
    assert!(o.json["warnings"].to_string().contains("could not update the record"), "{}", o.json);
    assert!(store.join("sessions.json").is_file());
    assert!(store.join("sessions.dirty").is_file());
    std::fs::remove_dir(&blocker).unwrap();
    env.run(&["tab", "other", "Beta", "--add-session", A]).unwrap();
    assert_eq!(tab_of(&env, "other", A)["leftOut"], LOST);
    assert!(!store.join("sessions.dirty").exists());
}

/// The plan's tab for session A's tab ("login fix"), as JSON.
fn planned_a(plan: &Value) -> Value {
    let tabs: Vec<Value> = plan["plan"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["tabs"].as_array().unwrap().clone())
        .collect();
    tabs.into_iter().find(|t| t["title"] == "login fix").unwrap_or_else(|| panic!("A is not in the plan: {plan}"))
}

/// `old` saved with A running without flags, then `work` with A in plan mode, then A closed.
fn plan_mode_after_old() -> Env {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    env
}

const NEWEST: &str = "using the session's newest known flags";

/// True when the plan resumes session A, and else whether it gives A's left-out reason.
fn resumes_a(plan: &Value) -> (bool, bool) {
    let actions = plan["plan"]["actions"].to_string();
    let reason = [&plan["plan"]["warnings"], &plan["plan"]["skipped"]].iter().any(|v| v.to_string().contains(LOST));
    (actions.contains(A), reason)
}

/// Editing a snapshot that still holds an older launch of A never replaces what the record knows.
#[test]
fn editing_an_older_snapshot_keeps_the_newest_flags() {
    let env = plan_mode_after_old();
    env.run(&["ws", "old", "Beta", "--rename", "Gamma"]).unwrap();
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
    env.run(&["tab", "work", "Alpha#1", "--remove"]).unwrap();
    env.run(&["tab", "work", "Alpha", "--add-session", A]).unwrap();
    assert_eq!(args(&tab_of(&env, "work", A)), PLAN);
}

#[test]
fn a_copy_keeps_the_newest_flags() {
    let env = plan_mode_after_old();
    env.run(&["save", "copy", "--from", "old"]).unwrap();
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
}

/// A save of some workspaces keeps the others as they were; those were not seen now.
#[test]
fn a_partial_save_keeps_the_newest_flags() {
    let env = plan_mode_after_old();
    // Beta changed, so the save writes a version: Beta as captured, Alpha (old A) kept.
    env.fake.state.borrow_mut().workspaces[1].cwd = "/w/b2".into();
    let o = env.run(&["save", "old", "-w", "Beta"]).unwrap();
    assert_eq!(o.json["version"], 2, "{}", o.json);
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
}

/// Restoring another snapshot that holds an older launch resumes A with its newest flags.
#[test]
fn restoring_another_snapshot_uses_the_newest_flags() {
    let env = plan_mode_after_old();
    let p = env.run(&["restore", "old", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
    assert!(p["plan"]["warnings"].to_string().contains(NEWEST), "{p}");
    // Apply rebuilds the same plan.
    env.run(&["apply", p["plan"]["planId"].as_str().unwrap()]).unwrap();
    let launched = env.fake.launched.borrow();
    let a = launched.iter().find_map(|t| t.command.clone().filter(|c| c.contains(A))).unwrap();
    assert!(a.contains("--permission-mode") && a.contains("plan"), "{a}");
}

/// An old version of the same snapshot, from before A was left out, doesn't resume it.
#[test]
fn restoring_an_old_version_keeps_a_left_out_session_out() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    run_a_with(&env, &["--frobnicate"]);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    let p = env.run(&["restore", "work@1", "--plan"]).unwrap().json;
    assert_eq!(resumes_a(&p), (false, true), "{p}");
    // In a new workspace it opens as a plain shell.
    env.fake.state.borrow_mut().workspaces.clear();
    let p = env.run(&["restore", "work@1", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
    assert_eq!(resumes_a(&p), (false, true), "{p}");
}

/// Left out once, left out for good: a later launch without the flag, saved, doesn't clear it,
/// and neither does any edit, copy or restore.
#[test]
fn left_out_sticks_everywhere() {
    let env = setup();
    run_a_with(&env, &["--frobnicate"]);
    env.run(&["save", "work"]).unwrap();
    run_a_with(&env, &[]);
    env.run(&["save", "later"]).unwrap();
    env.run(&["save", "copy", "--from", "later"]).unwrap();
    env.run(&["ws", "later", "Beta", "--rename", "Gamma"]).unwrap();
    close_a(&env);
    for snapshot in ["later", "copy", "work"] {
        let p = env.run(&["restore", snapshot, "--plan"]).unwrap().json;
        assert_eq!(resumes_a(&p), (false, true), "{snapshot}: {p}");
    }
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
    env.run(&["tab", "later", "Alpha", "--add-session", A]).unwrap();
    let shown = env.run(&["show", "later"]).unwrap().json;
    let tabs = shown["version"]["workspaces"][0]["tabs"].as_array().unwrap().clone();
    // The tab added now is left out (the one saved earlier stays as saved; restore leaves it out).
    assert_eq!(tabs.last().unwrap()["leftOut"], LOST, "{tabs:?}");
}

/// The launch command apply runs for session A, if any.
fn launched_a(env: &Env) -> Option<String> {
    env.fake.launched.borrow().iter().find_map(|t| t.command.clone().filter(|c| c.contains(A)))
}

/// An edit that reads A's live flags from cmux records them: restoring later uses them.
#[test]
fn an_edit_records_the_live_flags_it_reads() {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["tab", "old", "Alpha#1", "--remove"]).unwrap();
    env.run(&["tab", "old", "Alpha", "--add-session", A]).unwrap();
    close_a(&env);
    let p = env.run(&["restore", "old", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
    assert!(!p["plan"]["warnings"].to_string().contains(NEWEST), "{p}");
    let r = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&r)), PLAN, "{r}");
    env.run(&["apply", p["plan"]["planId"].as_str().unwrap()]).unwrap();
    assert!(launched_a(&env).unwrap().contains("plan"));
}

/// `ws --add-live` reads the live flags too.
#[test]
fn adding_a_live_workspace_records_its_flags() {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &PLAN);
    env.fake.state.borrow_mut().workspaces[0].name = "Gamma".into();
    env.run(&["ws", "old", "--add-live", "Gamma"]).unwrap();
    close_a(&env);
    env.fake.state.borrow_mut().workspaces.clear();
    let p = env.run(&["restore", "old", "--plan"]).unwrap().json;
    let tabs: Vec<Value> = p["plan"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["tabs"].as_array().unwrap().clone())
        .filter(|t| t["sessionId"] == A)
        .collect();
    assert!(!tabs.is_empty() && tabs.iter().all(|t| args(t) == PLAN), "{p}");
}

/// An edit that sees A's live launch left out leaves A out everywhere after.
#[test]
fn an_edit_that_sees_a_left_out_launch_records_it() {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &["--frobnicate"]);
    env.run(&["tab", "old", "Alpha#1", "--remove"]).unwrap();
    env.run(&["tab", "old", "Alpha", "--add-session", A]).unwrap();
    close_a(&env);
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
    env.fake.state.borrow_mut().workspaces.clear();
    env.run(&["save", "x", "--from", "old@1"]).unwrap();
    let p = env.run(&["restore", "x", "--plan"]).unwrap().json;
    assert_eq!(resumes_a(&p), (false, true), "{p}");
}

/// `old` v1 (A without flags), `work` (A in plan mode), A closed, then `old -w Beta` v2, which
/// keeps old's Alpha (A without flags) though it was not seen then.
fn partial_save_history() -> Env {
    let env = plan_mode_after_old();
    // Versions are stamped to the second: this one must look newer than `work`.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    env.fake.state.borrow_mut().workspaces[1].cwd = "/w/b2".into();
    env.run(&["save", "old", "-w", "Beta"]).unwrap();
    env
}

/// The record's entry for session A, as stored.
fn recorded_a(env: &Env) -> Value {
    let text = std::fs::read_to_string(env.dir.path().join("store/sessions.json")).unwrap();
    let record: Value = serde_json::from_str(&text).unwrap();
    record["sessions"][format!("claude:{A}")].clone()
}

/// Probe 5: a correct entry, then an unrelated failed update. Completing the record compares the
/// entry only with versions written since that failed update (and since the entry was seen), so
/// `old`'s older copy of A, saved without flags, doesn't make it undecided.
#[test]
fn an_unrelated_failed_update_leaves_a_correct_entry_alone() {
    let env = partial_save_history();
    let before = recorded_a(&env);
    assert_eq!(args(&before), PLAN);
    // Version times are to the second: the failed update comes later than `old` v2.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let store = env.dir.path().join("store");
    let blocker = store.join(format!("sessions.tmp-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    env.fake.state.borrow_mut().workspaces[1].cwd = "/w/b3".into();
    env.run(&["save", "third"]).unwrap();
    std::fs::remove_dir(&blocker).unwrap();
    assert!(store.join("sessions.dirty").is_file());
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
    assert_eq!(recorded_a(&env), before);
    let p = env.run(&["restore", "work", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
}

/// The update that failed held the newer, stricter launch: the looser entry doesn't win.
#[test]
fn a_missed_stricter_launch_is_not_lost() {
    let env = setup();
    env.run(&["save", "work"]).unwrap();
    assert!(args(&recorded_a(&env)).is_empty());
    run_a_with(&env, &PLAN);
    let store = env.dir.path().join("store");
    let blocker = store.join(format!("sessions.tmp-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    env.run(&["save", "work"]).unwrap();
    std::fs::remove_dir(&blocker).unwrap();
    close_a(&env);
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
}

/// Rebuilt from the snapshots, a session saved with different restrictions in different versions
/// is left out: which launch is newer can't be told from them.
#[test]
fn a_rebuilt_record_leaves_out_sessions_with_differing_restrictions() {
    let env = partial_save_history();
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
    assert!(p["plan"]["warnings"].to_string().contains("its flags differ between saved versions"), "{p}");
    let p = env.run(&["restore", "work", "--plan"]).unwrap().json;
    assert!(!p["plan"]["actions"].to_string().contains(A), "{p}");
}

/// Versions that agree on a session's restrictions give it those flags when the record is rebuilt.
#[test]
fn a_rebuilt_record_keeps_agreeing_flags() {
    let env = setup();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    env.run(&["save", "copy", "--from", "work"]).unwrap();
    close_a(&env);
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
}

/// Probe 8: an older, looser launch (`--dangerously-skip-permissions`) never wins a rebuild.
#[test]
fn a_rebuild_never_picks_a_looser_launch() {
    let env = setup();
    run_a_with(&env, &["--dangerously-skip-permissions"]);
    env.run(&["save", "aaa"]).unwrap();
    run_a_with(&env, &[]);
    env.run(&["save", "zzz"]).unwrap();
    close_a(&env);
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
    let p = env.run(&["restore", "zzz", "--plan"]).unwrap().json;
    assert!(!p["plan"]["actions"].to_string().contains("dangerously"), "{p}");
}

/// Probe 9: a session a rebuild couldn't decide is decided by the next live reading.
#[test]
fn a_live_save_decides_an_undecided_session() {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    // Any plan reads the record, rebuilding it.
    env.run(&["restore", "old", "--plan"]).unwrap();
    assert_eq!(recorded_a(&env)["leftOut"], "its flags differ between saved versions; resume it by hand");
    // A still runs in plan mode: a live save sees it.
    env.run(&["save", "work2"]).unwrap();
    assert_eq!(args(&recorded_a(&env)), PLAN);
    assert!(recorded_a(&env).get("leftOut").is_none(), "{}", recorded_a(&env));
    close_a(&env);
    let p = env.run(&["restore", "work2", "--plan"]).unwrap().json;
    assert_eq!(args(&planned_a(&p)), PLAN, "{p}");
}

/// A launch whose flags couldn't be kept stays left out, whatever later live saves see.
#[test]
fn a_lost_launch_stays_lost_after_a_live_save() {
    let env = setup();
    run_a_with(&env, &["--frobnicate"]);
    env.run(&["save", "work"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["save", "later"]).unwrap();
    assert_eq!(recorded_a(&env)["leftOut"], LOST);
}

/// A live launch whose flags hold a control character is recorded as left out, so the record's
/// older flags are never used for it.
#[test]
fn a_launch_with_control_characters_is_recorded_as_left_out() {
    let env = setup();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    run_a_with(&env, &["--model", "a\u{1b}b"]);
    env.run(&["save", "later"]).unwrap();
    assert_eq!(recorded_a(&env)["leftOut"], "its launch flags have control characters; resume it by hand");
    close_a(&env);
    let p = env.run(&["recent", "--plan"]).unwrap().json;
    assert_eq!(planned_a(&p)["type"], "terminal", "{p}");
}

/// Puts session A back in Alpha's first tab, running with `args`.
fn reopen_a(env: &Env, args: &[&str]) {
    let mut state = env.fake.state.borrow_mut();
    let mut ws = sessionsnap::testing::open_ws("WA", "Alpha", "/w/a", &[("t1", "/w/a", Some(A))]);
    ws.tabs[0].agent.as_mut().unwrap().args = args.iter().map(|a| a.to_string()).collect();
    state.workspaces[0].tabs.insert(0, ws.tabs.remove(0));
}

/// Probe 10: an edit that copies an undecided state into a tab keeps it undecided, so a later
/// rebuild doesn't take that tab for a lost launch, and a live save still decides the session.
#[test]
fn an_undecided_tab_never_becomes_a_lost_launch() {
    let env = setup();
    env.run(&["save", "old"]).unwrap();
    run_a_with(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    close_a(&env);
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    env.run(&["restore", "old", "--plan"]).unwrap();
    assert_eq!(recorded_a(&env)["unsure"], true);
    env.run(&["tab", "old", "Beta", "--add-session", A]).unwrap();
    let added = tab_of_last(&env, "old");
    assert_eq!(added["leftOutUnsure"], true, "{added}");
    // Readings come later than the versions (times are to the second).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    reopen_a(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    assert_eq!(args(&recorded_a(&env)), PLAN);
    close_a(&env);
    std::fs::remove_file(env.dir.path().join("store/sessions.json")).unwrap();
    env.run(&["restore", "old", "--plan"]).unwrap();
    assert_eq!(recorded_a(&env)["unsure"], true, "{}", recorded_a(&env));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    reopen_a(&env, &PLAN);
    env.run(&["save", "work"]).unwrap();
    assert_eq!(args(&recorded_a(&env)), PLAN, "{}", recorded_a(&env));
    // The decided record also overrides the tab's undecided state when `old` is restored.
    close_a(&env);
    env.fake.state.borrow_mut().workspaces.clear();
    let p = env.run(&["restore", "old", "--plan"]).unwrap().json;
    let tabs: Vec<Value> = p["plan"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["tabs"].as_array().unwrap().clone())
        .filter(|t| t["sessionId"] == A)
        .collect();
    assert!(!tabs.is_empty() && tabs.iter().all(|t| args(t) == PLAN), "{p}");
}

/// The last tab of the last workspace of `name`'s latest version.
fn tab_of_last(env: &Env, name: &str) -> Value {
    let shown = env.run(&["show", name]).unwrap().json;
    let ws = shown["version"]["workspaces"].as_array().unwrap().clone();
    ws.last().unwrap()["tabs"].as_array().unwrap().last().unwrap().clone()
}
