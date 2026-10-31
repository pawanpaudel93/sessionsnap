//! Pure domain logic: snapshots, diffs, edits, restore plans, capture, which saved flags are kept
//! and which agent sessions can be resumed.
//!
//! Layer rule: nothing here touches files, processes, the environment, cmux or an agent's data
//! directory. The domain may depend on `crate::ports` (and `crate::error`), never on `adapters`, `app`
//! or `interfaces`. What the domain needs from the outside world comes in through a port trait.

pub mod agents;
pub mod capture;
pub mod diff;
pub mod edit;
pub mod flags;
pub mod known;
pub mod model;
pub mod plan;
pub mod schedule;
