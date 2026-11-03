//! Use cases: what each command does, independent of how it was asked for (CLI, JSON, web UI)
//! and of how its result is shown. Functions take the context and typed inputs and return typed
//! results; `interfaces` turns those into output.
//!
//! Layer rule: app may use `domain`, `ports` and `adapters`; it never depends on `interfaces`, and
//! never prints or prompts.

pub mod agents;
pub mod edit;
pub mod install;
pub mod known;
pub mod restore;
pub mod schedule;
pub mod sessions;
pub mod snapshots;
pub mod uninstall;
pub mod update;

use std::io::IsTerminal;

use crate::adapters::agents::Agents;
use crate::adapters::process::ProcessRunner;
use crate::adapters::store::Store;
use crate::adapters::terminal::cmux::cli::CmuxCli;
use crate::adapters::terminal::procs::System;
use crate::adapters::terminal::tmux::Tmux;
use crate::domain::terminal::TerminalKind;
use crate::ports::{OpenState, Scheduler, Terminal};

/// Everything a command needs; tests build it with fakes.
///
/// `json`, `yes`, `interactive` and `input` describe how the caller runs; only `interfaces` reads
/// them.
pub struct Ctx {
    pub store: Store,
    /// The coding agents sessionsnap knows, with their session histories.
    pub agents: Agents,
    /// The terminal: cmux, or tmux (`terminal_for`).
    pub terminal: Box<dyn Terminal>,
    /// Runs automatic snapshots' saves (launchd on macOS).
    pub scheduler: Box<dyn Scheduler>,
    pub json: bool,
    pub yes: bool,
    /// Prompts allowed (a terminal on stdin and not --json).
    pub interactive: bool,
    /// Text for `--batch -` when the caller is not a terminal (the web UI); stdin when None.
    pub input: Option<String>,
    /// Whether a folder exists (tests that use made-up folders say yes to all).
    pub folder_exists: fn(&str) -> bool,
}

impl Ctx {
    /// `terminal`: a `TerminalKind` name (`--terminal`); None picks one with `terminal_for`.
    pub fn open_default(json: bool, yes: bool, terminal: Option<&str>) -> anyhow::Result<Ctx> {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        let inside = Inside { tmux: set("TMUX"), cmux: set("CMUX_WORKSPACE_ID") || set("CMUX_SURFACE_ID") };
        let terminal = Box::new(Detected {
            asked: terminal.and_then(TerminalKind::parse),
            inside,
            cmux: CmuxCli::open_default()?,
            tmux: Tmux::open_default(),
            chosen: std::cell::OnceCell::new(),
        });
        let store = Store::open_default()?;
        let agents = Agents::open_default(&store.root().join("cache"))?;
        Ok(Ctx {
            store,
            agents,
            terminal,
            scheduler: crate::adapters::scheduler::open_default()?,
            json,
            yes,
            interactive: !json && std::io::stdin().is_terminal(),
            input: None,
            folder_exists: |path| std::path::Path::new(path).is_dir(),
        })
    }
}

/// The terminal, picked (`terminal_for`) the first time a command uses it: picking may ask cmux
/// whether it runs, which commands that never touch a terminal (`list`, `show`, `update`…) skip.
struct Detected {
    asked: Option<TerminalKind>,
    inside: Inside,
    cmux: CmuxCli<ProcessRunner>,
    tmux: Tmux<ProcessRunner, System<ProcessRunner>>,
    chosen: std::cell::OnceCell<TerminalKind>,
}

impl Detected {
    fn get(&self) -> &dyn Terminal {
        let kind = *self.chosen.get_or_init(|| {
            terminal_for(self.asked, self.inside, || self.cmux.is_running(), || self.tmux.has_sessions())
        });
        match kind {
            TerminalKind::Cmux => &self.cmux,
            TerminalKind::Tmux => &self.tmux,
        }
    }
}

/// Shows `url` in terminal `kind`'s place for pages, with a client of its own (the web UI shows
/// pages from a thread of their own).
pub fn show_url(kind: TerminalKind, url: &str) -> anyhow::Result<crate::ports::Shown> {
    match kind {
        TerminalKind::Cmux => CmuxCli::open_default()?.show_url(url),
        TerminalKind::Tmux => Tmux::open_default().show_url(url),
    }
}

/// Shows `url` in the default browser, whichever terminal the page's commands use: tmux's place
/// for pages is the browser.
pub fn show_in_browser(url: &str) -> anyhow::Result<crate::ports::Shown> {
    Tmux::open_default().show_url(url)
}

impl Terminal for Detected {
    fn kind(&self) -> TerminalKind {
        self.get().kind()
    }
    fn is_running(&self) -> bool {
        self.get().is_running()
    }
    fn ensure_running(&self) -> anyhow::Result<()> {
        self.get().ensure_running()
    }
    fn read_open_state(&self) -> anyhow::Result<OpenState> {
        self.get().read_open_state()
    }
    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[crate::ports::LaunchTab]) -> anyhow::Result<String> {
        self.get().create_workspace(name, cwd, tabs)
    }
    fn add_tab(&self, workspace_ref: &str, tab: &crate::ports::LaunchTab) -> anyhow::Result<()> {
        self.get().add_tab(workspace_ref, tab)
    }
    fn show_url(&self, url: &str) -> anyhow::Result<crate::ports::Shown> {
        self.get().show_url(url)
    }
}

/// Which terminals sessionsnap runs inside (tmux in a cmux tab is inside both).
#[derive(Debug, Clone, Copy, Default)]
pub struct Inside {
    /// `$TMUX` is set.
    pub tmux: bool,
    /// cmux's `$CMUX_WORKSPACE_ID` or `$CMUX_SURFACE_ID` is set.
    pub cmux: bool,
}

/// The terminal to use: the one asked for; else the one sessionsnap runs inside, the innermost
/// (tmux) first; else, outside both (another terminal app, launchd), whichever runs: cmux, then a
/// tmux with sessions; else cmux.
pub fn terminal_for(
    asked: Option<TerminalKind>,
    inside: Inside,
    cmux_running: impl FnOnce() -> bool,
    tmux_running: impl FnOnce() -> bool,
) -> TerminalKind {
    match asked {
        Some(t) => t,
        None if inside.tmux => TerminalKind::Tmux,
        None if inside.cmux => TerminalKind::Cmux,
        None if !cmux_running() && tmux_running() => TerminalKind::Tmux,
        None => TerminalKind::Cmux,
    }
}

/// Version `version` (latest when None) of snapshot `name`, with agent tabs named by registry kind.
pub fn load_version(ctx: &Ctx, name: &str, version: Option<u32>) -> anyhow::Result<crate::domain::model::Version> {
    load_normalized(&ctx.store, &ctx.agents, name, version)
}

/// Saved snapshots read through `load_normalized`, for edit ops that copy from another snapshot.
pub struct NormalizedSnapshots<'a> {
    pub store: &'a Store,
    pub agents: &'a Agents,
}

impl crate::ports::Snapshots for NormalizedSnapshots<'_> {
    fn load(&self, name: &str, version: Option<u32>) -> anyhow::Result<crate::domain::model::Version> {
        load_normalized(self.store, self.agents, name, version)
    }
}

/// A version as stored, with its tabs normalized (see `domain::agents::normalize_tabs`): every read
/// of a snapshot goes through here, so none shows flags a save would leave out.
pub fn load_normalized(
    store: &Store,
    agents: &Agents,
    name: &str,
    version: Option<u32>,
) -> anyhow::Result<crate::domain::model::Version> {
    let mut v = store.load(name, version)?;
    crate::domain::agents::normalize_tabs(&mut v.workspaces, agents);
    Ok(v)
}

/// What is open in the terminal, with each agent named by its registry kind (cmux may report an
/// alias or program name), so tabs match saved sessions by (agent, ID); an invalid ID is replaced by
/// the terminal's other record of it when that one is valid. A terminal that only reports a tab's
/// processes (tmux) has its agents found in them (`domain::running`).
pub fn read_open(terminal: &dyn Terminal, agents: &Agents) -> anyhow::Result<OpenState> {
    let mut open = terminal.read_open_state()?;
    crate::domain::running::identify(&mut open, agents, std::time::SystemTime::now());
    let mut warnings = vec![];
    for w in open.workspaces.iter_mut() {
        for tab in w.tabs.iter_mut() {
            let Some(a) = &mut tab.agent else { continue };
            let Some(spec) = Agents::spec(&a.kind) else { continue };
            a.kind = spec.kind.to_string();
            // The terminal's other record of the session, when its first one is no valid ID.
            if !spec.valid_id(&a.session_id)
                && let Some(alt) = a.alt_session_id.take().filter(|alt| spec.valid_id(alt))
            {
                a.session_id = alt;
            }
            // Compared with saved sessions, which hold UUIDs in lowercase.
            a.session_id = crate::domain::model::canonical_id(&a.session_id);
            // An agent that hides its flags from the terminal (Pi): they come from its own record
            // of the launch, which the terminal's (empty or wrong) arguments never replace.
            if let Some(history) =
                crate::ports::AgentCatalog::history(agents, spec.kind).filter(|h| h.records_launches())
            {
                match history.launch_args(&a.session_id) {
                    Some(args) => a.args = args,
                    None => {
                        a.args.clear();
                        let message = format!(
                            "{} › {}: {} hides the flags it was started with, and this session has no record of \
                             them, so it resumes without them; run `sessionsnap install-pi` and start {} again to keep them",
                            w.name, tab.title, spec.display, spec.display
                        );
                        warnings.push(crate::ports::OpenWarning { workspace: Some(w.name.clone()), message });
                    }
                }
            }
        }
    }
    open.warnings.extend(warnings);
    Ok(open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::AgentRef;
    use crate::testing::{A, FakeTerminal, open_ws};

    #[test]
    fn picks_the_terminal_asked_for_else_the_one_it_runs_in_else_the_one_running() {
        use TerminalKind::{Cmux, Tmux};
        let never = || -> bool { panic!("not probed when the answer is known") };
        let (tmux, cmux, both) =
            (Inside { tmux: true, cmux: false }, Inside { tmux: false, cmux: true }, Inside { tmux: true, cmux: true });
        assert_eq!(terminal_for(Some(Cmux), tmux, never, never), Cmux);
        assert_eq!(terminal_for(Some(Tmux), cmux, never, never), Tmux);
        assert_eq!(terminal_for(None, tmux, never, never), Tmux);
        assert_eq!(terminal_for(None, both, never, never), Tmux, "tmux inside a cmux tab");
        assert_eq!(terminal_for(None, cmux, never, never), Cmux);
        // Outside both: the one running, cmux first.
        let outside = Inside::default();
        assert_eq!(terminal_for(None, outside, || true, never), Cmux);
        assert_eq!(terminal_for(None, outside, || false, || true), Tmux);
        assert_eq!(terminal_for(None, outside, || false, || false), Cmux);
    }

    #[test]
    fn open_tabs_get_registry_kinds_and_valid_ids() {
        let fake = FakeTerminal::default();
        let mut ws = open_ws("W", "W", "/w", &[("a", "/w", Some("agy:c1")), ("b", "/w", Some("claude:bad"))]);
        ws.tabs[1].agent = Some(AgentRef {
            kind: "Claude".into(),
            session_id: "bad".into(),
            args: vec![],
            alt_session_id: Some(A.into()),
        });
        fake.state.borrow_mut().workspaces = vec![ws];
        let open = read_open(&fake, &crate::testing::agents(vec![])).unwrap();
        let agents: Vec<_> = open.workspaces[0].tabs.iter().map(|t| t.agent.clone().unwrap()).collect();
        assert_eq!((agents[0].kind.as_str(), agents[0].session_id.as_str()), ("antigravity", "c1"));
        assert_eq!((agents[1].kind.as_str(), agents[1].session_id.as_str()), ("claude", A));
    }
}
