//! Coding agents whose sessions sessionsnap saves and resumes. Today: Claude Code.
//!
//! Layer rule: an adapter; implements `crate::ports::SessionHistory`, never depends on `app` or
//! `interfaces`.

pub mod claude;
pub mod codex;
#[cfg(test)]
mod contract;
pub mod registry;

use anyhow::Result;

use crate::ports::SessionHistory;
use claude::Claude;

/// The agents sessionsnap knows, with the session histories it can read.
pub struct Agents {
    claude: Claude,
}

impl Agents {
    pub fn new(claude: Claude) -> Self {
        Agents { claude }
    }

    /// Each agent's history at its default place (overridable for tests, see each agent).
    pub fn open_default() -> Result<Self> {
        Ok(Agents::new(Claude::open_default()?))
    }

    /// Claude Code's session history.
    pub fn history(&self) -> &dyn SessionHistory {
        &self.claude
    }
}
