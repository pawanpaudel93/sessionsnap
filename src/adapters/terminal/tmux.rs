//! tmux as a terminal: each tmux session is a workspace and each pane a tab. tmux doesn't know
//! which agent runs in a pane, so each tab carries the processes under its shell (exact arguments,
//! from `procs`), and `domain::running` finds the agent and its session in them.

use std::collections::HashMap;

use anyhow::Result;

use super::cmux::runner::{Output, ProcessRunner, Runner};
use super::procs::{Proc, Processes, System};
use crate::error::{Exit, failure};
use crate::ports::{LaunchTab, OpenState, OpenTab, OpenWorkspace, Shown, TabProcess, Terminal};

/// The session option that keeps a workspace's name when tmux can't (it turns `.` and `:` into `_`).
const NAME_OPTION: &str = "@sessionsnap_name";
/// Separates the fields of `list-panes` lines: a control character no name or path holds.
const SEP: char = '\u{1f}';
/// At most this many processes are read under one pane's shell.
const MAX_PROCESSES: usize = 20;

pub struct Tmux<R: Runner, P: Processes> {
    pub runner: R,
    pub program: String,
    pub procs: P,
}

impl Tmux<ProcessRunner, System<ProcessRunner>> {
    pub fn open_default() -> Self {
        Tmux {
            runner: ProcessRunner,
            program: std::env::var("SESSIONSNAP_TMUX_BIN").unwrap_or_else(|_| "tmux".into()),
            procs: System { runner: ProcessRunner },
        }
    }
}

/// One `list-panes` line.
struct Pane {
    session_id: String,
    name: String,
    window_name: String,
    pane_id: String,
    pid: u32,
    cwd: String,
}

impl<R: Runner, P: Processes> Tmux<R, P> {
    /// A tmux server runs, with at least one session.
    pub fn has_sessions(&self) -> bool {
        self.tmux(&["list-sessions"]).is_ok_and(|o| o.success)
    }

    fn tmux(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.runner.run(&self.program, &args).map_err(|_| self.not_installed())
    }

    fn not_installed(&self) -> anyhow::Error {
        failure(Exit::CmuxUnreachable, format!("could not run `{}`: is tmux installed and on PATH?", self.program))
    }

    fn tmux_ok(&self, args: &[&str]) -> Result<String> {
        let out = self.tmux(args)?;
        if !out.success {
            return Err(failure(
                Exit::Error,
                format!("`tmux {}` failed: {}", args.first().unwrap_or(&""), out.stderr.trim()),
            ));
        }
        Ok(out.stdout.trim().to_string())
    }

    /// Every pane; none when no tmux server runs.
    fn panes(&self) -> Result<Vec<Pane>> {
        let name_option = format!("#{{{NAME_OPTION}}}");
        let fields = [
            "#{session_id}",
            "#{session_name}",
            name_option.as_str(),
            "#{window_name}",
            "#{pane_id}",
            "#{pane_pid}",
            "#{pane_current_path}",
        ];
        let out = self.tmux(&["list-panes", "-a", "-F", &fields.join(&SEP.to_string())])?;
        if !out.success {
            let err = out.stderr.trim();
            if err.contains("no server running") || err.contains("error connecting") {
                return Ok(vec![]);
            }
            return Err(failure(Exit::Error, format!("`tmux list-panes` failed: {err}")));
        }
        Ok(out.stdout.lines().filter_map(parse_pane).collect())
    }

    /// What runs under each pane's shell, breadth first (the agent before the tools it runs).
    fn processes(&self, panes: &[Pane]) -> Result<HashMap<u32, Vec<TabProcess>>> {
        let procs = self.procs.list()?;
        let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
        for p in &procs {
            children.entry(p.ppid).or_default().push(p);
        }
        Ok(panes
            .iter()
            .map(|pane| {
                let mut queue: Vec<&Proc> = children.get(&pane.pid).cloned().unwrap_or_default();
                let mut i = 0;
                while i < queue.len() && queue.len() < MAX_PROCESSES {
                    queue.extend(children.get(&queue[i].pid).into_iter().flatten());
                    i += 1;
                }
                let found = queue
                    .iter()
                    .take(MAX_PROCESSES)
                    .filter_map(|p| Some(TabProcess { argv: self.procs.argv(p.pid)?, started: p.started }))
                    .collect();
                (pane.pid, found)
            })
            .collect())
    }

    /// Types `command` into the pane `target` and presses Enter, so the shell stays when it exits.
    fn type_command(&self, target: &str, tab: &LaunchTab) -> Result<()> {
        if let Some(command) = &tab.command {
            self.tmux_ok(&["send-keys", "-t", target, "-l", "--", command])?;
            self.tmux_ok(&["send-keys", "-t", target, "Enter"])?;
        }
        Ok(())
    }
}

impl<R: Runner, P: Processes> Terminal for Tmux<R, P> {
    fn name(&self) -> &'static str {
        "tmux"
    }

    /// tmux starts its server with its first session, so it "runs" whenever it can be run.
    fn is_running(&self) -> bool {
        self.tmux(&["-V"]).is_ok_and(|o| o.success)
    }

    fn ensure_running(&self) -> Result<()> {
        if self.is_running() { Ok(()) } else { Err(self.not_installed()) }
    }

    fn read_open_state(&self) -> Result<OpenState> {
        let panes = self.panes()?;
        let mut warnings = vec![];
        let mut processes = if panes.is_empty() {
            HashMap::new()
        } else {
            self.processes(&panes).unwrap_or_else(|e| {
                warnings.push(format!("could not read the processes in tmux, so no agent tabs were found: {e:#}"));
                HashMap::new()
            })
        };
        let mut workspaces: Vec<OpenWorkspace> = vec![];
        for pane in &panes {
            let tab = OpenTab {
                reference: Some(pane.pane_id.clone()),
                title: pane.window_name.clone(),
                cwd: pane.cwd.clone(),
                agent: None,
                processes: processes.remove(&pane.pid).unwrap_or_default(),
            };
            match workspaces.iter_mut().find(|w| w.id == pane.session_id) {
                Some(w) => w.tabs.push(tab),
                None => workspaces.push(OpenWorkspace {
                    id: pane.session_id.clone(),
                    reference: Some(pane.session_id.clone()),
                    name: pane.name.clone(),
                    cwd: pane.cwd.clone(),
                    git_branch: None,
                    pinned: false,
                    tabs: vec![tab],
                }),
            }
        }
        // A tmux server always has a session: none open means none runs, and tmux keeps no state.
        Ok(OpenState { live: !workspaces.is_empty(), state_version: None, workspaces, warnings })
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[LaunchTab]) -> Result<String> {
        tabs.iter().try_for_each(check_command)?;
        let first = tabs.first();
        let session_name = session_name(name);
        let mut args = vec!["new-session", "-d", "-P", "-F", "#{session_id} #{window_id}", "-s", session_name.as_str()];
        args.extend(["-c", first.map_or(cwd, |t| t.cwd.as_str())]);
        if let Some(t) = first.filter(|t| !t.title.is_empty()) {
            args.extend(["-n", t.title.as_str()]);
        }
        let out = self.tmux_ok(&args)?;
        let Some((session, window)) = out.split_once(' ') else {
            return Err(failure(Exit::Error, format!("tmux new-session printed no session: {out}")));
        };
        self.tmux_ok(&["set-option", "-t", session, NAME_OPTION, name])?;
        if let Some(t) = first {
            self.type_command(window, t)?;
        }
        for t in tabs.iter().skip(1) {
            self.add_tab(session, t)?;
        }
        Ok(session.to_string())
    }

    fn add_tab(&self, workspace_ref: &str, tab: &LaunchTab) -> Result<()> {
        check_command(tab)?;
        let target = format!("{workspace_ref}:");
        let mut args =
            vec!["new-window", "-d", "-P", "-F", "#{window_id}", "-t", target.as_str(), "-c", tab.cwd.as_str()];
        if !tab.title.is_empty() {
            args.extend(["-n", tab.title.as_str()]);
        }
        let window = self.tmux_ok(&args)?;
        self.type_command(&window, tab)
    }

    fn show_url(&self, _url: &str) -> Result<Shown> {
        Err(failure(Exit::Refused, "the web UI page shows in cmux; with tmux, open its link in a browser"))
    }
}

/// A `list-panes` line; None for one that doesn't parse.
fn parse_pane(line: &str) -> Option<Pane> {
    let f: Vec<&str> = line.split(SEP).collect();
    let [session_id, session_name, kept_name, window_name, pane_id, pid, cwd] = f[..] else { return None };
    Some(Pane {
        session_id: session_id.into(),
        name: if kept_name.is_empty() { session_name } else { kept_name }.into(),
        window_name: window_name.into(),
        pane_id: pane_id.into(),
        pid: pid.parse().ok()?,
        cwd: cwd.into(),
    })
}

/// tmux turns `.` and `:` in session names into `_`; the real name goes in `NAME_OPTION`.
fn session_name(name: &str) -> String {
    name.replace(['.', ':'], "_")
}

/// A command with a control character is never typed into a pane.
fn check_command(tab: &LaunchTab) -> Result<()> {
    match &tab.command {
        Some(c) if c.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') => Err(failure(
            Exit::Error,
            format!("not running the command of tab '{}': it has control characters", tab.title),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::SystemTime;

    use super::*;
    use crate::testing::A;

    /// tmux answering `list-panes` with `panes` (or "no server running" when None), logging calls.
    struct FakeTmux {
        panes: Option<String>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Runner for FakeTmux {
        fn run(&self, _program: &str, args: &[String]) -> Result<Output> {
            self.calls.borrow_mut().push(args.to_vec());
            let ok = |s: &str| Ok(Output { success: true, stdout: s.into(), stderr: String::new() });
            match args[0].as_str() {
                "list-panes" => match &self.panes {
                    Some(p) => ok(p),
                    None => Ok(Output {
                        success: false,
                        stdout: String::new(),
                        stderr: "no server running on /tmp/x".into(),
                    }),
                },
                "new-session" => ok("$7 @20\n"),
                "new-window" => ok("@21\n"),
                _ => ok(""),
            }
        }
    }

    #[derive(Default)]
    struct FakeProcs {
        procs: Vec<Proc>,
        argv: HashMap<u32, Vec<String>>,
    }

    impl Processes for FakeProcs {
        fn list(&self) -> Result<Vec<Proc>> {
            Ok(self.procs.clone())
        }
        fn argv(&self, pid: u32) -> Option<Vec<String>> {
            self.argv.get(&pid).cloned()
        }
    }

    fn line(session: &str, name: &str, kept: &str, window: &str, pane: &str, pid: u32, cwd: &str) -> String {
        [session, name, kept, window, pane, &pid.to_string(), cwd].join(&SEP.to_string())
    }

    /// Processes (pid, ppid, argv), all started at the epoch.
    fn procs(list: &[(u32, u32, &[&str])]) -> FakeProcs {
        FakeProcs {
            procs: list.iter().map(|&(pid, ppid, _)| Proc { pid, ppid, started: SystemTime::UNIX_EPOCH }).collect(),
            argv: list.iter().map(|&(pid, _, argv)| (pid, argv.iter().map(|a| a.to_string()).collect())).collect(),
        }
    }

    fn tmux(panes: &[String], procs: FakeProcs) -> Tmux<FakeTmux, FakeProcs> {
        let runner = FakeTmux { panes: Some(panes.join("\n")), calls: RefCell::default() };
        Tmux { runner, program: "tmux".into(), procs }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn sessions_are_workspaces_and_panes_are_tabs() {
        let t = tmux(
            &[
                line("$1", "api", "", "server", "%1", 100, "/w/api"),
                line("$1", "api", "", "logs", "%2", 101, "/w/api/logs"),
                line("$2", "my_app", "my.app", "edit", "%3", 102, "/w/app"),
            ],
            FakeProcs::default(),
        );
        let open = t.read_open_state().unwrap();
        assert!(open.live);
        let names: Vec<_> = open.workspaces.iter().map(|w| (w.name.as_str(), w.tabs.len())).collect();
        assert_eq!(names, [("api", 2), ("my.app", 1)]);
        let tab = &open.workspaces[0].tabs[1];
        assert_eq!(
            (tab.title.as_str(), tab.cwd.as_str(), tab.reference.as_deref()),
            ("logs", "/w/api/logs", Some("%2"))
        );
        assert_eq!(open.workspaces[0].reference.as_deref(), Some("$1"));
    }

    #[test]
    fn no_server_is_nothing_open() {
        let mut t = tmux(&[], FakeProcs::default());
        t.runner.panes = None;
        let open = t.read_open_state().unwrap();
        assert!(open.workspaces.is_empty() && !open.live);
    }

    #[test]
    fn each_tab_carries_what_runs_under_its_shell_nearest_first() {
        let t = tmux(
            &[line("$1", "w", "", "a", "%1", 100, "/w"), line("$1", "w", "", "b", "%2", 200, "/w")],
            procs(&[
                (100, 1, &["-zsh"]),
                (101, 100, &["claude", "--add-dir", "/a b"]),
                (102, 101, &["git", "status"]),
                (200, 1, &["-zsh"]),
                (300, 1, &["sshd"]),
            ]),
        );
        let open = t.read_open_state().unwrap();
        let first: Vec<Vec<String>> = open.workspaces[0].tabs[0].processes.iter().map(|p| p.argv.clone()).collect();
        assert_eq!(first, [strings(&["claude", "--add-dir", "/a b"]), strings(&["git", "status"])]);
        assert!(open.workspaces[0].tabs[1].processes.is_empty());
        assert!(open.workspaces[0].tabs.iter().all(|t| t.agent.is_none()), "named by domain::running");
    }

    #[test]
    fn creates_a_session_per_workspace_and_types_each_command() {
        let t = tmux(&[], FakeProcs::default());
        let tabs = [
            LaunchTab { title: "fix".into(), cwd: "/w/a".into(), command: Some(format!("claude --resume {A}")) },
            LaunchTab { title: "shell".into(), cwd: "/w/b".into(), command: None },
        ];
        assert_eq!(t.create_workspace("my.app", "/w", &tabs).unwrap(), "$7");
        let calls = t.runner.calls.borrow();
        let new_session =
            ["new-session", "-d", "-P", "-F", "#{session_id} #{window_id}", "-s", "my_app", "-c", "/w/a", "-n", "fix"];
        assert_eq!(calls[0], strings(&new_session));
        assert_eq!(calls[1], strings(&["set-option", "-t", "$7", NAME_OPTION, "my.app"]));
        assert_eq!(calls[2], strings(&["send-keys", "-t", "@20", "-l", "--", &format!("claude --resume {A}")]));
        assert_eq!(calls[3], strings(&["send-keys", "-t", "@20", "Enter"]));
        let new_window = ["new-window", "-d", "-P", "-F", "#{window_id}", "-t", "$7:", "-c", "/w/b", "-n", "shell"];
        assert_eq!(calls[4], strings(&new_window));
        assert_eq!(calls.len(), 5);
    }

    #[test]
    fn a_hostile_command_is_never_typed() {
        let t = tmux(&[], FakeProcs::default());
        let tab = LaunchTab { title: "x".into(), cwd: "/w".into(), command: Some("claude\nrm -rf ~".into()) };
        assert!(t.add_tab("$1", &tab).is_err());
        assert!(t.create_workspace("w", "/w", &[tab]).is_err());
        assert!(t.runner.calls.borrow().is_empty());
    }

    #[test]
    fn a_missing_tmux_is_unreachable() {
        struct Missing;
        impl Runner for Missing {
            fn run(&self, _: &str, _: &[String]) -> Result<Output> {
                anyhow::bail!("No such file or directory")
            }
        }
        let t = Tmux { runner: Missing, program: "tmux".into(), procs: FakeProcs::default() };
        assert!(!t.is_running());
        assert_eq!(crate::error::exit_of(&t.read_open_state().unwrap_err()), Exit::CmuxUnreachable);
    }
}
