//! Automatic snapshots: a snapshot the OS saves from what is open in its terminal (cmux or tmux)
//! every interval (`schedule add`), listed apart from snapshots saved by hand. Only scheduled saves
//! write to it.

use anyhow::Result;

use super::Ctx;
use super::known::now;
use super::snapshots::Commit;
use crate::domain::model::{validate_name, validate_new_name};
use crate::domain::schedule::{LastRun, Schedule, Schedules, parse_interval};
use crate::domain::terminal::TerminalKind;
use crate::error::{Exit, exit_of, failure};

/// The `source.kind` of a version a schedule saved.
pub const SCHEDULED: &str = "scheduled";

/// The stored name of automatic snapshot `name` (any case), if it is one.
pub fn find(schedules: &Schedules, name: &str) -> Option<String> {
    schedules.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned()
}

fn not_scheduled(name: &str) -> anyhow::Error {
    failure(Exit::NotFound, format!("'{name}' is not an automatic snapshot (see: sessionsnap schedule list)"))
}

/// Starts saving `name` every `interval` (e.g. "10m"), then saves it once now. A name that holds
/// versions saved by hand is refused (exit 4): automatic snapshots are kept apart.
pub fn add(ctx: &Ctx, name: &str, interval: &str, keep: Option<u32>) -> Result<(Schedule, Result<Commit>)> {
    validate_new_name(name)?;
    let interval = parse_interval(interval)?;
    {
        // The snapshot's first: a save by hand holding it finds the schedule, or ends before.
        let _snapshot = ctx.store.lock(name)?;
        let lock = ctx.store.lock_schedules()?;
        let mut all = ctx.store.read_schedules()?;
        if let Some(key) = find(&all, name) {
            return Err(failure(
                Exit::Refused,
                format!("'{key}' is already an automatic snapshot; change it with: sessionsnap schedule edit {key}"),
            ));
        }
        if has_manual_versions(ctx, name)? {
            return Err(failure(
                Exit::Refused,
                format!("snapshot '{name}' has versions saved by hand; give the automatic snapshot a new name"),
            ));
        }
        let name = if ctx.store.has_versions(name) { ctx.store.stored_name(name) } else { name.to_string() };
        all.insert(
            name.clone(),
            Schedule {
                interval,
                created_at: super::known::now(),
                stopped: false,
                last_run: None,
                terminal: Some(ctx.terminal.kind()).filter(|&t| t != TerminalKind::Cmux),
                keep: Some(keep.unwrap_or(crate::domain::schedule::DEFAULT_KEEP)),
            },
        );
        ctx.scheduler.install(&name, &all[&name])?;
        if let Err(e) = ctx.store.write_schedules(&lock, &all) {
            let _ = ctx.scheduler.remove(&name);
            return Err(e);
        }
    }
    let first = run(ctx, name);
    let all = ctx.store.read_schedules()?;
    let schedule = find(&all, name).and_then(|k| all.get(&k).cloned()).ok_or_else(|| not_scheduled(name))?;
    Ok((schedule, first))
}

/// True when `name` has a version not saved by a schedule (an unreadable one counts).
fn has_manual_versions(ctx: &Ctx, name: &str) -> Result<bool> {
    if !ctx.store.has_versions(name) {
        return Ok(false);
    }
    for n in ctx.store.versions(name)? {
        match ctx.store.read_version(name, n)? {
            Ok(v) if v.source.kind == SCHEDULED => {}
            _ => return Ok(true),
        }
    }
    Ok(false)
}

/// Changes how often `name` is saved and how many versions it keeps (a stopped one stays
/// stopped); returns its stored name and schedule.
pub fn edit(ctx: &Ctx, name: &str, interval: Option<&str>, keep: Option<u32>) -> Result<(String, Schedule)> {
    validate_name(name)?;
    if interval.is_none() && keep.is_none() {
        return Err(failure(Exit::Error, "nothing to change: pass --interval, --keep or both"));
    }
    let interval = interval.map(parse_interval).transpose()?;
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    let key = find(&all, name).ok_or_else(|| not_scheduled(name))?;
    let schedule = all.get_mut(&key).expect("found above");
    if let Some(interval) = interval {
        schedule.interval = interval;
    }
    if keep.is_some() {
        schedule.keep = keep;
    }
    let schedule = schedule.clone();
    if !schedule.stopped
        && let Err(e) = ctx.scheduler.install(&key, &schedule)
    {
        // The OS dropped the old schedule too: record it stopped, so `schedule list` says so.
        all.get_mut(&key).expect("found above").stopped = true;
        ctx.store.write_schedules(&lock, &all)?;
        return Err(e.context(format!("'{key}' is stopped; start it again with: sessionsnap schedule start {key}")));
    }
    ctx.store.write_schedules(&lock, &all)?;
    Ok((key, schedule))
}

/// Stops (`run: false`) or starts again (`run: true`) the saves of `name`; it stays an automatic
/// snapshot either way. Exit 3 when it already is so.
pub fn set_running(ctx: &Ctx, name: &str, run: bool) -> Result<(String, Schedule)> {
    validate_name(name)?;
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    let key = find(&all, name).ok_or_else(|| not_scheduled(name))?;
    let schedule = all.get_mut(&key).expect("found above");
    if schedule.stopped != run {
        let state = if run { "running" } else { "stopped" };
        return Err(failure(Exit::NothingToDo, format!("'{key}' is already {state}")));
    }
    if run {
        ctx.scheduler.install(&key, &Schedule { stopped: false, ..schedule.clone() })?;
    } else {
        ctx.scheduler.remove(&key)?;
    }
    schedule.stopped = !run;
    let schedule = schedule.clone();
    ctx.store.write_schedules(&lock, &all)?;
    Ok((key, schedule))
}

/// Stops saving `name`; its versions stay, and it lists with the snapshots saved by hand.
pub fn remove(ctx: &Ctx, name: &str) -> Result<String> {
    validate_name(name)?;
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    let key = find(&all, name).ok_or_else(|| not_scheduled(name))?;
    ctx.scheduler.remove(&key)?;
    all.remove(&key);
    ctx.store.write_schedules(&lock, &all)?;
    Ok(key)
}

/// An automatic snapshot as listed: its name, schedule, and latest version (number, time).
pub type Listed = (String, Schedule, Option<(u32, String)>);

pub fn list(ctx: &Ctx) -> Result<Vec<Listed>> {
    let mut out = vec![];
    for (name, schedule) in ctx.store.read_schedules()? {
        let latest = match ctx.store.versions(&name).ok().and_then(|v| v.last().copied()) {
            Some(n) => Some((n, ctx.store.read_version(&name, n)?.map(|v| v.created_at).unwrap_or_default())),
            None => None,
        };
        out.push((name, schedule, latest));
    }
    Ok(out)
}

/// Saves automatic snapshot `name` now, as its schedule does, and records how it went. Skipped
/// (exit 3) while cmux is not running or shows no workspaces, so a cmux that is starting never
/// becomes the latest version.
pub fn run(ctx: &Ctx, name: &str) -> Result<Commit> {
    validate_name(name)?;
    let all = ctx.store.read_schedules()?;
    let key = find(&all, name).ok_or_else(|| not_scheduled(name))?;
    // It saves only from the terminal it was added in: a run picking another one (an older
    // launchd agent with no --terminal, or a run by hand inside the other) is skipped.
    let wanted = all[&key].terminal.unwrap_or_default();
    let result = if ctx.terminal.kind() == wanted {
        super::snapshots::save_scheduled(ctx, &key).map(|commit| prune_after(ctx, &key, all[&key].keep(), commit))
    } else {
        Err(failure(
            Exit::NothingToDo,
            format!("'{key}' saves from {wanted}; run it with: sessionsnap --terminal {wanted} schedule run {key}"),
        ))
    };
    let last = match &result {
        Ok(Commit::Saved { version, .. }) => LastRun::saved(now(), version.version),
        Ok(Commit::Unchanged { version, .. }) => LastRun::unchanged(now(), *version),
        Err(e) if exit_of(e) == Exit::NothingToDo => LastRun::skipped(now(), format!("{e:#}")),
        Err(e) => LastRun::failed(now(), format!("{e:#}")),
    };
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    if let Some(schedule) = all.get_mut(&key) {
        schedule.last_run = Some(last);
        ctx.store.write_schedules(&lock, &all)?;
    }
    result
}

/// Deletes all but the newest `keep` versions after a save (none when `keep` is 0). The record of
/// sessions is completed first, and nothing is deleted while an update of it is still running:
/// it may need those versions to learn a session's newest launch. A failed prune is a warning:
/// the save itself went through.
fn prune_after(ctx: &Ctx, name: &str, keep: u32, mut commit: Commit) -> Commit {
    if keep == 0 {
        return commit;
    }
    let pruned = super::known::load(&ctx.store, &ctx.agents).and_then(|_| {
        if ctx.store.pending().is_empty() {
            super::snapshots::prune(ctx, name, keep as usize).map(|_| ())
        } else {
            Ok(())
        }
    });
    if let Err(e) = pruned
        && let Commit::Saved { warnings, .. } = &mut commit
    {
        warnings.push(format!("could not delete versions older than the newest {keep}: {e:#}"));
    }
    commit
}

/// After snapshot `old` was renamed to `new`: moves its schedule, if it had one.
pub fn renamed(ctx: &Ctx, old: &str, new: &str) -> Result<()> {
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    let Some(schedule) = all.remove(old) else { return Ok(()) };
    let (interval, stopped) = (schedule.interval, schedule.stopped);
    all.insert(new.to_string(), schedule.clone());
    ctx.store.write_schedules(&lock, &all)?;
    ctx.scheduler.remove(old)?;
    if stopped {
        return Ok(());
    }
    ctx.scheduler.install(new, &schedule).map_err(|e| {
        e.context(format!(
            "'{new}' is renamed, but its schedule could not be moved; restart it with: \
             sessionsnap schedule edit {new} --interval {}",
            crate::domain::schedule::format_interval(interval)
        ))
    })
}

/// Before snapshot `name` is deleted: stops its schedule, if it has one.
pub fn deleting(ctx: &Ctx, name: &str) -> Result<()> {
    let lock = ctx.store.lock_schedules()?;
    let mut all = ctx.store.read_schedules()?;
    let Some(key) = find(&all, name) else { return Ok(()) };
    ctx.scheduler.remove(&key)?;
    all.remove(&key);
    ctx.store.write_schedules(&lock, &all)
}
