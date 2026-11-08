//! Saving, listing, showing, comparing and deleting snapshot versions.

use anyhow::Result;

use super::Ctx;
use crate::adapters::store::lock::Lock;
use crate::domain::capture::{capture, carry_over, merge};
use crate::domain::diff::{self, Change};
use crate::domain::edit;
use crate::domain::model::{Source, Version, Workspace, map_folders, parse_ref, validate_name, validate_new_name};
use crate::domain::terminal::TerminalKind;
use crate::error::{Exit, failure};

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
    save_as(ctx, name, only, from, message, false)
}

/// An automatic snapshot is only saved by its schedule. Checked under the snapshot's lock, which
/// `schedule add` takes too, so a schedule added meanwhile is seen.
fn refuse_scheduled(ctx: &Ctx, name: &str) -> Result<()> {
    if let Some(key) = super::schedule::find(&ctx.store.read_schedules()?, name) {
        return Err(failure(
            Exit::Refused,
            format!(
                "'{key}' is an automatic snapshot, saved on its schedule; save it now with: sessionsnap schedule run {key}"
            ),
        ));
    }
    Ok(())
}

/// Saves `imported` (an exported version, from this machine or another) as the next version of
/// `name`, its folders moved by `maps`. It is checked like any version file when restored; only
/// what is in it comes along, never an agent's history of the session.
pub fn import(
    ctx: &Ctx,
    name: &str,
    imported: Version,
    maps: &[(String, String)],
    message: Option<String>,
) -> Result<Commit> {
    let mut workspaces = imported.workspaces;
    map_folders(&mut workspaces, maps);
    // Another machine's cmux IDs would match unrelated workspaces here.
    workspaces.iter_mut().for_each(|w| w.cmux_workspace_id = None);
    let from = format!("{}@{}", imported.snapshot, imported.version);
    let lock = ctx.store.lock(name)?;
    refuse_scheduled(ctx, name)?;
    let new = is_new(ctx, name);
    if new {
        validate_new_name(name)?;
    }
    let latest = if new { None } else { ctx.store.versions(name)?.last().copied() };
    let (previous, unreadable) = if new { (None, None) } else { newest_readable(ctx, name)? };
    let base = Base { lock: &lock, latest, previous: previous.as_ref() };
    let message = message.or_else(|| new.then(|| format!("imported {from}")));
    let source = Source { kind: "import".into(), from: Some(from), ..Default::default() };
    let force = unreadable.is_some();
    commit(ctx, name, base, workspaces, source, message, unreadable.into_iter().collect(), force)
}

/// A save of automatic snapshot `name` by its schedule (see `app::schedule::run`).
pub fn save_scheduled(ctx: &Ctx, name: &str) -> Result<Commit> {
    save_as(ctx, name, &[], None, None, true)
}

fn save_as(
    ctx: &Ctx,
    name: &str,
    only: &[String],
    from: Option<&str>,
    message: Option<String>,
    scheduled: bool,
) -> Result<Commit> {
    let lock = ctx.store.lock(name)?;
    if !scheduled {
        refuse_scheduled(ctx, name)?;
    } else if super::schedule::find(&ctx.store.read_schedules()?, name).is_none() {
        // Deleted or renamed since the run read its schedule: writing would bring the name back.
        return Err(failure(Exit::NothingToDo, format!("'{name}' is no longer an automatic snapshot")));
    }
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
                from: Some(format!("{}@{}", src.snapshot, src.version)),
                ..Default::default()
            };
            (src.workspaces, source, vec![])
        }
        None => {
            read_at = super::known::now();
            let open = super::read_open(ctx.terminal.as_ref(), &ctx.agents)?;
            let terminal = ctx.terminal.name();
            // Not running and nothing saved by it (tmux keeps no state): nothing to snapshot.
            // Not live: either the terminal isn't running (a save would be stale), or it runs but
            // doesn't answer this process (cmux's default socket mode, for a launchd job), and
            // what it wrote moments ago is the state to save.
            let running = !open.live && ctx.terminal.is_running();
            if !open.live && (open.workspaces.is_empty() || (scheduled && !running)) {
                return Err(failure(Exit::NothingToDo, format!("{terminal} is not running")));
            }
            if scheduled && open.workspaces.is_empty() {
                return Err(failure(Exit::NothingToDo, format!("{terminal} shows no workspaces")));
            }
            let (captured, mut notes) = capture(&open, &ctx.agents, only)?;
            // Notes about what is open, minus those about workspaces this save leaves alone.
            let saved = |ws: &str| only.is_empty() || captured.iter().any(|w| w.name.eq_ignore_ascii_case(ws));
            let open_notes = open.warnings.iter().filter(|w| w.workspace.as_deref().is_none_or(saved));
            notes.splice(0..0, open_notes.map(|w| w.message.clone()));
            if !open.live {
                let stale = if running {
                    format!(
                        "{terminal} didn't let sessionsnap connect (it only answers programs started inside it), so this was saved from the state it writes every few seconds"
                    )
                } else {
                    format!(
                        "{terminal} is not running: saved from what it last wrote to disk, which may be out of date"
                    )
                };
                notes.insert(0, stale);
            }
            live = Some(crate::domain::capture::seen(&open, &ctx.agents, &captured));
            let workspaces = match &previous {
                Some(p) if !only.is_empty() => merge(&p.workspaces, carry_over(&p.workspaces, captured)),
                Some(p) => carry_over(&p.workspaces, captured),
                None => captured,
            };
            let kind = if scheduled { super::schedule::SCHEDULED } else { "live" };
            let source = Source {
                kind: kind.into(),
                cmux_state_version: open.state_version,
                from: None,
                terminal: Some(ctx.terminal.kind()).filter(|&t| t != TerminalKind::Cmux),
            };
            (workspaces, source, notes)
        }
    };
    // A broken latest version is never "unchanged": the save writes one that can be read.
    let force = unreadable.is_some();
    let notes = unreadable.into_iter().chain(notes).collect();
    let base = Base { lock: &lock, latest, previous: previous.as_ref() };
    // Only what was just read from cmux is a launch seen now: never the workspaces kept from the
    // previous version, nor a copy. The record is marked before the version is written.
    let mark = if live.is_some() { Some(super::known::begin(&ctx.store, &read_at)?) } else { None };
    let mut result = match commit(ctx, name, base, workspaces, source, message, notes, force) {
        Ok(result) => result,
        Err(e) => {
            mark.iter().for_each(|m| ctx.store.end_pending(m));
            return Err(e);
        }
    };
    if let (Some(captured), Some(mark)) = (live, &mark)
        && let Err(e) = super::known::record_live(&ctx.store, &ctx.agents, &captured, &read_at, mark)
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
    if let Some(p) = previous {
        crate::domain::model::keep_layouts(&p.workspaces, &mut workspaces);
    }
    if let Some(p) = previous
        && !force
        && p.workspaces == workspaces
    {
        return Ok(Commit::Unchanged { snapshot: p.snapshot.clone(), version: p.version });
    }
    let changes = diff::diff(previous.map(|p| p.workspaces.as_slice()).unwrap_or(&[]), &workspaces);
    // A version is written only when something differs; with no tab added, removed, moved or
    // renamed, it is their order or a detail (a folder's branch, a window's layout).
    let message = message.unwrap_or_else(|| match previous {
        None => "initial save".to_string(),
        Some(_) if changes.is_empty() => "tab order or details changed".to_string(),
        Some(_) => diff::summary(&changes),
    });
    let v = ctx.store.save_locked(base.lock, name, base.latest, &message, source, workspaces)?;
    let warnings = [notes, edit::warnings(&v.workspaces, &ctx.agents)].concat();
    Ok(Commit::Saved { version: Box::new(v), changes, warnings })
}

/// Every snapshot's name.
pub fn names(ctx: &Ctx) -> Result<Vec<String>> {
    ctx.store.list()
}

/// The snapshot saved most recently (automatic ones too): what `restore` with no name restores.
pub fn most_recent(ctx: &Ctx) -> Result<String> {
    let at = |v: &Version| chrono::DateTime::parse_from_rfc3339(&v.created_at).ok();
    list(ctx)?
        .into_iter()
        .filter_map(|(name, (_, v))| Some((at(v.as_ref().ok()?)?, name)))
        .max()
        .map(|(_, name)| name)
        .ok_or_else(|| failure(Exit::NotFound, "no snapshots yet; save one with `sessionsnap save <name>`"))
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

/// Deletes snapshot `name`, then its schedule if it has one (a delete that fails keeps it
/// automatic). Under the snapshot's lock throughout, so a scheduled save waiting on it finds the
/// schedule gone and writes nothing.
pub fn delete(ctx: &Ctx, name: &str) -> Result<()> {
    let lock = ctx.store.lock(name)?;
    ctx.store.delete_locked(&lock, name)?;
    super::schedule::deleting(ctx, name)
}

/// Renames snapshot `old` to `new`, with its schedule; returns the name it had. Under both
/// snapshots' locks throughout, like `delete`.
pub fn rename(ctx: &Ctx, old: &str, new: &str) -> Result<String> {
    validate_name(old)?;
    validate_new_name(new)?;
    let locks = ctx.store.lock_rename(old, new)?;
    let from = ctx.store.rename_locked(&locks, old, new)?;
    super::schedule::renamed(ctx, &from, new)?;
    Ok(from)
}
