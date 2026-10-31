//! Where scheduled saves run from: launchd on macOS, systemd user timers on Linux. Elsewhere
//! scheduling is refused.
//!
//! Layer rule: an adapter; may use `crate::ports` and `crate::domain`, never `app` or `interfaces`.

pub mod launchd;
pub mod systemd;

pub use launchd::Launchd;
pub use systemd::Systemd;

use anyhow::Result;

use crate::error::{Exit, failure};
use crate::ports::Scheduler;

/// The scheduler for this OS.
pub fn open_default() -> Result<Box<dyn Scheduler>> {
    Ok(match std::env::consts::OS {
        "macos" => Box::new(Launchd::open_default()?),
        "linux" => Box::new(Systemd::open_default()?),
        _ => Box::new(Unsupported),
    })
}

/// What a scheduled run needs from this environment (launchd and systemd start it with almost
/// none): PATH finds cmux, tmux and the agents; `SESSIONSNAP_*` point at the same store and folders
/// as the command that made the schedule; `TMUX_TMPDIR` at the tmux server's socket.
pub fn run_env() -> Vec<(String, String)> {
    std::env::vars().filter(|(k, _)| k == "PATH" || k == "TMUX_TMPDIR" || k.starts_with("SESSIONSNAP_")).collect()
}

/// Refuses to schedule: sessionsnap can't save on this OS yet.
pub struct Unsupported;

impl Scheduler for Unsupported {
    fn install(&self, _name: &str, _schedule: &crate::domain::schedule::Schedule) -> Result<()> {
        Err(failure(Exit::Refused, "scheduled saves need launchd (macOS) or systemd (Linux)"))
    }

    fn remove(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}
