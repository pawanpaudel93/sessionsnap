# Contributing

Thanks for helping. sessionsnap is small on purpose; read [ARCHITECTURE.md](ARCHITECTURE.md) first,
it explains the layers and has step-by-step recipes for adding an agent or a terminal.

## Setup

- Rust 1.96 or newer (`rustup update`).
- Node.js, only for `node --check` on the web page's script, and shellcheck.
- cmux is not needed to build or test; tests use fakes and temp dirs.

## Before you open a pull request

```sh
scripts/check.sh
```

It runs `cargo fmt --check`, clippy, the tests, `node --check` on the web page's script, and
`sh -n` plus shellcheck on `install.sh` and `scripts/*.sh` (`brew install shellcheck` or
`apt install shellcheck`). CI runs the same script on macOS and Linux.

## Guidelines

- Keep the layer rules in ARCHITECTURE.md: the domain stays pure, terminals never see agents, and
  agent- or terminal-specific policy lives in its adapter.
- Snapshot contents end up as shell commands. Any new value that reaches a command must be validated
  and shell-quoted, with a test that a hostile value never reaches the terminal.
- Add tests with every change; use the fakes in `src/testing.rs` rather than real cmux or agent
  folders. Never commit real session files or anything from `~/.claude` or `~/.codex`.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org):
  `type(scope): lowercase summary`, e.g. `feat(agents): add gemini history`. Types: feat, fix,
  refactor, docs, test, chore, ci, style. Common scopes: domain, app, cli, ui, cmux, agents, claude,
  codex, store, install, skill, release.
- New agents start as resume-only. Say in the pull request which agent version you tested with.

## Reporting bugs

Include `sessionsnap --version`, your terminal and its version (e.g. `cmux --version`), the agent
and its version, and the command with `--json` output. Leave out session contents and anything
that looks like a token.
