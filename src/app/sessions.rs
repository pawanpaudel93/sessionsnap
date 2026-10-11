//! What is open in the terminal now, and agent sessions on disk.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;

use super::Ctx;
use crate::adapters::home_dir;
use crate::error::{Exit, failure};
use crate::ports::{OpenState, SessionInfo};

/// The workspaces and tabs open in the terminal now.
pub fn open_now(ctx: &Ctx) -> Result<OpenState> {
    ctx.terminal.read_open_state()
}

fn window(n: u64, unit: u64, flag: &str) -> Result<Duration> {
    n.checked_mul(unit).map(Duration::from_secs).ok_or_else(|| failure(Exit::Error, format!("{flag} is too large")))
}

/// Sessions changed in the last `days` days, filtered by `search` text and `cwd` prefix, and the
/// sessions open now (empty when the terminal cannot be read).
pub fn list(
    ctx: &Ctx,
    search: Option<&str>,
    cwd: Option<&str>,
    days: u64,
) -> Result<(Vec<SessionInfo>, HashSet<String>)> {
    let open = ctx.terminal.read_open_state().map(|s| s.open_sessions()).unwrap_or_default();
    let needle = search.map(str::to_lowercase);
    let list: Vec<SessionInfo> = ctx
        .agents
        .history()
        .scan(Some(window(days, 86_400, "--days")?))?
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

/// Interactive sessions active in the last `hours` hours that are not open now, and the sessions
/// open now.
pub fn recent(ctx: &Ctx, hours: u64) -> Result<(Vec<SessionInfo>, HashSet<String>)> {
    let open = ctx.terminal.read_open_state()?.open_sessions();
    let home = home_dir()?.to_string_lossy().into_owned();
    let list: Vec<SessionInfo> = ctx
        .agents
        .history()
        .scan(Some(window(hours, 3600, "--hours")?))?
        .into_iter()
        .filter(|s| is_recoverable(s, &open, &home))
        .collect();
    Ok((list, open))
}

/// Interactive sessions (not `claude -p`/SDK), outside ~/.claude*, not open in cmux.
fn is_recoverable(s: &SessionInfo, open: &HashSet<String>, home: &str) -> bool {
    let interactive = s.entrypoint.as_deref().is_none_or(|e| e == "cli");
    let internal = s.cwd.as_deref().is_some_and(|c| c.starts_with(&format!("{home}/.claude")));
    interactive && !internal && s.cwd.is_some() && !open.contains(&s.session_id)
}
