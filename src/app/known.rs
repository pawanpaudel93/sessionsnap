//! The record of every session seen (`domain::known`), kept in the store: updated by every live
//! read of cmux (`save`, and edits that read it), and completed from every snapshot on first use,
//! after a failed update, or when it can't be read.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapters::agents::Agents;
use crate::adapters::store::Store;
use crate::adapters::store::fs::{Pending, SessionsFile};
use crate::adapters::store::lock::{Lock, STALE};
use crate::domain::known::KnownSessions;
use crate::domain::model::Workspace;
use crate::error::{Exit, failure};

/// The record of every session seen. Built from every snapshot's readable versions when there is
/// none yet; after a failed update, completed from them; one that can't be read is moved aside
/// (never overwritten) and built again.
pub fn load(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    if let SessionsFile::Read(known) = store.read_sessions()
        && to_complete(store, None).is_empty()
    {
        return Ok(known);
    }
    let lock = store.lock_sessions()?;
    current(store, agents, &lock, None)
}

/// The marks of updates the record may have missed: failed ones, and ones left by a process that
/// stopped before finishing (older than a stale lock). `own` (this process's update in progress) is
/// not one of them.
fn to_complete(store: &Store, own: Option<&Path>) -> Vec<Pending> {
    store.pending().into_iter().filter(|p| Some(p.path.as_path()) != own && (p.failed || p.age > STALE)).collect()
}

/// The time to compare versions from when completing after the failed updates read at `times`:
/// the earliest. A time that can't be read, or lies in the future (a clock that went back), can't
/// be trusted: then every version counts ("").
fn since_of(times: &[String], now: &str) -> String {
    let mut earliest: Option<&str> = None;
    for t in times {
        let valid = chrono::DateTime::parse_from_rfc3339(t).is_ok() && t.as_str() <= now;
        if !valid {
            return String::new();
        }
        earliest = Some(earliest.map_or(t.as_str(), |e| e.min(t.as_str())));
    }
    earliest.unwrap_or_default().to_string()
}

/// The record, while holding its lock: read, or repaired and written back. Completing or
/// rebuilding it from the snapshots only adds what they tell (see `KnownSessions::fill_from`).
fn current(store: &Store, agents: &Agents, lock: &Lock, own: Option<&Path>) -> Result<KnownSessions> {
    let marks = to_complete(store, own);
    let mut known = match store.read_sessions() {
        SessionsFile::Read(known) if marks.is_empty() => return Ok(known),
        SessionsFile::Read(known) => known,
        SessionsFile::Missing => KnownSessions::default(),
        SessionsFile::Bad(why) => {
            let aside = store.set_aside_sessions(lock)?;
            eprintln!(
                "sessionsnap: the record of sessions could not be read ({why}); kept it as {} and built it again from the snapshots",
                aside.display()
            );
            KnownSessions::default()
        }
    };
    let versions = saved_versions(store, agents)?;
    let now = now();
    let since = since_of(&marks.iter().map(|m| m.at.clone()).collect::<Vec<_>>(), &now);
    known.fill_from_dated(versions.iter().map(|(at, ws)| (at.as_str(), ws.as_slice())), &since, &now, agents);
    store.write_sessions(lock, &known)?;
    for m in &marks {
        store.end_pending(&m.path);
    }
    Ok(known)
}

/// Marks the record as about to learn of launches read from cmux at `at`, before the version
/// holding them is written. When even that can't be done, the save must not go ahead: the record
/// could miss a launch with nothing to tell it so.
pub fn begin(store: &Store, at: &str) -> Result<PathBuf> {
    let lock = store.lock_sessions()?;
    store.begin_pending(&lock, at).map_err(|e| {
        failure(
            Exit::Error,
            format!("could not mark the record of sessions as about to change ({e:#}); not saving, so no launch goes unrecorded"),
        )
    })
}

/// Records live launches just read from the terminal, read at `at` (see `now`): what `save`
/// captured (not the workspaces it kept from the previous version), or every session open when an
/// edit read cmux. `mark` is this update's mark (`begin`): removed when the record is updated, kept
/// as failed otherwise, so the next read completes the record rather than miss a launch.
pub fn record_live(store: &Store, agents: &Agents, captured: &[Workspace], at: &str, mark: &Path) -> Result<()> {
    let mut captured = captured.to_vec();
    crate::domain::agents::normalize_tabs(&mut captured, agents);
    let result = (|| {
        let lock = store.lock_sessions()?;
        let mut known = current(store, agents, &lock, Some(mark))?;
        if known.record_live(&captured, at) {
            store.write_sessions(&lock, &known)?;
        }
        Ok(())
    })();
    match &result {
        Ok(()) => store.end_pending(mark),
        // Left as it is if even that fails: as a stale mark, it is completed later all the same.
        Err(_) => {
            let _ = store.fail_pending(mark);
        }
    }
    result
}

/// Now, as the record stores times: UTC, RFC 3339, so they sort as text. Taken before cmux is
/// read, so a slow read never makes its launches look newer than ones read after it started.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The workspaces of every readable version of every snapshot, normalized.
/// With each version's time (UTC, to the second; `ANY_TIME` when it can't be read).
fn saved_versions(store: &Store, agents: &Agents) -> Result<Vec<(String, Vec<Workspace>)>> {
    let mut out = vec![];
    for name in store.list()? {
        for n in store.versions(&name)? {
            if let Ok(Ok(mut v)) = store.read_version(&name, n) {
                crate::domain::agents::normalize_tabs(&mut v.workspaces, agents);
                let at = chrono::DateTime::parse_from_rfc3339(&v.created_at)
                    .map(|t| t.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_else(|_| crate::domain::known::ANY_TIME.to_string());
                out.push((at, v.workspaces));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_earliest_valid_failure_counts() {
        let now = "2026-01-02T00:00:00.000Z";
        let t1 = "2026-01-01T00:00:01.000Z";
        let t2 = "2026-01-01T00:00:02.000Z";
        assert_eq!(since_of(&[t2.into(), t1.into()], now), t1);
        // A time in the future, or one that can't be read: compare every version.
        assert_eq!(since_of(&[t1.into(), "2999-01-01T00:00:00.000Z".into()], now), "");
        assert_eq!(since_of(&[t1.into(), "soon".into()], now), "");
        assert_eq!(since_of(&[], now), "");
    }
}
