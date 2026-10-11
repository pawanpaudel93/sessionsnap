//! Ports: the traits `domain` and `app` depend on, and the data that crosses them. Adapters
//! implement them; tests implement them with fakes (`crate::testing`).
//!
//! Layer rule: ports may use `crate::domain` types and `crate::error`, nothing else in the crate.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use serde::Serialize;

use crate::domain::model::{Tab, Version};

/// What is open in the terminal.
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

/// What sessionsnap needs from a terminal. One implementation today:
/// `adapters::terminal::cmux::cli::CmuxCli`.
pub trait Terminal {
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

/// One agent session as its history records it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: Option<String>,
    /// The session's name (`/rename`, `claude --name`), else Claude's automatic title.
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    /// "cli" for interactive sessions; other values for `claude -p` and SDK runs.
    pub entrypoint: Option<String>,
    /// Last change, RFC 3339 local time.
    pub modified: String,
    #[serde(skip)]
    pub modified_at: SystemTime,
    #[serde(skip)]
    pub path: PathBuf,
}

impl SessionInfo {
    /// Best label: title, then first prompt, then the ID.
    pub fn label(&self) -> String {
        self.title.clone().or_else(|| self.first_prompt.clone()).unwrap_or_else(|| self.session_id.clone())
    }
}

/// What the domain needs from an agent's session history. One implementation today:
/// `adapters::agents::claude::Claude`.
pub trait SessionHistory {
    /// True when the history has session `session_id` (in any folder).
    fn contains(&self, session_id: &str) -> bool;
    /// The session's details; `Ok(None)` when the history does not have it.
    fn find(&self, session_id: &str) -> Result<Option<SessionInfo>>;
    /// The title for a session last seen in `cwd`: its name, else its first prompt.
    fn title_for(&self, cwd: &str, session_id: &str) -> Option<String>;
    /// Sessions changed within `within` (all when None), newest first.
    fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>>;
}

/// Read access to saved snapshots, for edits that copy from another snapshot.
pub trait Snapshots {
    /// Loads `version` of `name`, or the latest when `version` is None.
    fn load(&self, name: &str, version: Option<u32>) -> Result<Version>;
}
