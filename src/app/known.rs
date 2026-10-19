//! The record of every session seen (`domain::known`), kept in the store: built from every
//! snapshot on first use, and updated only by `save`'s live captures.

use anyhow::Result;

use crate::adapters::agents::Agents;
use crate::adapters::store::Store;
use crate::adapters::store::fs::SessionsFile;
use crate::adapters::store::lock::Lock;
use crate::domain::known::KnownSessions;
use crate::domain::model::Workspace;

/// The record of every session seen. Built from every snapshot's readable versions when there is
/// none yet; when an update failed, what the snapshots tell is added to it; one that can't be read
/// is moved aside (never overwritten) and built again.
pub fn load(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    if let SessionsFile::Read(known) = store.read_sessions()
        && !store.sessions_dirty()
    {
        return Ok(known);
    }
    let lock = store.lock_sessions()?;
    current(store, agents, &lock)
}

/// The record, while holding its lock: read, or repaired and written back.
fn current(store: &Store, agents: &Agents, lock: &Lock) -> Result<KnownSessions> {
    let dirty = store.sessions_dirty();
    let known = match store.read_sessions() {
        SessionsFile::Read(known) if !dirty => return Ok(known),
        SessionsFile::Read(mut known) => {
            known.merge(seed(store, agents)?);
            known
        }
        SessionsFile::Missing => seed(store, agents)?,
        SessionsFile::Bad(why) => {
            let aside = store.set_aside_sessions(lock)?;
            eprintln!(
                "sessionsnap: the record of sessions could not be read ({why}); kept it as {} and built it again from the snapshots",
                aside.display()
            );
            seed(store, agents)?
        }
    };
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
        store.mark_sessions_dirty();
    }
    result
}

/// Now, as the record stores times: UTC, RFC 3339, so they sort as text. Taken before cmux is
/// read, so a slow read never makes its launches look newer than ones read after it started.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A version's `createdAt` (local time with offset) as the record stores times.
fn utc_of(created_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|t| t.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// The record as the snapshots tell it: the launches of every readable live save, oldest first,
/// and every session left out in any version.
fn seed(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    let mut versions = vec![];
    for name in store.list()? {
        for n in store.versions(&name)? {
            if let Ok(Ok(mut v)) = store.read_version(&name, n) {
                crate::domain::agents::normalize_tabs(&mut v.workspaces, agents);
                versions.push((utc_of(&v.created_at), v));
            }
        }
    }
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    let mut known = KnownSessions::default();
    for (at, v) in &versions {
        // A live save may also hold workspaces it kept from before (`save -w`); taking those as
        // seen then can only happen here, once, for data from before the record existed.
        if v.source.kind == "live" {
            known.record_live(&v.workspaces, at);
        }
        known.record_left_out(&v.workspaces);
    }
    Ok(known)
}
