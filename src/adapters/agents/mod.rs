//! Coding agents whose sessions sessionsnap saves and resumes. Today: Claude Code.
//!
//! Layer rule: an adapter; implements `crate::ports::SessionHistory`, never depends on `app` or
//! `interfaces`.

pub mod claude;
