//! Use cases: what each command does, independent of how it was asked for (CLI, JSON, web UI)
//! and of how its result is shown. Functions take the context and typed inputs and return typed
//! results; `interfaces` turns those into output.
//!
//! Layer rule: app may use `domain`, `ports` and `adapters`; it never depends on `interfaces`, and
//! never prints or prompts.

pub mod agents;
pub mod edit;
pub mod install;
pub mod restore;
pub mod sessions;
pub mod snapshots;

use std::io::IsTerminal;

use crate::adapters::agents::Agents;
use crate::adapters::store::Store;
use crate::adapters::terminal::cmux::cli::CmuxCli;
use crate::ports::{OpenState, Terminal};

/// Everything a command needs; tests build it with fakes.
///
/// `json`, `yes`, `interactive` and `input` describe how the caller runs; only `interfaces` reads
/// them.
pub struct Ctx {
    pub store: Store,
    /// The coding agents sessionsnap knows, with their session histories.
    pub agents: Agents,
    /// The terminal (cmux).
    pub terminal: Box<dyn Terminal>,
    pub json: bool,
    pub yes: bool,
    /// Prompts allowed (a terminal on stdin and not --json).
    pub interactive: bool,
    /// Text for `--batch -` when the caller is not a terminal (the web UI); stdin when None.
    pub input: Option<String>,
}

impl Ctx {
    pub fn open_default(json: bool, yes: bool) -> anyhow::Result<Ctx> {
        Ok(Ctx {
            store: Store::open_default()?,
            agents: Agents::open_default()?,
            terminal: Box::new(CmuxCli::open_default()?),
            json,
            yes,
            interactive: !json && std::io::stdin().is_terminal(),
            input: None,
        })
    }
}

/// Version `version` (latest when None) of snapshot `name`, with agent tabs named by registry kind.
pub fn load_version(ctx: &Ctx, name: &str, version: Option<u32>) -> anyhow::Result<crate::domain::model::Version> {
    load_normalized(&ctx.store, &ctx.agents, name, version)
}

pub fn load_normalized(
    store: &Store,
    agents: &Agents,
    name: &str,
    version: Option<u32>,
) -> anyhow::Result<crate::domain::model::Version> {
    let mut v = store.load(name, version)?;
    crate::domain::agents::normalize_kinds(&mut v.workspaces, agents);
    Ok(v)
}

/// What is open in the terminal, with each agent named by its registry kind (cmux may report an
/// alias or program name), so tabs match saved sessions by (agent, ID).
pub fn read_open(terminal: &dyn Terminal) -> anyhow::Result<OpenState> {
    let mut open = terminal.read_open_state()?;
    for tab in open.workspaces.iter_mut().flat_map(|w| w.tabs.iter_mut()) {
        if let Some(a) = &mut tab.agent
            && let Some(spec) = Agents::spec(&a.kind)
        {
            a.kind = spec.kind.to_string();
        }
    }
    Ok(open)
}
