//! Coding agents whose sessions sessionsnap saves and resumes: the registry of known agents and
//! the session histories of the fully supported ones (Claude Code, Codex).
//!
//! Layer rule: an adapter; implements `crate::ports::{AgentCatalog, SessionHistory}`, never depends
//! on `app` or `interfaces`.

pub mod claude;
pub mod codex;
#[cfg(test)]
mod contract;
pub mod registry;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::ports::{AgentCatalog, SessionHistory};
use claude::Claude;
use codex::Codex;
use registry::AgentSpec;

/// The agents sessionsnap knows (the registry), with the session histories it can read and the
/// folders it looks for their programs in.
pub struct Agents {
    histories: Vec<Box<dyn SessionHistory>>,
    search_path: Vec<PathBuf>,
}

impl Agents {
    /// `histories` in registry order; `search_path` is where programs are looked for.
    pub fn new(histories: Vec<Box<dyn SessionHistory>>, search_path: Vec<PathBuf>) -> Self {
        Agents { histories, search_path }
    }

    /// Claude Code's and Codex's histories at their default places (see `Claude::open_default`,
    /// `Codex::open_default`), and `$PATH`.
    pub fn open_default() -> Result<Self> {
        let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        Ok(Agents::new(vec![Box::new(Claude::open_default()?), Box::new(Codex::open_default()?)], path))
    }

    /// The registry entry for an agent name, alias or program.
    pub fn spec(name: &str) -> Option<&'static AgentSpec> {
        registry::find(name)
    }

    /// True when `program` is an executable file in one of the search folders.
    pub fn on_path(&self, program: &str) -> bool {
        self.search_path.iter().any(|dir| is_executable(&dir.join(program)))
    }

    /// The command that resumes session `session_id` of agent `name` with the saved flags, quoted
    /// for the shell; None for unknown agents and invalid IDs.
    pub fn resume_command(&self, name: &str, session_id: &str, args: &[String]) -> Option<String> {
        Agents::spec(name)?.resume_command(session_id, args)
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

impl AgentCatalog for Agents {
    fn kind_of(&self, name: &str) -> Option<&'static str> {
        Agents::spec(name).map(|s| s.kind)
    }

    fn display_name(&self, name: &str) -> String {
        Agents::spec(name).map(|s| s.display.to_string()).unwrap_or_else(|| name.to_string())
    }

    fn valid_id(&self, kind: &str, session_id: &str) -> bool {
        Agents::spec(kind).is_some_and(|s| s.valid_id(session_id))
    }

    fn history(&self, kind: &str) -> Option<&dyn SessionHistory> {
        let kind = self.kind_of(kind)?;
        self.histories.iter().find(|h| h.agent() == kind).map(|h| h.as_ref())
    }

    fn histories(&self) -> Vec<&dyn SessionHistory> {
        self.histories.iter().map(|h| h.as_ref()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_the_search_path() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("codex");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::write(d.path().join("claude"), "not executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let agents = Agents::new(vec![], vec![d.path().to_path_buf()]);
        assert!(agents.on_path("codex"));
        assert!(!agents.on_path("claude"));
        assert!(!agents.on_path("droid"));
    }
}
