//! Helpers shared by unit and integration tests. Not part of sessionsnap's interface.

use std::path::{Path, PathBuf};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::adapters::agents::claude::Claude;
use crate::adapters::agents::codex::Codex;
use crate::adapters::agents::{Agents, registry};
use crate::adapters::store::Store;
use crate::app::Ctx;
use crate::domain::model::Tab;
use crate::ports::{AgentRef, LaunchTab, OpenState, OpenTab, OpenWorkspace, SessionHistory, SessionInfo, Terminal};

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

/// A context on a store under `dir/store`, Claude Code and Codex data under `dir/claude` and
/// `dir/codex`, and `terminal`, as the binary would build it for these flags. No program is on the
/// search path.
pub fn ctx(dir: &Path, terminal: FakeTerminal, json: bool, yes: bool, input: Option<String>) -> Ctx {
    Ctx {
        store: Store::new(dir.join("store")),
        agents: Agents::new(
            vec![Box::new(Claude::new(dir.join("claude"))), Box::new(Codex::new(dir.join("codex")))],
            vec![],
        ),
        terminal: Box::new(terminal),
        json,
        yes,
        interactive: false,
        input,
    }
}

/// The agents type the domain's tests use as their catalog.
pub type TestAgents = Agents;

/// The registry with these histories and nothing on the search path.
pub fn agents(histories: Vec<FakeHistory>) -> TestAgents {
    Agents::new(histories.into_iter().map(|h| Box::new(h) as Box<dyn SessionHistory>).collect(), vec![])
}

/// An agent tab without saved flags.
pub fn agent_tab(agent: &str, title: &str, cwd: &str, id: &str) -> Tab {
    Tab::Agent { title: title.into(), cwd: cwd.into(), agent: agent.into(), session_id: id.into(), args: vec![] }
}

/// Writes a minimal Codex rollout for `id`, started in `cwd`, whose first real prompt is `prompt`.
pub fn write_rollout(codex_root: &Path, cwd: &str, id: &str, prompt: &str) -> PathBuf {
    let dir = codex_root.join("sessions/2026/10/11");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-10-11T09-30-00-{id}.jsonl"));
    let user = |text: &str| {
        serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                           "content": [{"type": "input_text", "text": text}]}})
    };
    let lines = [
        serde_json::json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd, "timestamp": "2026-10-11T09:30:00Z",
                           "source": "cli", "originator": "codex_cli_rs", "cli_version": "0.50.0"}}),
        user("<environment_context>\n  <cwd>/w</cwd>\n</environment_context>"),
        user(prompt),
    ];
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    path
}

/// An open workspace with ref "workspace:<id>"; tabs are (title, cwd, session), where a session
/// is "<agent>:<id>", or a bare ID for Claude Code.
pub fn open_ws(id: &str, name: &str, cwd: &str, tabs: &[(&str, &str, Option<&str>)]) -> OpenWorkspace {
    OpenWorkspace {
        id: id.to_string(),
        reference: Some(format!("workspace:{id}")),
        name: name.to_string(),
        cwd: cwd.to_string(),
        git_branch: None,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(i, (title, cwd, session))| OpenTab {
                reference: Some(format!("surface:{id}-{i}")),
                title: title.to_string(),
                cwd: cwd.to_string(),
                agent: session.map(|s| {
                    let (kind, sid) = s.split_once(':').unwrap_or(("claude", s));
                    AgentRef { kind: kind.into(), session_id: sid.into(), args: vec![] }
                }),
            })
            .collect(),
    }
}

/// In-memory terminal for tests. Clones share state, so a test can inspect it after handing one over.
#[derive(Clone)]
pub struct FakeTerminal {
    pub state: Rc<RefCell<OpenState>>,
    pub log: Rc<RefCell<Vec<String>>>,
    /// Every tab opened, with its command.
    pub launched: Rc<RefCell<Vec<LaunchTab>>>,
    pub running: Rc<Cell<bool>>,
}

impl Default for FakeTerminal {
    fn default() -> Self {
        FakeTerminal {
            state: Rc::new(RefCell::new(OpenState { live: true, ..Default::default() })),
            log: Default::default(),
            launched: Default::default(),
            running: Rc::new(Cell::new(true)),
        }
    }
}

/// The agent session a resume command from the registry opens.
pub fn resumed_session(command: &str) -> Option<AgentRef> {
    let words = shell_words(command);
    let spec = registry::AGENTS.iter().find(|a| words.first().map(String::as_str) == Some(a.program))?;
    let n = spec.resume.len();
    if words.len() < n + 2 || words[1..=n] != *spec.resume {
        return None;
    }
    Some(AgentRef { kind: spec.kind.into(), session_id: words[n + 1].clone(), args: words[n + 2..].to_vec() })
}

/// Splits a command line made of plain words, single-quoted parts and `\'`.
fn shell_words(command: &str) -> Vec<String> {
    let (mut words, mut word, mut quoted, mut any) = (vec![], String::new(), false, false);
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if !quoted => {
                word.extend(chars.next());
                any = true;
            }
            ' ' if !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            _ => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(word);
    }
    words
}

/// The tab a terminal shows after opening `t`; a resume command opens its session.
fn open_tab(t: &LaunchTab) -> OpenTab {
    OpenTab {
        reference: None,
        title: t.title.clone(),
        cwd: t.cwd.clone(),
        agent: t.command.as_deref().and_then(resumed_session),
    }
}

impl Terminal for FakeTerminal {
    fn is_running(&self) -> bool {
        self.running.get()
    }

    fn ensure_running(&self) -> anyhow::Result<()> {
        self.running.set(true);
        Ok(())
    }

    fn read_open_state(&self) -> anyhow::Result<OpenState> {
        Ok(self.state.borrow().clone())
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[LaunchTab]) -> anyhow::Result<String> {
        self.log.borrow_mut().push(format!("create {name} {}", tabs.len()));
        self.launched.borrow_mut().extend(tabs.iter().cloned());
        let mut state = self.state.borrow_mut();
        let id = format!("NEW{}", state.workspaces.len());
        let mut ws = open_ws(&id, name, cwd, &[]);
        ws.tabs = tabs.iter().map(open_tab).collect();
        state.workspaces.push(ws);
        Ok(format!("workspace:{id}"))
    }

    fn add_tab(&self, workspace_ref: &str, tab: &LaunchTab) -> anyhow::Result<()> {
        self.log.borrow_mut().push(format!("add {workspace_ref} {}", tab.title));
        self.launched.borrow_mut().push(tab.clone());
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
pub fn session_info(id: &str, cwd: &str, title: &str) -> SessionInfo {
    SessionInfo {
        agent: "claude".into(),
        session_id: id.to_string(),
        cwd: Some(cwd.to_string()),
        title: Some(title.to_string()),
        first_prompt: Some(format!("start {title}")),
        modified: String::new(),
        modified_at: std::time::SystemTime::UNIX_EPOCH,
        recoverable: true,
    }
}

/// In-memory session history of one agent for tests: sessions are found by ID in any folder.
pub struct FakeHistory {
    pub agent: &'static str,
    pub sessions: Vec<SessionInfo>,
}

impl FakeHistory {
    /// Claude Code sessions (id, cwd, title) as `write_transcript` would record them.
    pub fn with(sessions: &[(&str, &str, &str)]) -> Self {
        FakeHistory::of("claude", sessions)
    }

    /// Codex sessions (id, cwd, title).
    pub fn codex(sessions: &[(&str, &str, &str)]) -> Self {
        FakeHistory::of("codex", sessions)
    }

    fn of(agent: &'static str, sessions: &[(&str, &str, &str)]) -> Self {
        let sessions = sessions
            .iter()
            .map(|(id, cwd, title)| SessionInfo { agent: agent.into(), ..session_info(id, cwd, title) })
            .collect();
        FakeHistory { agent, sessions }
    }
}

impl SessionHistory for FakeHistory {
    fn agent(&self) -> &'static str {
        self.agent
    }

    fn find(&self, session_id: &str) -> anyhow::Result<Option<SessionInfo>> {
        Ok(self.sessions.iter().find(|s| s.session_id == session_id).cloned())
    }

    fn title_for(&self, _cwd: &str, session_id: &str) -> Option<String> {
        let s = self.sessions.iter().find(|s| s.session_id == session_id)?;
        s.title.clone().or_else(|| s.first_prompt.clone())
    }

    fn scan(&self, _within: Option<std::time::Duration>) -> anyhow::Result<Vec<SessionInfo>> {
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
