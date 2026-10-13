//! What the domain decides about agent sessions: can a saved session be resumed, and which
//! agent's history holds a session ID.

use anyhow::Result;

use crate::domain::model::{SessionRef, Tab, Workspace, has_control};
use crate::error::{Exit, failure};
use crate::ports::{AgentCatalog, SessionInfo};

/// Names every agent tab's agent by its registry kind ("Codex", "agy" → "codex", "antigravity"),
/// so sessions compare equal however the tab was written (unknown agents are left as they are),
/// and keeps only the saved flags the agent allows (`AgentCatalog::check_flags`), so a snapshot
/// saved before the allow-list, or edited by hand, is never shown or carried forward with flags a
/// save would leave out today. Every tab's flags are filtered, whatever else it holds. A tab whose
/// flags lose the session, or hold a control character (it can't be resumed), becomes a plain
/// terminal tab without them, as a save today makes it. Every read of a snapshot goes through this
/// (`app::load_normalized`).
pub fn normalize_tabs(workspaces: &mut [Workspace], agents: &dyn AgentCatalog) {
    for tab in workspaces.iter_mut().flat_map(|w| w.tabs.iter_mut()) {
        let Tab::Agent { agent, args, title, cwd, .. } = tab else { continue };
        if let Some(kind) = agents.kind_of(agent)
            && kind != agent
        {
            *agent = kind.to_string();
        }
        let check = agents.check_flags(agent, args);
        if check.lost.is_some() || args.iter().any(|a| has_control(a)) {
            *tab = Tab::Terminal { title: title.clone(), cwd: cwd.clone(), command: None };
        } else {
            *args = check.kept;
        }
    }
}

/// Whether a saved agent session can be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Ready,
    /// sessionsnap does not know the agent.
    UnknownAgent,
    /// Not a session ID of that agent; never passed to a command.
    InvalidId,
    /// The agent keeps a history, and the session is no longer in it.
    NotInHistory,
}

pub fn resume_status(agents: &dyn AgentCatalog, s: &SessionRef) -> Resume {
    let Some(kind) = agents.kind_of(&s.agent) else { return Resume::UnknownAgent };
    if !agents.valid_id(kind, &s.session_id) {
        return Resume::InvalidId;
    }
    match agents.history(kind) {
        Some(h) if !h.contains(&s.session_id) => Resume::NotInHistory,
        _ => Resume::Ready,
    }
}

/// Why `s` cannot be resumed, in words ("" when it can).
pub fn problem(agents: &dyn AgentCatalog, s: &SessionRef, status: Resume) -> String {
    match status {
        Resume::Ready => String::new(),
        Resume::UnknownAgent => format!("sessionsnap doesn't know how to resume {} yet", s.agent),
        Resume::InvalidId => format!("'{}' is not a {} session ID", s.session_id, agents.display_name(&s.agent)),
        Resume::NotInHistory => "transcript not found".to_string(),
    }
}

/// A session to put in a tab: in `agent`'s history when given (agents without one take the ID as
/// is), else in every history, where it must be found exactly once. Returns the agent's kind and,
/// when its history has one, the session's details.
pub fn find_session(
    agents: &dyn AgentCatalog,
    session_id: &str,
    agent: Option<&str>,
) -> Result<(String, Option<SessionInfo>)> {
    if let Some(name) = agent {
        let kind = agents.kind_of(name).ok_or_else(|| {
            failure(Exit::Error, format!("unknown agent '{name}'; `sessionsnap agents` lists the known ones"))
        })?;
        if !agents.valid_id(kind, session_id) {
            return Err(failure(
                Exit::Error,
                format!("'{session_id}' is not a {} session ID", agents.display_name(kind)),
            ));
        }
        return match agents.history(kind) {
            Some(h) => {
                let info = h.find(session_id)?.ok_or_else(|| unknown_session(session_id))?;
                Ok((kind.to_string(), Some(info)))
            }
            None => Ok((kind.to_string(), None)),
        };
    }
    let histories: Vec<_> = agents.histories().into_iter().filter(|h| agents.valid_id(h.agent(), session_id)).collect();
    if histories.is_empty() {
        let names: Vec<String> = agents.histories().iter().map(|h| agents.display_name(h.agent())).collect();
        return Err(failure(
            Exit::Error,
            format!("'{session_id}' is not a {} session ID; give --agent for other agents", names.join(" or ")),
        ));
    }
    let mut hits = vec![];
    for h in histories {
        if let Some(info) = h.find(session_id)? {
            hits.push(info);
        }
    }
    match hits.len() {
        0 => Err(unknown_session(session_id)),
        1 => {
            let info = hits.remove(0);
            Ok((info.agent.clone(), Some(info)))
        }
        _ => Err(failure(
            Exit::Error,
            format!(
                "session {session_id} is in several agents' histories ({}); give --agent",
                hits.iter().map(|i| i.agent.as_str()).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

pub fn unknown_session(id: &str) -> anyhow::Error {
    failure(Exit::NotFound, format!("no transcript for session {id}"))
}
