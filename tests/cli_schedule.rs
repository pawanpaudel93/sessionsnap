mod common;

use common::setup;
use sessionsnap::error::{Exit, exit_of};
use sessionsnap::testing::FakeScheduler;

#[test]
fn add_schedules_and_saves_once_now() {
    let env = setup();
    let o = env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    assert_eq!(o.json["schedule"]["interval"], "10m");
    assert_eq!(o.json["schedule"]["intervalSeconds"], 600);
    assert_eq!(o.json["firstSave"]["version"], 1);
    assert_eq!(FakeScheduler::log(env.dir.path()), ["install work 600"]);
    let v = env.run(&["show", "work", "--json"]).unwrap();
    assert_eq!(v.json["version"]["source"]["kind"], "scheduled");
    let listed = env.run(&["schedule", "ls"]).unwrap();
    assert_eq!(listed.json["schedules"][0]["name"], "work");
    assert_eq!(listed.json["schedules"][0]["latestVersion"], 1);
    assert_eq!(listed.json["schedules"][0]["lastRun"]["result"], "saved");
}

#[test]
fn several_schedules_run_apart_and_only_save_changes() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    env.run(&["schedule", "add", "daily", "--interval", "1d"]).unwrap();
    assert_eq!(env.run(&["schedule", "run", "work"]).unwrap().exit, Exit::NothingToDo, "nothing changed");
    env.fake.state.borrow_mut().workspaces.pop();
    assert_eq!(env.run(&["schedule", "run", "work"]).unwrap().json["version"], 2);
    let listed = env.run(&["schedule", "list"]).unwrap().json;
    let rows = listed["schedules"].as_array().unwrap();
    assert_eq!(rows.iter().map(|r| r["name"].as_str().unwrap()).collect::<Vec<_>>(), ["daily", "work"]);
    assert_eq!(rows[0]["latestVersion"], 1, "daily did not run again");
    assert_eq!(rows[1]["lastRun"]["result"], "saved");
}

#[test]
fn a_scheduled_save_skips_an_empty_or_stopped_cmux() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    let saved = env.fake.state.borrow().workspaces.clone();
    env.fake.state.borrow_mut().workspaces.clear();
    let o = env.run(&["schedule", "run", "work"]).unwrap();
    assert_eq!((o.exit, o.json["skipped"].clone()), (Exit::NothingToDo, true.into()));
    assert!(o.json["reason"].as_str().unwrap().contains("no workspaces"));
    env.fake.state.borrow_mut().workspaces = saved;
    env.fake.state.borrow_mut().live = false;
    env.fake.running.set(false);
    let o = env.run(&["schedule", "run", "work"]).unwrap();
    assert!(o.json["reason"].as_str().unwrap().contains("not running"));
    let listed = env.run(&["schedule", "list"]).unwrap().json;
    assert_eq!(listed["schedules"][0]["lastRun"]["result"], "skipped");
    assert_eq!(env.run(&["history", "work"]).unwrap().json["versions"].as_array().unwrap().len(), 1);
}

/// cmux's default socket mode answers only programs started inside it, so a launchd job finds
/// cmux running but can't connect: it saves from the state cmux writes every few seconds.
#[test]
fn a_scheduled_save_uses_the_state_file_when_cmux_runs_but_will_not_connect() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    env.fake.state.borrow_mut().workspaces.pop();
    env.fake.state.borrow_mut().live = false; // read from the state file
    let o = env.run(&["schedule", "run", "work"]).unwrap();
    assert_eq!(o.json["version"], 2, "{}", o.json);
    assert!(o.json["warnings"].to_string().contains("didn't let sessionsnap connect"), "{}", o.json);
    assert_eq!(env.run(&["schedule", "list"]).unwrap().json["schedules"][0]["lastRun"]["result"], "saved");
}

#[test]
fn automatic_and_manual_snapshots_stay_apart() {
    let env = setup();
    env.run(&["save", "mine"]).unwrap();
    // A name with versions saved by hand can't become automatic.
    let e = env.run(&["schedule", "add", "Mine", "--interval", "10m"]).unwrap_err();
    assert_eq!(exit_of(&e), Exit::Refused);
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    // Nor can a person save into an automatic one, or add it twice.
    assert_eq!(exit_of(&env.run(&["save", "Work"]).unwrap_err()), Exit::Refused);
    assert_eq!(exit_of(&env.run(&["save", "work", "--from", "mine"]).unwrap_err()), Exit::Refused);
    assert_eq!(exit_of(&env.run(&["schedule", "add", "work", "--interval", "1h"]).unwrap_err()), Exit::Refused);
    // A version copied out of it is a manual one.
    env.run(&["save", "keep", "--from", "work@1"]).unwrap();
    let list = env.run(&["list"]).unwrap();
    assert_eq!(list.text.lines().collect::<Vec<_>>()[0], "Manual");
    assert!(list.text.contains("Automatic\n  work"), "{}", list.text);
    assert!(list.text.contains("every 10m"), "{}", list.text);
    let rows = list.json["snapshots"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|r| r.get("schedule").is_some()).count(), 1);
}

#[test]
fn edit_changes_the_interval_and_remove_keeps_the_versions() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    let o = env.run(&["schedule", "edit", "WORK", "--interval", "2h"]).unwrap();
    assert_eq!((o.json["snapshot"].clone(), o.json["schedule"]["interval"].clone()), ("work".into(), "2h".into()));
    env.run(&["schedule", "rm", "work"]).unwrap();
    assert_eq!(FakeScheduler::log(env.dir.path()), ["install work 600", "install work 7200", "remove work"]);
    assert_eq!(env.run(&["history", "work"]).unwrap().json["versions"].as_array().unwrap().len(), 1);
    assert!(env.run(&["schedule", "list"]).unwrap().json["schedules"].as_array().unwrap().is_empty());
    assert!(!env.run(&["list"]).unwrap().text.contains("Automatic"));
    // It may be scheduled again: every version came from a schedule.
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    for missing in [
        &["schedule", "edit", "nope", "--interval", "1h"][..],
        &["schedule", "rm", "nope"],
        &["schedule", "run", "nope"],
    ] {
        assert_eq!(exit_of(&env.run(missing).unwrap_err()), Exit::NotFound, "{missing:?}");
    }
}

#[test]
fn bad_intervals_and_names_are_refused_before_anything_is_scheduled() {
    let env = setup();
    for args in [
        &["schedule", "add", "work", "--interval", "10"][..],
        &["schedule", "add", "work", "--interval", "30s"],
        &["schedule", "add", "--interval", "10m", "--", "--bad"],
        &["schedule", "add", "a/b", "--interval", "10m"],
    ] {
        assert!(env.run(args).is_err(), "{args:?}");
    }
    assert!(FakeScheduler::log(env.dir.path()).is_empty());
    assert!(env.run(&["schedule", "list"]).unwrap().json["schedules"].as_array().unwrap().is_empty());
}

#[test]
fn a_schedule_the_os_refuses_is_not_recorded() {
    let env = setup();
    std::fs::write(env.dir.path().join("scheduler.fail"), "").unwrap();
    assert!(env.run(&["schedule", "add", "work", "--interval", "10m"]).is_err());
    assert!(env.run(&["schedule", "list"]).unwrap().json["schedules"].as_array().unwrap().is_empty());
    assert_eq!(exit_of(&env.run(&["show", "work"]).unwrap_err()), Exit::NotFound);
}

#[test]
fn rename_moves_a_snapshot_and_its_schedule() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    env.run(&["save", "mine"]).unwrap();
    let o = env.run(&["rename", "Work", "job"]).unwrap();
    assert_eq!((o.json["from"].clone(), o.json["to"].clone()), ("work".into(), "job".into()));
    assert_eq!(FakeScheduler::log(env.dir.path())[1..], ["remove work", "install job 600"]);
    assert_eq!(env.run(&["schedule", "list"]).unwrap().json["schedules"][0]["name"], "job");
    assert_eq!(env.run(&["show", "job@1", "--json"]).unwrap().json["version"]["snapshot"], "job");
    assert_eq!(env.run(&["schedule", "run", "job"]).unwrap().exit, Exit::NothingToDo);
    // A manual snapshot renames too; an existing name is refused.
    env.run(&["rename", "mine", "ours"]).unwrap();
    assert_eq!(exit_of(&env.run(&["rename", "ours", "job"]).unwrap_err()), Exit::Refused);
    assert_eq!(exit_of(&env.run(&["rename", "gone", "x"]).unwrap_err()), Exit::NotFound);
}

#[test]
fn delete_stops_the_schedule() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    env.run(&["delete", "work", "--yes"]).unwrap();
    assert_eq!(FakeScheduler::log(env.dir.path()), ["install work 600", "remove work"]);
    assert!(env.run(&["schedule", "list"]).unwrap().json["schedules"].as_array().unwrap().is_empty());
}

#[test]
fn stop_and_start_keep_it_automatic() {
    let env = setup();
    env.run(&["schedule", "add", "work", "--interval", "10m"]).unwrap();
    let o = env.run(&["schedule", "stop", "work"]).unwrap();
    assert_eq!(o.json["schedule"]["running"], false);
    assert_eq!(env.run(&["schedule", "stop", "work"]).unwrap().exit, Exit::NothingToDo);
    // Stopped: still automatic, still not saved by hand, its interval can change, it can run now.
    assert!(env.run(&["list"]).unwrap().text.contains("every 10m (stopped)"));
    assert_eq!(exit_of(&env.run(&["save", "work"]).unwrap_err()), Exit::Refused);
    env.run(&["schedule", "edit", "work", "--interval", "1h"]).unwrap();
    env.fake.state.borrow_mut().workspaces.pop();
    assert_eq!(env.run(&["schedule", "run", "work"]).unwrap().json["version"], 2);
    env.run(&["rename", "work", "job"]).unwrap();
    assert_eq!(FakeScheduler::log(env.dir.path()), ["install work 600", "remove work", "remove work"]);
    let o = env.run(&["schedule", "start", "job"]).unwrap();
    assert_eq!(
        (o.json["schedule"]["running"].clone(), o.json["schedule"]["interval"].clone()),
        (true.into(), "1h".into())
    );
    assert_eq!(FakeScheduler::log(env.dir.path()).last().unwrap(), "install job 3600");
    assert_eq!(env.run(&["schedule", "start", "job"]).unwrap().exit, Exit::NothingToDo);
    assert!(env.run(&["schedule", "list"]).unwrap().text.contains("running"));
}
