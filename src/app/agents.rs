//! The agents sessionsnap knows, and whether each can be resumed here.

use serde::Serialize;

use super::Ctx;
use crate::adapters::agents::registry::AGENTS;
pub use crate::adapters::agents::registry::Support;

/// One registry entry as `sessionsnap agents` shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub kind: &'static str,
    pub name: &'static str,
    /// Other names the agent goes by (aliases, its program).
    pub aliases: Vec<&'static str>,
    pub program: &'static str,
    /// The resume command's shape, e.g. "codex resume <id>".
    pub resume: String,
    pub support: Support,
    /// True when its program is on PATH.
    pub on_path: bool,
}

pub fn list(ctx: &Ctx) -> Vec<AgentStatus> {
    AGENTS
        .iter()
        .map(|a| AgentStatus {
            kind: a.kind,
            name: a.display,
            aliases: a.names().filter(|n| *n != a.kind).collect(),
            program: a.program,
            resume: a.usage(),
            support: a.support,
            on_path: ctx.agents.on_path(a.program),
        })
        .collect()
}
