pub mod human;
pub mod pick;
pub mod run;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use crate::claude::Claude;
use crate::cmux::Backend;
use crate::cmux::cli::CmuxCli;
use crate::error::{Exit, exit_of};
use crate::store::Store;

#[derive(Parser, Debug)]
#[command(name = "ccsnap", version, about = "Named, versioned snapshots of cmux workspaces and Claude Code sessions")]
pub struct Cli {
    /// Print machine-readable JSON (never prompts)
    #[arg(long, global = true)]
    pub json: bool,
    /// Answer yes to confirmations
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)] // parsed once per process
pub enum Command {
    /// Save what is open in cmux as a new version of a snapshot
    Save {
        /// Snapshot name (letters, digits, '.', '_', '-')
        name: Option<String>,
        /// Only capture these open workspaces; the rest of the snapshot is kept
        #[arg(long = "workspace", short = 'w')]
        workspaces: Vec<String>,
        /// Copy another snapshot version instead of capturing: <name>[@v]
        #[arg(long)]
        from: Option<String>,
        /// Version message (default: a summary of changes)
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Choose or name the snapshot interactively
        #[arg(long)]
        pick: bool,
    },
    /// List snapshots
    List,
    /// Show a snapshot's workspaces and tabs: <name>[@v]
    Show { snapshot: String },
    /// List a snapshot's versions
    History { name: String },
    /// Compare two versions: <name>@a <name>@b
    Diff { a: String, b: String },
    /// Delete all but the newest versions of a snapshot
    Prune {
        name: String,
        #[arg(long)]
        keep: usize,
    },
    /// Delete a snapshot and all its versions
    Delete { name: String },
    /// Edit a workspace in a saved snapshot
    Ws(WsArgs),
    /// Edit a tab in a saved snapshot
    Tab(TabArgs),
    /// Apply a JSON array of edit operations as one version ('-' reads stdin)
    Edit {
        name: String,
        #[arg(long)]
        batch: String,
        #[arg(long, short = 'm')]
        message: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct WsArgs {
    /// Snapshot to edit
    pub name: String,
    /// Workspace in the snapshot (name or a unique part of it)
    pub ws: Option<String>,
    #[arg(long)]
    pub rename: Option<String>,
    #[arg(long)]
    pub color: Option<String>,
    /// New position, 1-based
    #[arg(long)]
    pub index: Option<usize>,
    #[arg(long)]
    pub remove: bool,
    /// Add a workspace from what is open in cmux now
    #[arg(long)]
    pub add_live: Option<String>,
    /// Copy a workspace from another snapshot: <name>[@v]:<workspace>
    #[arg(long)]
    pub from: Option<String>,
    #[arg(long, short = 'm')]
    pub message: Option<String>,
}

#[derive(Args, Debug)]
pub struct TabArgs {
    /// Snapshot to edit
    pub name: String,
    /// A tab (<ws>#<n> or <ws>:"title"); a workspace with --add-session / --add-terminal
    pub target: String,
    #[arg(long)]
    pub remove: bool,
    #[arg(long)]
    pub rename: Option<String>,
    /// New position within the workspace, 1-based
    #[arg(long)]
    pub index: Option<usize>,
    /// Move to another workspace
    #[arg(long = "move")]
    pub move_to: Option<String>,
    /// Change the folder (or the folder of a new terminal tab)
    #[arg(long)]
    pub cwd: Option<String>,
    /// Change the Claude session
    #[arg(long)]
    pub session: Option<String>,
    /// Add a Claude tab for this session ID
    #[arg(long)]
    pub add_session: Option<String>,
    /// Add a terminal tab (needs --cwd)
    #[arg(long)]
    pub add_terminal: bool,
    /// Command for a new terminal tab
    #[arg(long)]
    pub command: Option<String>,
    /// Title for a new tab
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long, short = 'm')]
    pub message: Option<String>,
}

/// Everything a command needs; tests build it with fakes.
pub struct Ctx {
    pub store: Store,
    pub claude: Claude,
    pub backend: Box<dyn Backend>,
    pub json: bool,
    pub yes: bool,
    /// Prompts allowed (a terminal on stdin and not --json).
    pub interactive: bool,
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
        })
    }
}

/// A command's result: exit code, JSON for --json, text for people.
#[derive(Debug)]
pub struct Outcome {
    pub exit: Exit,
    pub json: Value,
    pub text: String,
}

impl Outcome {
    pub fn ok(json: Value, text: impl Into<String>) -> Self {
        Outcome { exit: Exit::Ok, json, text: text.into() }
    }

    pub fn with_exit(mut self, exit: Exit) -> Self {
        self.exit = exit;
        self
    }
}

pub fn main_entry() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // clap would exit 2, which means "cmux unreachable" here.
            let code = if e.use_stderr() { 1 } else { 0 };
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    let json = cli.json;
    let result = Ctx::open_default(cli.json, cli.yes).and_then(|ctx| run::dispatch(&ctx, cli.command));
    match result {
        Ok(o) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&o.json).unwrap_or_default());
            } else if !o.text.is_empty() {
                println!("{}", o.text);
            }
            ExitCode::from(o.exit.code())
        }
        Err(e) => {
            let exit = exit_of(&e);
            if json {
                println!("{}", json!({"error": {"code": exit.name(), "exit": exit.code(), "message": format!("{e:#}")}}));
            } else {
                eprintln!("ccsnap: {e:#}");
            }
            ExitCode::from(exit.code())
        }
    }
}
