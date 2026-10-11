# Architecture

sessionsnap is a single Rust crate organised as **ports and adapters**. The snapshot logic in the
middle knows nothing about cmux, Claude Code, Codex or the file system; everything outside the
program plugs in through a small interface (a *port*). Supporting a new agent or a new terminal
means adding an adapter, not changing the core.

```
interfaces/   cli, web          parse input, print output; call app/
     │
app/          use cases         save, restore, apply, edit, sessions, recent, agents, install
     │
domain/       pure logic        model (snapshot format), capture, diff, edit ops, restore plans
     │  depends only on
ports.rs      interfaces        Terminal, SessionHistory, AgentCatalog, Snapshots
     ▲  implemented by
adapters/     the outside       terminal/cmux, agents/{registry,claude,codex}, store/fs
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
  only runs it. Implemented by `adapters/terminal/cmux`.
- **`SessionHistory`**: one agent's session history: `find`, `title_for`, `scan` (recent sessions).
  Each adapter decides which of its sessions are *recoverable* (interactive, not scripted).
  Implemented by `adapters/agents/claude.rs` and `codex.rs`; a shared contract test
  (`adapters/agents/contract.rs`) runs against both.
- **`AgentCatalog`**: the agent registry seen from the core: canonical kind, display name, session-id
  validation, histories.
- **`Snapshots`**: loading a snapshot version (used by edit ops that copy from another snapshot).

## The snapshot format

`domain/model.rs`, format version 2. A tab is either a plain terminal (`title`, `cwd`, optional
`command`) or an agent tab (`title`, `cwd`, `agent`, `sessionId`, `args`). The format names no
terminal, so a snapshot can be restored by any terminal adapter.

Snapshots are user-editable files whose contents end up as commands in a shell. Every value that
reaches a command is validated (no control characters; session IDs checked per agent) and every
word is shell-quoted. Keep it that way.

## Recipes

### Add an agent

1. Add an entry to `AGENTS` in `src/adapters/agents/registry.rs`: `kind` (use the name cmux reports,
   plus any `aliases`), `display`, `program`, and `resume` (the words between the program and the
   session ID). That alone gives **resume-only** support: the agent's tabs are saved and restored
   with their original flags.
2. Add a test in `registry.rs` for its resume command, including saved flags that repeat its own
   resume words.
3. For **full** support (titles, `sessions`, `recent`, search, adding sessions by ID): implement
   `SessionHistory` in `src/adapters/agents/<agent>.rs`, register it in `Agents::open_default`
   (`adapters/agents/mod.rs`), set `support: Support::Full` and its `ids`/`flags` rules, and add it to
   the contract test.
4. Add it to the supported-agents table in the README.

### Add a terminal

1. Create `src/adapters/terminal/<name>/` implementing `Terminal`: report open workspaces and tabs
   (and, where the terminal can tell, the agent running in each tab), and create workspaces/tabs
   that run a `LaunchTab` command.
2. Choose it in `Ctx::open_default` (`src/app/mod.rs`) — for now by configuration or detection.
3. Add a fake-driven test for anything terminal-specific; the use cases are already tested through
   `testing::FakeTerminal`.

## Tests

- Unit tests live next to their code; `src/testing.rs` holds fakes for every port.
- `tests/` drives the real binary against temp dirs and fixtures (`tests/fixtures/`).
- No test runs cmux, an agent or a browser.
