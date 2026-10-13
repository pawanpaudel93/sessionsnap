# sessionsnap — instructions for coding agents

sessionsnap takes named, versioned snapshots of [cmux](https://cmux.com) workspaces and the
coding-agent sessions in their tabs, and later reopens only what is missing, with each agent tab
resumed with its original flags (those on the agent's allow-list). Claude Code and Codex are fully
supported (their session histories are read); 15 more agents are resume-only and untested, and
resume without saved flags. It is one Rust binary: a CLI
with `--json`, a web UI in the cmux Dock, cmux palette commands, and a Claude Code skill. It runs
on macOS with cmux 0.64+. On Linux, sessionsnap installs, updates and uninstalls, but saving and
restoring don't work yet: cmux on Linux is a different app with a different command line.

## Using sessionsnap

- Always pass `--json` and read the result; never parse the human output. Errors print
  `{"error": {"code", "exit", "message"}}`.
- Restoring is two steps: `sessionsnap restore <name> --plan --json`, show the user what the plan
  opens, skips and warns about, then `sessionsnap apply <planId> --json` once they agree.
- Pass `--yes` only for an action the user asked for by name (`delete`, `prune`, `install-cmux`,
  `uninstall`). Without it, a command that would ask exits 4 instead of prompting.
- `save` only reads cmux; with no changes it writes nothing and exits 3.
- Never schedule or automate saving unless the user asks. Don't start `sessionsnap ui` unless they
  ask for the UI.
- `skill/SKILL.md` (installed with `sessionsnap install-skill`) has the full command table and the
  edit operations.

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 1 | error (also: an `apply` where some steps failed; the result JSON lists them) |
| 2 | cmux not running or not reachable |
| 3 | nothing to do |
| 4 | refused (needs `--yes`, or would overwrite something) |
| 5 | unknown cmux state or snapshot format |
| 6 | plan stale (make a new one) |
| 7 | not found |

Maintenance: `sessionsnap update --check` reports a newer release, `sessionsnap update` installs
it, `sessionsnap uninstall [--keep-data]` removes sessionsnap (asks first). The cmux palette
(`install-cmux`) is macOS-only; on Linux it refuses with exit 4.

## Working on this repo

- Read [ARCHITECTURE.md](ARCHITECTURE.md) first: ports and adapters, the layer rules, and recipes
  for adding an agent or a terminal. `src/testing.rs` has fakes for every port.
- Verify with `scripts/check.sh`: fmt, clippy, tests, `node --check` on the web page's script,
  `sh -n` plus shellcheck on `install.sh` and `scripts/*.sh`, `scripts/test-install.sh` (install.sh
  offline against fake releases) and `scripts/test-site.py`. CI runs the same script on macOS and
  Linux; `scripts/release.sh` runs it too. No test may run cmux, an agent, a browser, or touch
  the network.
- Commits follow Conventional Commits: `type(scope): lowercase summary`, e.g.
  `feat(agents): add gemini history`. Types: feat, fix, refactor, docs, test, chore, ci, style.
  Never commit `target/`, `dist/`, `docs/` or `.superpowers/`.
- Releases: `scripts/release.sh X.Y.Z` on a Mac with Docker (see the README's "Releasing"). Try it
  with `RELEASE_ALLOW_DIRTY=1 scripts/release.sh X.Y.Z --dry-run`.

### Anything that reaches a shell

Snapshots are user-editable files whose contents end up as commands in a shell.

- Validate every value that reaches a command: no control characters, session IDs checked per
  agent. Shell-quote every word (`adapters::shell_word` / `shell_quote`); a terminal adapter only
  runs the command `app/` built, it never builds one.
- A command with a backslash is not typed into an open workspace (`cmux send` turns `\n` into
  Enter).
- Never store environment variables. Agent flags are saved only from each agent's allow-list
  (`saved` in `src/adapters/agents/registry.rs`, checked by `src/domain/flags.rs`); restore filters
  through the same list. Add a flag only after checking the agent's `--help`, with the narrowest
  value kind, and never one that carries a credential or inline config.
- Add a test that a hostile value never reaches the terminal with any change of this kind.
- The web UI runs only allowlisted commands (`interfaces/web/api.rs`); `update`, `uninstall`,
  `install-cmux` and `ui` stay off that list.
- Never read or commit real session files or anything from `~/.claude` or `~/.codex`; tests use
  temp dirs and fixtures.
