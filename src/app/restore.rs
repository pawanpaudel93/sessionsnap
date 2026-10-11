//! Restoring: making a plan from a snapshot (or recovered sessions), keeping it for
//! `sessionsnap apply`, and applying it to the terminal.

use std::time::Duration;

use anyhow::Result;

use super::Ctx;
use crate::adapters::store::Store;
use crate::adapters::store::fs::{read_json, write_json_atomic};
use crate::core::model::{Workspace, parse_ref};
use crate::core::plan::{Action, ApplyStep, Plan, PlanSource, Selection, build_actions, recovered_workspace, select};
use crate::error::{Exit, failure};
use crate::ports::{OpenState, SessionHistory, SessionInfo, Terminal};

/// Restoring a snapshot (`<name>[@v]`), limited to these workspaces and tabs (none = all).
pub fn snapshot_source(ctx: &Ctx, snapshot: &str, workspaces: Vec<String>, tabs: Vec<String>) -> Result<PlanSource> {
    let (name, version) = parse_ref(snapshot)?;
    let v = ctx.store.load(&name, version)?;
    Ok(PlanSource::Snapshot { snapshot: name, version: v.version, selection: Selection { workspaces, tabs } })
}

/// Reopening these sessions.
pub fn sessions_source(sessions: &[SessionInfo]) -> PlanSource {
    PlanSource::Sessions { session_ids: sessions.iter().map(|s| s.session_id.clone()).collect() }
}

/// Only planning needs the terminal running already; restoring starts it.
pub fn require_terminal(ctx: &Ctx, plan_only: bool) -> Result<()> {
    if plan_only {
        if !ctx.backend.is_running() {
            return Err(failure(
                Exit::CmuxUnreachable,
                "cmux is not running; start it first (restore without --plan starts it)",
            ));
        }
    } else {
        ctx.backend.ensure_running()?;
    }
    Ok(())
}

/// Makes a plan against what is open now and, unless it is empty, keeps it for `apply`.
pub fn prepare(ctx: &Ctx, source: PlanSource, plan_only: bool) -> Result<Plan> {
    require_terminal(ctx, plan_only)?;
    let open = ctx.backend.read_open_state()?;
    let plan = make_plan(source, &ctx.store, &ctx.claude, &open)?;
    if !plan.is_empty() {
        save_plan(&ctx.store, &plan)?;
    }
    Ok(plan)
}

/// A kept plan, with the terminal started to apply it.
pub fn load_for_apply(ctx: &Ctx, plan_id: &str) -> Result<Plan> {
    let plan = load_plan(&ctx.store, plan_id)?;
    ctx.backend.ensure_running()?;
    Ok(plan)
}

/// Applies `plan` to the terminal; one step per workspace created or tab added.
pub fn run_plan(ctx: &Ctx, plan: &Plan) -> Result<Vec<ApplyStep>> {
    apply_plan(plan, &ctx.store, &ctx.claude, ctx.backend.as_ref())
}

/// The workspaces a plan restores: a snapshot selection, or recovered sessions in one workspace.
pub fn source_workspaces(source: &PlanSource, store: &Store, history: &dyn SessionHistory) -> Result<Vec<Workspace>> {
    match source {
        PlanSource::Snapshot { snapshot, version, selection } => {
            select(store.load(snapshot, Some(*version))?.workspaces, selection)
        }
        PlanSource::Sessions { session_ids } => recovered_workspace(session_ids, history),
    }
}

pub fn make_plan(source: PlanSource, store: &Store, history: &dyn SessionHistory, open: &OpenState) -> Result<Plan> {
    let built = build_actions(&source_workspaces(&source, store, history)?, open, history);
    let now = chrono::Local::now();
    Ok(Plan {
        plan_id: format!("p{}", now.format("%Y%m%d%H%M%S%3f")),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        source,
        actions: built.actions,
        skipped: built.skipped,
        warnings: built.warnings,
    })
}

/// Saves a plan for `sessionsnap apply`, deleting plans older than an hour.
pub fn save_plan(store: &Store, plan: &Plan) -> Result<()> {
    let dir = store.plans_dir();
    std::fs::create_dir_all(&dir)?;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m.elapsed().unwrap_or_default() > Duration::from_secs(3600))
            .unwrap_or(false);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    write_json_atomic(&dir.join(format!("{}.json", plan.plan_id)), plan)
}

pub fn load_plan(store: &Store, id: &str) -> Result<Plan> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(failure(Exit::Error, format!("invalid plan id '{id}'")));
    }
    let path = store.plans_dir().join(format!("{id}.json"));
    if !path.is_file() {
        return Err(failure(Exit::NotFound, format!("plan {id} not found (plans expire after an hour)")));
    }
    read_json(&path)
}

/// Applies a plan if it still matches what is open; continues past failed steps.
pub fn apply_plan(
    plan: &Plan,
    store: &Store,
    history: &dyn SessionHistory,
    terminal: &dyn Terminal,
) -> Result<Vec<ApplyStep>> {
    let open = terminal.read_open_state()?;
    let rebuilt = build_actions(&source_workspaces(&plan.source, store, history)?, &open, history);
    if rebuilt.actions != plan.actions {
        return Err(failure(Exit::PlanStale, "what is open in cmux changed since this plan was made; make a new plan"));
    }
    let mut steps = vec![];
    for action in &plan.actions {
        match action {
            Action::CreateWorkspace { name, cwd, tabs } => steps.push(step(
                format!("create workspace {name} with {} tab(s)", tabs.len()),
                terminal.create_workspace(name, cwd, tabs).map(|_| ()),
            )),
            Action::AddTabs { workspace_ref, workspace_name, tabs } => {
                for tab in tabs {
                    steps.push(step(
                        format!("add {} to {workspace_name}", tab.title()),
                        terminal.add_tab(workspace_ref, tab),
                    ));
                }
            }
        }
    }
    let _ = std::fs::remove_file(store.plans_dir().join(format!("{}.json", plan.plan_id)));
    Ok(steps)
}

fn step(description: String, result: Result<()>) -> ApplyStep {
    match result {
        Ok(()) => ApplyStep { description, ok: true, error: None },
        Err(e) => ApplyStep { description, ok: false, error: Some(format!("{e:#}")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::agents::claude::Claude;
    use crate::core::model::Tab;
    use crate::error::exit_of;
    use crate::testing::{A, B, C, FakeBackend, open_ws, write_transcript};

    const D: &str = "44444444-4444-4444-8444-444444444444";

    fn claude_tab(title: &str, cwd: &str, id: &str) -> Tab {
        Tab::Claude { title: title.into(), cwd: cwd.into(), session_id: id.into() }
    }

    fn workspace(name: &str, cwd: &str, id: Option<&str>, tabs: Vec<Tab>) -> Workspace {
        Workspace {
            name: name.into(),
            color: None,
            cwd: cwd.into(),
            git_branch: None,
            cmux_workspace_id: id.map(str::to_string),
            tabs,
        }
    }

    #[test]
    fn apply_runs_actions_and_detects_stale_plans() {
        let d = tempfile::tempdir().unwrap();
        let croot = d.path().join("c");
        write_transcript(&croot, "/w/a", A, "one");
        write_transcript(&croot, "/w/a", B, "two");
        write_transcript(&croot, "/w/c", C, "three");
        let (store, claude) = (Store::new(d.path().join("s")), Claude::new(croot));
        let saved = vec![
            workspace(
                "Alpha",
                "/w/a",
                Some("WA"),
                vec![
                    claude_tab("one", "/w/a", A),
                    claude_tab("two", "/w/a", B),
                    Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None },
                ],
            ),
            workspace("Gamma", "/w/c", None, vec![claude_tab("three", "/w/c", C)]),
            workspace("Delta", "/w/d", None, vec![claude_tab("lost", "/w/d", D)]),
        ];
        store.save_new("work", "m", Default::default(), saved).unwrap();
        let fake = FakeBackend::default();
        *fake.state.borrow_mut() = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("WA", "Alpha", "/w/a", &[("one", "/w/a", Some(A))])],
        };
        let source = PlanSource::Snapshot { snapshot: "work".into(), version: 1, selection: Selection::default() };
        let plan = make_plan(source.clone(), &store, &claude, &fake.read_open_state().unwrap()).unwrap();
        assert_eq!(plan.tab_count(), 3);
        save_plan(&store, &plan).unwrap();
        assert_eq!(load_plan(&store, &plan.plan_id).unwrap(), plan);
        let steps = apply_plan(&plan, &store, &claude, &fake).unwrap();
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert_eq!(*fake.log.borrow(), ["add workspace:WA two", "create Gamma 1", "create Delta 1"]);
        assert_eq!(exit_of(&apply_plan(&plan, &store, &claude, &fake).unwrap_err()), Exit::PlanStale);
        assert!(make_plan(source, &store, &claude, &fake.read_open_state().unwrap()).unwrap().is_empty());
    }
}
