//! Editing a saved snapshot: a list of edit operations applied as one new version.

use anyhow::Result;

use super::Ctx;
use super::snapshots::{Base, Commit, commit};
use crate::domain::capture::capture;
use crate::domain::diff::{self, Change};
use crate::domain::edit::{self, EditCtx, Op};
use crate::domain::model::{Source, Workspace};
use crate::error::{Exit, failure};

/// What an edit did.
#[derive(Debug)]
pub enum Edited {
    /// `--dry-run`: the edited workspaces, not saved.
    DryRun {
        base_version: u32,
        workspaces: Vec<Workspace>,
        changes: Vec<Change>,
        warnings: Vec<String>,
    },
    Committed(Commit),
}

/// Applies `ops` to the latest version of `name` and saves the result (unless `dry_run`). With
/// `base`, refuses unless the latest version is that one: the ops were previewed on it. A save
/// holds the snapshot's lock from reading the latest version to writing the next, so a version
/// that lands meanwhile is waited for, then seen.
pub fn edit(
    ctx: &Ctx,
    name: &str,
    ops: Vec<Op>,
    message: Option<String>,
    dry_run: bool,
    base: Option<u32>,
) -> Result<Edited> {
    let lock = if dry_run { None } else { Some(ctx.store.lock(name)?) };
    let previous = super::load_version(ctx, name, None)?;
    if let Some(base) = base
        && base != previous.version
    {
        return Err(failure(
            Exit::PlanStale,
            format!(
                "snapshot '{name}' is now at v{}, not v{base}: it changed since these edits were previewed; preview them again",
                previous.version
            ),
        ));
    }
    let mut workspaces = previous.workspaces.clone();
    // Ops that put a session in a tab take its flags from the record of saved sessions, or from
    // cmux when it is open there.
    let sets_session =
        ops.iter().any(|o| matches!(o, Op::TabAddSession { .. } | Op::TabSetSession { .. } | Op::WsFrom { .. }));
    let read_at = super::known::now();
    let open = if ops.iter().any(|o| matches!(o, Op::WsAddLive { .. })) {
        Some(super::read_open(ctx.terminal.as_ref())?)
    } else if sets_session {
        // Without cmux's state, the record still applies.
        super::read_open(ctx.terminal.as_ref()).ok()
    } else {
        None
    };
    let known = if sets_session { super::known::load(&ctx.store, &ctx.agents)? } else { Default::default() };
    let notes = edit::apply_ops(
        &mut workspaces,
        &ops,
        &EditCtx {
            agents: &ctx.agents,
            snapshots: &super::NormalizedSnapshots { store: &ctx.store, agents: &ctx.agents },
            open: open.as_ref(),
            known,
        },
    )?;
    if dry_run {
        let changes = diff::diff(&previous.workspaces, &workspaces);
        let warnings = [notes, edit::warnings(&workspaces, &ctx.agents)].concat();
        return Ok(Edited::DryRun { base_version: previous.version, workspaces, changes, warnings });
    }
    let source = Source { kind: "edit".into(), cmux_state_version: None, from: None };
    let lock = lock.ok_or_else(|| anyhow::anyhow!("an edit is saved under its snapshot's lock"))?;
    let base = Base { lock: &lock, latest: Some(previous.version), previous: Some(&previous) };
    // What the edit read from cmux is a live launch seen now: every open session's, checked. The
    // record is marked before the version is written.
    let seen = open.as_ref().and_then(|open| {
        capture(open, &ctx.agents, &[]).ok().map(|(c, _)| crate::domain::capture::seen(open, &ctx.agents, &c))
    });
    let mark = if seen.is_some() { Some(super::known::begin(&ctx.store, &read_at)?) } else { None };
    let mut result = match commit(ctx, name, base, workspaces, source, message, notes, false) {
        Ok(result) => result,
        Err(e) => {
            mark.iter().for_each(|m| ctx.store.end_pending(m));
            return Err(e);
        }
    };
    if let (Some(seen), Some(mark)) = (seen, &mark)
        && let Err(e) = super::known::record_live(&ctx.store, &ctx.agents, &seen, &read_at, mark)
        && let Commit::Saved { warnings, .. } = &mut result
    {
        warnings.push(format!("could not update the record of sessions: {e:#}"));
    }
    Ok(Edited::Committed(result))
}
