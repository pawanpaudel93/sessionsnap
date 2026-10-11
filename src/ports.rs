//! Ports: the traits `domain` and `app` depend on, and the data that crosses them. Adapters
//! implement them; tests implement them with fakes (`crate::testing`).
//!
//! Layer rule: ports may use `crate::domain` types and `crate::error`, nothing else in the crate.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::domain::model::{SessionRef, Version};
use anyhow::Result;

/// What is open in the terminal.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenState {
    /// False when the terminal is not running and this was read from what it last saved: refs are
    /// then None.
    pub live: bool,
    pub state_version: Option<u64>,
    pub workspaces: Vec<OpenWorkspace>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenWorkspace {
    /// The terminal's stable ID for the workspace.
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
    /// The agent running in the tab, when the terminal knows one.
    pub agent: Option<AgentRef>,
}

/// A coding agent's session running in a terminal tab, as the terminal reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRef {
    /// The agent's kind as the terminal names it (e.g. "claude", "codex").
    pub kind: String,
    pub session_id: String,
    /// The flags the agent was started with (without the program).
    pub args: Vec<String>,
    /// Another ID the terminal recorded for this session (cmux's resume binding), used when
    /// `session_id` is not a valid ID of the agent.
    pub alt_session_id: Option<String>,
}

impl OpenTab {
    pub fn session(&self) -> Option<SessionRef> {
        self.agent.as_ref().map(|a| SessionRef::new(&a.kind, &a.session_id))
    }
}

impl OpenState {
    /// Every agent session open in a tab.
    pub fn open_sessions(&self) -> HashSet<SessionRef> {
        self.workspaces.iter().flat_map(|w| &w.tabs).filter_map(OpenTab::session).collect()
    }
}

/// A tab for the terminal to open. `command` is a complete command line, already quoted for the
/// shell; the terminal runs it as is and never builds one itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchTab {
    pub title: String,
    pub cwd: String,
    pub command: Option<String>,
}

/// What sessionsnap needs from a terminal: read what is open, open workspaces and tabs. One
/// implementation today:
/// `adapters::terminal::cmux::cli::CmuxCli`.
pub trait Terminal {
    /// True when the terminal is running and answers.
    fn is_running(&self) -> bool;
    /// Starts the terminal if needed and waits until it answers.
    fn ensure_running(&self) -> Result<()>;
    /// Open workspaces and tabs: live when the terminal runs, else from what it last saved.
    fn read_open_state(&self) -> Result<OpenState>;
    /// Creates a workspace with these tabs; returns its ref (e.g. "workspace:12").
    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[LaunchTab]) -> Result<String>;
    /// Adds one tab to an open workspace.
    fn add_tab(&self, workspace_ref: &str, tab: &LaunchTab) -> Result<()>;
    /// Shows `url` in the terminal: points a browser already on its address (ignoring the `#` part)
    /// at it ("reused"), else adds a Dock browser tab ("dock"), else, with the Dock off, opens a
    /// browser tab ("tab").
    fn show_url(&self, url: &str) -> Result<&'static str>;
}

/// One agent session as its history records it. The same fields for every agent.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// The agent's kind, e.g. "claude".
    pub agent: String,
    pub session_id: String,
    /// The folder the session ran in.
    pub cwd: Option<String>,
    /// The session's name or title, when the agent records one.
    pub title: Option<String>,
    /// The first prompt a person typed.
    pub first_prompt: Option<String>,
    /// Last change, RFC 3339 local time.
    pub modified: String,
    pub modified_at: SystemTime,
    /// A session a person ran and may want back, as the agent's history judges it; false for
    /// one-shot and scripted runs and the agent's own internal sessions.
    pub recoverable: bool,
}

impl SessionInfo {
    /// Best label: title, then first prompt, then the ID.
    pub fn label(&self) -> String {
        self.title.clone().or_else(|| self.first_prompt.clone()).unwrap_or_else(|| self.session_id.clone())
    }
}

/// What the domain needs from one agent's session history. Implementations live in
/// `adapters::agents`.
pub trait SessionHistory {
    /// The agent's kind in the registry, e.g. "claude".
    fn agent(&self) -> &'static str;
    /// True when the history has session `session_id` (in any folder).
    fn contains(&self, session_id: &str) -> bool {
        self.find(session_id).ok().flatten().is_some()
    }
    /// The session's details; `Ok(None)` when the history does not have it.
    fn find(&self, session_id: &str) -> Result<Option<SessionInfo>>;
    /// The title for a session last seen in `cwd`: its name, else its first prompt.
    fn title_for(&self, cwd: &str, session_id: &str) -> Option<String>;
    /// Sessions changed within `within` (all when None), newest first.
    fn scan(&self, within: Option<Duration>) -> Result<Vec<SessionInfo>>;
}

/// What the domain needs to know about coding agents: their names, what their session IDs look
/// like, and the session histories sessionsnap can read. Implemented by `adapters::agents::Agents`.
pub trait AgentCatalog {
    /// The kind of the agent with this name, alias or program name; None when unknown.
    fn kind_of(&self, name: &str) -> Option<&'static str>;
    /// A name for people, e.g. "Codex"; the name itself for unknown agents.
    fn display_name(&self, name: &str) -> String;
    /// True when `session_id` is a session ID of agent `kind` (false for unknown agents).
    fn valid_id(&self, kind: &str, session_id: &str) -> bool;
    /// The session history of agent `kind`, for agents sessionsnap fully supports.
    fn history(&self, kind: &str) -> Option<&dyn SessionHistory>;
    /// Every session history, in registry order.
    fn histories(&self) -> Vec<&dyn SessionHistory>;
}

/// Read access to saved snapshots, for edits that copy from another snapshot.
pub trait Snapshots {
    /// Loads `version` of `name`, or the latest when `version` is None.
    fn load(&self, name: &str, version: Option<u32>) -> Result<Version>;
}

/// Where released builds come from, and the few file steps an update needs (download, checksum,
/// unpack, ask a binary its version). Implemented by `adapters::releases::GitHubReleases`.
pub trait Releases {
    /// The latest release's tag (e.g. "v0.2.0"); `Ok(None)` when none is published or it can't be
    /// seen (a private repo looks the same).
    fn latest_tag(&self) -> Result<Option<String>>;
    /// Downloads `url` to `dest`.
    fn download(&self, url: &str, dest: &Path) -> Result<()>;
    /// The file's SHA-256, lowercase hex.
    fn sha256(&self, path: &Path) -> Result<String>;
    /// Unpacks a .tar.gz into the folder `into`.
    fn extract(&self, archive: &Path, into: &Path) -> Result<()>;
    /// What `<binary> --version` prints, trimmed.
    fn version_of(&self, binary: &Path) -> Result<String>;
}
