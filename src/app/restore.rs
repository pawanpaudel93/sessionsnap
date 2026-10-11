//! Restoring: making a plan from a snapshot (or recovered sessions), keeping it for
//! `sessionsnap apply`, and applying it to the terminal.

use std::collections::BTreeSet;

use anyhow::Result;

use super::{Ctx, read_open};
use crate::adapters::agents::Agents;
use crate::adapters::home_dir;
use crate::adapters::store::Store;
use crate::domain::model::{SessionRef, Tab, Workspace, has_control, parse_ref};
use crate::domain::plan::{Action, ApplyStep, Plan, PlanSource, Selection, build_actions, recovered_workspace, select};
use crate::error::{Exit, failure};
use crate::ports::{AgentCatalog, LaunchTab, OpenState, SessionInfo, Terminal};

/// Restoring a snapshot (`<name>[@v]`), limited to these workspaces and tabs (none = all).
pub fn snapshot_source(ctx: &Ctx, snapshot: &str, workspaces: Vec<String>, tabs: Vec<String>) -> Result<PlanSource> {
    let (name, version) = parse_ref(snapshot)?;
    let v = ctx.store.load(&name, version)?;
    Ok(PlanSource::Snapshot { snapshot: name, version: v.version, selection: Selection { workspaces, tabs } })
}

/// Reopening these sessions.
pub fn sessions_source(sessions: &[SessionInfo]) -> PlanSource {
    PlanSource::Sessions { sessions: sessions.iter().map(|s| SessionRef::new(&s.agent, &s.session_id)).collect() }
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

/// Makes a plan against what is open now and, unless it is empty, keeps it for `apply`. The plan
/// also warns about agents whose program is not on PATH.
pub fn prepare(ctx: &Ctx, source: PlanSource, plan_only: bool) -> Result<Plan> {
    require_terminal(ctx, plan_only)?;
    let open = read_open(ctx.terminal.as_ref())?;
    let mut plan = make_plan(source, &ctx.store, &ctx.agents, &open)?;
    plan.warnings.extend(missing_programs(&plan, &ctx.agents));
    if !plan.is_empty() {
        ctx.store.save_plan(&plan)?;
    }
    Ok(plan)
}

/// One warning per agent the plan resumes whose program is not on PATH.
fn missing_programs(plan: &Plan, agents: &Agents) -> Vec<String> {
    let kinds: BTreeSet<&str> = plan
        .actions
        .iter()
        .flat_map(|a| match a {
            Action::CreateWorkspace { tabs, .. } | Action::AddTabs { tabs, .. } => tabs.iter(),
        })
        .filter_map(|t| match t {
            Tab::Agent { agent, .. } => Some(agent.as_str()),
            Tab::Terminal { .. } => None,
        })
        .collect();
    kinds
        .into_iter()
        .filter_map(Agents::spec)
        .filter(|spec| !agents.on_path(spec.program))
        .map(|spec| format!("{} (`{}`) is not on PATH; its tabs may not resume", spec.display, spec.program))
        .collect()
}

/// A kept plan, with the terminal started to apply it.
pub fn load_for_apply(ctx: &Ctx, plan_id: &str) -> Result<Plan> {
    let plan = ctx.store.load_plan(plan_id)?;
    ctx.terminal.ensure_running()?;
    Ok(plan)
}

/// Applies `plan` to the terminal; one step per workspace created or tab added.
pub fn run_plan(ctx: &Ctx, plan: &Plan) -> Result<Vec<ApplyStep>> {
    apply_plan(plan, &ctx.store, &ctx.agents, ctx.terminal.as_ref())
}

/// The workspaces a plan restores: a snapshot selection, or recovered sessions in one workspace.
pub fn source_workspaces(source: &PlanSource, store: &Store, agents: &dyn AgentCatalog) -> Result<Vec<Workspace>> {
    match source {
        PlanSource::Snapshot { snapshot, version, selection } => {
            select(store.load(snapshot, Some(*version))?.workspaces, selection)
        }
        PlanSource::Sessions { sessions } => recovered_workspace(sessions, agents),
    }
}

pub fn make_plan(source: PlanSource, store: &Store, agents: &dyn AgentCatalog, open: &OpenState) -> Result<Plan> {
    let built = build_actions(&source_workspaces(&source, store, agents)?, open, agents);
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
pub fn apply_plan(plan: &Plan, store: &Store, agents: &Agents, terminal: &dyn Terminal) -> Result<Vec<ApplyStep>> {
    let open = read_open(terminal)?;
    let rebuilt = build_actions(&source_workspaces(&plan.source, store, agents)?, &open, agents);
    if rebuilt.actions != plan.actions {
        return Err(failure(Exit::PlanStale, "what is open in cmux changed since this plan was made; make a new plan"));
    }
    let launch = |t: &Tab| launch_tab(agents, t);
    let mut steps = vec![];
    for action in &plan.actions {
        match action {
            Action::CreateWorkspace { name, cwd, tabs } => steps.push(step(
                format!("create workspace {name} with {} tab(s)", tabs.len()),
                terminal
                    .create_workspace(name, &safe_cwd(cwd), &tabs.iter().map(launch).collect::<Vec<_>>())
                    .map(|_| ()),
            )),
            Action::AddTabs { workspace_ref, workspace_name, tabs } => {
                for tab in tabs {
                    steps.push(step(
                        format!("add {} to {workspace_name}", tab.title()),
                        terminal.add_tab(workspace_ref, &launch(tab)),
                    ));
                }
            }
        }
    }
    store.remove_plan(&plan.plan_id);
    Ok(steps)
}

/// What the terminal opens for a saved tab: the agent's resume command (with the saved flags,
/// quoted for the shell), or the terminal tab's own command. An agent tab that cannot be resumed
/// and any command with a control character get no command (the plan has already turned those
/// tabs into shells, with a warning); a folder with one, or none, becomes the home folder.
pub fn launch_tab(agents: &Agents, tab: &Tab) -> LaunchTab {
    let command = match tab {
        Tab::Terminal { command, .. } => command.clone(),
        Tab::Agent { agent, session_id, args, .. } => agents.resume_command(agent, session_id, args),
    };
    LaunchTab { title: tab.title().to_string(), cwd: safe_cwd(tab.cwd()), command: command.filter(|c| !has_control(c)) }
}

/// `cwd`, or the home folder when it is empty or has a control character.
fn safe_cwd(cwd: &str) -> String {
    if cwd.is_empty() || has_control(cwd) {
        home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into())
    } else {
        cwd.to_string()
    }
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
    use crate::error::exit_of;
    use crate::testing::{A, B, C, FakeTerminal, agent_tab, open_ws, write_transcript};

    const D: &str = "44444444-4444-4444-8444-444444444444";

    fn claude_tab(title: &str, cwd: &str, id: &str) -> Tab {
        agent_tab("claude", title, cwd, id)
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
        let (store, agents) = (Store::new(d.path().join("s")), Agents::new(vec![Box::new(Claude::new(croot))], vec![]));
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
        let plan = make_plan(source.clone(), &store, &agents, &fake.read_open_state().unwrap()).unwrap();
        assert_eq!(plan.tab_count(), 3);
        store.save_plan(&plan).unwrap();
        assert_eq!(store.load_plan(&plan.plan_id).unwrap(), plan);
        let steps = apply_plan(&plan, &store, &agents, &fake).unwrap();
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert_eq!(*fake.log.borrow(), ["add workspace:WA two", "create Gamma 1", "create Delta 1"]);
        let commands: Vec<Option<String>> = fake.launched.borrow().iter().map(|t| t.command.clone()).collect();
        assert_eq!(commands, [Some(format!("claude --resume {B}")), Some(format!("claude --resume {C}")), None]);
        assert_eq!(exit_of(&apply_plan(&plan, &store, &agents, &fake).unwrap_err()), Exit::PlanStale);
        assert!(make_plan(source, &store, &agents, &fake.read_open_state().unwrap()).unwrap().is_empty());
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    use crate::testing::A;

    #[test]
    fn launch_tabs_resume_agents_with_saved_flags() {
        let agents = Agents::new(vec![], vec![]);
        let tab = |agent: &str, id: &str, args: &[&str]| Tab::Agent {
            title: "t".into(),
            cwd: "/w".into(),
            agent: agent.into(),
            session_id: id.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        };
        let cmd = |t: &Tab| launch_tab(&agents, t).command;
        assert_eq!(
            cmd(&tab("codex", A, &["-a", "on-request", "-m", "it's"])),
            Some(format!(r"codex resume {A} -a on-request -m 'it'\''s'"))
        );
        assert_eq!(
            cmd(&tab("claude", A, &["--resume", "old", "--model", "opus"])),
            Some(format!("claude --resume {A} --model opus"))
        );
        assert_eq!(cmd(&tab("cursor-agent", "c1", &[])), Some("cursor-agent --resume c1".into()));
        assert_eq!(cmd(&tab("claude", "x; rm -rf ~", &[])), None);
        assert_eq!(cmd(&tab("aider", "x", &[])), None);
        // Nothing with a control character ever reaches the terminal.
        assert_eq!(cmd(&tab("codex", A, &["\u{3}touch /tmp/pwned\r"])), None);
        assert_eq!(cmd(&tab("opencode", "s\n1", &[])), None);
        let bad = Tab::Terminal { title: "t".into(), cwd: "/w\r".into(), command: Some("make\nrm -rf ~".into()) };
        let launched = launch_tab(&agents, &bad);
        assert_eq!(launched.command, None);
        assert!(!launched.cwd.contains('\r') && !launched.cwd.is_empty());
        let term = Tab::Terminal { title: "t".into(), cwd: "/w".into(), command: Some("npm run dev".into()) };
        assert_eq!(cmd(&term).as_deref(), Some("npm run dev"));
    }
}
