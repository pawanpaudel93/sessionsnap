//! Saving, listing, showing, comparing and deleting snapshot versions.

use anyhow::Result;

use super::Ctx;
use crate::adapters::store::lock::Lock;
use crate::domain::capture::{capture, carry_over, merge};
use crate::domain::diff::{self, Change};
use crate::domain::edit;
use crate::domain::model::{Source, Version, Workspace, parse_ref, validate_name, validate_new_name};

/// What writing a version did.
#[derive(Debug)]
pub enum Commit {
    /// Nothing changed since `version` of `snapshot`; nothing was written.
    Unchanged { snapshot: String, version: u32 },
    /// A new version, the changes from the previous one, and problems worth showing.
    Saved { version: Box<Version>, changes: Vec<Change>, warnings: Vec<String> },
}

/// Saves what is open (all, or only the workspaces in `only`) — or a copy of `from` — as the next
/// version of `name`. Holds the snapshot's lock from reading its latest version to writing the
/// next, so a save or edit landing meanwhile is never undone.
pub fn save(ctx: &Ctx, name: &str, only: &[String], from: Option<&str>, message: Option<String>) -> Result<Commit> {
    validate_name(name)?;
    let lock = ctx.store.lock(name)?;
    let new = is_new(ctx, name);
    if new {
        validate_new_name(name)?;
    }
    let latest = if new { None } else { ctx.store.versions(name)?.last().copied() };
    let (previous, unreadable) = if new { (None, None) } else { newest_readable(ctx, name)? };
    let mut live = None;
    let mut read_at = String::new();
    let (workspaces, source, notes) = match from {
        Some(from) => {
            let (src_name, version) = parse_ref(from)?;
            let src = super::load_version(ctx, &src_name, version)?;
            let source = Source {
                kind: "copy".into(),
                cmux_state_version: None,
                from: Some(format!("{}@{}", src.snapshot, src.version)),
            };
            (src.workspaces, source, vec![])
        }
        None => {
            read_at = super::known::now();
            let open = super::read_open(ctx.terminal.as_ref())?;
            let (captured, notes) = capture(&open, &ctx.agents, only)?;
            live = Some(captured.clone());
            let workspaces = match &previous {
                Some(p) if !only.is_empty() => merge(&p.workspaces, captured),
                Some(p) => carry_over(&p.workspaces, captured),
                None => captured,
            };
            (workspaces, Source { kind: "live".into(), cmux_state_version: open.state_version, from: None }, notes)
        }
    };
    // A broken latest version is never "unchanged": the save writes one that can be read.
    let force = unreadable.is_some();
    let notes = unreadable.into_iter().chain(notes).collect();
    let base = Base { lock: &lock, latest, previous: previous.as_ref() };
    let mut result = commit(ctx, name, base, workspaces, source, message, notes, force)?;
    // Only what was just read from cmux is a launch seen now: never the workspaces kept from the
    // previous version, nor a copy.
    if let Some(captured) = live
        && let Err(e) = super::known::record_live(&ctx.store, &ctx.agents, &captured, &read_at)
        && let Commit::Saved { warnings, .. } = &mut result
    {
        warnings.push(format!("could not update the record of sessions: {e:#}"));
    }
    Ok(result)
}

/// What a new version is written on: the snapshot's lock, held since its latest version was read;
/// that latest version's number (None: no version yet); and the version it is compared with.
pub struct Base<'a> {
    pub lock: &'a Lock,
    pub latest: Option<u32>,
    pub previous: Option<&'a Version>,
}

/// True when saving to `name` starts a new snapshot: it has no version yet (an empty folder left
/// behind doesn't count).
pub fn is_new(ctx: &Ctx, name: &str) -> bool {
    !ctx.store.has_versions(name)
}

/// The newest version of `name` that can be read, and, when the latest one can't be, a note that
/// says so: a broken latest version (edited by hand, a crash) must not stop later saves.
fn newest_readable(ctx: &Ctx, name: &str) -> Result<(Option<Version>, Option<String>)> {
    let versions = ctx.store.versions(name)?;
    let Some(&latest) = versions.last() else { return Ok((None, None)) };
    // A file that can't even be opened counts as unreadable too.
    let read = |n| read_normalized(ctx, name, n).unwrap_or_else(|e| Err(e.to_string()));
    let why = match read(latest) {
        Ok(v) => return Ok((Some(v), None)),
        Err(why) => why,
    };
    for &n in versions.iter().rev().skip(1) {
        if let Ok(v) = read(n) {
            return Ok((Some(v), Some(format!("v{latest} is unreadable ({why}); compared with v{n}"))));
        }
    }
    Ok((None, Some(format!("v{latest} is unreadable ({why}), and no older version can be read"))))
}

/// Writes a new version unless nothing changed (or `force`). Shared by save and edits. `notes`
/// (what capture left out) are shown first among the warnings.
#[allow(clippy::too_many_arguments)]
pub fn commit(
    ctx: &Ctx,
    name: &str,
    base: Base,
    workspaces: Vec<Workspace>,
    source: Source,
    message: Option<String>,
    notes: Vec<String>,
    force: bool,
) -> Result<Commit> {
    let previous = base.previous;
    let mut workspaces = workspaces;
    crate::domain::agents::normalize_tabs(&mut workspaces, &ctx.agents);
    if let Some(p) = previous
        && !force
        && p.workspaces == workspaces
    {
        return Ok(Commit::Unchanged { snapshot: p.snapshot.clone(), version: p.version });
    }
    let changes = diff::diff(previous.map(|p| p.workspaces.as_slice()).unwrap_or(&[]), &workspaces);
    let message = message
        .unwrap_or_else(|| if previous.is_some() { diff::summary(&changes) } else { "initial save".to_string() });
    let v = ctx.store.save_locked(base.lock, name, base.latest, &message, source, workspaces)?;
    let warnings = [notes, edit::warnings(&v.workspaces, &ctx.agents)].concat();
    Ok(Commit::Saved { version: Box::new(v), changes, warnings })
}

/// Every snapshot's name.
pub fn names(ctx: &Ctx) -> Result<Vec<String>> {
    ctx.store.list()
}

/// A version as listed: its number, and its contents or why its file cannot be read.
pub type Listed = (u32, std::result::Result<Version, String>);

/// Every snapshot's name with its latest version. A version that cannot be read (another format,
/// broken JSON) is listed with the reason instead of failing the whole list.
pub fn list(ctx: &Ctx) -> Result<Vec<(String, Listed)>> {
    let mut out = vec![];
    for name in ctx.store.list()? {
        let Some(&n) = ctx.store.versions(&name)?.last() else { continue };
        out.push((name.clone(), (n, read_normalized(ctx, &name, n)?)));
    }
    Ok(out)
}

/// A version (`<name>[@v]`) and the problems worth showing with it.
pub fn show(ctx: &Ctx, snapshot: &str) -> Result<(Version, Vec<String>)> {
    let (name, version) = parse_ref(snapshot)?;
    let v = super::load_version(ctx, &name, version)?;
    let warnings = edit::warnings(&v.workspaces, &ctx.agents);
    Ok((v, warnings))
}

/// All versions of `name`, oldest first; unreadable ones with the reason.
pub fn history(ctx: &Ctx, name: &str) -> Result<Vec<Listed>> {
    let mut out = vec![];
    for n in ctx.store.versions(name)? {
        out.push((n, read_normalized(ctx, name, n)?));
    }
    Ok(out)
}

/// `Store::read_version` with its tabs normalized, like every other read of a snapshot.
fn read_normalized(ctx: &Ctx, name: &str, n: u32) -> Result<std::result::Result<Version, String>> {
    Ok(ctx.store.read_version(name, n)?.map(|mut v| {
        crate::domain::agents::normalize_tabs(&mut v.workspaces, &ctx.agents);
        v
    }))
}

/// Two versions (`<name>[@v]` each) and the changes from the first to the second.
pub fn diff(ctx: &Ctx, a: &str, b: &str) -> Result<(Version, Version, Vec<Change>)> {
    let load = |r: &str| -> Result<Version> {
        let (n, v) = parse_ref(r)?;
        super::load_version(ctx, &n, v)
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
