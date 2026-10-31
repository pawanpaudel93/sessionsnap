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
/// The tmux name the session got with it: once the session is renamed, the kept name is stale.
const AS_OPTION: &str = "@sessionsnap_as";
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
    /// The session's ID with the server's start time: tmux numbers sessions from `$0` again after
    /// a restart, and a saved ID must not match an unrelated session then.
    workspace_id: String,
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

    /// Runs tmux with `-u`: without a UTF-8 locale (launchd gives none) tmux turns control
    /// characters, `SEP` included, and non-ASCII names into `_`.
    fn tmux(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<String> = std::iter::once("-u").chain(args.iter().copied()).map(String::from).collect();
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
        let (name_option, as_option) = (format!("#{{{NAME_OPTION}}}"), format!("#{{{AS_OPTION}}}"));
        let fields = [
            "#{start_time}",
            "#{session_id}",
            "#{session_name}",
            name_option.as_str(),
            as_option.as_str(),
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

    /// `name`, or `name-2`, `name-3`… when a session already has it (tmux refuses duplicates).
    fn free_name(&self, name: &str) -> String {
        let taken = |n: &str| self.tmux(&["has-session", "-t", &format!("={n}")]).is_ok_and(|o| o.success);
        (1..100)
            .map(|i| if i == 1 { name.to_string() } else { format!("{name}-{i}") })
            .find(|n| !taken(n))
            .unwrap_or_else(|| name.to_string())
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
                let message = format!("could not read the processes in tmux, so no agent tabs were found: {e:#}");
                warnings.push(crate::ports::OpenWarning { workspace: None, message });
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
            match workspaces.iter_mut().find(|w| w.id == pane.workspace_id) {
                Some(w) => w.tabs.push(tab),
                None => workspaces.push(OpenWorkspace {
                    id: pane.workspace_id.clone(),
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
        let tmux_name = self.free_name(&session_name(name));
        let (session_arg, cwd_arg) = (literal(&tmux_name), literal(first.map_or(cwd, |t| t.cwd.as_str())));
        let title_arg = first.map(|t| literal(&t.title)).filter(|t| !t.is_empty());
        let mut args = vec!["new-session", "-d", "-P", "-F", "#{session_id} #{window_id}", "-s", &session_arg];
        args.extend(["-c", &cwd_arg]);
        if let Some(title) = &title_arg {
            args.extend(["-n", title]);
        }
        let out = self.tmux_ok(&args)?;
        let Some((session, window)) = out.split_once(' ') else {
            return Err(failure(Exit::Error, format!("tmux new-session printed no session: {out}")));
        };
        // Option values are not formats, so these go as they are.
        self.tmux_ok(&["set-option", "-t", session, NAME_OPTION, name])?;
        self.tmux_ok(&["set-option", "-t", session, AS_OPTION, &tmux_name])?;
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
        let (target, cwd, title) = (format!("{workspace_ref}:"), literal(&tab.cwd), literal(&tab.title));
        let mut args = vec!["new-window", "-d", "-P", "-F", "#{window_id}", "-t", &target, "-c", &cwd];
        if !title.is_empty() {
            args.extend(["-n", &title]);
        }
        let window = self.tmux_ok(&args)?;
        self.type_command(&window, tab)
    }

    /// tmux has no browser: the default browser shows the page (`browser_open`).
    fn show_url(&self, url: &str) -> Result<Shown> {
        let (program, args, input, fragment_error) = browser_open(std::env::consts::OS, url)?;
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let out = match &input {
            Some(input) => self.runner.run_input(program, &args, input)?,
            None => self.runner.run(program, &args)?,
        };
        if !out.success {
            // Its message could quote the link; the caller keeps the key out of what it shows.
            return Err(failure(Exit::Error, format!("`{program}` could not open the browser: {}", out.stderr.trim())));
        }
        Ok(Shown { place: "browser", fragment_error })
    }
}

/// How to open `url` in the default browser on `os`: the program, its arguments, its stdin, and
/// why the `#` part (which carries the page's key, and never goes on a command line) was left off.
/// macOS: `osascript` reads `open location "<url>"` from stdin. Elsewhere: `xdg-open` with the
/// link before its `#`.
type BrowserOpen<'a> = (&'static str, Vec<&'a str>, Option<String>, Option<String>);
fn browser_open<'a>(os: &str, url: &'a str) -> Result<BrowserOpen<'a>> {
    // The server builds the link from a port and a hex key; nothing here may end the string.
    if url.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(failure(Exit::Error, "not opening a link with quotes, backslashes or control characters"));
    }
    if os == "macos" {
        return Ok(("osascript", vec!["-"], Some(format!("open location \"{url}\"")), None));
    }
    let bare = url.split('#').next().unwrap_or(url);
    let why = (bare.len() < url.len()).then(|| "xdg-open takes the link only on its command line".to_string());
    Ok(("xdg-open", vec![bare], None, why))
}

/// A `list-panes` line; None for one that doesn't parse.
fn parse_pane(line: &str) -> Option<Pane> {
    let f: Vec<&str> = line.split(SEP).collect();
    let [started, session_id, session_name, kept_name, kept_as, window_name, pane_id, pid, cwd] = f[..] else {
        return None;
    };
    // The kept name holds only while the session still has the tmux name it was made with.
    let kept = !kept_name.is_empty() && kept_as == session_name;
    Some(Pane {
        workspace_id: format!("{session_id}@{started}"),
        session_id: session_id.into(),
        name: if kept { kept_name } else { session_name }.into(),
        window_name: window_name.into(),
        pane_id: pane_id.into(),
        pid: pid.parse().ok()?,
        cwd: cwd.into(),
    })
}

/// `value` for a tmux argument that tmux expands as a format (`-s`, `-c`, `-n`): `#` doubled, so
/// `#(…)` in a saved name or folder is never run as a command.
fn literal(value: &str) -> String {
    value.replace('#', "##")
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

    /// tmux answering `list-panes` with `panes` (or "no server running" when None), with sessions
    /// named `taken` open, logging calls (each must start with `-u`, which the log leaves out).
    struct FakeTmux {
        panes: Option<String>,
        taken: Vec<String>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Runner for FakeTmux {
        fn run(&self, _program: &str, args: &[String]) -> Result<Output> {
            assert_eq!(args[0], "-u", "every tmux call writes UTF-8 whatever the locale");
            let args = &args[1..];
            self.calls.borrow_mut().push(args.to_vec());
            if args[0] == "has-session" {
                let open = self.taken.iter().any(|n| args[2] == format!("={n}"));
                return Ok(Output { success: open, ..Default::default() });
            }
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

    /// A `list-panes` line of a server started at 1700; a kept name is kept for tmux name `name`.
    fn line(session: &str, name: &str, kept: &str, window: &str, pane: &str, pid: u32, cwd: &str) -> String {
        let kept_as = if kept.is_empty() { "" } else { name };
        ["1700", session, name, kept, kept_as, window, pane, &pid.to_string(), cwd].join(&SEP.to_string())
    }

    /// Processes (pid, ppid, argv), all started at the epoch.
    fn procs(list: &[(u32, u32, &[&str])]) -> FakeProcs {
        FakeProcs {
            procs: list.iter().map(|&(pid, ppid, _)| Proc { pid, ppid, started: SystemTime::UNIX_EPOCH }).collect(),
            argv: list.iter().map(|&(pid, _, argv)| (pid, argv.iter().map(|a| a.to_string()).collect())).collect(),
        }
    }

    fn tmux(panes: &[String], procs: FakeProcs) -> Tmux<FakeTmux, FakeProcs> {
        let runner = FakeTmux { panes: Some(panes.join("\n")), taken: vec![], calls: RefCell::default() };
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
        assert_eq!(calls[0], strings(&["has-session", "-t", "=my_app"]));
        assert_eq!(calls[1], strings(&new_session));
        assert_eq!(calls[2], strings(&["set-option", "-t", "$7", NAME_OPTION, "my.app"]));
        assert_eq!(calls[3], strings(&["set-option", "-t", "$7", AS_OPTION, "my_app"]));
        assert_eq!(calls[4], strings(&["send-keys", "-t", "@20", "-l", "--", &format!("claude --resume {A}")]));
        assert_eq!(calls[5], strings(&["send-keys", "-t", "@20", "Enter"]));
        let new_window = ["new-window", "-d", "-P", "-F", "#{window_id}", "-t", "$7:", "-c", "/w/b", "-n", "shell"];
        assert_eq!(calls[6], strings(&new_window));
        assert_eq!(calls.len(), 7);
    }

    #[test]
    fn a_hash_in_a_name_folder_or_title_reaches_tmux_escaped() {
        let t = tmux(&[], FakeProcs::default());
        let tab = LaunchTab { title: "#(touch /tmp/t)".into(), cwd: "/w/#(id)".into(), command: None };
        t.create_workspace("ws#(id)", "/w", std::slice::from_ref(&tab)).unwrap();
        t.add_tab("$7", &tab).unwrap();
        let calls = t.runner.calls.borrow();
        let new_session = &calls[1];
        assert!(new_session.windows(2).any(|w| w == ["-s", "ws##(id)"]), "{new_session:?}");
        assert!(new_session.windows(2).any(|w| w == ["-c", "/w/##(id)"]), "{new_session:?}");
        assert!(new_session.windows(2).any(|w| w == ["-n", "##(touch /tmp/t)"]), "{new_session:?}");
        let new_window = calls.iter().find(|c| c[0] == "new-window").unwrap();
        assert!(
            new_window.windows(2).any(|w| w == ["-c", "/w/##(id)"])
                && new_window.windows(2).any(|w| w[0] == "-n" && w[1].starts_with("##("))
        );
        // The kept name is the real one, unescaped (options are not formats).
        assert!(calls.iter().any(|c| c == &strings(&["set-option", "-t", "$7", NAME_OPTION, "ws#(id)"])));
    }

    #[test]
    fn a_taken_session_name_gets_a_number() {
        let mut t = tmux(&[], FakeProcs::default());
        t.runner.taken = vec!["my_app".into(), "my_app-2".into()];
        t.create_workspace("my.app", "/w", &[]).unwrap();
        let calls = t.runner.calls.borrow();
        let new_session = calls.iter().find(|c| c[0] == "new-session").unwrap();
        assert!(new_session.windows(2).any(|w| w == ["-s", "my_app-3"]), "{new_session:?}");
        assert!(calls.iter().any(|c| c == &strings(&["set-option", "-t", "$7", AS_OPTION, "my_app-3"])));
    }

    #[test]
    fn a_renamed_session_drops_its_stale_kept_name_and_ids_carry_the_server_start() {
        let renamed = ["1700", "$2", "work", "my.app", "my_app", "edit", "%3", "102", "/w"].join(&SEP.to_string());
        let t = tmux(&[renamed, line("$3", "my_app", "my.app", "x", "%4", 103, "/w")], FakeProcs::default());
        let open = t.read_open_state().unwrap();
        let names: Vec<_> = open.workspaces.iter().map(|w| (w.name.as_str(), w.id.as_str())).collect();
        assert_eq!(names, [("work", "$2@1700"), ("my.app", "$3@1700")]);
        assert_eq!(open.workspaces[0].reference.as_deref(), Some("$2"));
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
    fn the_browser_gets_the_key_only_off_the_command_line() {
        let url = "http://127.0.0.1:7777/#t=abc123";
        let (program, args, input, why) = browser_open("macos", url).unwrap();
        assert_eq!((program, args), ("osascript", vec!["-"]));
        assert_eq!(input.as_deref(), Some("open location \"http://127.0.0.1:7777/#t=abc123\""));
        assert_eq!(why, None);
        let (program, args, input, why) = browser_open("linux", url).unwrap();
        assert_eq!((program, args, input), ("xdg-open", vec!["http://127.0.0.1:7777/"], None));
        assert!(why.is_some(), "the page gets no key; the server prints the keyed link");
        assert!(browser_open("macos", "http://x/\" & do shell script \"rm\"").is_err());
    }

    #[test]
    fn shows_the_page_in_the_browser() {
        struct Osascript(RefCell<Option<String>>);
        impl Runner for Osascript {
            fn run(&self, _: &str, _: &[String]) -> Result<Output> {
                unreachable!("macOS opens the link from stdin")
            }
            fn run_input(&self, program: &str, args: &[String], input: &str) -> Result<Output> {
                assert_eq!((program, args), ("osascript", &["-".to_string()][..]));
                *self.0.borrow_mut() = Some(input.into());
                Ok(Output { success: true, ..Default::default() })
            }
        }
        if std::env::consts::OS != "macos" {
            return;
        }
        let t = Tmux { runner: Osascript(RefCell::default()), program: "tmux".into(), procs: FakeProcs::default() };
        let shown = t.show_url("http://127.0.0.1:7777/#t=k").unwrap();
        assert_eq!((shown.place, shown.fragment_error), ("browser", None));
        assert_eq!(t.runner.0.borrow().as_deref(), Some("open location \"http://127.0.0.1:7777/#t=k\""));
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
