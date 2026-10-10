use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use super::{Command, Ctx, Outcome, TabArgs, WsArgs, human, pick};
use crate::edit::{EditCtx, Op};
use crate::capture::{capture, merge};
use crate::claude::SessionInfo;
use crate::diff;
use crate::edit;
use crate::error::{Exit, failure};
use crate::model::{Source, Version, Workspace, tab_count};
use crate::plan::{self, Plan, PlanSource, Selection};
use crate::store::{home_dir, parse_ref, validate_name};

pub fn dispatch(ctx: &Ctx, command: Command) -> Result<Outcome> {
    match command {
        Command::Save { name, workspaces, from, message, pick } => save(ctx, name, &workspaces, from, message, pick),
        Command::List => list(ctx),
        Command::Show { snapshot } => show(ctx, &snapshot),
        Command::History { name } => history(ctx, &name),
        Command::Diff { a, b } => diff_cmd(ctx, &a, &b),
        Command::Prune { name, keep } => prune(ctx, &name, keep),
        Command::Delete { name } => delete(ctx, &name),
        Command::Ws(args) => {
            let ops = ws_ops(&args)?;
            edit_snapshot(ctx, &args.name, ops, args.message)
        }
        Command::Tab(args) => {
            let ops = tab_ops(&args)?;
            edit_snapshot(ctx, &args.name, ops, args.message)
        }
        Command::Edit { name, batch, message } => {
            let text = if batch == "-" { std::io::read_to_string(std::io::stdin())? } else { std::fs::read_to_string(&batch)? };
            let ops: Vec<Op> = serde_json::from_str(&text).map_err(|e| failure(Exit::Error, format!("invalid batch: {e}")))?;
            edit_snapshot(ctx, &name, ops, message)
        }
        Command::Restore { snapshot, workspaces, tabs, plan, pick } => restore(ctx, snapshot, workspaces, tabs, plan, pick),
        Command::Apply { plan_id } => apply(ctx, &plan_id),
        Command::Sessions { search, cwd, days } => sessions(ctx, search.as_deref(), cwd.as_deref(), days),
        Command::Recent { hours, restore, plan, pick } => recent(ctx, hours, restore, plan, pick),
    }
}

fn save(ctx: &Ctx, name: Option<String>, only: &[String], from: Option<String>, message: Option<String>, pick_flag: bool) -> Result<Outcome> {
    let name = match name {
        Some(n) => n,
        None if pick_flag && ctx.interactive => pick::choose_save_name(&ctx.store)?,
        None => return Err(failure(Exit::Error, "give a snapshot name (or --pick in a terminal)")),
    };
    validate_name(&name)?;
    let previous = if ctx.store.exists(&name) { Some(ctx.store.load(&name, None)?) } else { None };
    let (workspaces, source) = match from {
        Some(from) => {
            let (src_name, version) = parse_ref(&from)?;
            let src = ctx.store.load(&src_name, version)?;
            let source = Source { kind: "copy".into(), cmux_state_version: None, from: Some(format!("{}@{}", src.snapshot, src.version)) };
            (src.workspaces, source)
        }
        None => {
            let open = ctx.backend.read_open_state()?;
            let captured = capture(&open, &ctx.claude, only)?;
            let workspaces = match &previous {
                Some(p) if !only.is_empty() => merge(&p.workspaces, captured),
                _ => captured,
            };
            (workspaces, Source { kind: "live".into(), cmux_state_version: open.state_version, from: None })
        }
    };
    commit(ctx, &name, previous.as_ref(), workspaces, source, message)
}

/// Writes a new version unless nothing changed (exit 3). Shared by save and edits.
pub(crate) fn commit(ctx: &Ctx, name: &str, previous: Option<&Version>, workspaces: Vec<Workspace>, source: Source, message: Option<String>) -> Result<Outcome> {
    if let Some(p) = previous
        && p.workspaces == workspaces
    {
        return Ok(Outcome::ok(
            json!({"snapshot": name, "version": p.version, "changed": false}),
            format!("{name}: no changes since v{}", p.version),
        )
        .with_exit(Exit::NothingToDo));
    }
    let changes = diff::diff(previous.map(|p| p.workspaces.as_slice()).unwrap_or(&[]), &workspaces);
    let message = message.unwrap_or_else(|| if previous.is_some() { diff::summary(&changes) } else { "initial save".to_string() });
    let v = ctx.store.save_new(name, &message, source, workspaces)?;
    let warnings = edit::warnings(&v.workspaces, &ctx.claude);
    Ok(Outcome::ok(
        json!({
            "snapshot": name, "version": v.version, "changed": true, "message": v.message, "changes": changes,
            "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces), "warnings": warnings,
        }),
        human::saved(&v, &warnings),
    ))
}

fn list(ctx: &Ctx) -> Result<Outcome> {
    let mut rows = vec![];
    for name in ctx.store.list()? {
        let v = ctx.store.load(&name, None)?;
        rows.push(json!({
            "name": name, "latestVersion": v.version, "createdAt": v.created_at, "message": v.message,
            "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces),
        }));
    }
    let text = human::list(&rows);
    Ok(Outcome::ok(json!({"snapshots": rows}), text))
}

fn show(ctx: &Ctx, snapshot: &str) -> Result<Outcome> {
    let (name, version) = parse_ref(snapshot)?;
    let v = ctx.store.load(&name, version)?;
    let warnings = edit::warnings(&v.workspaces, &ctx.claude);
    let text = human::show(&v, &warnings);
    Ok(Outcome::ok(json!({"version": v, "warnings": warnings}), text))
}

fn history(ctx: &Ctx, name: &str) -> Result<Outcome> {
    let mut rows = vec![];
    for n in ctx.store.versions(name)? {
        let v = ctx.store.load(name, Some(n))?;
        rows.push(json!({
            "version": v.version, "createdAt": v.created_at, "message": v.message, "source": v.source,
            "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces),
        }));
    }
    let text = human::history(name, &rows);
    Ok(Outcome::ok(json!({"snapshot": name, "versions": rows}), text))
}

fn diff_cmd(ctx: &Ctx, a: &str, b: &str) -> Result<Outcome> {
    let load = |r: &str| -> Result<Version> {
        let (n, v) = parse_ref(r)?;
        ctx.store.load(&n, v)
    };
    let (va, vb) = (load(a)?, load(b)?);
    let changes = diff::diff(&va.workspaces, &vb.workspaces);
    Ok(Outcome::ok(
        json!({
            "from": format!("{}@{}", va.snapshot, va.version), "to": format!("{}@{}", vb.snapshot, vb.version),
            "changes": changes, "summary": diff::summary(&changes),
        }),
        human::changes(&changes),
    ))
}

fn prune(ctx: &Ctx, name: &str, keep: usize) -> Result<Outcome> {
    let count = ctx.store.versions(name)?.len();
    if count <= keep {
        return Ok(Outcome::ok(json!({"snapshot": name, "deleted": []}), format!("{name}: {count} version(s), nothing to prune"))
            .with_exit(Exit::NothingToDo));
    }
    confirm(ctx, &format!("Delete {} old version(s) of {name}?", count - keep))?;
    let deleted = ctx.store.prune(name, keep)?;
    Ok(Outcome::ok(json!({"snapshot": name, "deleted": deleted}), format!("{name}: deleted versions {deleted:?}")))
}

fn delete(ctx: &Ctx, name: &str) -> Result<Outcome> {
    let count = ctx.store.versions(name)?.len();
    confirm(ctx, &format!("Delete snapshot {name} and its {count} version(s)?"))?;
    ctx.store.delete(name)?;
    Ok(Outcome::ok(json!({"snapshot": name, "deleted": true}), format!("Deleted {name}")))
}

/// Proceeds with --yes or an interactive yes; otherwise refuses (exit 4).
pub(crate) fn confirm(ctx: &Ctx, question: &str) -> Result<()> {
    if ctx.yes || (ctx.interactive && pick::confirm(question)?) {
        Ok(())
    } else {
        Err(failure(Exit::Refused, format!("{question} Not confirmed (pass --yes)")))
    }
}

fn edit_snapshot(ctx: &Ctx, name: &str, ops: Vec<Op>, message: Option<String>) -> Result<Outcome> {
    let previous = ctx.store.load(name, None)?;
    let mut workspaces = previous.workspaces.clone();
    let open = if ops.iter().any(|o| matches!(o, Op::WsAddLive { .. })) { Some(ctx.backend.read_open_state()?) } else { None };
    edit::apply_ops(&mut workspaces, &ops, &EditCtx { claude: &ctx.claude, store: &ctx.store, open: open.as_ref() })?;
    let source = Source { kind: "edit".into(), cmux_state_version: None, from: None };
    commit(ctx, name, Some(&previous), workspaces, source, message)
}

fn ws_ops(a: &WsArgs) -> Result<Vec<Op>> {
    let mut ops = vec![];
    if let Some(live) = &a.add_live {
        ops.push(Op::WsAddLive { ws: live.clone() });
    }
    if let Some(from) = &a.from {
        let (src, ws) = from
            .split_once(':')
            .ok_or_else(|| failure(Exit::Error, "--from needs <name>[@v]:<workspace>"))?;
        ops.push(Op::WsFrom { from: src.to_string(), ws: ws.to_string() });
    }
    if let Some(ws) = &a.ws {
        if let Some(color) = &a.color {
            ops.push(Op::WsColor { ws: ws.clone(), color: Some(color.clone()) });
        }
        if let Some(index) = a.index {
            ops.push(Op::WsIndex { ws: ws.clone(), index });
        }
        if a.remove {
            ops.push(Op::WsRemove { ws: ws.clone() });
        }
        // Rename last so the ops above still find the workspace by its old name.
        if let Some(to) = &a.rename {
            ops.push(Op::WsRename { ws: ws.clone(), to: to.clone() });
        }
    }
    if ops.is_empty() {
        return Err(failure(Exit::Error, "nothing to change: give a workspace with --rename/--color/--index/--remove, or --add-live/--from"));
    }
    Ok(ops)
}

fn tab_ops(a: &TabArgs) -> Result<Vec<Op>> {
    if let Some(id) = &a.add_session {
        return Ok(vec![Op::TabAddSession { ws: a.target.clone(), session_id: id.clone(), title: a.title.clone() }]);
    }
    if a.add_terminal {
        let cwd = a.cwd.clone().ok_or_else(|| failure(Exit::Error, "--add-terminal needs --cwd"))?;
        return Ok(vec![Op::TabAddTerminal { ws: a.target.clone(), cwd, command: a.command.clone(), title: a.title.clone() }]);
    }
    let positional = [a.index.is_some(), a.move_to.is_some(), a.remove].iter().filter(|b| **b).count();
    let others = [a.cwd.is_some(), a.session.is_some(), a.rename.is_some()].iter().filter(|b| **b).count();
    if positional > 1 || (positional == 1 && others > 0) {
        return Err(failure(Exit::Error, "--index, --move and --remove can't be combined with other changes; run them separately"));
    }
    let t = a.target.clone();
    let mut ops = vec![];
    if let Some(index) = a.index {
        ops.push(Op::TabIndex { tab: t.clone(), index });
    }
    if let Some(to) = &a.move_to {
        ops.push(Op::TabMove { tab: t.clone(), to: to.clone() });
    }
    if a.remove {
        ops.push(Op::TabRemove { tab: t.clone() });
    }
    if let Some(cwd) = &a.cwd {
        ops.push(Op::TabSetCwd { tab: t.clone(), cwd: cwd.clone() });
    }
    if let Some(session) = &a.session {
        ops.push(Op::TabSetSession { tab: t.clone(), session_id: session.clone() });
    }
    // Rename last: a title-based ref would not match after renaming.
    if let Some(to) = &a.rename {
        ops.push(Op::TabRename { tab: t.clone(), to: to.clone() });
    }
    if ops.is_empty() {
        return Err(failure(Exit::Error, "nothing to change: give --rename/--index/--move/--remove/--cwd/--session, or --add-session/--add-terminal"));
    }
    Ok(ops)
}

fn restore(ctx: &Ctx, snapshot: Option<String>, workspaces: Vec<String>, tabs: Vec<String>, plan_only: bool, pick_flag: bool) -> Result<Outcome> {
    let snapshot = match snapshot {
        Some(s) => s,
        None if pick_flag && ctx.interactive => pick::choose_snapshot(&ctx.store)?,
        None => return Err(failure(Exit::Error, "give a snapshot (or --pick in a terminal)")),
    };
    let (name, version) = parse_ref(&snapshot)?;
    let v = ctx.store.load(&name, version)?;
    let source = PlanSource::Snapshot { snapshot: name, version: v.version, selection: Selection { workspaces, tabs } };
    plan_then_apply(ctx, source, plan_only)
}

/// Makes and saves a plan; applies it with --yes or an interactive yes, else prints how to apply.
fn plan_then_apply(ctx: &Ctx, source: PlanSource, plan_only: bool) -> Result<Outcome> {
    if plan_only {
        if !ctx.backend.is_running() {
            return Err(failure(Exit::CmuxUnreachable, "cmux is not running; start it first (restore without --plan starts it)"));
        }
    } else {
        ctx.backend.ensure_running()?;
    }
    let open = ctx.backend.read_open_state()?;
    let plan = plan::make_plan(source, &ctx.store, &ctx.claude, &open)?;
    if plan.is_empty() {
        return Ok(Outcome::ok(
            json!({"plan": plan, "applied": false}),
            format!("{}\nNothing to restore: everything is already open.", human::plan(&plan)),
        )
        .with_exit(Exit::NothingToDo));
    }
    plan::save_plan(&ctx.store, &plan)?;
    let go = !plan_only
        && (ctx.yes || (ctx.interactive && pick::confirm(&format!("{}\nRestore {} tab(s)?", human::plan(&plan), plan.tab_count()))?));
    if !go {
        let apply = format!("ccsnap apply {}", plan.plan_id);
        return Ok(Outcome::ok(
            json!({"plan": plan, "applied": false, "apply": apply}),
            format!("{}\nTo apply: {apply}", human::plan(&plan)),
        ));
    }
    applied(ctx, &plan)
}

fn applied(ctx: &Ctx, plan: &Plan) -> Result<Outcome> {
    let steps = plan::apply_plan(plan, &ctx.store, &ctx.claude, ctx.backend.as_ref())?;
    let failed = steps.iter().filter(|s| !s.ok).count();
    let out = Outcome::ok(json!({"planId": plan.plan_id, "applied": true, "steps": steps, "failed": failed}), human::steps(&steps));
    Ok(if failed > 0 { out.with_exit(Exit::Error) } else { out })
}

fn apply(ctx: &Ctx, plan_id: &str) -> Result<Outcome> {
    let plan = plan::load_plan(&ctx.store, plan_id)?;
    ctx.backend.ensure_running()?;
    applied(ctx, &plan)
}

fn sessions(ctx: &Ctx, search: Option<&str>, cwd: Option<&str>, days: u64) -> Result<Outcome> {
    let open = ctx.backend.read_open_state().map(|s| s.open_sessions()).unwrap_or_default();
    let needle = search.map(str::to_lowercase);
    let list: Vec<SessionInfo> = ctx
        .claude
        .scan(Some(Duration::from_secs(days * 86_400)))?
        .into_iter()
        .filter(|s| cwd.is_none_or(|c| s.cwd.as_deref().is_some_and(|sc| sc.starts_with(c))))
        .filter(|s| {
            needle.as_ref().is_none_or(|n| {
                [s.title.as_deref(), s.first_prompt.as_deref(), s.cwd.as_deref(), Some(s.session_id.as_str())]
                    .iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(n.as_str()))
            })
        })
        .collect();
    let rows: Vec<Value> = list.iter().map(|s| json!({"session": s, "open": open.contains(&s.session_id)})).collect();
    Ok(Outcome::ok(json!({"sessions": rows}), human::sessions(&list, &open)))
}

fn recent(ctx: &Ctx, hours: u64, restore: bool, plan_only: bool, pick_flag: bool) -> Result<Outcome> {
    let wants_plan = restore || plan_only || (pick_flag && ctx.interactive);
    if wants_plan {
        if plan_only {
            if !ctx.backend.is_running() {
                return Err(failure(Exit::CmuxUnreachable, "cmux is not running; start it first (restore without --plan starts it)"));
            }
        } else {
            ctx.backend.ensure_running()?;
        }
    }
    let open = ctx.backend.read_open_state()?.open_sessions();
    let home = home_dir()?.to_string_lossy().into_owned();
    let list: Vec<SessionInfo> = ctx
        .claude
        .scan(Some(Duration::from_secs(hours * 3600)))?
        .into_iter()
        .filter(|s| is_recoverable(s, &open, &home))
        .collect();
    if list.is_empty() {
        return Ok(Outcome::ok(json!({"sessions": []}), format!("No Claude sessions from the last {hours}h are missing from cmux."))
            .with_exit(Exit::NothingToDo));
    }
    if !wants_plan {
        return Ok(Outcome::ok(json!({"sessions": list}), human::sessions(&list, &open)));
    }
    let source = PlanSource::Sessions { session_ids: list.iter().map(|s| s.session_id.clone()).collect() };
    plan_then_apply(ctx, source, plan_only)
}

/// Interactive sessions (not `claude -p`/SDK), outside ~/.claude*, not open in cmux.
fn is_recoverable(s: &SessionInfo, open: &HashSet<String>, home: &str) -> bool {
    let interactive = s.entrypoint.as_deref().is_none_or(|e| e == "cli");
    let internal = s.cwd.as_deref().is_some_and(|c| c.starts_with(&format!("{home}/.claude")));
    interactive && !internal && s.cwd.is_some() && !open.contains(&s.session_id)
}
