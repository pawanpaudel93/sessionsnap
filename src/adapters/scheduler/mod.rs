//! Where scheduled saves run from: launchd on macOS. Elsewhere scheduling is refused.
//!
//! Layer rule: an adapter; may use `crate::ports` and `crate::domain`, never `app` or `interfaces`.

pub mod launchd;

pub use launchd::Launchd;

use anyhow::Result;

use crate::error::{Exit, failure};
use crate::ports::Scheduler;

/// The scheduler for this OS.
pub fn open_default() -> Result<Box<dyn Scheduler>> {
    if std::env::consts::OS == "macos" { Ok(Box::new(Launchd::open_default()?)) } else { Ok(Box::new(Unsupported)) }
}

/// Refuses to schedule: sessionsnap can't save on this OS yet.
pub struct Unsupported;

impl Scheduler for Unsupported {
    fn install(&self, _name: &str, _interval: u64) -> Result<()> {
        Err(failure(Exit::Refused, "scheduled saves need macOS (launchd)"))
    }

    fn remove(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}
