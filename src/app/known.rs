//! The record of every session seen (`domain::known`), kept in the store: updated by every live
//! read of cmux (`save`, and edits that read it), and completed from every snapshot on first use,
//! after a failed update, or when it can't be read.

use anyhow::Result;

use crate::adapters::agents::Agents;
use crate::adapters::store::Store;
use crate::adapters::store::fs::SessionsFile;
use crate::adapters::store::lock::Lock;
use crate::domain::known::KnownSessions;
use crate::domain::model::Workspace;

/// The record of every session seen. Built from every snapshot's readable versions when there is
/// none yet; when an update failed, sessions it lacks are added from them; one that can't be read
/// is moved aside (never overwritten) and built again.
pub fn load(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    if let SessionsFile::Read(known) = store.read_sessions()
        && store.sessions_dirty().is_none()
    {
        return Ok(known);
    }
    let lock = store.lock_sessions()?;
    current(store, agents, &lock)
}

/// The record, while holding its lock: read, or repaired and written back. Completing or
/// rebuilding it from the snapshots only adds sessions it lacks (see `KnownSessions::fill_from`).
fn current(store: &Store, agents: &Agents, lock: &Lock) -> Result<KnownSessions> {
    let dirty = store.sessions_dirty();
    let mut known = match store.read_sessions() {
        SessionsFile::Read(known) if dirty.is_none() => return Ok(known),
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
    let since = dirty.unwrap_or_default();
    known.fill_from_dated(versions.iter().map(|(at, ws)| (at.as_str(), ws.as_slice())), &since, &now(), agents);
    store.write_sessions(lock, &known)?;
    store.clear_sessions_dirty(lock);
    Ok(known)
}

/// Records live launches just read from the terminal, read at `at` (see `now`): what `save`
/// captured (not the workspaces it kept from the previous version), or every session open when an
/// edit read cmux. When that fails, the record is kept and marked, so the next
/// read adds what the snapshots tell rather than miss a launch.
pub fn record_live(store: &Store, agents: &Agents, captured: &[Workspace], at: &str) -> Result<()> {
    let mut captured = captured.to_vec();
    crate::domain::agents::normalize_tabs(&mut captured, agents);
    let result = (|| {
        let lock = store.lock_sessions()?;
        let mut known = current(store, agents, &lock)?;
        if known.record_live(&captured, at) {
            store.write_sessions(&lock, &known)?;
        }
        Ok(())
    })();
    if result.is_err() {
        store.mark_sessions_dirty(at);
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
