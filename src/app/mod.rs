//! Use cases: what each command does, independent of how it was asked for (CLI, JSON, web UI)
//! and of how its result is shown. Functions take the context and typed inputs and return typed
//! results; `interfaces` turns those into output.
//!
//! Layer rule: app may use `domain`, `ports` and `adapters`; it never depends on `interfaces`, and
//! never prints or prompts.

pub mod edit;
pub mod install;
pub mod restore;
pub mod sessions;
pub mod snapshots;

use std::io::IsTerminal;

use crate::adapters::agents::claude::Claude;
use crate::adapters::store::Store;
use crate::adapters::terminal::cmux::cli::CmuxCli;
use crate::ports::Terminal;

/// Everything a command needs; tests build it with fakes.
///
/// `json`, `yes`, `interactive` and `input` describe how the caller runs; only `interfaces` reads
/// them.
pub struct Ctx {
    pub store: Store,
    pub claude: Claude,
    /// The terminal (cmux).
    pub backend: Box<dyn Terminal>,
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
            claude: Claude::open_default()?,
            backend: Box::new(CmuxCli::open_default()?),
            json,
            yes,
            interactive: !json && std::io::stdin().is_terminal(),
            input: None,
        })
    }
}
