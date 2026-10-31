# sessionsnap

[![CI](https://github.com/pawanpaudel93/sessionsnap/actions/workflows/ci.yml/badge.svg)](https://github.com/pawanpaudel93/sessionsnap/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Terminal: cmux | tmux](https://img.shields.io/badge/terminal-cmux%20%7C%20tmux-8A2BE2)

**Save your cmux or tmux workspaces and the coding-agent sessions in them; bring them back resumed.**

Website: <https://sessionsnap.pawanpaudel.com.np>

sessionsnap takes named, versioned snapshots of [cmux](https://cmux.com) workspaces (or tmux
sessions) and the agent sessions in their tabs (Claude Code, Codex and more). Save what is open, edit it, and later reopen
only what is missing, with each agent tab resumed in its own folder with the flags it was started
with that sessionsnap knows to be safe to pass again. It is a single Rust binary with a CLI, a web UI in the cmux Dock, and cmux palette commands.

Nothing runs in the background unless you add an automatic snapshot: every other snapshot is one
you ask for.

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
  `codex resume <id>`, …) with their original flags (the ones on the agent's allow-list); plain
  tabs reopen in their folder.
- **Restore only what is missing:** a restore is a plan you can preview (`--plan`) before anything
  opens; workspaces and tabs that are already open are left alone.
- **Edit snapshots:** rename, reorder and remove workspaces, move tabs between them, add agent
  sessions by ID.
- **Find lost sessions:** `recent` lists agent sessions from the last day that are not open in cmux.
- **Scriptable:** every command takes `--json` and `--yes`, with stable exit codes.
- **Web UI and palette:** a page in the cmux Dock, and commands in the cmux command palette.
- **tmux too:** inside tmux, sessionsnap saves and restores tmux sessions, on macOS and Linux.

## Requirements

| Requirement | Details |
| --- | --- |
| OS | macOS (tested on macOS 15), or Linux (x86_64, arm64) with tmux. cmux on Linux is a different app with a different command line, not supported yet |
| Terminal | [cmux](https://cmux.com) 0.64 or newer (tested on 0.64.22), or tmux |
| Agents | The agents you want to restore, on your `PATH` (`sessionsnap agents` checks) |
| Build (from source only) | Rust 1.96 or newer |

## Installation

```sh
curl -fsSL https://sessionsnap.pawanpaudel.com.np/install.sh | sh
```

The installer:

1. Picks the build for your system: one universal binary for macOS (Apple Silicon and Intel), a
   static binary for Linux x86_64 or arm64.
2. Downloads the latest release from GitHub, checks its SHA-256, and checks that the new binary
   runs and reports that version before it replaces an installed one.
3. Installs `sessionsnap` into `~/.local/bin` and adds that folder to `PATH` in your shell's
   startup file (zsh, bash or fish) if it is not there yet.
4. On macOS, if `cmux` is on your `PATH`, offers to add the sessionsnap commands to the cmux
   palette (`sessionsnap install-cmux`, which asks before changing anything). The palette is
   macOS-only; on Linux the installer skips this step.

Running it again upgrades in place; so does `sessionsnap update`.

| Variable | Default | Effect |
|---|---|---|
| `SESSIONSNAP_VERSION` | latest release | Install this version, e.g. `0.1.0` |
| `SESSIONSNAP_INSTALL_DIR` | `~/.local/bin` | Where the binary goes: an absolute path (a leading `~` is expanded) without quotes, `$`, backquotes, backslashes or control characters |
| `SESSIONSNAP_NO_CMUX` | unset | `1` skips the cmux palette step (macOS) |

For example: `curl -fsSL https://sessionsnap.pawanpaudel.com.np/install.sh | SESSIONSNAP_NO_CMUX=1 sh`.

From source:

```sh
cargo install --git https://github.com/pawanpaudel93/sessionsnap
```

Optional integrations:

```sh
sessionsnap install-cmux     # palette commands in cmux on macOS (asks first, backs up cmux.json)
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
sessionsnap restore work --tab 'Cascadia#3'   # just that tab, into Cascadia if it is open
sessionsnap recent                    # agent sessions from the last 24h not open in cmux
sessionsnap sessions --agent codex    # Codex sessions on disk
sessionsnap tab work Cascadia#4 --move "Cascadia Community"
sessionsnap tab work Cascadia --add-session <session-id>
sessionsnap tab work Cascadia --add-session <id> --agent opencode
sessionsnap agents                    # supported agents and whether each is on PATH
sessionsnap rename work job           # rename a snapshot (and its schedule)
```

### Automatic snapshots

An automatic snapshot is saved from what is open in cmux every interval by launchd (macOS only),
even when sessionsnap isn't running. A run with no changes writes nothing, so its history holds one
version per change. Runs are skipped while cmux is not running or shows no workspaces. `list` shows
automatic snapshots under their own heading, and only their schedule saves to them: copy a version
out with `sessionsnap save <name> --from <auto>@<v>` to keep it.

```sh
sessionsnap schedule add work --interval 30m   # new automatic snapshot; saves once now
sessionsnap schedule add daily --interval 1d   # as many as you like, each with its own interval
sessionsnap schedule list                      # interval, running/stopped, latest version, last run
sessionsnap schedule edit work --interval 2h
sessionsnap schedule stop work                 # pause; `schedule start work` resumes
sessionsnap schedule run work                  # save it now
sessionsnap schedule remove work               # make it a normal snapshot (versions kept)
```

Intervals are a number and a unit (`30m`, `2h`, `1d`; at least `1m`). Each schedule is a launchd
agent in `~/Library/LaunchAgents/sessionsnap.schedule.<name>.plist`; `uninstall` removes them.

### tmux

sessionsnap works out which terminal it runs in: inside tmux it uses tmux (even tmux in a cmux tab),
inside cmux it uses cmux, and from anywhere else (another terminal app) the one that is running:
cmux, else a tmux with sessions. `--terminal tmux` or `--terminal cmux` picks one. Each tmux session is a workspace and each pane a
tab; a restore opens each tab in its own window (splits are not saved), types the agent's resume
command into its shell, and keeps a name tmux can't hold (`.` or `:`) in the session option
`@sessionsnap_name`.

tmux doesn't say which agent runs in a pane, so sessionsnap looks under the pane's shell for an
agent's process and reads its arguments exactly (`/proc` on Linux, the kernel on macOS). Its session
is the one the arguments name (`--resume <id>`, `codex resume <id>`); else, for Claude Code and
Codex, the session in that folder changed most recently since the agent started. When several panes
run the same agent in one folder with no ID in their command, none is guessed: they are saved as
plain tabs, and `save` says so. Each version records the terminal it came from, and an automatic
snapshot keeps saving from the terminal it was added in. The web UI and palette stay in cmux.

```sh
sessionsnap update --check            # is there a newer release?
sessionsnap update                    # install it over this binary
```

Every command takes `--json` (for scripts and agents) and `--yes` (skip confirmations).

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 1 | error (also an `apply`, `restore` or `uninstall` where some steps failed: the normal result, listing them) |
| 2 | terminal unreachable (cmux not running, or tmux not installed) |
| 3 | nothing to do |
| 4 | refused |
| 5 | unknown cmux state or snapshot format |
| 6 | plan stale |
| 7 | not found |

## Supported agents

| Agent | Support | Resumes with | Flags restored |
|---|---|---|---|
| Claude Code | Full (tested) | `claude --resume <id>` | [listed below](#how-flags-are-restored) |
| Codex | Full (tested) | `codex resume <id>` | [listed below](#how-flags-are-restored) |
| Grok | Resume only (untested) | `grok -r <id>` | only launches without flags |
| OpenCode | Resume only (untested) | `opencode --session <id>` | only launches without flags |
| Pi | Resume only (untested) | `pi --session <id>` | only launches without flags |
| omp | Resume only (untested) | `omp --session <id>` | only launches without flags |
| Campfire | Resume only (untested) | `campfire --session <id>` | only launches without flags |
| Amp | Resume only (untested) | `amp threads continue <id>` | only launches without flags |
| Cursor Agent | Resume only (untested) | `cursor-agent --resume <id>` | only launches without flags |
| Gemini CLI | Resume only (untested) | `gemini --resume <id>` | only launches without flags |
| Antigravity | Resume only (untested) | `agy --conversation <id>` | only launches without flags |
| Rovo Dev | Resume only (untested) | `acli rovodev run --restore <id>` | only launches without flags |
| Hermes Agent | Resume only (untested) | `hermes --resume <id>` | only launches without flags |
| GitHub Copilot CLI | Resume only (untested) | `copilot --resume <id>` | only launches without flags |
| CodeBuddy | Resume only (untested) | `codebuddy --resume <id>` | only launches without flags |
| Factory Droid | Resume only (untested) | `droid --resume <id>` | only launches without flags |
| Qoder CLI | Resume only (untested) | `qodercli --resume <id>` | only launches without flags |

- **Full:** sessionsnap reads the agent's session history (titles, `sessions`, `recent`, checks that
  a saved session still exists) and resumes it.
- **Resume only:** sessionsnap saves and resumes the session with the command cmux documents for
  that agent; it cannot read its history. These have not been tried with sessionsnap yet, so their
  flags are not known: a session started with no flags resumes, and one started with any flag is
  left out (an unknown flag may limit the agent, such as a sandbox), with a note to resume it by
  hand.
- cmux records which agent runs in a tab. For agents other than Claude Code it does so only after
  `cmux hooks setup <agent>`.

### How flags are restored

A restored tab runs the agent's resume command with the flags it was started with (from cmux's
record). Only the flags on the agent's allow-list are saved and passed again; every other flag the
agent's command line knows is left out when you save, and `save` names the flags it left out (never
their values). A flag sessionsnap doesn't know at all (not in `claude --help` or `codex --help` as
of Claude Code 2.1.296 and Codex 0.160.0) leaves out the session, since what follows it can't be
read for certain. So does a Codex `-c` key on neither list below, since it may limit Codex; only
`model_*`, `hide_agent_reasoning`, `show_raw_agent_reasoning`, `file_opener`, `tui.*` and
`shell_environment_policy.set.*` (environment variables) and `model_providers.*` (a provider's
definition, which may hold its token or headers) are left out with just a note, never saved; the
note says the provider's settings were not saved, so a resume that picks it with
`-c model_provider=<id>` can't reach it until you pass them again.
A note shows a `-c` key only up to its first segment (`tools.*`). Claude Code flags missing from
`claude --help` (hidden flags such as `--permission-prompt-tool` or `--max-turns`, and the teammate
flags cmux passes to Claude Code teams, such as `--agent-id` or `--plan-mode-required`) leave out the
session, so teammate tabs are not restored. Short flags may be grouped (`-vs read-only`) as the agents allow. Supporting
another flag is one line in `src/adapters/agents/registry.rs`.

| Agent | Flags kept | Restricting (also kept, fail closed) |
|---|---|---|
| Claude Code | `--model`, `--fallback-model`, `--effort`, `--betas`; `--allowedTools`/`--allowed-tools`; `--add-dir`, `--mcp-config`, `--append-system-prompt-file`, `--system-prompt-file` (paths only); `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--ide`, `--verbose`, `--chrome`, `--no-chrome` | `--permission-mode`, `--agent`, `--setting-sources`, `--max-budget-usd`, `--disallowedTools`/`--disallowed-tools`, `--tools`; `--settings`, `--agents`, `--plugin-dir` (paths only: inline JSON leaves out the session); `--strict-mcp-config`, `--restricted`, `--safe-mode`, `--disable-slash-commands`, `--bare` |
| Codex | `--model`/`-m`, `--cd`/`-C`, `--add-dir`, `--dangerously-bypass-approvals-and-sandbox`, `--search`, `--oss`; `-c`/`--config` for `model`, `model_provider`, `model_reasoning_effort`, `model_reasoning_summary`, `model_verbosity` | `--sandbox`/`-s`, `--ask-for-approval`/`-a`, `--profile`/`-p`, `--enable`, `--disable`, `--approve-for-me`; `-c`/`--config` for `approval_policy`, `sandbox_mode`, `profile`, `sandbox_workspace_write.*`, `features.*`, `shell_environment_policy.inherit`/`.exclude`/`.include_only`/`.ignore_default_excludes` |
| Every other agent | none: a session started with any flag is left out | every flag |

Both `--flag value` and `--flag=value` work. A kept flag's values are checked too:

- a name or mode (`--model`, `-s`, a `-c` value, …) must be one word of letters, digits and
  `-_.:/@,`, at most 100 characters;
- a path flag (`--settings`, `--mcp-config`, `--add-dir`, …) must hold a path: inline JSON
  (a value starting with `{` or `[`) is left out;
- no value may hold a credential in a format that can be told for certain: a token with a known
  prefix (`sk-ant-`, `sk-proj-`, `sk-svcacct-`, `sk-admin-`, Stripe `sk_live_`/`rk_live_`, GitHub
  `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`/`github_pat_`, Slack `xox…-`/`xapp-`, GitLab `glpat-`, Hugging
  Face `hf_`, npm `npm_`, AWS `AKIA`/`ASIA` plus 16, Google `AIza` plus 35), a JWT, a URL with a
  password (`scheme://user:pass@host`), a `Bearer` token, an `Authorization:`, `X-Api-Key:` or
  `Api-Key:` header value. A password typed inside a permission rule or other free text (such as
  `curl -u user:password`) is not detected; only token formats, auth headers and URLs with a
  password are. Keep passwords out of permission rules and flag values.

A value that fails is left out; a flag left with no value goes too, with a note. A flag that limits
the agent (the "restricting" column) is never lost quietly: if it, or any of its values, can't be
saved (a value of the wrong shape, a credential in it, a missing value, a `-c` key of a restricting
family with an unexpected value), the tab's agent session is left out: the tab is saved with its
session ID but none of its flags, and the reason in `leftOut`, and `save` says "left out its agent
session: a restricting flag (`<name>`) could not be saved; resume it by hand". Restore then never
resumes it: it warns with the reason and opens a plain shell, or skips the tab while its session is
open, because resuming without the flag would allow more than you did. The reason names flags,
never their values. (An agent record with a control character in its kind, ID or flags is saved as
a plain terminal tab: its ID can't be trusted.) A launch
that is not a session (Claude Code with `--print`/`-p` or `--no-session-persistence`, or a
subcommand such as `claude mcp`) is left out the same way.

**Caveat (Claude Code `--settings` under cmux):** cmux 0.65, the version checked, usually records a
Claude Code launch without your `--settings`, when it can merge your settings file into its own hook
settings (that needs Node). sessionsnap then never sees the flag, and restrictions in that settings
file are not restored. Resume such sessions by hand, or pass the restrictions with other flags
(`--disallowedTools`, `--permission-mode`). When the merge can't run, cmux records your
`--settings` as given, and the rules above apply. Other cmux versions are unverified. Codex launches
are recorded as you typed them.

Loading a snapshot applies the same lists and rules (for `show`, the web UI, diffs, saves and
restore), so a snapshot saved by an older sessionsnap never shows or passes a flag that is no longer
allowed. A tab whose saved flags can't be passed again (a restricting flag that can't be kept, or an
untested agent's tab with flags) keeps its session ID but none of its flags, with the reason
(`leftOut`): `show` gives it, and restore opens a plain shell in a new workspace, or skips the tab
in an open one, with the reason. A `leftOut` edited by hand that is blank, longer than 200
characters, or holds a control character or a credential is shown as "its flags were left out;
resume it by hand".

sessionsnap keeps a record of every session it has seen (`sessions.json`): the flags of its newest
launch as read from cmux, or why it was left out. Every live read of cmux writes it, stamped with
the time cmux was read: `save` (what it captured), and edits that read cmux (`--add-session`,
`--session`, `ws --add-live`: every open session's launch). Copies (`save --from`, `ws --from`),
other edits and restores never do, so an older copy of a session can't replace what was seen
last. Once left out, a session stays left out. Pruning or deleting versions never removes from
it. It is built from every snapshot the first time it is needed; a record that can't be read is
kept as `sessions.json.bad-<time>` and built again. Before a save writes a version, it marks the
record as about to change (`sessions.pending/`), and refuses to save if it can't; when the update
then fails, the record is kept and completed the next time it is read, from the earliest failed
update on (every version, when that time can't be trusted). Building or completing it from the snapshots compares every
flag the versions keep, and version times are not trusted to tell which launch came last: a session
the record lacks takes the flags its versions agree on; one whose versions disagree, or whose
entry is contradicted by a version written since it was seen and since the failed update, is left
out ("its flags differ between saved versions; resume it by
hand") until the next save or edit reads it live from cmux and records that launch; one left out
in any version, or whose live launch had flags that couldn't be kept, is left out for good.

Everything that resumes a session reads it when the plan is made (`restore` of any snapshot or
version, `recent`, and `apply`, which rebuilds the plan): a session the record has left out is left
out (a plain shell with the reason, or skipped in an open workspace), and one it has flags for
resumes with those, with a warning ("using the session's newest known flags") when the tab's own
differ. A session put in a tab by an edit (`--add-session`, `--session`, `ws --from`) takes its
flags from there too, or, when it is open in cmux, the flags it runs with now (a launch that loses
it leaves it out); a session seen nowhere resumes with no flags and a warning ("launch flags
unknown; resuming with defaults").

The tokens that pick another session are taken out before the list applies: the agent's own resume
words and resume flags with the old ID, its continue flag, Claude Code's `--session-id <id>` and
`--fork-session`, Codex's `resume|fork <uuid|--last>`. A Codex launch that is a subcommand
(`codex exec`, `codex review`, …) is not a session and is left out.

A tab with an invalid session ID, or an agent sessionsnap does not know, reopens as a plain shell
in its folder, with a warning.

Want another agent supported? See [Add an agent](ARCHITECTURE.md#add-an-agent).

## Web UI

`sessionsnap ui` serves a small page on `http://127.0.0.1:47317/` and shows it in the cmux Dock (the
right sidebar). From the palette: "sessionsnap: Open UI" (it runs in its own "sessionsnap UI"
workspace).

- **Snapshots:** save what is open, update a snapshot from what is open, rename (the pencil or a
  double-click on its name) or delete one. The list has Manual and Automatic sections.
- **Automatic snapshots:** start one with a name and any interval ("Save automatically"); on its
  page, see whether it runs and how its last run went, run it now, stop or start it, change its
  interval, or make it a normal snapshot.
- **Workspaces:** rename (double-click), reorder, remove, drag tabs to reorder or move them, add an
  agent session or a shell tab, add a workspace from cmux or from another snapshot. Edits are
  previewed and saved together as one version. Hover an agent tab's icon for the flags it resumes
  with.
- **History:** compare two versions; restore an older one.
- **Restore:** tick workspaces and tabs, see what would open, then restore. Each tab in the
  Workspaces view also has a **Restore this tab** button.

A full restore adds only missing agent tabs to a workspace that is already open (a shell might be
a duplicate). A tab you pick (`--tab`, or a subset ticked in the UI) is added to its open workspace
even when it is a shell, or as a plain shell, with the reason, when its session can't be resumed.
- **Recent:** agent sessions (Claude Code, Codex) from the last days that are not open in cmux.

Details:

- It runs while its terminal is open and stops after 30 minutes without use; nothing starts at
  login. Stop it with Ctrl-C, or close the "sessionsnap UI" workspace.
- It answers four connections at once and lets 16 more wait (beyond that, 503). It runs one
  command at a time; a command that would wait over 3 s for another is refused, and the page says
  so. It shows its page in cmux at most once a second (otherwise 429).
- Flags: `--port <n>` (default 47317), `--idle-minutes <n>` (default 30), `--no-open` (serve
  without showing the page).
- The link carries a key after `#` (`…/#t=<key>`), which browsers never send to a server. The key
  lasts as long as the server: each start makes a new one. It never goes on a command line, where
  other users could read it with `ps`: cmux opens the page without it, and the key is handed over
  through cmux's socket. If that fails, the page opens without a key and `sessionsnap ui` prints
  the keyed link in its terminal. If the page has no key or an old one, click **Reconnect**, open
  that link, or run `sessionsnap ui` again.
- In cmux 0.64 the Dock is a beta feature (Settings → Beta Features → Dock). Without it the page
  opens as a normal browser tab.

## Configuration and files

| What | Where | Override |
|---|---|---|
| Snapshots | `~/.sessionsnap/snapshots/<name>/<NNNN>-<time>.json` | `SESSIONSNAP_HOME` |
| Saved sessions' flags | `~/.sessionsnap/sessions.json` (lock files in `~/.sessionsnap/locks`) | `SESSIONSNAP_HOME` |
| Claude Code transcripts (read) | `~/.claude/projects` | `SESSIONSNAP_CLAUDE_DIR` (another `~/.claude`) |
| Codex sessions (read) | `~/.codex/sessions` | `SESSIONSNAP_CODEX_DIR` (another `~/.codex`) |
| cmux binary | `cmux` on `PATH` | `SESSIONSNAP_CMUX_BIN` |
| tmux binary | `tmux` on `PATH` | `SESSIONSNAP_TMUX_BIN` |
| cmux state file (read) | macOS: `~/Library/Application Support/cmux/session-com.cmuxterm.app.json`; Linux: `$XDG_STATE_HOME/cmux/` (default `~/.local/state/cmux/`), same name; a placeholder until Linux is supported | `SESSIONSNAP_CMUX_STATE` |
| cmux socket (web UI) | `$CMUX_SOCKET_PATH` (or its older name `$CMUX_SOCKET`), else the path in `~/Library/Application Support/cmux/last-socket-path` | `SESSIONSNAP_CMUX_SOCKET` |
| cmux palette config | `~/.config/cmux/cmux.json` (Linux: `$XDG_CONFIG_HOME/cmux/cmux.json` when set) | `SESSIONSNAP_CMUX_CONFIG` |

sessionsnap also reads cmux's state file and `cmux --json --id-format both tree --all`, and matches open tabs to the state file by surface ID (by tty, with a warning, on a cmux that reports no IDs).

Snapshots are format 2 (`"sessionsnap": 2`). Files from pre-release builds (format 1, with
`"type": "claude"` tabs) are shown as "unreadable" until converted by hand: each
`{"type": "claude", …, "sessionId": "<id>"}` tab becomes
`{"type": "agent", …, "agent": "claude", "sessionId": "<id>"}`, and `"sessionsnap": 1` becomes `2`.
When the latest version can't be read, `save` compares with the newest one that can, says so, and
writes a new version. Saves, edits, prunes and deletes of one snapshot take turns (a lock file in
`~/.sessionsnap/locks`, broken after a minute if its holder crashed): a save or edit holds it from
reading the latest version to writing the next, so saves at the same moment never lose a version or
share a number, and an edit never undoes a version saved while it was being made (it refuses with
exit 6 instead).

## Security

Snapshots are user-editable files whose contents end up as commands in a shell, so:

- Every word of a restore command is shell-quoted, and session IDs are validated first (UUIDs for
  Claude Code and Codex).
- A tab whose flags, ID, folder or command hold a control character (newline, ^C, ESC …) is never
  run, and an agent record with one is not saved. A resume command with a backslash is not typed
  into an already open workspace (`cmux send` would turn `\n` into Enter).
- Environment variables from cmux's state are never stored. Of an agent's flags, only those on its
  allow-list are saved; every other known flag (`--append-system-prompt`, `-c mcp_servers…`, …) is
  left out, and `save` names what it left out. A flag sessionsnap doesn't know (`--api-key`,
  `--header`, …) leaves out the whole agent session. Kept values are checked for their kind (inline
  JSON is not a path) and for known credential formats (see
  [How flags are restored](#how-flags-are-restored)). A password typed inside a permission rule or
  other free text is not detected; recognised are token formats, auth headers and URLs with a
  password. A flag that limits the agent (deny rules,
  sandbox, permission mode, settings) that can't be saved exactly as given leaves out the whole
  agent session rather than resume it with fewer limits; so does any flag of an agent whose flags
  are untested. Restore filters old snapshots through the same rules.
- Snapshot versions written by an earlier sessionsnap (before the allow-list) may still hold the
  flags they saved, on disk, until you remove them: save again, then `sessionsnap prune <name>
  --keep 1`, or `sessionsnap delete <name>`. They are never shown or replayed, but the files keep
  them.
- Snapshot files are readable only by you (folder 0700, files 0600).

## Status

Early: cmux on macOS, and tmux on macOS and Linux; the code is organised so other terminals can be
added (see [ARCHITECTURE.md](ARCHITECTURE.md)). cmux on Linux is a different app (cmux Browser,
backed by cmux-tui) with a different command line and state files, and needs its own terminal
adapter. Automatic snapshots need launchd, so macOS only.

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
sessionsnap uninstall               # asks first; --yes skips the prompt
sessionsnap uninstall --keep-data   # keep your snapshots in ~/.sessionsnap
```

This removes the `"sessionsnap: …"` palette commands from cmux's `cmux.json` (after backing it
up next to itself; your other commands and comments stay; if its `"commands"` array holds
comments, the step is skipped and you remove those entries by hand), the Claude Code skill
(`~/.claude/skills/sessionsnap`, only if sessionsnap wrote it), `~/.sessionsnap` (or
`SESSIONSNAP_HOME`) unless `--keep-data` (left alone if it is a symlink or holds files sessionsnap
did not make), and the `sessionsnap` binary. Anything already gone is
reported, not an error. If a step fails (say the binary's folder is not writable), the others still
run, the report lists what was removed and what failed with its error, and the exit code is 1. It
leaves the `PATH` line the installer added in place (other tools may
use `~/.local/bin`) and lists it.

Installed with `cargo install`? `sessionsnap uninstall` removes everything but the binary; then run
`cargo uninstall sessionsnap`.

## Development

```sh
cargo build                                   # target/debug/sessionsnap
cargo test
scripts/check.sh                              # every CI check: fmt, clippy, tests, node, shellcheck
```

No test runs cmux, an agent or a browser; they use fakes and temp dirs.
[ARCHITECTURE.md](ARCHITECTURE.md) describes the code layout for contributors and coding agents;
[AGENTS.md](AGENTS.md) has the short version for coding agents.

<details>
<summary>Releasing (maintainers)</summary>

```sh
scripts/release.sh 0.2.0 --dry-run   # build dist/ and show what would be published
scripts/release.sh 0.2.0             # bump, check, build, commit, tag, push, GitHub release
```

Run it on a Mac, on a clean `main` that matches `origin/main` and has passed CI, with Docker
running, the `aarch64-apple-darwin` and `x86_64-apple-darwin` Rust targets, and `gh` logged in. It
refuses a version older than the current one, sets the version in `Cargo.toml`, `Cargo.lock`
and the site's nav badge and JSON-LD `version` (examples in the text are left alone), runs
`scripts/check.sh`, builds a universal macOS binary (`lipo`) and static Linux binaries for
x86_64 and arm64 (musl, in the pinned `rust:1.99.0-alpine` Docker image; `RELEASE_RUST_IMAGE` overrides
it), and writes to `dist/` one
`sessionsnap-X.Y.Z-<target>.tar.gz` plus `.sha256` per target (`universal-apple-darwin`,
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`) and `NOTES.md` (release notes from the
Conventional Commit subjects since the last tag). Then it asks, commits `chore: release vX.Y.Z`,
tags, pushes `main` and the tag, and creates the GitHub Release. `-y` skips the question;
`RELEASE_ALLOW_DIRTY=1` lets a dry run work on a dirty tree or another branch.

</details>

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks, and
[ARCHITECTURE.md](ARCHITECTURE.md) for the layout and recipes for adding an agent or a terminal.

## License

[MIT](LICENSE) © 2026 Pawan Paudel
