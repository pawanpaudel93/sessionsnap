# Changelog

All notable changes to sessionsnap are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

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
