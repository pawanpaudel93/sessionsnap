# sessionsnap

[![CI](https://github.com/pawanpaudel93/sessionsnap/actions/workflows/ci.yml/badge.svg)](https://github.com/pawanpaudel93/sessionsnap/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Terminal: cmux](https://img.shields.io/badge/terminal-cmux-8A2BE2)

**Save your cmux workspaces and the coding-agent sessions in them; bring them back resumed.**

sessionsnap takes named, versioned snapshots of [cmux](https://cmux.com) workspaces and the agent
sessions in their tabs (Claude Code, Codex and more). Save what is open, edit it, and later reopen
only what is missing, with each agent tab resumed in its own folder with the flags it was started
with. It is a single Rust binary with a CLI, a web UI in the cmux Dock, and cmux palette commands.

Nothing runs in the background: every snapshot is one you ask for.

## Contents

- [Features](#features)
- [Requirements](#requirements)
- [Installation](#installation)
- [Usage](#usage)
- [Supported agents](#supported-agents)
- [Web UI](#web-ui)
- [Configuration and files](#configuration-and-files)
- [Security](#security)
- [Status](#status)
- [Uninstalling](#uninstalling)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Features

- **Snapshots with history:** every `save` to an existing name is a new version; list, diff and
  restore any version.
- **Agent-aware restore:** agent tabs come back resumed (`claude --resume <id>`,
  `codex resume <id>`, …) with their original flags; plain tabs reopen in their folder.
- **Restore only what is missing:** a restore is a plan you can preview (`--plan`) before anything
  opens; workspaces and tabs that are already open are left alone.
- **Edit snapshots:** rename, reorder and remove workspaces, move tabs between them, add agent
  sessions by ID.
- **Find lost sessions:** `recent` lists agent sessions from the last day that are not open in cmux.
- **Scriptable:** every command takes `--json` and `--yes`, with stable exit codes.
- **Web UI and palette:** a page in the cmux Dock, and commands in the cmux command palette.

## Requirements

| Requirement | Details |
| --- | --- |
| OS | macOS (tested on macOS 15) |
| Terminal | [cmux](https://cmux.com) 0.64 or newer (tested on 0.64.22) |
| Agents | The agents you want to restore, on your `PATH` (`sessionsnap agents` checks) |
| Build | Rust 1.96 or newer |

## Installation

```sh
cargo install --git https://github.com/pawanpaudel93/sessionsnap
```

Optional integrations:

```sh
sessionsnap install-cmux     # palette commands in cmux (asks first, backs up cmux.json)
sessionsnap install-skill    # teaches Claude Code to use sessionsnap
```

## Usage

```sh
sessionsnap save work                 # snapshot everything open (new version if "work" exists)
sessionsnap save work -w Cascadia     # update only one workspace in the snapshot
sessionsnap show work                 # workspaces and tabs
sessionsnap history work              # versions
sessionsnap diff work@3 work@4
sessionsnap restore work --plan       # what would be reopened
sessionsnap restore work              # plan, confirm, reopen what is missing
sessionsnap recent                    # agent sessions from the last 24h not open in cmux
sessionsnap sessions --agent codex    # Codex sessions on disk
sessionsnap tab work Cascadia#4 --move "Cascadia Community"
sessionsnap tab work Cascadia --add-session <session-id>
sessionsnap tab work Cascadia --add-session <id> --agent opencode
sessionsnap agents                    # supported agents and whether each is on PATH
```

Every command takes `--json` (for scripts and agents) and `--yes` (skip confirmations).

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 1 | error |
| 2 | cmux unreachable |
| 3 | nothing to do |
| 4 | refused |
| 5 | unknown cmux state or snapshot format |
| 6 | plan stale |
| 7 | not found |

## Supported agents

| Agent | Support | Resumes with |
|---|---|---|
| Claude Code | Full (tested) | `claude --resume <id>` |
| Codex | Full (tested) | `codex resume <id>` |
| Grok | Resume only (untested) | `grok -r <id>` |
| OpenCode | Resume only (untested) | `opencode --session <id>` |
| Pi | Resume only (untested) | `pi --session <id>` |
| omp | Resume only (untested) | `omp --session <id>` |
| Campfire | Resume only (untested) | `campfire --session <id>` |
| Amp | Resume only (untested) | `amp threads continue <id>` |
| Cursor Agent | Resume only (untested) | `cursor-agent --resume <id>` |
| Gemini CLI | Resume only (untested) | `gemini --resume <id>` |
| Antigravity | Resume only (untested) | `agy --conversation <id>` |
| Rovo Dev | Resume only (untested) | `acli rovodev run --restore <id>` |
| Hermes Agent | Resume only (untested) | `hermes --resume <id>` |
| GitHub Copilot CLI | Resume only (untested) | `copilot --resume <id>` |
| CodeBuddy | Resume only (untested) | `codebuddy --resume <id>` |
| Factory Droid | Resume only (untested) | `droid --resume <id>` |
| Qoder CLI | Resume only (untested) | `qodercli --resume <id>` |

- **Full:** sessionsnap reads the agent's session history (titles, `sessions`, `recent`, checks that
  a saved session still exists) and resumes it.
- **Resume only:** sessionsnap saves and resumes the session with the command cmux documents for
  that agent; it cannot read its history. These have not been tried with sessionsnap yet.
- cmux records which agent runs in a tab. For agents other than Claude Code it does so only after
  `cmux hooks setup <agent>`.

### How flags are restored

A restored tab runs the agent's resume command with the flags it was started with (from cmux's
record, which holds only flags, never the prompt). Flags are kept as saved except the exact tokens
that would pick another session:

- the agent's own resume words and resume flags with the old ID (e.g. `--session <id>`, `-s <id>`,
  `rovodev run`, `threads continue <id>`);
- its continue flag where `-c`/`--continue` means that (Claude Code, OpenCode);
- for Claude Code also `--session-id <id>`, `--fork-session` and `--print`/`-p`;
- for Codex `resume <uuid|--last>`, `fork <uuid|--last>`, and a leading `exec` run.

A tab with an invalid session ID, or an agent sessionsnap does not know, reopens as a plain shell
in its folder, with a warning.

Want another agent supported? See [Add an agent](ARCHITECTURE.md#add-an-agent).

## Web UI

`sessionsnap ui` serves a small page on `http://127.0.0.1:47317/` and shows it in the cmux Dock (the
right sidebar). From the palette: "sessionsnap: Open UI" (it runs in its own "sessionsnap UI"
workspace).

- **Snapshots:** save what is open, update a snapshot from what is open, delete one.
- **Workspaces:** rename (double-click), reorder, remove, drag tabs to reorder or move them, add an
  agent session or a shell tab, add a workspace from cmux or from another snapshot. Edits are
  previewed and saved together as one version. Hover an agent tab's icon for the flags it resumes
  with.
- **History:** compare two versions; restore an older one.
- **Restore:** tick workspaces and tabs, see what would open, then restore.
- **Recent:** agent sessions (Claude Code, Codex) from the last days that are not open in cmux.

Details:

- It runs while its terminal is open and stops after 30 minutes without use; nothing starts at
  login. Stop it with Ctrl-C, or close the "sessionsnap UI" workspace.
- Flags: `--port <n>` (default 47317), `--idle-minutes <n>` (default 30), `--no-open` (serve
  without showing the page).
- The link carries a one-time key after `#` (`…/#t=<key>`), which browsers never send to a server.
  A new start makes a new key. If the page has no key or an old one, click **Reconnect** or run
  `sessionsnap ui` again.
- In cmux 0.64 the Dock is a beta feature (Settings → Beta Features → Dock). Without it the page
  opens as a normal browser tab.

## Configuration and files

| What | Where | Override |
|---|---|---|
| Snapshots | `~/.sessionsnap/snapshots/<name>/<NNNN>-<time>.json` | `SESSIONSNAP_HOME` |
| Claude Code transcripts (read) | `~/.claude/projects` | `SESSIONSNAP_CLAUDE_DIR` (another `~/.claude`) |
| Codex sessions (read) | `~/.codex/sessions` | `SESSIONSNAP_CODEX_DIR` (another `~/.codex`) |
| cmux binary | `cmux` on `PATH` | `SESSIONSNAP_CMUX_BIN` |

sessionsnap also reads cmux's state file and `cmux --json tree`.

Snapshots are format 2 (`"sessionsnap": 2`). Files from pre-release builds (format 1, with
`"type": "claude"` tabs) are shown as "unreadable" until converted by hand: each
`{"type": "claude", …, "sessionId": "<id>"}` tab becomes
`{"type": "agent", …, "agent": "claude", "sessionId": "<id>"}`, and `"sessionsnap": 1` becomes `2`.

## Security

Snapshots are user-editable files whose contents end up as commands in a shell, so:

- Every word of a restore command is shell-quoted, and session IDs are validated first (UUIDs for
  Claude Code and Codex).
- A tab whose flags, ID, folder or command hold a control character (newline, ^C, ESC …) is never
  run, and an agent record with one is not saved. A resume command with a backslash is not typed
  into an already open workspace (`cmux send` would turn `\n` into Enter).
- Environment variables from cmux's state are never stored, nor flags that look like credentials
  (a flag name or `-c` key ending in token, secret, password, credential(s), auth, or holding an
  API key, and values that look like tokens). They are dropped with their values, and `save` says
  so.
- Snapshot files are readable only by you (folder 0700, files 0600).

## Status

Early, macOS only, cmux only for now; the code is organised so other terminals can be added (see
[ARCHITECTURE.md](ARCHITECTURE.md)).

Verified with cmux 0.64.22 on macOS:

- `save` captured 9 workspaces and 29 Claude Code tabs; every session ID matched cmux's own record.
- `restore` reopened closed workspaces and resumed their conversations: Claude Code 2.1.296
  (`claude --resume <id>`) and Codex 0.160.0 (`codex resume <id> -a on-request -s workspace-write`,
  saved flags kept, prompt not stored).
- `install-cmux` added the palette commands to cmux's commented `cmux.json`; ⌘⇧P shows them.
- "sessionsnap: Open UI" showed the UI in the Dock; running it again pointed the open page at the
  new key.

Not yet checked:

- Dragging a tab between workspaces in cmux's own browser.
- `restore` when it has to start cmux itself (after a reboot). Until then, open cmux first, wait
  for it to reopen your workspaces, then restore.

## Uninstalling

```sh
cargo uninstall sessionsnap
rm -rf ~/.sessionsnap                 # your snapshots; skip to keep them
rm -rf ~/.claude/skills/sessionsnap   # if you ran install-skill
```

If you ran `install-cmux`, remove the `"sessionsnap: …"` entries from `"commands"` in
`~/.config/cmux/cmux.json` (a backup from before the install sits next to it).

## Development

```sh
cargo build                                   # target/debug/sessionsnap
cargo test
cargo clippy --all-targets -- -D warnings
```

No test runs cmux, an agent or a browser; they use fakes and temp dirs.
[ARCHITECTURE.md](ARCHITECTURE.md) describes the code layout for contributors and coding agents.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks, and
[ARCHITECTURE.md](ARCHITECTURE.md) for the layout and recipes for adding an agent or a terminal.

## License

[MIT](LICENSE) © 2026 Pawan Paudel
