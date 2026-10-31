# Architecture

sessionsnap is a single Rust crate organised as **ports and adapters**. The snapshot logic in the
middle knows nothing about cmux, Claude Code, Codex or the file system; everything outside the
program plugs in through a small interface (a *port*). Supporting a new agent or a new terminal
means adding an adapter, not changing the core.

```
interfaces/   cli, web          parse input, print output; call app/
     │
app/          use cases         save, restore, apply, edit, sessions, recent, agents, schedule,
                                install, update, uninstall
     │
domain/       pure logic        model (snapshot format), capture, flags, diff, edit ops, restore plans,
                                running (the agent in a tab's processes)
     │  depends only on
ports.rs      interfaces        Terminal, SessionHistory, AgentCatalog, Snapshots, Releases, Scheduler
     ▲  implemented by
adapters/     the outside       terminal/{cmux,tmux,procs}, agents/{registry,claude,codex}, store/fs,
                                releases, scheduler/launchd
```

## Layers and their rules

| Layer | Path | May use | Must not |
|---|---|---|---|
| Domain | `src/domain/` | `ports.rs`, `util/` | touch files, processes, env, cmux or agent folders |
| Ports | `src/ports.rs` | domain types | contain logic |
| Use cases | `src/app/` | domain, ports, adapters (wiring) | hold agent- or terminal-specific policy |
| Adapters | `src/adapters/` | ports, domain types, `util/` | depend on each other (terminals never see agents) |
| Interfaces | `src/interfaces/` | app | contain logic beyond parsing and printing |

The web UI speaks the CLI's command protocol on purpose (`POST /api/run` runs an allowed
`sessionsnap` command), so the page, agents and people all drive the same commands.

## Ports

- **`Terminal`**: read what is open (`OpenState`: workspaces, tabs, and for each tab the agent
  running in it as `AgentRef { kind, session_id, args }`), create workspaces and add tabs from
  `LaunchTab { title, cwd, command }`. The command is built and quoted by `app/`; a terminal adapter
  only runs it. Implemented by `adapters/terminal/cmux` and `adapters/terminal/tmux`. A terminal that
  can't name a tab's agent (tmux) reports the processes under the tab's shell instead
  (`OpenTab::processes`, exact arguments from `adapters/terminal/procs`), and `domain::running` finds
  the agent and its session in them through `AgentCatalog`, so terminals still never see agents.
- **`SessionHistory`**: one agent's session history: `find`, `title_for`, `scan` (recent sessions).
  Each adapter decides which of its sessions are *recoverable* (interactive, not scripted).
  Implemented by `adapters/agents/claude.rs` and `codex.rs`; a shared contract test
  (`adapters/agents/contract.rs`) runs against both.
- **`AgentCatalog`**: the agent registry seen from the core: canonical kind, display name, session-id
  validation, histories, and which saved flags are kept (`check_flags`, the same at save and
  restore).
- **`Snapshots`**: loading a snapshot version (used by edit ops that copy from another snapshot).
- **`Releases`**: what `update` needs: the latest release tag, downloads, a SHA-256, unpacking and
  asking a binary its version. Implemented by `adapters/releases.rs` with `curl`, `shasum` or
  `sha256sum`, and `tar`, so the binary needs no HTTP crate; tests use `testing::FakeReleases`.
- **`Scheduler`**: installs or removes a periodic `sessionsnap --terminal <t> schedule run -- <name>`
  for an automatic snapshot. Implemented by `adapters/scheduler/launchd.rs` on macOS; elsewhere
  scheduling is refused. Tests use `testing::FakeScheduler`.

## The snapshot format

`domain/model.rs`, format version 2. A tab is either a plain terminal (`title`, `cwd`, optional
`command`) or an agent tab (`title`, `cwd`, `agent`, `sessionId`, `args`, and `leftOut`: why its flags were
left out when one could not be kept, so it restores as a plain shell; `leftOutUnsure` marks a
`leftOut` that only says the session's flags are undecided, which a live reading decides). A version records the
terminal it was captured from (`source.terminal`, when not cmux), for information only; any terminal
adapter can restore it. A workspace records `pinned` (cmux's `isPinned`), which restore does not
reapply.

Snapshots are user-editable files whose contents end up as commands in a shell. Every value that
reaches a command is validated (no control characters; session IDs checked per agent) and every
word is shell-quoted. Keep it that way.

## Recipes

### Add an agent

1. Add an entry to `AGENTS` in `src/adapters/agents/registry.rs`: `kind` (use the name cmux reports,
   plus any `aliases`), `display`, `program`, and `resume` (the words between the program and the
   session ID). That alone gives **resume-only** support: the agent's tabs are saved and restored
   only when started without flags (`saved: Policy::NONE`; a launch with any flag is left out, since
   an unknown flag may limit the agent).
2. Add a test in `registry.rs` for its resume command, including saved flags that repeat its own
   resume words.
3. To restore some of its flags, give it a `saved` policy: one `Flag::…` line per flag (all its
   spellings, and its value kind from `domain::flags::Value`: `switch`, `word`, `path`, `rules`,
   `config`; `.restricting()` for every flag that limits the agent), checked against the agent's
   `--help`; `skip` lines for flags that are
   not restored but take a value; and the subcommands and flags that make a launch not a session.
   Every flag not listed is left out at save and at restore.
4. For **full** support (titles, `sessions`, `recent`, search, adding sessions by ID): implement
   `SessionHistory` in `src/adapters/agents/<agent>.rs`, register it in `Agents::open_default`
   (`adapters/agents/mod.rs`), set `support: Support::Full` and its `ids`/`flags` rules, and add it to
   the contract test.
5. Add it to the supported-agents table in the README (with the flags it restores).

### Add a terminal

1. Create `src/adapters/terminal/<name>/` implementing `Terminal` (and `name`): report open
   workspaces and tabs (and the agent running in each tab where the terminal can tell, else the
   tab's processes, as `tmux.rs` does), and create workspaces/tabs that run a `LaunchTab` command.
2. Choose it in `Ctx::open_default` (`src/app/mod.rs`): add it to `--terminal` and, if it can be
   detected, to `terminal_for`.
3. Add a fake-driven test for anything terminal-specific; the use cases are already tested through
   `testing::FakeTerminal`.

## Tests

- Unit tests live next to their code; `src/testing.rs` holds fakes for every port. It is built only
  for tests and with the `testing` feature, which `tests/` turns on (a dev-dependency of the crate
  on itself), so release builds never contain it.
- `tests/` drives the real binary against temp dirs and fixtures (`tests/fixtures/`).
- No test runs cmux, an agent or a browser.
