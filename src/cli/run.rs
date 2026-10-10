use anyhow::Result;
use serde_json::json;

use super::{Command, Ctx, Outcome, human, pick};
use crate::capture::{capture, merge};
use crate::diff;
use crate::edit;
use crate::error::{Exit, failure};
use crate::model::{Source, Version, Workspace, tab_count};
use crate::store::{parse_ref, validate_name};

pub fn dispatch(ctx: &Ctx, command: Command) -> Result<Outcome> {
    match command {
        Command::Save { name, workspaces, from, message, pick } => save(ctx, name, &workspaces, from, message, pick),
        Command::List => list(ctx),
        Command::Show { snapshot } => show(ctx, &snapshot),
        Command::History { name } => history(ctx, &name),
        Command::Diff { a, b } => diff_cmd(ctx, &a, &b),
        Command::Prune { name, keep } => prune(ctx, &name, keep),
        Command::Delete { name } => delete(ctx, &name),
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
