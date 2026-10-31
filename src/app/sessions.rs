//! What is open in the terminal now, and agent sessions on disk.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;

use super::{Ctx, read_open};
use crate::adapters::agents::Agents;
use crate::domain::model::SessionRef;
use crate::error::{Exit, failure};
use crate::ports::{AgentCatalog, OpenState, SessionHistory, SessionInfo};

/// The workspaces and tabs open in the terminal now.
pub fn open_now(ctx: &Ctx) -> Result<OpenState> {
    read_open(ctx.terminal.as_ref(), &ctx.agents)
}

fn window(n: u64, unit: u64, flag: &str) -> Result<Duration> {
    n.checked_mul(unit).map(Duration::from_secs).ok_or_else(|| failure(Exit::Error, format!("{flag} is too large")))
}

/// The histories to read: every one, or only `agent`'s (an error for agents without one).
fn histories<'a>(agents: &'a Agents, agent: Option<&str>) -> Result<Vec<&'a dyn SessionHistory>> {
    let Some(name) = agent else { return Ok(agents.histories()) };
    let spec = Agents::spec(name).ok_or_else(|| {
        failure(Exit::Error, format!("unknown agent '{name}'; `sessionsnap agents` lists the known ones"))
    })?;
    let history = agents.history(spec.kind).ok_or_else(|| {
        failure(
            Exit::Error,
            format!("sessionsnap cannot read {}'s session history (it can only resume them)", spec.display),
        )
    })?;
    Ok(vec![history])
}

/// Sessions of all agents with a history (or only `agent`) changed within `within`, newest first.
fn scan(ctx: &Ctx, agent: Option<&str>, within: Duration) -> Result<Vec<SessionInfo>> {
    let mut out = vec![];
    for h in histories(&ctx.agents, agent)? {
        out.extend(h.scan(Some(within))?);
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified_at));
    Ok(out)
}

/// Sessions changed in the last `days` days (of every agent, or only `agent`), filtered by
/// `search` text and `cwd` prefix, and the sessions open now (empty when the terminal cannot be
/// read).
pub fn list(
    ctx: &Ctx,
    search: Option<&str>,
    cwd: Option<&str>,
    days: u64,
    agent: Option<&str>,
) -> Result<(Vec<SessionInfo>, HashSet<SessionRef>)> {
    let open = read_open(ctx.terminal.as_ref(), &ctx.agents).map(|s| s.open_sessions()).unwrap_or_default();
    let needle = search.map(str::to_lowercase);
    let list: Vec<SessionInfo> = scan(ctx, agent, window(days, 86_400, "--days")?)?
        .into_iter()
        .filter(|s| cwd.is_none_or(|c| s.cwd.as_deref().is_some_and(|sc| sc.starts_with(c))))
        .filter(|s| {
            needle.as_ref().is_none_or(|n| {
                [s.title.as_deref(), s.first_prompt.as_deref(), s.cwd.as_deref(), Some(s.session_id.as_str())]
                    .iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(n.as_str()))
            })
        })
        .collect();
    Ok((list, open))
}

/// Sessions (of every agent, or only `agent`) active in the last `hours` hours that their history
/// judges recoverable, that have a folder and are not open now; and the sessions open now.
pub fn recent(ctx: &Ctx, hours: u64, agent: Option<&str>) -> Result<(Vec<SessionInfo>, HashSet<SessionRef>)> {
    let open = read_open(ctx.terminal.as_ref(), &ctx.agents)?.open_sessions();
    let list: Vec<SessionInfo> = scan(ctx, agent, window(hours, 3600, "--hours")?)?
        .into_iter()
        .filter(|s| s.recoverable && s.cwd.is_some() && !open.contains(&SessionRef::new(&s.agent, &s.session_id)))
        .collect();
    Ok((list, open))
}
