mod common;

use sessionsnap::cli::run_json;
use sessionsnap::error::Exit;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn returns_exit_and_json() {
    let env = common::setup();
    let (exit, v) = run_json(&args(&["save", "work"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::Ok);
    assert_eq!(v["version"], 1);
    let (exit, v) = run_json(&args(&["show", "nope"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::NotFound);
    assert_eq!(v["error"]["code"], "not-found");
    assert_eq!(v["error"]["exit"], 7);
}

#[test]
fn reports_bad_arguments_as_json() {
    let env = common::setup();
    let (exit, v) = run_json(&args(&["bogus"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::Error);
    assert_eq!(v["error"]["code"], "error");
    assert!(v["error"]["message"].as_str().unwrap().contains("bogus"));
    // --json given by the caller is not added twice.
    let (exit, _) = run_json(&args(&["list", "--json"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::Ok);
}

#[test]
fn passes_yes_and_input() {
    let env = common::setup();
    run_json(&args(&["save", "work"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    let (exit, _) = run_json(&args(&["delete", "work"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::Refused);
    let ops = r#"[{"op":"tab-remove","tab":"Alpha#2"}]"#.to_string();
    let (exit, v) =
        run_json(&args(&["edit", "work", "--batch", "-", "--dry-run"]), |cli| Ok(env.ctx(true, cli.yes, Some(ops))));
    assert_eq!(exit, Exit::Ok);
    assert_eq!(v["dryRun"], true);
    let (exit, v) = run_json(&args(&["delete", "work", "--yes"]), |cli| Ok(env.ctx(true, cli.yes, None)));
    assert_eq!(exit, Exit::Ok);
    assert_eq!(v["deleted"], true);
}

#[test]
fn json_stays_effective_after_double_dash() {
    let env = common::setup();
    let mut json_seen = false;
    let (exit, v) = run_json(&args(&["show", "--", "nope"]), |cli| {
        json_seen = cli.json;
        Ok(env.ctx(true, cli.yes, None))
    });
    assert!(json_seen, "--json must not be swallowed as a positional after --");
    assert_eq!(exit, Exit::NotFound);
    assert_eq!(v["error"]["code"], "not-found");
}
