use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};

use super::runner::{Output, ProcessRunner, Runner};
use super::{Backend, OpenState, state_file, tab_command, tree};
use crate::error::{Exit, failure};
use crate::model::Tab;

/// The cmux backend: reads state from the `cmux` CLI and the state file, opens tabs via the CLI.
pub struct CmuxCli<R: Runner> {
    pub runner: R,
    pub program: String,
    pub state_path: PathBuf,
    pub launch_timeout: Duration,
}

impl CmuxCli<ProcessRunner> {
    pub fn open_default() -> Result<Self> {
        Ok(CmuxCli {
            runner: ProcessRunner,
            program: std::env::var("CCSNAP_CMUX_BIN").unwrap_or_else(|_| "cmux".into()),
            state_path: state_file::default_path()?,
            launch_timeout: Duration::from_secs(30),
        })
    }
}

impl<R: Runner> CmuxCli<R> {
    fn cmux(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        self.runner.run(&self.program, &args)
    }

    /// Runs a cmux command and returns its stdout, failing on a non-zero exit.
    fn cmux_ok(&self, args: &[&str]) -> Result<String> {
        let out = self.cmux(args)?;
        if !out.success {
            return Err(failure(
                Exit::Error,
                format!("`cmux {}` failed: {}", args.first().unwrap_or(&""), out.stderr.trim()),
            ));
        }
        Ok(out.stdout)
    }
}

impl<R: Runner> Backend for CmuxCli<R> {
    fn is_running(&self) -> bool {
        self.cmux(&["ping"]).map(|o| o.success).unwrap_or(false)
    }

    fn ensure_running(&self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        self.runner.run("open", &["-a".to_string(), "cmux".to_string()])?;
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
            return Ok(OpenState { live: false, state_version: Some(version), workspaces: tree::from_file(&file) });
        }
        let tree = tree::parse(&self.cmux_ok(&["--json", "tree", "--all"])?)?;
        Ok(OpenState { live: true, state_version: Some(version), workspaces: tree::join(tree, &file) })
    }

    fn create_workspace(&self, name: &str, cwd: &str, tabs: &[Tab]) -> Result<String> {
        let layout = layout_json(tabs);
        let out = self.cmux_ok(&["new-workspace", "--name", name, "--cwd", cwd, "--layout", layout.as_str()])?;
        find_ref(&out, "workspace")
            .ok_or_else(|| failure(Exit::Error, format!("cmux new-workspace printed no workspace ref: {}", out.trim())))
    }

    fn add_tab(&self, workspace_ref: &str, tab: &Tab) -> Result<()> {
        let out = self.cmux_ok(&["new-surface", "--workspace", workspace_ref, "--working-directory", tab.cwd()])?;
        let surface = find_ref(&out, "surface")
            .ok_or_else(|| failure(Exit::Error, format!("cmux new-surface printed no surface ref: {}", out.trim())))?;
        if let Some(command) = tab_command(tab) {
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
            tab.title(),
        ])?;
        Ok(())
    }

    fn show_url(&self, url: &str) -> Result<&'static str> {
        // Match an open UI tab by its address without the fragment (the key).
        let base = url.split('#').next().unwrap_or(url);
        if let Some(surface) = tree::find_browser(&self.cmux_ok(&["--json", "tree", "--all"])?, base)? {
            self.cmux_ok(&["browser", surface.as_str(), "goto", url])?;
            return Ok("reused");
        }
        if self.cmux(&["new-surface", "--type", "browser", "--placement", "dock", "--url", url])?.success {
            return Ok("dock");
        }
        // cmux 0.64 keeps the Dock behind Settings → Beta Features; without it use a normal tab.
        self.cmux_ok(&["browser", "open", url])?;
        Ok("tab")
    }
}

/// `--layout` JSON for `cmux new-workspace`: one pane holding every tab.
pub fn layout_json(tabs: &[Tab]) -> String {
    let surfaces: Vec<Value> = tabs
        .iter()
        .map(|t| {
            let mut s = json!({"type": "terminal", "name": t.title(), "cwd": t.cwd()});
            if let Some(command) = tab_command(t) {
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
    use crate::cmux::tree::TEST_TREE;
    use crate::error::exit_of;
    use crate::testing::A;
    use serde_json::json;
    use std::cell::RefCell;

    struct FakeRunner {
        calls: RefCell<Vec<Vec<String>>>,
        ping_ok: bool,
        dock_ok: bool,
        tree: &'static str,
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            let mut call = vec![program.to_string()];
            call.extend(args.iter().cloned());
            self.calls.borrow_mut().push(call);
            let ok = |s: &str| Ok(Output { success: true, stdout: s.into(), stderr: String::new() });
            match args.first().map(String::as_str) {
                Some("ping") => Ok(Output { success: self.ping_ok, ..Default::default() }),
                Some("--json") => ok(self.tree),
                Some("new-workspace") => ok("OK workspace:12\n"),
                Some("new-surface") if !self.dock_ok && args.iter().any(|a| a == "dock") => Ok(Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "Error: invalid_params: Dock placement is disabled".into(),
                }),
                Some("new-surface") => ok("OK surface:40 pane:3 workspace:1\n"),
                _ => ok("OK\n"),
            }
        }
    }

    fn backend(ping_ok: bool) -> CmuxCli<FakeRunner> {
        CmuxCli {
            runner: FakeRunner { calls: RefCell::new(vec![]), ping_ok, dock_ok: true, tree: TEST_TREE },
            program: "cmux".into(),
            state_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cmux-state.json"),
            launch_timeout: Duration::from_millis(10),
        }
    }

    const UI: &str = "http://127.0.0.1:47317/#t=abc";

    #[test]
    fn show_url_adds_a_dock_browser() {
        let b = backend(true);
        assert_eq!(b.show_url(UI).unwrap(), "dock");
        let calls = b.runner.calls.borrow();
        assert_eq!(
            &calls.last().unwrap()[1..],
            ["new-surface", "--type", "browser", "--placement", "dock", "--url", UI]
        );
    }

    #[test]
    fn show_url_falls_back_to_a_tab_without_dock() {
        let mut b = backend(true);
        b.runner.dock_ok = false;
        assert_eq!(b.show_url(UI).unwrap(), "tab");
        let calls = b.runner.calls.borrow();
        assert_eq!(&calls.last().unwrap()[1..], ["browser", "open", UI]);
    }

    #[test]
    fn show_url_reuses_an_open_page() {
        let mut b = backend(true);
        b.runner.tree = crate::cmux::tree::TEST_BROWSER_TREE;
        assert_eq!(b.show_url(UI).unwrap(), "reused");
        let calls = b.runner.calls.borrow();
        assert_eq!(&calls.last().unwrap()[1..], ["browser", "surface:71", "goto", UI]);
    }

    #[test]
    fn reads_live_state_when_running() {
        let s = backend(true).read_open_state().unwrap();
        assert!(s.live);
        assert_eq!(s.state_version, Some(1));
        assert_eq!(s.workspaces[0].tabs[1].session_id.as_deref(), Some(A));
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
            Tab::Claude { title: "fix login".into(), cwd: "/w/alpha/sub".into(), session_id: A.into() },
            Tab::Terminal { title: "zsh".into(), cwd: "/w/alpha".into(), command: None },
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
        let tab = Tab::Claude { title: "fix login".into(), cwd: "/w/alpha/sub".into(), session_id: A.into() };
        b.add_tab("workspace:1", &tab).unwrap();
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
    fn ensure_running_launches_then_times_out() {
        let b = backend(false);
        assert_eq!(exit_of(&b.ensure_running().unwrap_err()), Exit::CmuxUnreachable);
        assert!(b.runner.calls.borrow().iter().any(|c| c[0] == "open"));
    }

    #[test]
    fn find_ref_picks_first_matching_token() {
        assert_eq!(find_ref("OK workspace:12", "workspace").as_deref(), Some("workspace:12"));
        assert_eq!(find_ref("created (surface:3)", "surface").as_deref(), Some("surface:3"));
        assert_eq!(find_ref("workspace:abc", "workspace"), None);
    }
}
