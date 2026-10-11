//! Restoring: making a plan from a snapshot (or recovered sessions), keeping it for
//! `sessionsnap apply`, and applying it to the terminal.

use anyhow::Result;

use super::Ctx;
use crate::adapters::agents::claude::tab_command;
use crate::adapters::store::Store;
use crate::domain::model::Tab;
use crate::domain::model::{Workspace, parse_ref};
use crate::domain::plan::{Action, ApplyStep, Plan, PlanSource, Selection, build_actions, recovered_workspace, select};
use crate::error::{Exit, failure};
use crate::ports::{LaunchTab, OpenState, SessionHistory, SessionInfo, Terminal};

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
        if !ctx.terminal.is_running() {
            return Err(failure(
                Exit::CmuxUnreachable,
                "cmux is not running; start it first (restore without --plan starts it)",
            ));
        }
    } else {
        ctx.terminal.ensure_running()?;
    }
    Ok(())
}

/// Makes a plan against what is open now and, unless it is empty, keeps it for `apply`.
pub fn prepare(ctx: &Ctx, source: PlanSource, plan_only: bool) -> Result<Plan> {
    require_terminal(ctx, plan_only)?;
    let open = ctx.terminal.read_open_state()?;
    let plan = make_plan(source, &ctx.store, ctx.agents.history(), &open)?;
    if !plan.is_empty() {
        ctx.store.save_plan(&plan)?;
    }
    Ok(plan)
}

/// A kept plan, with the terminal started to apply it.
pub fn load_for_apply(ctx: &Ctx, plan_id: &str) -> Result<Plan> {
    let plan = ctx.store.load_plan(plan_id)?;
    ctx.terminal.ensure_running()?;
    Ok(plan)
}

/// Applies `plan` to the terminal; one step per workspace created or tab added.
pub fn run_plan(ctx: &Ctx, plan: &Plan) -> Result<Vec<ApplyStep>> {
    apply_plan(plan, &ctx.store, ctx.agents.history(), ctx.terminal.as_ref())
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
                terminal.create_workspace(name, cwd, &tabs.iter().map(launch_tab).collect::<Vec<_>>()).map(|_| ()),
            )),
            Action::AddTabs { workspace_ref, workspace_name, tabs } => {
                for tab in tabs {
                    steps.push(step(
                        format!("add {} to {workspace_name}", tab.title()),
                        terminal.add_tab(workspace_ref, &launch_tab(tab)),
                    ));
                }
            }
        }
    }
    store.remove_plan(&plan.plan_id);
    Ok(steps)
}

/// What the terminal opens for a saved tab: the agent's resume command, or the terminal's command.
pub fn launch_tab(tab: &Tab) -> LaunchTab {
    LaunchTab { title: tab.title().to_string(), cwd: tab.cwd().to_string(), command: tab_command(tab) }
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
    use crate::adapters::agents::Agents;
    use crate::adapters::agents::claude::Claude;
    use crate::domain::model::Tab;
    use crate::error::exit_of;
    use crate::testing::{A, B, C, FakeTerminal, open_ws, write_transcript};

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
        let (store, agents) = (Store::new(d.path().join("s")), Agents::new(Claude::new(croot)));
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
        let fake = FakeTerminal::default();
        *fake.state.borrow_mut() = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("WA", "Alpha", "/w/a", &[("one", "/w/a", Some(A))])],
        };
        let source = PlanSource::Snapshot { snapshot: "work".into(), version: 1, selection: Selection::default() };
        let plan = make_plan(source.clone(), &store, agents.history(), &fake.read_open_state().unwrap()).unwrap();
        assert_eq!(plan.tab_count(), 3);
        store.save_plan(&plan).unwrap();
        assert_eq!(store.load_plan(&plan.plan_id).unwrap(), plan);
        let steps = apply_plan(&plan, &store, agents.history(), &fake).unwrap();
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert_eq!(*fake.log.borrow(), ["add workspace:WA two", "create Gamma 1", "create Delta 1"]);
        assert_eq!(exit_of(&apply_plan(&plan, &store, agents.history(), &fake).unwrap_err()), Exit::PlanStale);
        assert!(make_plan(source, &store, agents.history(), &fake.read_open_state().unwrap()).unwrap().is_empty());
    }
}
