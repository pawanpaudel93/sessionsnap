//! Helpers shared by unit and integration tests. Not part of sessionsnap's interface.

use std::path::{Path, PathBuf};

use crate::adapters::agents::Agents;
use crate::adapters::agents::claude::Claude;
use crate::adapters::store::Store;
use crate::app::Ctx;

pub const A: &str = "11111111-1111-4111-8111-111111111111";
pub const B: &str = "22222222-2222-4222-8222-222222222222";
pub const C: &str = "33333333-3333-4333-8333-333333333333";

/// Writes a minimal interactive transcript for `id`, started in `cwd`, named `title`.
pub fn write_transcript(claude_root: &Path, cwd: &str, id: &str, title: &str) -> PathBuf {
    let dir = claude_root.join("projects").join(Claude::project_dir_name(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let lines = [
        serde_json::json!({"type": "user", "cwd": cwd, "entrypoint": "cli", "sessionId": id,
                           "message": {"content": format!("start {title}")}}),
        serde_json::json!({"type": "custom-title", "customTitle": title, "sessionId": id}),
    ];
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    path
}

/// A context on a store under `dir/store`, Claude data under `dir/claude` and `terminal`, as the
/// binary would build it for these flags.
pub fn ctx(dir: &Path, terminal: FakeTerminal, json: bool, yes: bool, input: Option<String>) -> Ctx {
    Ctx {
        store: Store::new(dir.join("store")),
        agents: Agents::new(Claude::new(dir.join("claude"))),
        terminal: Box::new(terminal),
        json,
        yes,
        interactive: false,
        input,
    }
}

/// An open workspace with ref "workspace:<id>"; tabs are (title, cwd, session).
pub fn open_ws(id: &str, name: &str, cwd: &str, tabs: &[(&str, &str, Option<&str>)]) -> crate::ports::OpenWorkspace {
    crate::ports::OpenWorkspace {
        id: id.to_string(),
        reference: Some(format!("workspace:{id}")),
        name: name.to_string(),
        cwd: cwd.to_string(),
        git_branch: None,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(i, (title, cwd, session))| crate::ports::OpenTab {
                reference: Some(format!("surface:{id}-{i}")),
                title: title.to_string(),
                cwd: cwd.to_string(),
                session_id: session.map(str::to_string),
            })
            .collect(),
    }
}

/// In-memory terminal for tests. Clones share state, so a test can inspect it after handing one over.
#[derive(Clone)]
pub struct FakeTerminal {
    pub state: std::rc::Rc<std::cell::RefCell<crate::ports::OpenState>>,
    pub log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    pub running: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Default for FakeTerminal {
    fn default() -> Self {
        FakeTerminal {
            state: std::rc::Rc::new(std::cell::RefCell::new(crate::ports::OpenState {
                live: true,
                ..Default::default()
            })),
            log: Default::default(),
            running: std::rc::Rc::new(std::cell::Cell::new(true)),
        }
    }
}

fn open_tab(t: &crate::domain::model::Tab) -> crate::ports::OpenTab {
    crate::ports::OpenTab {
        reference: None,
        title: t.title().to_string(),
        cwd: t.cwd().to_string(),
        session_id: t.session_id().map(str::to_string),
    }
}

impl crate::ports::Terminal for FakeTerminal {
    fn is_running(&self) -> bool {
        self.running.get()
    }

    fn ensure_running(&self) -> anyhow::Result<()> {
        self.running.set(true);
        Ok(())
    }

    fn read_open_state(&self) -> anyhow::Result<crate::ports::OpenState> {
        Ok(self.state.borrow().clone())
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[crate::domain::model::Tab]) -> anyhow::Result<String> {
        self.log.borrow_mut().push(format!("create {name} {}", tabs.len()));
        let mut state = self.state.borrow_mut();
        let id = format!("NEW{}", state.workspaces.len());
        let mut ws = open_ws(&id, name, cwd, &[]);
        ws.tabs = tabs.iter().map(open_tab).collect();
        state.workspaces.push(ws);
        Ok(format!("workspace:{id}"))
    }

    fn add_tab(&self, workspace_ref: &str, tab: &crate::domain::model::Tab) -> anyhow::Result<()> {
        self.log.borrow_mut().push(format!("add {workspace_ref} {}", tab.title()));
        let mut state = self.state.borrow_mut();
        let ws = state
            .workspaces
            .iter_mut()
            .find(|w| w.reference.as_deref() == Some(workspace_ref))
            .ok_or_else(|| anyhow::anyhow!("no workspace {workspace_ref}"))?;
        ws.tabs.push(open_tab(tab));
        Ok(())
    }

    fn show_url(&self, url: &str) -> anyhow::Result<&'static str> {
        self.log.borrow_mut().push(format!("show {url}"));
        Ok("dock")
    }
}

/// What `write_transcript` records, as a history entry: named `title`, first prompt "start <title>".
pub fn session_info(id: &str, cwd: &str, title: &str) -> crate::ports::SessionInfo {
    crate::ports::SessionInfo {
        session_id: id.to_string(),
        cwd: Some(cwd.to_string()),
        title: Some(title.to_string()),
        first_prompt: Some(format!("start {title}")),
        entrypoint: Some("cli".into()),
        modified: String::new(),
        modified_at: std::time::SystemTime::UNIX_EPOCH,
        path: PathBuf::new(),
    }
}

/// In-memory session history for tests: sessions are found by ID in any folder.
#[derive(Default)]
pub struct FakeHistory {
    pub sessions: Vec<crate::ports::SessionInfo>,
}

impl FakeHistory {
    /// Sessions (id, cwd, title) as `write_transcript` would record them.
    pub fn with(sessions: &[(&str, &str, &str)]) -> Self {
        FakeHistory { sessions: sessions.iter().map(|(id, cwd, title)| session_info(id, cwd, title)).collect() }
    }
}

impl crate::ports::SessionHistory for FakeHistory {
    fn contains(&self, session_id: &str) -> bool {
        self.sessions.iter().any(|s| s.session_id == session_id)
    }

    fn find(&self, session_id: &str) -> anyhow::Result<Option<crate::ports::SessionInfo>> {
        Ok(self.sessions.iter().find(|s| s.session_id == session_id).cloned())
    }

    fn title_for(&self, _cwd: &str, session_id: &str) -> Option<String> {
        let s = self.sessions.iter().find(|s| s.session_id == session_id)?;
        s.title.clone().or_else(|| s.first_prompt.clone())
    }

    fn scan(&self, _within: Option<std::time::Duration>) -> anyhow::Result<Vec<crate::ports::SessionInfo>> {
        Ok(self.sessions.clone())
    }
}

/// In-memory saved snapshots for tests.
#[derive(Default)]
pub struct FakeSnapshots {
    pub versions: Vec<crate::domain::model::Version>,
}

impl crate::ports::Snapshots for FakeSnapshots {
    fn load(&self, name: &str, version: Option<u32>) -> anyhow::Result<crate::domain::model::Version> {
        self.versions
            .iter()
            .filter(|v| v.snapshot == name && version.is_none_or(|n| v.version == n))
            .max_by_key(|v| v.version)
            .cloned()
            .ok_or_else(|| crate::error::failure(crate::error::Exit::NotFound, format!("snapshot '{name}' not found")))
    }
}
