//! The command line: clap definitions, prompts, human and JSON output, and `run_json` (the
//! in-process `--json` runner the web UI uses). Commands run through `app` use cases.

pub mod human;
pub mod pick;
pub mod run;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use crate::domain::terminal::TerminalKind;
use crate::error::{Exit, exit_of};

#[derive(Parser, Debug)]
#[command(
    name = "sessionsnap",
    version,
    about = "Save and resume your coding-agent sessions in cmux and tmux (Claude Code, Codex and more), as named, versioned snapshots"
)]
pub struct Cli {
    /// Print machine-readable JSON (never prompts)
    #[arg(long, global = true)]
    pub json: bool,
    /// Answer yes to the confirmations of delete, prune, restore, recent --restore, install-cmux and
    /// uninstall
    /// (agents: only when the user asked for that action)
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    /// Terminal to save from and restore into (default: the one this runs in, tmux first; outside
    /// both, cmux if it runs, else tmux if it has sessions)
    #[arg(long, global = true, value_parser = clap::builder::PossibleValuesParser::new(TerminalKind::ALL.map(TerminalKind::name)))]
    pub terminal: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)] // parsed once per process
pub enum Command {
    /// Save what is open in the terminal (cmux or tmux) as a new version of a snapshot
    Save {
        /// Snapshot name (letters, digits, '.', '_', '-'; a new one can't start with '--')
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
    List {
        /// Only snapshots with a workspace or tab in this folder (or below), e.g. `--cwd .`
        #[arg(long)]
        cwd: Option<String>,
    },
    /// List the workspaces and tabs open in the terminal now
    Open,
    /// Show a snapshot's workspaces and tabs: <name>[@v]
    Show {
        /// <name>[@v]; the latest version when @v is left out
        snapshot: String,
    },
    /// List a snapshot's versions
    History {
        /// Snapshot name
        name: String,
    },
    /// Compare two versions: <name>@a <name>@b
    Diff {
        /// <name>@a
        a: String,
        /// <name>@b
        b: String,
    },
    /// Delete all but the newest versions of a snapshot
    Prune {
        /// Snapshot name
        name: String,
        /// How many of the newest versions to keep (at least 1)
        #[arg(long)]
        keep: usize,
    },
    /// Delete a snapshot and all its versions (and its schedule)
    Delete {
        /// Snapshot name
        name: String,
    },
    /// Rename a snapshot (and its schedule)
    Rename {
        /// Current name
        old: String,
        /// New name
        new: String,
    },
    /// Write a version of a snapshot as one JSON file, for another machine: <name>[@v]
    Export {
        /// <name>[@v]
        snapshot: String,
        /// The file to write (refused if it exists); without it, the JSON goes to stdout
        #[arg(long, short)]
        output: Option<String>,
    },
    /// Save an exported version as the next version of a snapshot ('-' reads stdin)
    Import {
        /// The exported file ('-' reads stdin)
        file: String,
        /// The snapshot to save it to (default: the name in the file)
        #[arg(long = "as", value_name = "NAME")]
        name: Option<String>,
        /// Move folders under OLD to NEW, e.g. --map /Users/me=/home/me (repeatable)
        #[arg(long, value_name = "OLD=NEW")]
        map: Vec<String>,
        /// Version message (default: a summary of changes)
        #[arg(long, short)]
        message: Option<String>,
    },
    /// Save a snapshot automatically every so often (launchd on macOS, systemd on Linux)
    #[command(subcommand)]
    Schedule(ScheduleCommand),
    /// Edit a workspace in a saved snapshot
    Ws(WsArgs),
    /// Edit a tab in a saved snapshot
    Tab(TabArgs),
    /// Apply a JSON array of edit operations as one version ('-' reads stdin)
    Edit {
        /// Snapshot to edit
        name: String,
        /// The file with the JSON array ('-' reads stdin)
        #[arg(long)]
        batch: String,
        /// Version message (default: a summary of changes)
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Show the result without saving a version
        #[arg(long)]
        dry_run: bool,
        /// Refuse (exit 6) unless the latest version is this one, e.g. the dry run's baseVersion
        #[arg(long)]
        base: Option<u32>,
    },
    /// Reopen what a snapshot has that is not open now (plan, then apply)
    Restore {
        /// <name>[@v]; the most recently saved snapshot when left out
        snapshot: Option<String>,
        /// Only these workspaces
        #[arg(long = "workspace", short = 'w')]
        workspaces: Vec<String>,
        /// Only these tabs (<ws>#<n> or <ws>:"title")
        #[arg(long = "tab")]
        tabs: Vec<String>,
        /// Only make and print the plan
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
        /// Choose the snapshot interactively
        #[arg(long)]
        pick: bool,
    },
    /// Apply a plan made by restore or recent
    Apply {
        /// The planId a plan printed
        plan_id: String,
    },
    /// List agent sessions on disk (agents with a readable history: Claude Code, Codex)
    Sessions {
        /// Text to find in titles, prompts, folders or IDs
        #[arg(long)]
        search: Option<String>,
        /// Only sessions started in this folder (or below)
        #[arg(long)]
        cwd: Option<String>,
        /// Only sessions active in the last this many days
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Only this agent's sessions (e.g. claude, codex)
        #[arg(long)]
        agent: Option<String>,
        /// Show at most this many, newest first (`total` in the JSON says how many there are)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Interactive agent sessions active recently that are not open in the terminal
    Recent {
        /// How many hours back to look
        #[arg(long, default_value_t = 24)]
        hours: u64,
        /// Only this agent's sessions (e.g. claude, codex)
        #[arg(long)]
        agent: Option<String>,
        /// Plan reopening them (applies with --yes or after confirmation)
        #[arg(long)]
        restore: bool,
        /// Only make and print the plan
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
        /// Show them and ask whether to reopen
        #[arg(long)]
        pick: bool,
        /// List at most this many, newest first (a restore still takes them all)
        #[arg(long)]
        limit: Option<usize>,
    },
    /// List the agents sessionsnap can resume, how well, and whether their program is on PATH
    Agents,
    /// Serve the web UI on 127.0.0.1 and show it in the cmux Dock, or the default browser from tmux
    Ui {
        /// Port to serve on
        #[arg(long, default_value_t = crate::interfaces::web::DEFAULT_PORT)]
        port: u16,
        /// Stop after this many minutes without requests
        #[arg(long, default_value_t = 30)]
        idle_minutes: u64,
        /// Do not show the page
        #[arg(long)]
        no_open: bool,
        /// Show the page in the default browser, also from cmux (save and restore still use cmux)
        #[arg(long)]
        browser: bool,
    },
    /// Add sessionsnap commands to the cmux command palette (~/.config/cmux/cmux.json)
    InstallCmux {
        /// Print the commands instead of installing them
        #[arg(long)]
        print: bool,
    },
    /// Install the sessionsnap skill for Claude Code (~/.claude/skills/sessionsnap)
    InstallSkill,
    /// Print shell completions, e.g. `sessionsnap completions zsh > ~/.zfunc/_sessionsnap`
    Completions {
        /// bash, zsh, fish, elvish or powershell
        shell: clap_complete::Shell,
    },
    /// Print the man page (roff), e.g. `sessionsnap man > ~/.local/share/man/man1/sessionsnap.1`
    Man,
    /// Remove sessionsnap: palette commands, skill, schedules (launchd agents or systemd timers), snapshots and this binary (asks first)
    Uninstall {
        /// Keep your snapshots (~/.sessionsnap)
        #[arg(long)]
        keep_data: bool,
    },
    /// Replace this binary with the latest release from GitHub
    Update {
        /// Only say whether a newer release exists
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScheduleCommand {
    /// Start an automatic snapshot: save what is open in the terminal now and every --interval
    Add {
        /// A new snapshot name (automatic snapshots are kept apart from the ones you save)
        name: String,
        /// How often: a number and a unit, e.g. 30m, 2h, 1d (at least 1m)
        #[arg(long)]
        interval: String,
    },
    /// List automatic snapshots, how often each is saved and how its last save went
    #[command(visible_alias = "ls")]
    List,
    /// Change how often an automatic snapshot is saved
    Edit {
        /// Automatic snapshot name
        name: String,
        /// How often: a number and a unit, e.g. 30m, 2h, 1d (at least 1m)
        #[arg(long)]
        interval: String,
    },
    /// Pause an automatic snapshot: no saves until it is started again
    Stop {
        /// Automatic snapshot name
        name: String,
    },
    /// Resume a stopped automatic snapshot
    Start {
        /// Automatic snapshot name
        name: String,
    },
    /// Make an automatic snapshot a normal one: no more saves, its versions are kept
    #[command(visible_alias = "rm")]
    Remove {
        /// Automatic snapshot name
        name: String,
    },
    /// Save an automatic snapshot now, as its schedule does
    Run {
        /// Automatic snapshot name
        name: String,
    },
}

#[derive(Args, Debug)]
pub struct WsArgs {
    /// Snapshot to edit
    pub name: String,
    /// Workspace in the snapshot (name or a unique part of it)
    pub ws: Option<String>,
    /// New name
    #[arg(long)]
    pub rename: Option<String>,
    /// New color
    #[arg(long)]
    pub color: Option<String>,
    /// New position, 1-based
    #[arg(long)]
    pub index: Option<usize>,
    /// Remove the workspace
    #[arg(long)]
    pub remove: bool,
    /// Add a workspace from what is open in the terminal now
    #[arg(long)]
    pub add_live: Option<String>,
    /// Copy a workspace from another snapshot: <name>[@v]:<workspace>
    #[arg(long)]
    pub from: Option<String>,
    /// Version message (default: a summary of changes)
    #[arg(long, short = 'm')]
    pub message: Option<String>,
}

#[derive(Args, Debug)]
pub struct TabArgs {
    /// Snapshot to edit
    pub name: String,
    /// A tab (<ws>#<n> or <ws>:"title"); a workspace with --add-session / --add-terminal
    pub target: String,
    /// Remove the tab
    #[arg(long)]
    pub remove: bool,
    /// New title
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
    /// Change the agent session (an ID from `sessionsnap sessions`)
    #[arg(long)]
    pub session: Option<String>,
    /// Add an agent tab for this session ID
    #[arg(long)]
    pub add_session: Option<String>,
    /// The agent of --session/--add-session (needed when no history has the ID, or several do)
    #[arg(long)]
    pub agent: Option<String>,
    /// Add a terminal tab (needs --cwd)
    #[arg(long)]
    pub add_terminal: bool,
    /// Command for a new terminal tab
    #[arg(long)]
    pub command: Option<String>,
    /// Title for a new tab
    #[arg(long)]
    pub title: Option<String>,
    /// Version message (default: a summary of changes)
    #[arg(long, short = 'm')]
    pub message: Option<String>,
}

pub use crate::app::Ctx;

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

/// The JSON printed for an error under `--json`.
pub fn error_json(e: &anyhow::Error) -> Value {
    let exit = exit_of(e);
    json!({"error": {"code": crate::error::code_of(e), "exit": exit.code(), "message": format!("{e:#}")}})
}

/// Runs `sessionsnap <args> --json` in-process: the exit and the JSON the binary would print.
/// `make_ctx` builds the context from the parsed flags (the web UI passes its own).
pub fn run_json(args: &[String], make_ctx: impl FnOnce(&Cli) -> anyhow::Result<Ctx>) -> (Exit, Value) {
    let mut argv = vec!["sessionsnap".to_string()];
    // Before the caller's args, so a `--` among them cannot turn it into a positional.
    if !args.iter().any(|a| a == "--json") {
        argv.push("--json".to_string());
    }
    argv.extend(args.iter().cloned());
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            // The same code the binary prints for a usage error under --json.
            let exit = Exit::Error;
            return (exit, json!({"error": {"code": "usage", "exit": exit.code(), "message": e.to_string()}}));
        }
    };
    match make_ctx(&cli).and_then(|ctx| run::dispatch(&ctx, cli.command)) {
        Ok(o) => (o.exit, o.json),
        Err(e) => (exit_of(&e), error_json(&e)),
    }
}

/// What `completions` and `man` print; None for every other command. They need no context, so
/// a shell's startup file can run them without sessionsnap looking for a terminal.
pub fn generated(command: &Command) -> Option<String> {
    use clap::CommandFactory;
    let mut out = Vec::new();
    match command {
        Command::Completions { shell } => clap_complete::generate(*shell, &mut Cli::command(), "sessionsnap", &mut out),
        Command::Man => clap_mangen::Man::new(Cli::command()).render(&mut out).ok()?,
        _ => return None,
    }
    String::from_utf8(out).ok()
}

pub fn main_entry() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // clap would exit 2, which means "terminal unreachable" here.
            let code = if e.use_stderr() { 1 } else { 0 };
            // Under --json a mistyped command is JSON too, so a caller can read why.
            if e.use_stderr() && std::env::args().any(|a| a == "--json") {
                let message = e.render().to_string();
                println!("{}", json!({"error": {"code": "usage", "exit": code, "message": message.trim()}}));
            } else {
                let _ = e.print();
            }
            return ExitCode::from(code);
        }
    };
    if let Some(text) = generated(&cli.command) {
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    let json = cli.json;
    let result =
        Ctx::open_default(cli.json, cli.yes, cli.terminal.as_deref()).and_then(|ctx| run::dispatch(&ctx, cli.command));
    match result {
        Ok(o) => {
            if json {
                // Indented for a person at a terminal; one compact line for a program (fewer tokens).
                let text = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                    serde_json::to_string_pretty(&o.json)
                } else {
                    serde_json::to_string(&o.json)
                };
                println!("{}", text.unwrap_or_default());
            } else if !o.text.is_empty() {
                println!("{}", human::printable(&o.text));
            }
            ExitCode::from(o.exit.code())
        }
        Err(e) => {
            let exit = exit_of(&e);
            if json {
                println!("{}", error_json(&e));
            } else {
                eprintln!("sessionsnap: {}", human::printable(&format!("{e:#}")));
            }
            ExitCode::from(exit.code())
        }
    }
}
