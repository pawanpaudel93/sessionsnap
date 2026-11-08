# Changelog

All notable changes to sessionsnap are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `sessionsnap ui --browser`: the web UI in your default browser from cmux too; save and restore
  still use cmux.
- Web UI from cmux: a page without its key (an address pasted into a browser) offers **Open in
  cmux** and **Open in browser** instead of only Reconnect into cmux.
- Web UI: the sessionsnap logo as the page's icon.
- tmux: a window split into panes is saved with its layout, and restoring the whole session splits
  it again the same way (before, each pane came back as a window of its own). Resizing panes alone
  is not a change.
- Automatic snapshots keep their newest versions: `schedule add` and `schedule edit` take
  `--keep N` (default 50 for a new one; `--keep 0` keeps every version), and older versions are
  deleted after each save. Schedules added before keep every version. Lowering `--keep` below the
  versions a snapshot holds asks first, as `prune` does. Nothing is deleted while the record of
  sessions is still being updated from those versions.
- Pi is fully supported in cmux and tmux (tested with Pi 1.5.0): its sessions
  (`~/.pi/agent/sessions`, `PI_CODING_AGENT_SESSION_DIR` or `PI_CODING_AGENT_DIR`) are read for
  titles, `sessions`, `recent` and tmux. Pi hides the flags it was started with, so
  `sessionsnap install-pi` adds a Pi extension that records them in the session (never
  `--api-key`); restore resumes Pi with the flags in `pi --help` (the ones that limit it fail
  closed). A session started without the extension resumes without flags, with a warning.
- `install-skill` also installs the skill in `~/.agents/skills`, the shared Agent Skills folder
  that Pi and other agents read; `uninstall` removes it from both. Its JSON `installed` is now a
  list of paths.

### Changed

- tmux: after `/clear` in a pane started without a session ID, the new session is saved (before,
  the pane was saved as a plain tab); a pane whose command names a session that was left for a
  newer one is saved with a warning. Agent histories are read once per save, not once per tab.
- Web UI: a cleaner look. Content sits in a centred column on wide windows, with cards on a soft
  background and the logo in the header. Warnings are amber and stacked one per line; red is kept
  for errors. Folders are cut at the start so the project folder stays in view. Buttons respond to
  a press, and motion follows the reduce-motion setting.

### Fixed

- Opening or closing a second shell in the same folder counted as "no changes": versions now
  compare tabs one for one, so it shows as +1 or -1 tab. A save where only tab order or a detail
  changed is labelled "tab order or details changed".
- cmux 0.65: restoring tabs into an open workspace failed with "cmux new-surface printed no surface
  ref" (cmux now prints the new tab's UUID instead of its ref).
- When cmux refuses to hand the web UI its key (it lets only processes started inside cmux use its
  socket), the message says so instead of "answered something other than JSON".
- `schedule edit` that launchd failed to load again left the schedule shown as running while
  nothing ran: loading is retried, and a schedule that still can't be loaded is recorded as
  stopped.
- `uninstall` kept the whole data folder after a save had crashed, taking the files it left behind
  for someone else's.
- `update` now writes the new binary to disk before it replaces the old one, so a crash right after
  can't leave an empty `sessionsnap`.
- tmux inside cmux: a Codex launch through cmux's wrapper (which adds its computer-use MCP server)
  was saved as a plain tab; that server is now left out with a note and the session kept.
- `ui --port 0` and `ui --idle-minutes 0` are refused instead of serving a page nobody could open.
- `install-cmux` and `uninstall` replaced a symlinked `cmux.json` with a plain file; they now write
  the file it points to and keep its permissions.

## [0.1.1] - 2026-10-31

### Added

- Automatic snapshots on Linux, as systemd user timers.
- `export` and `import`: move a snapshot version to another machine as one file, with `--map` for
  its folders. Only the snapshot travels, never an agent's history of a session.
- Export and Import in the web UI.
- Web UI: tabs move by keyboard or touch too (Move up, Move down, Move to…); a restore plan shows
  the command each shell tab will run.

### Fixed

- Web UI: "Save automatically" on the list page always failed.
- A partial save (`save <name> -w <workspace>`) dropped the saved commands of that workspace's
  shell tabs.
- A scheduled save running during a `delete` or `rename` could bring the old name back, and a save
  by hand could land in a snapshot that had just become automatic. A failed `delete` no longer
  leaves an automatic snapshot without its schedule.
- A Claude Code session ID is checked before it names a transcript file.
- Two restore plans made in the same millisecond no longer share an ID.
- `prune --keep 0` is refused before asking; `import` exits 7 only for a missing file; a usage
  error has code `usage` in the web UI too; an empty `recent --plan` has the shape of
  `restore --plan`'s.
- A failed write no longer leaves a temporary file, and `rename` onto a folder holding only a
  crashed save's leftovers works.
- install.sh warns when another sessionsnap comes first on PATH, and on Linux names what works
  there (tmux).
- `scripts/release.sh` publishes the release only once all its files are up, and prints how to
  finish or undo a release that stopped halfway.
- Diffs and version messages: a workspace cmux renamed showed as one removed and one added
  with every tab moved, and a new folder or branch showed as "no changes". They now show as
  `workspace-renamed` and `workspace-changed`. A rename is found by the cmux workspace ID or,
  without one (tmux, or after cmux gave new IDs), by the workspace keeping most of its tabs.
- tmux older than 3.5 (as on Ubuntu 24.04): its sessions were read as none, so saves from it were
  skipped.

### Changed

- Web UI: Discard asks before dropping unsaved edits, and leaving the page with unsaved edits asks
  first.
- A restore plan shows the command each terminal tab will run.
- What sessionsnap prints never carries a control character from a snapshot (an escape sequence in
  a hand-edited title): it is shown escaped.
- A save reads only what was appended to each Claude Code transcript since the last one.

## [0.1.0] - 2026-10-31

The first release.

### Added

- Named, versioned snapshots of cmux workspaces and tmux sessions, with the coding-agent session in
  each tab: `save`, `list`, `show`, `history`, `diff`, `prune`, `delete`, `rename`.
- Restore that reopens only what is missing, as a plan you see first (`restore --plan`, `apply`),
  resuming each agent with the flags on its allow-list. Claude Code and Codex are fully supported;
  15 more agents are resumed with the commands cmux documents for them (untested).
- tmux: sessions are workspaces and panes are tabs; each pane's agent is found in its processes,
  with its session named by its arguments or matched from its history (never guessed).
- Terminal detection: the terminal sessionsnap runs in (tmux first), else the one running;
  `--terminal cmux|tmux` picks one.
- Automatic snapshots saved every interval by launchd on macOS (`schedule add|list|edit|stop|start|run|remove`).
- Editing snapshots: workspaces and tabs (`ws`, `tab`, `edit --batch`, `edit --dry-run`).
- Finding lost sessions: `recent` and `sessions`.
- `restore` with no name restores the most recently saved snapshot; `list` names each snapshot's
  workspaces, folders and agents, and takes `--cwd`.
- A web UI in the cmux Dock, or the default browser from tmux; cmux palette commands; a Claude
  Code skill (`install-skill`); shell completions (`completions`) and a man page (`man`).
- `--json` on every command, stable exit codes and error codes, `update` and `uninstall`.

[0.1.0]: https://github.com/pawanpaudel93/sessionsnap/releases/tag/v0.1.0
