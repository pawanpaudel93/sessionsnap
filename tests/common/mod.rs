#![allow(dead_code)]

use ccsnap::claude::Claude;
use ccsnap::cli::{Cli, Ctx, Outcome, run::dispatch};
use ccsnap::store::Store;
use ccsnap::testing::{A, B, FakeBackend, open_ws, write_transcript};
use clap::Parser;

/// Temporary store + Claude dir + fake cmux; `run` parses args like the real binary.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub fake: FakeBackend,
}

impl Env {
    pub fn new() -> Self {
        Env { dir: tempfile::tempdir().unwrap(), fake: FakeBackend::default() }
    }

    pub fn claude_root(&self) -> std::path::PathBuf {
        self.dir.path().join("claude")
    }

    pub fn run(&self, args: &[&str]) -> anyhow::Result<Outcome> {
        let cli = Cli::try_parse_from(std::iter::once("ccsnap").chain(args.iter().copied())).expect("valid arguments");
        let ctx = Ctx {
            store: Store::new(self.dir.path().join("store")),
            claude: Claude::new(self.claude_root()),
            backend: Box::new(self.fake.clone()),
            json: cli.json,
            yes: cli.yes,
            interactive: false,
        };
        dispatch(&ctx, cli.command)
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
