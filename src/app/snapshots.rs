//! Saving, listing, showing, comparing and deleting snapshot versions.

use anyhow::Result;

use super::Ctx;
use crate::domain::capture::{capture, carry_over, merge};
use crate::domain::diff::{self, Change};
use crate::domain::edit;
use crate::domain::model::{Source, Version, Workspace, parse_ref, validate_name};

/// What writing a version did.
#[derive(Debug)]
pub enum Commit {
    /// Nothing changed since `version`; nothing was written.
    Unchanged { version: u32 },
    /// A new version, the changes from the previous one, and problems worth showing.
    Saved { version: Box<Version>, changes: Vec<Change>, warnings: Vec<String> },
}

/// Saves what is open (all, or only the workspaces in `only`) — or a copy of `from` — as the next
/// version of `name`.
pub fn save(ctx: &Ctx, name: &str, only: &[String], from: Option<&str>, message: Option<String>) -> Result<Commit> {
    validate_name(name)?;
    let previous = if ctx.store.exists(name) { Some(ctx.store.load(name, None)?) } else { None };
    let (workspaces, source, notes) = match from {
        Some(from) => {
            let (src_name, version) = parse_ref(from)?;
            let src = ctx.store.load(&src_name, version)?;
            let source = Source {
                kind: "copy".into(),
                cmux_state_version: None,
                from: Some(format!("{}@{}", src.snapshot, src.version)),
            };
            (src.workspaces, source, vec![])
        }
        None => {
            let open = super::read_open(ctx.terminal.as_ref())?;
            let (captured, notes) = capture(&open, &ctx.agents, only)?;
            let workspaces = match &previous {
                Some(p) if !only.is_empty() => merge(&p.workspaces, captured),
                Some(p) => carry_over(&p.workspaces, captured),
                None => captured,
            };
            (workspaces, Source { kind: "live".into(), cmux_state_version: open.state_version, from: None }, notes)
        }
    };
    commit(ctx, name, previous.as_ref(), workspaces, source, message, notes)
}

/// Writes a new version unless nothing changed. Shared by save and edits. `notes` (what capture
/// left out) are shown first among the warnings.
#[allow(clippy::too_many_arguments)]
pub fn commit(
    ctx: &Ctx,
    name: &str,
    previous: Option<&Version>,
    workspaces: Vec<Workspace>,
    source: Source,
    message: Option<String>,
    notes: Vec<String>,
) -> Result<Commit> {
    if let Some(p) = previous
        && p.workspaces == workspaces
    {
        return Ok(Commit::Unchanged { version: p.version });
    }
    let changes = diff::diff(previous.map(|p| p.workspaces.as_slice()).unwrap_or(&[]), &workspaces);
    let message = message
        .unwrap_or_else(|| if previous.is_some() { diff::summary(&changes) } else { "initial save".to_string() });
    let v = ctx.store.save_new(name, &message, source, workspaces)?;
    let warnings = [notes, edit::warnings(&v.workspaces, &ctx.agents)].concat();
    Ok(Commit::Saved { version: Box::new(v), changes, warnings })
}

/// Every snapshot's name.
pub fn names(ctx: &Ctx) -> Result<Vec<String>> {
    ctx.store.list()
}

/// Every snapshot's name with its latest version.
pub fn list(ctx: &Ctx) -> Result<Vec<(String, Version)>> {
    let mut out = vec![];
    for name in ctx.store.list()? {
        let v = ctx.store.load(&name, None)?;
        out.push((name, v));
    }
    Ok(out)
}

/// A version (`<name>[@v]`) and the problems worth showing with it.
pub fn show(ctx: &Ctx, snapshot: &str) -> Result<(Version, Vec<String>)> {
    let (name, version) = parse_ref(snapshot)?;
    let v = ctx.store.load(&name, version)?;
    let warnings = edit::warnings(&v.workspaces, &ctx.agents);
    Ok((v, warnings))
}

/// All versions of `name`, oldest first.
pub fn history(ctx: &Ctx, name: &str) -> Result<Vec<Version>> {
    let mut out = vec![];
    for n in ctx.store.versions(name)? {
        out.push(ctx.store.load(name, Some(n))?);
    }
    Ok(out)
}

/// Two versions (`<name>[@v]` each) and the changes from the first to the second.
pub fn diff(ctx: &Ctx, a: &str, b: &str) -> Result<(Version, Version, Vec<Change>)> {
    let load = |r: &str| -> Result<Version> {
        let (n, v) = parse_ref(r)?;
        ctx.store.load(&n, v)
    };
    let (va, vb) = (load(a)?, load(b)?);
    let changes = diff::diff(&va.workspaces, &vb.workspaces);
    Ok((va, vb, changes))
}

pub fn version_count(ctx: &Ctx, name: &str) -> Result<usize> {
    Ok(ctx.store.versions(name)?.len())
}

/// Deletes all but the newest `keep` versions; returns the deleted version numbers.
pub fn prune(ctx: &Ctx, name: &str, keep: usize) -> Result<Vec<u32>> {
    ctx.store.prune(name, keep)
}

pub fn delete(ctx: &Ctx, name: &str) -> Result<()> {
    ctx.store.delete(name)
}
