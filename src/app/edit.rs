//! Editing a saved snapshot: a list of edit operations applied as one new version.

use anyhow::Result;

use super::Ctx;
use super::snapshots::{Commit, commit};
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
/// `base`, refuses unless the latest version is that one: the ops were previewed on it.
pub fn edit(
    ctx: &Ctx,
    name: &str,
    ops: Vec<Op>,
    message: Option<String>,
    dry_run: bool,
    base: Option<u32>,
) -> Result<Edited> {
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
    let sets_session = ops.iter().any(|o| matches!(o, Op::TabAddSession { .. } | Op::TabSetSession { .. }));
    let open = if ops.iter().any(|o| matches!(o, Op::WsAddLive { .. })) {
        Some(super::read_open(ctx.terminal.as_ref())?)
    } else if sets_session {
        // Only to see whether a session put in a tab is open with flags that lose it; without
        // cmux's state the snapshot's own history still applies.
        super::read_open(ctx.terminal.as_ref()).ok()
    } else {
        None
    };
    // Sessions left out in any readable version of this snapshot.
    let left_out = if sets_session {
        let mut versions = vec![];
        for n in ctx.store.versions(name)? {
            if let Ok(mut v) = ctx.store.read_version(name, n)? {
                crate::domain::agents::normalize_tabs(&mut v.workspaces, &ctx.agents);
                versions.push(v);
            }
        }
        edit::left_out_sessions(&versions)
    } else {
        Default::default()
    };
    let notes = edit::apply_ops(
        &mut workspaces,
        &ops,
        &EditCtx {
            agents: &ctx.agents,
            snapshots: &super::NormalizedSnapshots { store: &ctx.store, agents: &ctx.agents },
            open: open.as_ref(),
            left_out,
        },
    )?;
    if dry_run {
        let changes = diff::diff(&previous.workspaces, &workspaces);
        let warnings = [notes, edit::warnings(&workspaces, &ctx.agents)].concat();
        return Ok(Edited::DryRun { base_version: previous.version, workspaces, changes, warnings });
    }
    let source = Source { kind: "edit".into(), cmux_state_version: None, from: None };
    Ok(Edited::Committed(commit(ctx, name, Some(&previous), workspaces, source, message, notes)?))
}
