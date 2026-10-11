pub mod cli;
pub mod palette;
pub mod runner;
pub mod state_file;
pub mod tree;

use std::collections::HashSet;

use anyhow::Result;

use crate::core::model::Tab;

/// What is open in cmux.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenState {
    /// False when read from the state file only (cmux not running): refs are then None.
    pub live: bool,
    pub state_version: Option<u64>,
    pub workspaces: Vec<OpenWorkspace>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenWorkspace {
    /// cmux's workspace UUID (`workspaceId` in the state file, `id` in the tree).
    pub id: String,
    /// Live ref such as "workspace:3".
    pub reference: Option<String>,
    pub name: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub tabs: Vec<OpenTab>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenTab {
    /// Live ref such as "surface:12".
    pub reference: Option<String>,
    pub title: String,
    pub cwd: String,
    pub session_id: Option<String>,
}

impl OpenState {
    pub fn open_sessions(&self) -> HashSet<String> {
        self.workspaces.iter().flat_map(|w| &w.tabs).filter_map(|t| t.session_id.clone()).collect()
    }
}

/// What sessionsnap needs from a terminal. v1 has one implementation, `cli::CmuxCli`.
pub trait Backend {
    /// True when cmux answers on its socket.
    fn is_running(&self) -> bool;
    /// Starts cmux if needed and waits until it answers.
    fn ensure_running(&self) -> Result<()>;
    /// Open workspaces and tabs: live when cmux runs, else from its state file only.
    fn read_open_state(&self) -> Result<OpenState>;
    /// Creates a workspace with these tabs; returns its ref (e.g. "workspace:12").
    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[Tab]) -> Result<String>;
    /// Adds one tab to an open workspace.
    fn add_tab(&self, workspace_ref: &str, tab: &Tab) -> Result<()>;
    /// Shows `url` in cmux: points a browser already on its address (ignoring the `#` part)
    /// at it ("reused"), else adds a Dock browser tab ("dock"), else, with the Dock off, opens a
    /// browser tab ("tab").
    fn show_url(&self, url: &str) -> Result<&'static str>;
}

/// Strips status glyphs cmux and Claude put before titles ("✳ fix login" → "fix login").
pub fn clean_title(s: &str) -> String {
    let t = s.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    if t.is_empty() { s.trim().to_string() } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_title_strips_status_glyphs() {
        assert_eq!(clean_title("✳ fix login"), "fix login");
        assert_eq!(clean_title("⠂ Cascadia"), "Cascadia");
        assert_eq!(clean_title("✳"), "✳");
    }
}
