//! Helpers shared by unit and integration tests. Not part of ccsnap's interface.

use std::path::{Path, PathBuf};

use crate::claude::Claude;

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

/// An open workspace with ref "workspace:<id>"; tabs are (title, cwd, session).
pub fn open_ws(id: &str, name: &str, cwd: &str, tabs: &[(&str, &str, Option<&str>)]) -> crate::cmux::OpenWorkspace {
    crate::cmux::OpenWorkspace {
        id: id.to_string(),
        reference: Some(format!("workspace:{id}")),
        name: name.to_string(),
        cwd: cwd.to_string(),
        git_branch: None,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(i, (title, cwd, session))| crate::cmux::OpenTab {
                reference: Some(format!("surface:{id}-{i}")),
                title: title.to_string(),
                cwd: cwd.to_string(),
                session_id: session.map(str::to_string),
            })
            .collect(),
    }
}

/// In-memory cmux for tests. Clones share state, so a test can inspect it after handing one over.
#[derive(Clone)]
pub struct FakeBackend {
    pub state: std::rc::Rc<std::cell::RefCell<crate::cmux::OpenState>>,
    pub log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    pub running: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        FakeBackend {
            state: std::rc::Rc::new(std::cell::RefCell::new(crate::cmux::OpenState {
                live: true,
                ..Default::default()
            })),
            log: Default::default(),
            running: std::rc::Rc::new(std::cell::Cell::new(true)),
        }
    }
}

fn open_tab(t: &crate::model::Tab) -> crate::cmux::OpenTab {
    crate::cmux::OpenTab {
        reference: None,
        title: t.title().to_string(),
        cwd: t.cwd().to_string(),
        session_id: t.session_id().map(str::to_string),
    }
}

impl crate::cmux::Backend for FakeBackend {
    fn is_running(&self) -> bool {
        self.running.get()
    }

    fn ensure_running(&self) -> anyhow::Result<()> {
        self.running.set(true);
        Ok(())
    }

    fn read_open_state(&self) -> anyhow::Result<crate::cmux::OpenState> {
        Ok(self.state.borrow().clone())
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[crate::model::Tab]) -> anyhow::Result<String> {
        self.log.borrow_mut().push(format!("create {name} {}", tabs.len()));
        let mut state = self.state.borrow_mut();
        let id = format!("NEW{}", state.workspaces.len());
        let mut ws = open_ws(&id, name, cwd, &[]);
        ws.tabs = tabs.iter().map(open_tab).collect();
        state.workspaces.push(ws);
        Ok(format!("workspace:{id}"))
    }

    fn add_tab(&self, workspace_ref: &str, tab: &crate::model::Tab) -> anyhow::Result<()> {
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
