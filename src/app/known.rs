//! The record of every session seen (`domain::known`), kept in the store: built from every
//! snapshot on first use, and updated only by `save`'s live captures.

use anyhow::Result;

use crate::adapters::agents::Agents;
use crate::adapters::store::Store;
use crate::domain::known::KnownSessions;
use crate::domain::model::Workspace;

/// The record, built from every snapshot's readable versions when there is none yet.
pub fn load(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    if let Some(known) = store.read_sessions()? {
        return Ok(known);
    }
    let lock = store.lock_sessions()?;
    if let Some(known) = store.read_sessions()? {
        return Ok(known);
    }
    let known = seed(store, agents)?;
    store.write_sessions(&lock, &known)?;
    Ok(known)
}

/// Records the live launches `save` just captured from the terminal (only those: not workspaces it
/// kept from the previous version). When that fails, the record is removed, so the next read
/// builds it again from the snapshots rather than miss a launch.
pub fn record_live(store: &Store, agents: &Agents, captured: &[Workspace]) -> Result<()> {
    let mut captured = captured.to_vec();
    crate::domain::agents::normalize_tabs(&mut captured, agents);
    let at = utc_now();
    let result = (|| {
        let lock = store.lock_sessions()?;
        let (mut known, seeded) = match store.read_sessions()? {
            Some(known) => (known, false),
            None => (seed(store, agents)?, true),
        };
        if known.record_live(&captured, &at) || seeded {
            store.write_sessions(&lock, &known)?;
        }
        Ok(())
    })();
    if result.is_err() {
        store.forget_sessions();
    }
    result
}

/// Now, as the record stores times: UTC, RFC 3339, so they sort as text.
fn utc_now() -> String {
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
