//! Editing a saved snapshot: a list of edit operations applied as one new version.

use anyhow::Result;

use super::Ctx;
use super::snapshots::{Commit, commit};
use crate::domain::diff::{self, Change};
use crate::domain::edit::{self, EditCtx, Op};
use crate::domain::model::{Source, Workspace};

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

/// Applies `ops` to the latest version of `name` and saves the result (unless `dry_run`).
pub fn edit(ctx: &Ctx, name: &str, ops: Vec<Op>, message: Option<String>, dry_run: bool) -> Result<Edited> {
    let previous = super::load_version(ctx, name, None)?;
    let mut workspaces = previous.workspaces.clone();
    let open = if ops.iter().any(|o| matches!(o, Op::WsAddLive { .. })) {
        Some(super::read_open(ctx.terminal.as_ref())?)
    } else {
        None
    };
    let notes = edit::apply_ops(
        &mut workspaces,
        &ops,
        &EditCtx { agents: &ctx.agents, snapshots: &ctx.store, open: open.as_ref() },
    )?;
    if dry_run {
        let changes = diff::diff(&previous.workspaces, &workspaces);
        let warnings = [notes, edit::warnings(&workspaces, &ctx.agents)].concat();
        return Ok(Edited::DryRun { base_version: previous.version, workspaces, changes, warnings });
    }
    let source = Source { kind: "edit".into(), cmux_state_version: None, from: None };
    Ok(Edited::Committed(commit(ctx, name, Some(&previous), workspaces, source, message, notes)?))
}
