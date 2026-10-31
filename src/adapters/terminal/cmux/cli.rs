use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};

use super::runner::{Output, ProcessRunner, Runner};
use super::{socket, state_file, tree};
use crate::error::{Exit, failure};
use crate::ports::{LaunchTab, OpenState, Shown, Terminal};

/// The cmux terminal: reads state from the `cmux` CLI and the state file, opens tabs via the CLI.
pub struct CmuxCli<R: Runner> {
    pub runner: R,
    pub program: String,
    pub state_path: PathBuf,
    pub launch_timeout: Duration,
    /// The command that starts the cmux app; None where sessionsnap does not know one.
    pub launcher: Option<Vec<String>>,
    /// cmux's control socket, for links that must stay off command lines; None when unknown.
    pub socket: Option<PathBuf>,
    /// How long each cmux call of `show_url` may take before it is killed: the web UI calls it,
    /// and a hung cmux must not hold one of its workers.
    pub call_timeout: Duration,
}

/// How to start cmux on `os`: `open -a cmux` on macOS. On Linux cmux ships as a separate app
/// whose launcher sessionsnap does not know yet, so the user starts it.
pub fn launcher_for(os: &str) -> Option<Vec<String>> {
    (os == "macos").then(|| vec!["open".to_string(), "-a".to_string(), "cmux".to_string()])
}

impl CmuxCli<ProcessRunner> {
    pub fn open_default() -> Result<Self> {
        Ok(CmuxCli {
            runner: ProcessRunner,
            program: std::env::var("SESSIONSNAP_CMUX_BIN").unwrap_or_else(|_| "cmux".into()),
            state_path: state_file::default_path()?,
            launch_timeout: Duration::from_secs(30),
            launcher: launcher_for(std::env::consts::OS),
            socket: socket::default_path(),
            call_timeout: Duration::from_secs(15),
        })
    }
}

impl<R: Runner> CmuxCli<R> {
    /// Brings up the Dock in the right sidebar: cmux pauses pages in a hidden Dock.
    fn show_dock(&self) -> Result<()> {
        self.cmux_ok_within(&["right-sidebar", "set", "dock", "--no-focus"]).map(drop)
    }

    fn cmux(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.runner.run(&self.program, &args)
    }

    /// `cmux`, killed after `call_timeout`.
    fn cmux_within(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.runner.run_timeout(&self.program, &args, self.call_timeout)
    }

    /// Runs a cmux command and returns its stdout, failing on a non-zero exit.
    fn cmux_ok(&self, args: &[&str]) -> Result<String> {
        Self::stdout_of(args, self.cmux(args)?)
    }

    /// `cmux_ok`, killed after `call_timeout`.
    fn cmux_ok_within(&self, args: &[&str]) -> Result<String> {
        Self::stdout_of(args, self.cmux_within(args)?)
    }

    fn stdout_of(args: &[&str], out: Output) -> Result<String> {
        if !out.success {
            return Err(failure(
                Exit::Error,
                format!("`cmux {}` failed: {}", args.first().unwrap_or(&""), out.stderr.trim()),
            ));
        }
        Ok(out.stdout)
    }
}

impl<R: Runner> Terminal for CmuxCli<R> {
    fn is_running(&self) -> bool {
        self.cmux_within(&["ping"]).map(|o| o.success).unwrap_or(false)
    }

    fn ensure_running(&self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let Some((program, args)) = self.launcher.as_deref().and_then(<[String]>::split_first) else {
            return Err(failure(Exit::CmuxUnreachable, "cmux is not running; start it and try again"));
        };
        self.runner.run(program, args)?;
        let deadline = Instant::now() + self.launch_timeout;
        loop {
            if self.is_running() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    Exit::CmuxUnreachable,
                    format!("cmux did not start within {}s", self.launch_timeout.as_secs()),
                ));
            }
            std::thread::sleep(Duration::from_millis(250).min(self.launch_timeout));
        }
    }

    fn read_open_state(&self) -> Result<OpenState> {
        let (version, file) = state_file::read(&self.state_path)?;
        if !self.is_running() {
            return Ok(OpenState {
                live: false,
                state_version: Some(version),
                workspaces: tree::from_file(&file),
                warnings: vec![],
            });
        }
        // `--id-format both` keeps surface IDs, which match panels in the state file.
        // Bounded: `save` holds its snapshot's lock meanwhile, and a lock is broken as stale after
        // `lock::STALE`.
        let with_ids = ["--json", "--id-format", "both", "tree", "--all"];
        let out = self.cmux_within(&with_ids)?;
        let text = if !out.success && out.stderr.contains("id-format") {
            // A cmux without the flag: its tree has no surface IDs, and `join` matches by tty.
            self.cmux_ok_within(&["--json", "tree", "--all"])?
        } else {
            Self::stdout_of(&with_ids, out)?
        };
        let tree = tree::parse(&text)?;
        let (workspaces, warning) = tree::join(tree, &file);
        if let Some(warning) = warning {
            eprintln!("sessionsnap: {warning}");
        }
        Ok(OpenState { live: true, state_version: Some(version), workspaces, warnings: vec![] })
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[LaunchTab]) -> Result<String> {
        for t in tabs {
            check_command(t, false)?;
        }
        let layout = layout_json(tabs);
        let out = self.cmux_ok(&["new-workspace", "--name", name, "--cwd", cwd, "--layout", layout.as_str()])?;
        find_ref(&out, "workspace")
            .ok_or_else(|| failure(Exit::Error, format!("cmux new-workspace printed no workspace ref: {}", out.trim())))
    }

    fn add_tab(&self, workspace_ref: &str, tab: &LaunchTab) -> Result<()> {
        check_command(tab, true)?;
        let out =
            self.cmux_ok(&["new-surface", "--workspace", workspace_ref, "--working-directory", tab.cwd.as_str()])?;
        let surface = find_ref(&out, "surface")
            .ok_or_else(|| failure(Exit::Error, format!("cmux new-surface printed no surface ref: {}", out.trim())))?;
        if let Some(command) = &tab.command {
            // `cmux send` turns the two characters \n into Enter.
            let text = format!("{command}\\n");
            self.cmux_ok(&["send", "--workspace", workspace_ref, "--surface", surface.as_str(), "--", text.as_str()])?;
        }
        self.cmux_ok(&[
            "rename-tab",
            "--workspace",
            workspace_ref,
            "--surface",
            surface.as_str(),
            "--title",
            tab.title.as_str(),
        ])?;
        Ok(())
    }

    fn show_url(&self, url: &str) -> Result<Shown> {
        // The `#` part may carry the UI's key, and any local user can read a command line with
        // `ps`: cmux's command line gets only the address, the whole link goes over its socket.
        let (base, fragment) = url.split_once('#').map_or((url, None), |(b, f)| (b, Some(f).filter(|f| !f.is_empty())));
        let (surface, place) = if let Some((surface, in_dock)) =
            tree::find_browser(&self.cmux_ok_within(&["--json", "tree", "--all"])?, base)?
        {
            if fragment.is_none() {
                self.cmux_ok_within(&["browser", surface.as_str(), "goto", base])?;
            }
            if in_dock {
                self.show_dock()?;
            }
            (Some(surface), "reused")
        } else {
            let out = self.cmux_within(&["new-surface", "--type", "browser", "--placement", "dock", "--url", base])?;
            if out.success {
                self.show_dock()?;
                (find_ref(&out.stdout, "surface"), "dock")
            } else {
                // cmux 0.64 keeps the Dock behind Settings → Beta Features; without it use a normal tab.
                let out = self.cmux_ok_within(&["browser", "open", base])?;
                (find_ref(&out, "surface"), "tab")
            }
        };
        let Some(fragment) = fragment else { return Ok(Shown::whole(place)) };
        let result = self.navigate(surface, base, url).map_err(|e| {
            // cmux's message could quote the link.
            format!("{e:#}").replace(url, base).replace(fragment, "…")
        });
        Ok(Shown { place, fragment_error: result.err() })
    }
}

impl<R: Runner> CmuxCli<R> {
    /// Points the browser `surface` (looked up by `base` when cmux printed no ref) at `url` over
    /// cmux's socket.
    fn navigate(&self, surface: Option<String>, base: &str, url: &str) -> Result<()> {
        let path = self.socket.as_deref().ok_or_else(|| anyhow::anyhow!("cmux's socket was not found"))?;
        let surface = match surface {
            Some(s) => s,
            None => tree::find_browser(&self.cmux_ok_within(&["--json", "tree", "--all"])?, base)?
                .map(|(s, _)| s)
                .ok_or_else(|| anyhow::anyhow!("cmux did not report the new browser tab"))?,
        };
        socket::call(path, "browser.navigate", json!({"surface_id": surface, "url": url})).map(drop)
    }
}

/// Refuses a command cmux would not type as given: a control character acts on the terminal
/// (^C, Enter, ESC sequences), and `cmux send` (`typed`) turns backslash sequences such as `\n`
/// into keys, so a backslash cannot be sent as itself.
fn check_command(tab: &LaunchTab, typed: bool) -> Result<()> {
    let Some(command) = &tab.command else { return Ok(()) };
    if command.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(failure(
            Exit::Error,
            format!("not running the command of tab '{}': it has control characters", tab.title),
        ));
    }
    if typed && command.contains('\\') {
        return Err(failure(
            Exit::Error,
            format!(
                "not typing the command of tab '{}' into cmux: it has a backslash, which `cmux send` would turn into a key",
                tab.title
            ),
        ));
    }
    Ok(())
}

/// `--layout` JSON for `cmux new-workspace`: one pane holding every tab.
pub fn layout_json(tabs: &[LaunchTab]) -> String {
    let surfaces: Vec<Value> = tabs
        .iter()
        .map(|t| {
            let mut s = json!({"type": "terminal", "name": t.title, "cwd": t.cwd});
            if let Some(command) = &t.command {
                s["command"] = json!(command);
            }
            s
        })
        .collect();
    json!({"pane": {"surfaces": surfaces}}).to_string()
}

/// The first "<kind>:<digits>" token in cmux output.
pub fn find_ref(text: &str, kind: &str) -> Option<String> {
    let prefix = format!("{kind}:");
    text.split(|c: char| c.is_whitespace() || ",\"()".contains(c))
        .find(|tok| tok.strip_prefix(&prefix).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::terminal::cmux::tree::TEST_TREE;
    use crate::error::exit_of;
    use crate::testing::A;
    use serde_json::json;
    use std::cell::RefCell;

    struct FakeRunner {
        calls: RefCell<Vec<Vec<String>>>,
        ping_ok: bool,
        dock_ok: bool,
        tree: &'static str,
        /// A cmux that doesn't know `--id-format` (and so prints no surface IDs).
        old_cmux: bool,
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            let mut call = vec![program.to_string()];
            call.extend(args.iter().cloned());
            self.calls.borrow_mut().push(call);
            let ok = |s: &str| Ok(Output { success: true, stdout: s.into(), stderr: String::new() });
            match args.first().map(String::as_str) {
                Some("ping") => Ok(Output { success: self.ping_ok, ..Default::default() }),
                Some("--json") if self.old_cmux && args.iter().any(|a| a == "--id-format") => Ok(Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "Error: unknown option '--id-format'".into(),
                }),
                Some("--json") if self.old_cmux => {
                    let tree: Value = serde_json::from_str(self.tree).unwrap();
                    ok(&strip_ids(tree).to_string())
                }
                Some("--json") => ok(self.tree),
                Some("new-workspace") => ok("OK workspace:12\n"),
                Some("new-surface") if !self.dock_ok && args.iter().any(|a| a == "dock") => Ok(Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "Error: invalid_params: Dock placement is disabled".into(),
                }),
                Some("new-surface") => ok("OK surface:40 pane:3 workspace:1\n"),
                Some("browser") if args.get(1).is_some_and(|a| a == "open") => ok("OK surface:41 pane:3 workspace:1\n"),
                _ => ok("OK\n"),
            }
        }
    }

    fn backend(ping_ok: bool) -> CmuxCli<FakeRunner> {
        CmuxCli {
            runner: FakeRunner {
                calls: RefCell::new(vec![]),
                ping_ok,
                dock_ok: true,
                tree: TEST_TREE,
                old_cmux: false,
            },
            program: "cmux".into(),
            state_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cmux-state.json"),
            launch_timeout: Duration::from_millis(10),
            launcher: launcher_for("macos"),
            socket: None,
            call_timeout: Duration::from_secs(15),
        }
    }

    /// A cmux that answers `ping` but hangs on `tree`: reading what is open fails after
    /// `call_timeout` (well under a lock's stale age), so a save never holds its lock for long.
    #[cfg(unix)]
    #[test]
    fn a_hung_tree_does_not_hold_a_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cmux = dir.path().join("cmux");
        std::fs::write(&cmux, "#!/bin/sh\necho $$ >> \"$(dirname \"$0\")/pids\"\n[ \"$1\" = ping ] && exit 0\nwhile :; do sleep 1; done\n").unwrap();
        std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        let b = CmuxCli {
            runner: ProcessRunner,
            program: cmux.to_string_lossy().into_owned(),
            state_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cmux-state.json"),
            launch_timeout: Duration::from_millis(10),
            launcher: None,
            socket: None,
            // Long enough for the first run of a new script (macOS checks it), short for the test.
            call_timeout: Duration::from_secs(3),
        };
        // macOS checks a new script the first time it runs, which can take seconds under load.
        std::process::Command::new(&cmux).arg("ping").status().unwrap();
        let started = Instant::now();
        let err = b.read_open_state().unwrap_err().to_string();
        assert!(started.elapsed() < Duration::from_secs(20), "took {:?}", started.elapsed());
        assert!(err.contains("did not answer"), "{err}");
        assert!(b.call_timeout < crate::adapters::store::lock::STALE);
        assert_stopped(dir.path());
    }

    /// Every fake cmux started in `dir` (it writes its PID to `dir/pids`) is gone.
    #[cfg(unix)]
    fn assert_stopped(dir: &std::path::Path) {
        let pids = std::fs::read_to_string(dir.join("pids")).unwrap_or_default();
        for pid in pids.split_whitespace() {
            let alive =
                std::process::Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status();
            assert!(!alive.unwrap().success(), "fake cmux {pid} is still running");
        }
    }

    /// A cmux that never answers: each call `show_url` makes is killed after `call_timeout`.
    #[cfg(unix)]
    #[test]
    fn a_hung_cmux_does_not_hold_show_url() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cmux = dir.path().join("cmux");
        std::fs::write(&cmux, "#!/bin/sh\necho $$ >> \"$(dirname \"$0\")/pids\"\nwhile :; do sleep 1; done\n").unwrap();
        std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        let b = CmuxCli {
            runner: ProcessRunner,
            program: cmux.to_string_lossy().into_owned(),
            state_path: dir.path().join("state.json"),
            launch_timeout: Duration::from_millis(10),
            launcher: None,
            socket: None,
            call_timeout: Duration::from_millis(300),
        };
        let started = Instant::now();
        let err = b.show_url("http://127.0.0.1:1/#t=secret-key").unwrap_err().to_string();
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert!(err.contains("did not answer"), "{err}");
        assert!(!err.contains("secret-key"), "{err}");
        assert_stopped(dir.path());
    }

    /// A cmux socket in a short temp folder (macOS caps socket paths at 104 bytes) that answers one
    /// request with `answer`. The thread returns the request line, or None if none came in 5 s.
    fn fake_socket(answer: &'static str) -> (tempfile::TempDir, PathBuf, std::thread::JoinHandle<Option<String>>) {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::Builder::new().prefix("ss").tempdir_in("/tmp").unwrap();
        let path = dir.path().join("s");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => return None,
                }
            };
            stream.set_nonblocking(false).unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(answer.as_bytes()).unwrap();
            Some(line)
        });
        (dir, path, server)
    }

    const OK_ANSWER: &str = "{\"ok\":true,\"result\":{},\"id\":\"sessionsnap\"}\n";

    /// No argument cmux was run with holds the link's `#` part.
    fn assert_no_key_in_argv(b: &CmuxCli<FakeRunner>) {
        for call in b.runner.calls.borrow().iter() {
            assert!(call.iter().all(|a| !a.contains("#t=") && !a.contains(KEY)), "{call:?}");
        }
    }

    /// The request the fake socket received, as JSON.
    fn navigate_request(server: std::thread::JoinHandle<Option<String>>) -> serde_json::Value {
        let line = server.join().unwrap().expect("no request reached cmux's socket");
        assert!(line.ends_with('\n'), "{line:?}");
        serde_json::from_str(&line).unwrap()
    }

    fn launch(title: &str, cwd: &str, command: Option<&str>) -> LaunchTab {
        LaunchTab { title: title.into(), cwd: cwd.into(), command: command.map(str::to_string) }
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef";
    const BASE: &str = "http://127.0.0.1:47317/";

    fn ui() -> String {
        format!("{BASE}#t={KEY}")
    }

    #[test]
    fn show_url_adds_a_dock_browser_and_hands_it_the_key_over_the_socket() {
        let (_dir, path, server) = fake_socket(OK_ANSWER);
        let mut b = backend(true);
        b.socket = Some(path);
        assert_eq!(b.show_url(&ui()).unwrap(), Shown::whole("dock"));
        {
            let calls = b.runner.calls.borrow();
            let n = calls.len();
            assert_eq!(&calls[n - 2][1..], ["new-surface", "--type", "browser", "--placement", "dock", "--url", BASE]);
            // A hidden Dock pauses its pages, so the Dock is brought up too.
            assert_eq!(&calls[n - 1][1..], ["right-sidebar", "set", "dock", "--no-focus"]);
        }
        let req = navigate_request(server);
        assert_eq!(req["method"], "browser.navigate");
        assert_eq!(req["params"], json!({"surface_id": "surface:40", "url": ui()}));
        assert_no_key_in_argv(&b);
    }

    #[test]
    fn show_url_falls_back_to_a_tab_without_dock() {
        let (_dir, path, server) = fake_socket(OK_ANSWER);
        let mut b = backend(true);
        b.runner.dock_ok = false;
        b.socket = Some(path);
        assert_eq!(b.show_url(&ui()).unwrap(), Shown::whole("tab"));
        assert_eq!(&b.runner.calls.borrow().last().unwrap()[1..], ["browser", "open", BASE]);
        assert_eq!(navigate_request(server)["params"], json!({"surface_id": "surface:41", "url": ui()}));
        assert_no_key_in_argv(&b);
    }

    #[test]
    fn show_url_reuses_an_open_page() {
        let (_dir, path, server) = fake_socket(OK_ANSWER);
        let mut b = backend(true);
        b.runner.tree = crate::adapters::terminal::cmux::tree::TEST_BROWSER_TREE;
        b.socket = Some(path);
        assert_eq!(b.show_url(&ui()).unwrap(), Shown::whole("reused"));
        assert_eq!(&b.runner.calls.borrow().last().unwrap()[1..], ["right-sidebar", "set", "dock", "--no-focus"]);
        assert_eq!(navigate_request(server)["params"], json!({"surface_id": "surface:71", "url": ui()}));
        assert_no_key_in_argv(&b);
    }

    #[test]
    fn show_url_reuses_a_tab_outside_the_dock_without_opening_the_dock() {
        let (_dir, path, server) = fake_socket(OK_ANSWER);
        let mut b = backend(true);
        b.runner.tree = r#"{"windows":[{"workspaces":[{"panes":[{"surfaces":[
            {"ref":"surface:3","type":"browser","url":"http://127.0.0.1:47317/"}]}]}]}]}"#;
        b.socket = Some(path);
        assert_eq!(b.show_url(&ui()).unwrap(), Shown::whole("reused"));
        assert!(b.runner.calls.borrow().iter().all(|c| c[1] != "right-sidebar"));
        assert_eq!(navigate_request(server)["params"]["surface_id"], "surface:3");
    }

    #[test]
    fn a_link_without_a_key_goes_on_the_command_line() {
        let mut b = backend(true);
        b.runner.tree = crate::adapters::terminal::cmux::tree::TEST_BROWSER_TREE;
        assert_eq!(b.show_url(BASE).unwrap(), Shown::whole("reused"));
        assert!(b.runner.calls.borrow().iter().any(|c| c[1..] == ["browser", "surface:71", "goto", BASE]));
    }

    #[test]
    fn without_the_socket_shows_the_page_without_its_key() {
        for socket in [None, Some(PathBuf::from("/nonexistent/cmux.sock"))] {
            let mut b = backend(true);
            b.socket = socket;
            let shown = b.show_url(&ui()).unwrap();
            assert_eq!(shown.place, "dock");
            let why = shown.fragment_error.expect("the key cannot have reached the page");
            assert!(!why.contains(KEY), "{why}");
            assert_no_key_in_argv(&b);
        }
    }

    #[test]
    fn a_refused_navigate_keeps_the_key_out_of_the_reason() {
        let answer = "{\"ok\":false,\"error\":{\"code\":\"not_found\",\"message\":\"no surface for http://127.0.0.1:47317/#t=0123456789abcdef0123456789abcdef\"}}\n";
        let (_dir, path, server) = fake_socket(answer);
        let mut b = backend(true);
        b.socket = Some(path);
        let why = b.show_url(&ui()).unwrap().fragment_error.unwrap();
        assert!(why.contains("not_found"), "{why}");
        assert!(!why.contains(KEY), "{why}");
        server.join().unwrap().unwrap();
        assert_no_key_in_argv(&b);
    }

    /// `tree` as a cmux prints it without `--id-format both`: no surface IDs.
    fn strip_ids(mut v: Value) -> Value {
        if let Some(surfaces) = v.get_mut("surfaces").and_then(Value::as_array_mut) {
            for s in surfaces {
                s.as_object_mut().unwrap().remove("id");
            }
        }
        match &mut v {
            Value::Object(map) => map.values_mut().for_each(|c| *c = strip_ids(c.take())),
            Value::Array(items) => items.iter_mut().for_each(|c| *c = strip_ids(c.take())),
            _ => {}
        }
        v
    }

    /// A cmux that rejects `--id-format`: asked again without it, matched by tty (never titles).
    #[test]
    fn a_cmux_without_id_format_is_matched_by_tty() {
        let mut b = backend(true);
        b.runner.old_cmux = true;
        let s = b.read_open_state().unwrap();
        assert_eq!(s.workspaces[0].tabs[1].agent.as_ref().unwrap().session_id, A);
        let trees: Vec<Vec<String>> =
            b.runner.calls.borrow().iter().filter(|c| c.get(1).is_some_and(|a| a == "--json")).cloned().collect();
        assert_eq!(trees.len(), 2, "{trees:?}");
        assert_eq!(trees[1], ["cmux", "--json", "tree", "--all"].map(String::from));
    }

    #[test]
    fn reads_live_state_when_running() {
        let b = backend(true);
        let s = b.read_open_state().unwrap();
        // Without `--id-format both` cmux leaves out surface IDs, and tabs can't be told apart.
        let tree_call = ["cmux", "--json", "--id-format", "both", "tree", "--all"].map(String::from).to_vec();
        assert!(b.runner.calls.borrow().contains(&tree_call), "{:?}", b.runner.calls.borrow());
        assert!(s.live);
        assert_eq!(s.state_version, Some(1));
        assert_eq!(s.workspaces[0].tabs[1].agent.as_ref().unwrap().session_id, A);
    }

    #[test]
    fn falls_back_to_state_file_when_not_running() {
        let s = backend(false).read_open_state().unwrap();
        assert!(!s.live);
        assert_eq!(s.workspaces.len(), 2);
    }

    #[test]
    fn create_workspace_sends_layout_and_returns_ref() {
        let b = backend(true);
        let tabs = vec![
            launch("fix login", "/w/alpha/sub", Some(&format!("claude --resume {A}"))),
            launch("zsh", "/w/alpha", None),
        ];
        assert_eq!(b.create_workspace("Alpha", "/w/alpha", &tabs).unwrap(), "workspace:12");
        let calls = b.runner.calls.borrow();
        let call = calls.last().unwrap();
        assert_eq!(&call[1..6], ["new-workspace", "--name", "Alpha", "--cwd", "/w/alpha"]);
        assert_eq!(call[6], "--layout");
        let layout: serde_json::Value = serde_json::from_str(&call[7]).unwrap();
        assert_eq!(
            layout["pane"]["surfaces"][0],
            json!({"type": "terminal", "name": "fix login", "cwd": "/w/alpha/sub", "command": format!("claude --resume {A}")})
        );
        assert_eq!(layout["pane"]["surfaces"][1], json!({"type": "terminal", "name": "zsh", "cwd": "/w/alpha"}));
    }

    #[test]
    fn add_tab_creates_surface_then_sends_and_renames() {
        let b = backend(true);
        b.add_tab("workspace:1", &launch("fix login", "/w/alpha/sub", Some(&format!("claude --resume {A}")))).unwrap();
        let calls = b.runner.calls.borrow();
        let send = format!("claude --resume {A}\\n");
        assert_eq!(
            calls[0],
            ["cmux", "new-surface", "--workspace", "workspace:1", "--working-directory", "/w/alpha/sub"]
        );
        assert_eq!(
            calls[1],
            ["cmux", "send", "--workspace", "workspace:1", "--surface", "surface:40", "--", send.as_str()]
        );
        assert_eq!(
            calls[2],
            ["cmux", "rename-tab", "--workspace", "workspace:1", "--surface", "surface:40", "--title", "fix login"]
        );
    }

    #[test]
    fn never_types_control_characters_or_backslashes() {
        let b = backend(true);
        for bad in ["codex resume x '\u{3}touch /tmp/pwned\r'", "make\nrm -rf ~", "a\u{1b}[2J", "x\u{7f}"] {
            assert!(b.add_tab("workspace:1", &launch("t", "/w", Some(bad))).is_err(), "{bad:?}");
            assert!(b.create_workspace("W", "/w", &[launch("t", "/w", Some(bad))]).is_err(), "{bad:?}");
        }
        // `cmux send` would turn \n into Enter, so a backslash is never typed.
        let err = b.add_tab("workspace:1", &launch("t", "/w", Some(r"codex resume x -c 'k=a\nb'"))).unwrap_err();
        assert!(err.to_string().contains("backslash"), "{err}");
        // Nothing was sent to cmux.
        assert!(b.runner.calls.borrow().is_empty());
        // An apostrophe is quoted without a backslash, so such a flag is still typed.
        let cmd = crate::adapters::agents::registry::find("claude")
            .unwrap()
            .resume_command(A, &["--append-system-prompt".into(), "Don't push".into()])
            .unwrap();
        assert!(!cmd.contains('\\'), "{cmd}");
        assert!(b.add_tab("workspace:1", &launch("t", "/w", Some(&cmd))).is_ok());
        // The layout JSON carries the command as a JSON value, not typed by `cmux send`.
        assert!(b.create_workspace("W", "/w", &[launch("t", "/w", Some(r"echo 'a\b'"))]).is_ok());
    }

    #[test]
    fn ensure_running_launches_then_times_out() {
        let b = backend(false);
        assert_eq!(exit_of(&b.ensure_running().unwrap_err()), Exit::CmuxUnreachable);
        assert!(b.runner.calls.borrow().iter().any(|c| c[..] == ["open", "-a", "cmux"]));
    }

    #[test]
    fn without_a_launcher_asks_the_user_to_start_cmux() {
        let mut b = backend(false);
        b.launcher = launcher_for("linux");
        let err = b.ensure_running().unwrap_err();
        assert_eq!(exit_of(&err), Exit::CmuxUnreachable);
        assert!(err.to_string().contains("start it"), "{err}");
        assert!(!b.runner.calls.borrow().iter().any(|c| c[0] == "open"));
    }

    #[test]
    fn find_ref_picks_first_matching_token() {
        assert_eq!(find_ref("OK workspace:12", "workspace").as_deref(), Some("workspace:12"));
        assert_eq!(find_ref("created (surface:3)", "surface").as_deref(), Some("surface:3"));
        assert_eq!(find_ref("workspace:abc", "workspace"), None);
    }
}
