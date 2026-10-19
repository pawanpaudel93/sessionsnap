//! The record of every saved session (`domain::known`), kept in the store: built from every
//! snapshot on first use, and updated by every save and edit.

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

/// Adds the agent tabs of a version just written. When that fails, the record is removed, so the
/// next read builds it again from the snapshots (which hold that version) rather than miss it.
pub fn record(store: &Store, agents: &Agents, workspaces: &[Workspace]) -> Result<()> {
    let result = (|| {
        let lock = store.lock_sessions()?;
        let (mut known, seeded) = match store.read_sessions()? {
            Some(known) => (known, false),
            None => (seed(store, agents)?, true),
        };
        if known.record(workspaces) || seeded {
            store.write_sessions(&lock, &known)?;
        }
        Ok(())
    })();
    if result.is_err() {
        store.forget_sessions();
    }
    result
}

/// Every readable version of every snapshot, oldest first.
fn seed(store: &Store, agents: &Agents) -> Result<KnownSessions> {
    let mut versions = vec![];
    for name in store.list()? {
        for n in store.versions(&name)? {
            if let Ok(Ok(mut v)) = store.read_version(&name, n) {
                crate::domain::agents::normalize_tabs(&mut v.workspaces, agents);
                versions.push(v);
            }
        }
    }
    versions.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut known = KnownSessions::default();
    for v in &versions {
        known.record(&v.workspaces);
    }
    Ok(known)
}
