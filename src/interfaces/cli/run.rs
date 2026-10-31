//! Runs a parsed command: resolves prompts, calls the `app` use case, and turns its result into
//! an `Outcome` (JSON for `--json`, text for people).

use anyhow::Result;
use serde_json::{Value, json};

use super::{Command, Ctx, Outcome, ScheduleCommand, TabArgs, WsArgs, human, pick};
use crate::app;
use crate::app::edit::Edited;
use crate::app::snapshots::Commit;
use crate::app::update::Update;
use crate::domain::diff;
use crate::domain::edit::Op;
use crate::domain::model::SessionRef;
use crate::domain::model::tab_count;
use crate::domain::plan::{Plan, PlanSource};
use crate::domain::schedule::{Schedule, format_interval};
use crate::error::{Exit, failure};
use crate::ports::SessionInfo;

pub fn dispatch(ctx: &Ctx, command: Command) -> Result<Outcome> {
    match command {
        Command::Save { name, workspaces, from, message, pick } => save(ctx, name, &workspaces, from, message, pick),
        Command::List { cwd } => list(ctx, cwd.as_deref()),
        Command::Open => open_now(ctx),
        Command::Show { snapshot } => show(ctx, &snapshot),
        Command::History { name } => history(ctx, &name),
        Command::Diff { a, b } => diff_cmd(ctx, &a, &b),
        Command::Prune { name, keep } => prune(ctx, &name, keep),
        Command::Delete { name } => delete(ctx, &name),
        Command::Export { snapshot, output } => export(ctx, &snapshot, output.as_deref()),
        Command::Import { file, name, map, message } => import(ctx, &file, name, &map, message),
        Command::Rename { old, new } => {
            let from = app::snapshots::rename(ctx, &old, &new)?;
            Ok(Outcome::ok(json!({"from": from, "to": new}), format!("Renamed {from} to {new}")))
        }
        Command::Schedule(command) => schedule(ctx, command),
        Command::Ws(args) => {
            let ops = ws_ops(&args)?;
            edit_snapshot(ctx, &args.name, ops, args.message, false, None)
        }
        Command::Tab(args) => {
            let ops = tab_ops(&args)?;
            edit_snapshot(ctx, &args.name, ops, args.message, false, None)
        }
        Command::Edit { name, batch, message, dry_run, base } => {
            let text = if batch == "-" {
                match &ctx.input {
                    Some(text) => text.clone(),
                    None => std::io::read_to_string(std::io::stdin())?,
                }
            } else {
                std::fs::read_to_string(&batch)?
            };
            let ops: Vec<Op> =
                serde_json::from_str(&text).map_err(|e| failure(Exit::Error, format!("invalid batch: {e}")))?;
            edit_snapshot(ctx, &name, ops, message, dry_run, base)
        }
        Command::Restore { snapshot, workspaces, tabs, plan, pick } => {
            restore(ctx, snapshot, workspaces, tabs, plan, pick)
        }
        Command::Apply { plan_id } => apply(ctx, &plan_id),
        Command::Sessions { search, cwd, days, agent, limit } => {
            sessions(ctx, search.as_deref(), cwd.as_deref(), days, agent.as_deref(), limit)
        }
        Command::Recent { hours, agent, restore, plan, pick, limit } => {
            recent(ctx, hours, agent.as_deref(), restore, plan, pick, limit)
        }
        Command::Agents => {
            let list = app::agents::list(ctx);
            let text = human::agents(&list);
            Ok(Outcome::ok(json!({"agents": list}), text))
        }
        Command::Ui { port, idle_minutes, no_open } => crate::interfaces::web::ui(ctx, port, idle_minutes, !no_open),
        Command::InstallCmux { print } => install_cmux(ctx, print),
        Command::Completions { .. } | Command::Man => {
            let text = super::generated(&command).unwrap_or_default();
            Ok(Outcome::ok(json!({"text": text}), text))
        }
        Command::InstallSkill => {
            let path = app::install::install_skill()?;
            Ok(Outcome::ok(
                json!({"installed": path}),
                format!("Installed the sessionsnap skill at {}", path.display()),
            ))
        }
        Command::Update { check } => Ok(updated(app::update::run(check)?)),
        Command::Uninstall { keep_data } => uninstall(ctx, keep_data),
    }
}

fn uninstall(ctx: &Ctx, keep_data: bool) -> Result<Outcome> {
    let paths = app::uninstall::paths(ctx.store.root())?;
    let removes: Vec<String> = app::uninstall::preview(&paths, keep_data).iter().map(|l| format!("  - {l}")).collect();
    confirm(ctx, &format!("This removes:\n{}\nUninstall sessionsnap?", removes.join("\n")))?;
    let report = app::uninstall::run(&paths, keep_data)?;
    let mut text: Vec<String> = report
        .steps
        .iter()
        .map(|s| match &s.note {
            Some(note) => format!("{}: {} ({note})", s.what, s.status),
            None => format!("{}: {}", s.what, s.status),
        })
        .collect();
    let failed = report.failed();
    if failed.is_empty() {
        text.push("sessionsnap is uninstalled.".into());
    } else {
        text.push(format!("sessionsnap is not fully uninstalled: {} step(s) failed (see above).", failed.len()));
    }
    if !report.left.is_empty() {
        text.push("Left in place, since other tools may use them:".into());
        text.extend(report.left.iter().map(|l| format!("  - {l}")));
    }
    let out = Outcome::ok(json!({"steps": report.steps, "left": report.left}), text.join("\n"));
    // Like `apply`: the report lists every step, and a failed one makes the exit code 1.
    Ok(if failed.is_empty() { out } else { out.with_exit(Exit::Error) })
}

fn updated(update: Update) -> Outcome {
    match update {
        Update::Latest { current } => Outcome::ok(
            json!({"current": current, "latest": current, "updateAvailable": false, "updated": false}),
            format!("sessionsnap {current} is the latest release"),
        ),
        Update::Available { current, latest, how } => Outcome::ok(
            json!({"current": current, "latest": latest, "updateAvailable": true, "updated": false, "run": how}),
            format!("sessionsnap {latest} is available (you have {current}); run: {how}"),
        ),
        Update::Updated { from, to, path } => Outcome::ok(
            json!({"current": from, "latest": to, "updateAvailable": true, "updated": true, "path": path}),
            format!(
                "Updated sessionsnap {from} -> {to} at {}. A running `sessionsnap ui` keeps the old version until restarted.",
                path.display()
            ),
        ),
    }
}

fn install_cmux(ctx: &Ctx, print: bool) -> Result<Outcome> {
    let exe = app::install::current_exe()?;
    if print {
        let commands = app::install::palette_commands(&exe);
        let text = serde_json::to_string_pretty(&commands)?;
        return Ok(Outcome::ok(json!({"commands": commands}), text));
    }
    app::install::check_palette_supported(std::env::consts::OS)?;
    let config = app::install::cmux_config_path()?;
    confirm(ctx, &format!("Add sessionsnap commands to {}?", config.display()))?;
    let backup = app::install::install_cmux(&config, &exe)?;
    let note = backup.as_ref().map(|b| format!(" (backup: {})", b.display())).unwrap_or_default();
    Ok(Outcome::ok(
        json!({"config": config, "backup": backup}),
        format!("Added sessionsnap commands to {}{note}. Run `cmux reload-config` to load them.", config.display()),
    ))
}

fn save(
    ctx: &Ctx,
    name: Option<String>,
    only: &[String],
    from: Option<String>,
    message: Option<String>,
    pick_flag: bool,
) -> Result<Outcome> {
    let name = match name {
        Some(n) => n,
        None if pick_flag && ctx.interactive => pick::choose_save_name(app::snapshots::names(ctx)?)?,
        None => return Err(failure(Exit::Error, "give a snapshot name (or --pick in a terminal)")),
    };
    let commit = app::snapshots::save(ctx, &name, only, from.as_deref(), message)?;
    Ok(committed(commit))
}

fn export(ctx: &Ctx, snapshot: &str, output: Option<&str>) -> Result<Outcome> {
    let (name, version) = crate::domain::model::parse_ref(snapshot)?;
    let v = app::load_version(ctx, &name, version)?;
    let Some(file) = output else {
        return Ok(Outcome::ok(serde_json::to_value(&v)?, serde_json::to_string_pretty(&v)?));
    };
    let path = std::path::Path::new(file);
    if path.symlink_metadata().is_ok() {
        return Err(failure(Exit::Refused, format!("{file} exists; pick another file or remove it first")));
    }
    crate::util::json::write_private_json_atomic(path, &v)?;
    Ok(Outcome::ok(
        json!({"snapshot": v.snapshot, "version": v.version, "file": file}),
        format!("Exported {} v{} to {file}", v.snapshot, v.version),
    ))
}

fn import(ctx: &Ctx, file: &str, name: Option<String>, map: &[String], message: Option<String>) -> Result<Outcome> {
    let text = match (file, &ctx.input) {
        ("-", Some(text)) => text.clone(),
        ("-", None) => std::io::read_to_string(std::io::stdin())?,
        _ => std::fs::read_to_string(file).map_err(|e| {
            let exit = if e.kind() == std::io::ErrorKind::NotFound { Exit::NotFound } else { Exit::Error };
            failure(exit, format!("can't read {file}: {e}"))
        })?,
    };
    // A file from elsewhere is printed (show, list, plans) as well as restored: a control character
    // anywhere in it (an escape sequence in a title) is refused, not just kept out of commands.
    fn has_control(v: &Value) -> bool {
        match v {
            Value::String(s) => s.chars().any(char::is_control),
            Value::Array(a) => a.iter().any(has_control),
            Value::Object(o) => o.iter().any(|(k, v)| k.chars().any(char::is_control) || has_control(v)),
            _ => false,
        }
    }
    if serde_json::from_str::<Value>(&text).is_ok_and(|v| has_control(&v)) {
        return Err(failure(Exit::Error, format!("{file} holds a control character; not importing it")));
    }
    let imported = crate::domain::model::parse_version(&text).map_err(|why| {
        let exit = if why.starts_with("format") { Exit::UnknownFormat } else { Exit::Error };
        failure(exit, format!("{file} is not an exported snapshot: {why}"))
    })?;
    let maps = map
        .iter()
        .map(|m| {
            let (old, new) = m.split_once('=').unwrap_or((m, ""));
            let ok = |p: &str| p.starts_with('/') && !p.chars().any(char::is_control);
            if ok(old) && ok(new) {
                Ok((old.to_string(), new.to_string()))
            } else {
                Err(failure(Exit::Error, format!("--map needs two absolute folders, OLD=NEW: {m:?}")))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let name = name.unwrap_or_else(|| imported.snapshot.clone());
    Ok(committed(app::snapshots::import(ctx, &name, imported, &maps, message)?))
}

/// The outcome of writing a version: exit 3 when nothing changed. Shared by save and edits. Names
/// the snapshot as stored ("main" for `save Main`).
fn committed(commit: Commit) -> Outcome {
    match commit {
        Commit::Unchanged { snapshot: name, version } => Outcome::ok(
            json!({"snapshot": name, "version": version, "changed": false}),
            format!("{name}: no changes since v{version}"),
        )
        .with_exit(Exit::NothingToDo),
        Commit::Saved { version: v, changes, warnings } => Outcome::ok(
            json!({
                "snapshot": v.snapshot, "version": v.version, "changed": true, "message": v.message, "changes": changes,
                "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces), "warnings": warnings,
            }),
            human::saved(&v, &warnings),
        ),
    }
}

fn open_now(ctx: &Ctx) -> Result<Outcome> {
    let open = app::sessions::open_now(ctx)?;
    let rows: Vec<Value> = open
        .workspaces
        .iter()
        .map(|w| {
            let tabs: Vec<Value> = w
                .tabs
                .iter()
                .map(|t| {
                    let (agent, id) = t.agent.as_ref().map(|a| (&a.kind, &a.session_id)).unzip();
                    json!({"title": t.title, "cwd": t.cwd, "agent": agent, "sessionId": id})
                })
                .collect();
            json!({"name": w.name, "cwd": w.cwd, "tabs": tabs})
        })
        .collect();
    let text = open.workspaces.iter().map(|w| format!("{} ({} tabs)", w.name, w.tabs.len())).collect::<Vec<_>>();
    Ok(Outcome::ok(json!({"live": open.live, "workspaces": rows}), text.join("\n")))
}

fn list(ctx: &Ctx, cwd: Option<&str>) -> Result<Outcome> {
    let schedules = ctx.store.read_schedules()?;
    // `--cwd .` and other relative folders mean the folder they name from here.
    let dir = cwd.map(|d| {
        std::fs::canonicalize(d)
            .map_or_else(|_| d.trim_end_matches('/').to_string(), |p| p.to_string_lossy().into_owned())
    });
    let under = |path: &str| dir.as_deref().is_none_or(|d| path == d || path.starts_with(&format!("{d}/")));
    let rows: Vec<Value> = app::snapshots::list(ctx)?
        .into_iter()
        .filter(|(_, (_, v))| match (&dir, v) {
            (None, _) => true,
            (Some(_), Ok(v)) => v.workspaces.iter().any(|w| under(&w.cwd) || w.tabs.iter().any(|t| under(t.cwd()))),
            (Some(_), Err(_)) => false,
        })
        .map(|(name, (n, v))| {
            let mut row = match v {
                // What it holds, so a person or an agent can pick one without `show`-ing each.
                Ok(v) => {
                    let mut seen = std::collections::HashSet::new();
                    let folders: Vec<&str> =
                        v.workspaces.iter().map(|w| w.cwd.as_str()).filter(|f| seen.insert(*f)).collect();
                    let mut agents: Vec<&str> = v
                        .workspaces
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .filter_map(|t| match t {
                            crate::domain::model::Tab::Agent { agent, .. } => Some(agent.as_str()),
                            crate::domain::model::Tab::Terminal { .. } => None,
                        })
                        .collect();
                    agents.sort_unstable();
                    agents.dedup();
                    json!({
                        "name": name, "latestVersion": v.version, "createdAt": v.created_at, "message": v.message,
                        "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces),
                        "workspaceNames": v.workspaces.iter().map(|w| &w.name).collect::<Vec<_>>(),
                        "folders": folders, "agents": agents,
                    })
                }
                Err(why) => json!({"name": name, "latestVersion": n, "error": format!("unreadable: {why}")}),
            };
            if let Some(s) = schedules.get(&name) {
                row["schedule"] = schedule_json(s);
            }
            row
        })
        .collect();
    let text = human::list(&rows);
    Ok(Outcome::ok(json!({"snapshots": rows}), text))
}

fn show(ctx: &Ctx, snapshot: &str) -> Result<Outcome> {
    let (v, warnings) = app::snapshots::show(ctx, snapshot)?;
    let text = human::show(&v, &warnings);
    Ok(Outcome::ok(json!({"version": v, "warnings": warnings}), text))
}

fn history(ctx: &Ctx, name: &str) -> Result<Outcome> {
    let rows: Vec<Value> = app::snapshots::history(ctx, name)?
        .into_iter()
        .map(|(n, v)| match v {
            Ok(v) => json!({
                "version": v.version, "createdAt": v.created_at, "message": v.message, "source": v.source,
                "workspaces": v.workspaces.len(), "tabs": tab_count(&v.workspaces),
            }),
            Err(why) => json!({"version": n, "error": format!("unreadable: {why}")}),
        })
        .collect();
    let text = human::history(name, &rows);
    Ok(Outcome::ok(json!({"snapshot": name, "versions": rows}), text))
}

fn diff_cmd(ctx: &Ctx, a: &str, b: &str) -> Result<Outcome> {
    let (va, vb, changes) = app::snapshots::diff(ctx, a, b)?;
    Ok(Outcome::ok(
        json!({
            "from": format!("{}@{}", va.snapshot, va.version), "to": format!("{}@{}", vb.snapshot, vb.version),
            "changes": changes, "summary": diff::summary(&changes),
        }),
        human::changes(&changes),
    ))
}

fn prune(ctx: &Ctx, name: &str, keep: usize) -> Result<Outcome> {
    // Before asking: the store refuses it anyway, after the user said yes.
    if keep == 0 {
        return Err(failure(Exit::Error, "--keep must be at least 1"));
    }
    let count = app::snapshots::version_count(ctx, name)?;
    if count <= keep {
        return Ok(Outcome::ok(
            json!({"snapshot": name, "deleted": []}),
            format!("{name}: {count} version(s), nothing to prune"),
        )
        .with_exit(Exit::NothingToDo));
    }
    confirm(ctx, &format!("Delete {} old version(s) of {name}?", count - keep))?;
    let deleted = app::snapshots::prune(ctx, name, keep)?;
    Ok(Outcome::ok(json!({"snapshot": name, "deleted": deleted}), format!("{name}: deleted versions {deleted:?}")))
}

fn delete(ctx: &Ctx, name: &str) -> Result<Outcome> {
    let count = app::snapshots::version_count(ctx, name)?;
    confirm(ctx, &format!("Delete snapshot {name} and its {count} version(s)?"))?;
    app::snapshots::delete(ctx, name)?;
    Ok(Outcome::ok(json!({"snapshot": name, "deleted": true}), format!("Deleted {name}")))
}

fn schedule_json(s: &Schedule) -> Value {
    json!({
        "interval": format_interval(s.interval), "intervalSeconds": s.interval, "running": !s.stopped,
        "createdAt": s.created_at, "lastRun": s.last_run,
    })
}

fn schedule(ctx: &Ctx, command: ScheduleCommand) -> Result<Outcome> {
    match command {
        ScheduleCommand::Add { name, interval } => {
            let (s, first) = app::schedule::add(ctx, &name, &interval)?;
            let every = format_interval(s.interval);
            let (first_json, first_text) = match first {
                Ok(Commit::Saved { version, .. }) => (
                    json!({"saved": true, "version": version.version}),
                    format!(
                        "saved v{} ({}, {})",
                        version.version,
                        human::count(version.workspaces.len(), "workspace"),
                        human::count(tab_count(&version.workspaces), "tab")
                    ),
                ),
                Ok(Commit::Unchanged { version, .. }) => {
                    (json!({"saved": false, "version": version}), format!("no changes since v{version}"))
                }
                Err(e) => (json!({"saved": false, "reason": format!("{e:#}")}), format!("not saved yet ({e:#})")),
            };
            Ok(Outcome::ok(
                json!({"snapshot": name, "schedule": schedule_json(&s), "firstSave": first_json}),
                format!("Saving {name} every {every}; first save: {first_text}"),
            ))
        }
        ScheduleCommand::List => {
            let listed = app::schedule::list(ctx)?;
            let rows: Vec<Value> = listed
                .iter()
                .map(|(name, s, latest)| {
                    let mut row = schedule_json(s);
                    row["name"] = json!(name);
                    if let Some((n, at)) = latest {
                        row["latestVersion"] = json!(n);
                        row["latestAt"] = json!(at);
                    }
                    row
                })
                .collect();
            let text = human::schedules(&rows);
            Ok(Outcome::ok(json!({"schedules": rows}), text))
        }
        ScheduleCommand::Edit { name, interval } => {
            let (key, s) = app::schedule::edit(ctx, &name, &interval)?;
            let every = format_interval(s.interval);
            Ok(Outcome::ok(
                json!({"snapshot": key, "schedule": schedule_json(&s)}),
                format!("Saving {key} every {every}"),
            ))
        }
        ScheduleCommand::Stop { name } => start_stop(ctx, &name, false),
        ScheduleCommand::Start { name } => start_stop(ctx, &name, true),
        ScheduleCommand::Remove { name } => {
            let key = app::schedule::remove(ctx, &name)?;
            Ok(Outcome::ok(
                json!({"snapshot": key, "removed": true}),
                format!("{key} is no longer saved automatically; its versions are kept"),
            ))
        }
        ScheduleCommand::Run { name } => match app::schedule::run(ctx, &name) {
            Err(e) if crate::error::exit_of(&e) == Exit::NothingToDo => Ok(Outcome::ok(
                json!({"snapshot": name, "changed": false, "skipped": true, "reason": format!("{e:#}")}),
                format!("{name}: skipped ({e:#})"),
            )
            .with_exit(Exit::NothingToDo)),
            result => Ok(committed(result?)),
        },
    }
}

fn start_stop(ctx: &Ctx, name: &str, run: bool) -> Result<Outcome> {
    let (key, s) = match app::schedule::set_running(ctx, name, run) {
        Err(e) if crate::error::exit_of(&e) == Exit::NothingToDo => {
            return Ok(
                Outcome::ok(json!({"snapshot": name, "changed": false}), format!("{e:#}")).with_exit(Exit::NothingToDo)
            );
        }
        result => result?,
    };
    let text = if run {
        format!("Saving {key} every {} again", format_interval(s.interval))
    } else {
        format!("Stopped {key}: no saves until `sessionsnap schedule start {key}`")
    };
    Ok(Outcome::ok(json!({"snapshot": key, "changed": true, "schedule": schedule_json(&s)}), text))
}

/// Proceeds with --yes or an interactive yes; otherwise refuses (exit 4).
pub(crate) fn confirm(ctx: &Ctx, question: &str) -> Result<()> {
    if ctx.yes || (ctx.interactive && pick::confirm(question)?) {
        Ok(())
    } else {
        Err(crate::error::needs_confirmation(question))
    }
}

fn edit_snapshot(
    ctx: &Ctx,
    name: &str,
    ops: Vec<Op>,
    message: Option<String>,
    dry_run: bool,
    base: Option<u32>,
) -> Result<Outcome> {
    Ok(match app::edit::edit(ctx, name, ops, message, dry_run, base)? {
        Edited::DryRun { base_version, workspaces, changes, warnings } => {
            let summary = if changes.is_empty() { "no changes".to_string() } else { diff::summary(&changes) };
            Outcome::ok(
                json!({
                    "snapshot": name, "baseVersion": base_version, "dryRun": true,
                    "workspaces": workspaces, "changes": changes, "warnings": warnings,
                }),
                format!("{name} (not saved): {summary}"),
            )
        }
        Edited::Committed(commit) => committed(commit),
    })
}

fn ws_ops(a: &WsArgs) -> Result<Vec<Op>> {
    let mut ops = vec![];
    if let Some(live) = &a.add_live {
        ops.push(Op::WsAddLive { ws: live.clone() });
    }
    if let Some(from) = &a.from {
        let (src, ws) =
            from.split_once(':').ok_or_else(|| failure(Exit::Error, "--from needs <name>[@v]:<workspace>"))?;
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
        return Err(failure(
            Exit::Error,
            "nothing to change: give a workspace with --rename/--color/--index/--remove, or --add-live/--from",
        ));
    }
    Ok(ops)
}

fn tab_ops(a: &TabArgs) -> Result<Vec<Op>> {
    if let Some(id) = &a.add_session {
        return Ok(vec![Op::TabAddSession {
            ws: a.target.clone(),
            session_id: id.clone(),
            agent: a.agent.clone(),
            title: a.title.clone(),
        }]);
    }
    if a.add_terminal {
        let cwd = a.cwd.clone().ok_or_else(|| failure(Exit::Error, "--add-terminal needs --cwd"))?;
        return Ok(vec![Op::TabAddTerminal {
            ws: a.target.clone(),
            cwd,
            command: a.command.clone(),
            title: a.title.clone(),
        }]);
    }
    let positional = [a.index.is_some(), a.move_to.is_some(), a.remove].iter().filter(|b| **b).count();
    let others = [a.cwd.is_some(), a.session.is_some(), a.rename.is_some()].iter().filter(|b| **b).count();
    if positional > 1 || (positional == 1 && others > 0) {
        return Err(failure(
            Exit::Error,
            "--index, --move and --remove can't be combined with other changes; run them separately",
        ));
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
        ops.push(Op::TabSetSession { tab: t.clone(), session_id: session.clone(), agent: a.agent.clone() });
    }
    // Rename last: a title-based ref would not match after renaming.
    if let Some(to) = &a.rename {
        ops.push(Op::TabRename { tab: t.clone(), to: to.clone() });
    }
    if ops.is_empty() {
        return Err(failure(
            Exit::Error,
            "nothing to change: give --rename/--index/--move/--remove/--cwd/--session, or --add-session/--add-terminal",
        ));
    }
    Ok(ops)
}

fn restore(
    ctx: &Ctx,
    snapshot: Option<String>,
    workspaces: Vec<String>,
    tabs: Vec<String>,
    plan_only: bool,
    pick_flag: bool,
) -> Result<Outcome> {
    let (snapshot, latest) = match snapshot {
        Some(s) => (s, false),
        None if pick_flag && ctx.interactive => (pick::choose_snapshot(app::snapshots::names(ctx)?)?, false),
        // After a reboot, the thing to bring back is usually the last one saved.
        None => (app::snapshots::most_recent(ctx)?, true),
    };
    let source = app::restore::snapshot_source(ctx, &snapshot, workspaces, tabs)?;
    let mut out = plan_then_apply(ctx, source, plan_only)?;
    if latest {
        out.json["snapshot"] = json!(snapshot);
        out.text = format!("The most recently saved snapshot: {snapshot}\n{}", out.text);
    }
    Ok(out)
}

/// Makes and saves a plan; applies it with --yes or an interactive yes, else prints how to apply.
fn plan_then_apply(ctx: &Ctx, source: PlanSource, plan_only: bool) -> Result<Outcome> {
    let plan = app::restore::prepare(ctx, source, plan_only)?;
    if plan.is_empty() {
        return Ok(Outcome::ok(
            json!({"plan": plan, "applied": false}),
            format!("{}\nNothing to restore: everything is already open.", human::plan(&plan)),
        )
        .with_exit(Exit::NothingToDo));
    }
    let go = !plan_only
        && (ctx.yes
            || (ctx.interactive
                && pick::confirm(&format!(
                    "{}\nRestore {}?",
                    human::plan(&plan),
                    human::count(plan.tab_count(), "tab")
                ))?));
    if !go {
        let apply = format!("sessionsnap apply {}", plan.plan_id);
        return Ok(Outcome::ok(
            json!({"plan": plan, "applied": false, "apply": apply}),
            format!("{}\nTo apply: {apply}", human::plan(&plan)),
        ));
    }
    applied(ctx, &plan)
}

fn applied(ctx: &Ctx, plan: &Plan) -> Result<Outcome> {
    let steps = app::restore::run_plan(ctx, plan)?;
    let failed = steps.iter().filter(|s| !s.ok).count();
    // The plan's warnings too: a restore applied at once (--yes, a yes at the prompt) shows them
    // nowhere else.
    let mut text = human::steps(&steps);
    for w in &plan.warnings {
        text.push_str(&format!("\n! {w}"));
    }
    let out = Outcome::ok(
        json!({"planId": plan.plan_id, "applied": true, "steps": steps, "failed": failed, "warnings": plan.warnings}),
        text,
    );
    Ok(if failed > 0 { out.with_exit(Exit::Error) } else { out })
}

fn apply(ctx: &Ctx, plan_id: &str) -> Result<Outcome> {
    let plan = app::restore::load_for_apply(ctx, plan_id)?;
    applied(ctx, &plan)
}

fn sessions(
    ctx: &Ctx,
    search: Option<&str>,
    cwd: Option<&str>,
    days: u64,
    agent: Option<&str>,
    limit: Option<usize>,
) -> Result<Outcome> {
    let (list, open) = app::sessions::list(ctx, search, cwd, days, agent)?;
    let total = list.len();
    let list = &list[..limit.unwrap_or(total).min(total)];
    let rows: Vec<Value> = list
        .iter()
        .map(|s| json!({"session": session_json(s), "open": open.contains(&SessionRef::new(&s.agent, &s.session_id))}))
        .collect();
    Ok(Outcome::ok(json!({"sessions": rows, "total": total}), human::sessions(list, &open)))
}

/// A session as `sessions` and `recent` print it.
fn session_json(s: &SessionInfo) -> Value {
    json!({
        "agent": s.agent, "sessionId": s.session_id, "title": s.title, "firstPrompt": s.first_prompt,
        "cwd": s.cwd, "modified": s.modified,
    })
}

fn recent(
    ctx: &Ctx,
    hours: u64,
    agent: Option<&str>,
    restore: bool,
    plan_only: bool,
    pick_flag: bool,
    limit: Option<usize>,
) -> Result<Outcome> {
    let wants_plan = restore || plan_only || (pick_flag && ctx.interactive);
    if wants_plan {
        app::restore::require_terminal(ctx, plan_only)?;
    }
    let (list, open) = app::sessions::recent(ctx, hours, agent)?;
    if list.is_empty() {
        let text = format!("No agent sessions from the last {hours}h are missing from {}.", ctx.terminal.name());
        // A plan was asked for: the same empty plan as `restore --plan` gives.
        if wants_plan {
            let mut out = plan_then_apply(ctx, app::restore::sessions_source(&list), plan_only)?;
            out.text = text;
            return Ok(out);
        }
        return Ok(Outcome::ok(json!({"sessions": []}), text).with_exit(Exit::NothingToDo));
    }
    if !wants_plan {
        let total = list.len();
        let shown = &list[..limit.unwrap_or(total).min(total)];
        let rows: Vec<Value> = shown.iter().map(session_json).collect();
        return Ok(Outcome::ok(json!({"sessions": rows, "total": total}), human::sessions(shown, &open)));
    }
    plan_then_apply(ctx, app::restore::sessions_source(&list), plan_only)
}
