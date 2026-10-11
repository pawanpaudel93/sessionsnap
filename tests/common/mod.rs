#![allow(dead_code)]

use clap::Parser;
use sessionsnap::interfaces::cli::{Cli, Ctx, Outcome, run::dispatch};
use sessionsnap::testing::{A, B, FakeTerminal, open_ws, write_transcript};

/// Temporary store + Claude dir + fake terminal; `run` parses args like the real binary.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub fake: FakeTerminal,
}

impl Env {
    pub fn new() -> Self {
        Env { dir: tempfile::tempdir().unwrap(), fake: FakeTerminal::default() }
    }

    pub fn claude_root(&self) -> std::path::PathBuf {
        self.dir.path().join("claude")
    }

    /// A context like the binary's, on this test's store, Claude dir and fake terminal.
    pub fn ctx(&self, json: bool, yes: bool, input: Option<String>) -> Ctx {
        sessionsnap::testing::ctx(self.dir.path(), self.fake.clone(), json, yes, input)
    }

    pub fn run(&self, args: &[&str]) -> anyhow::Result<Outcome> {
        self.run_with(args, None)
    }

    /// Runs with `input` as the text for `--batch -`.
    pub fn run_with_input(&self, args: &[&str], input: &str) -> anyhow::Result<Outcome> {
        self.run_with(args, Some(input.to_string()))
    }

    fn run_with(&self, args: &[&str], input: Option<String>) -> anyhow::Result<Outcome> {
        let cli =
            Cli::try_parse_from(std::iter::once("sessionsnap").chain(args.iter().copied())).expect("valid arguments");
        dispatch(&self.ctx(cli.json, cli.yes, input), cli.command)
    }
}

/// Alpha: Claude "login fix" (A) + zsh; Beta: Claude "review" (B).
pub fn setup() -> Env {
    let env = Env::new();
    write_transcript(&env.claude_root(), "/w/a", A, "login fix");
    write_transcript(&env.claude_root(), "/w/b", B, "review");
    env.fake.state.borrow_mut().workspaces = vec![
        open_ws("WA", "Alpha", "/w/a", &[("t1", "/w/a", Some(A)), ("zsh", "/w/a", None)]),
        open_ws("WB", "Beta", "/w/b", &[("t2", "/w/b", Some(B))]),
    ];
    env
}
