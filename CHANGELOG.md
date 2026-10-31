# Changelog

All notable changes to sessionsnap are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Automatic snapshots on Linux, as systemd user timers.
- `export` and `import`: move a snapshot version to another machine as one file, with `--map` for
  its folders. Only the snapshot travels, never an agent's history of a session.
- Export and Import in the web UI.

### Fixed

- Diffs and version messages: a workspace cmux renamed showed as one removed and one added
  with every tab moved, and a new folder or branch showed as "no changes". They now show as
  `workspace-renamed` and `workspace-changed`.
- tmux older than 3.5 (as on Ubuntu 24.04): its sessions were read as none, so saves from it were
  skipped.
- Scheduled cmux saves: cmux's default socket mode refuses programs it didn't start, so every
  scheduled save skipped; they now save from the state cmux writes every few seconds.

### Changed

- A restore plan shows the command each terminal tab will run.
- What sessionsnap prints never carries a control character from a snapshot (an escape sequence in
  a hand-edited title): it is shown escaped.
- A save reads only what was appended to each Claude Code transcript since the last one.
- The terminal is picked only when a command needs one; `list`, `show` and `update` never wait on it.
- `restore` with no name restores the most recently saved snapshot; `list` names each snapshot's
  workspaces, folders and agents, and takes `--cwd`.

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
- A web UI in the cmux Dock, or the default browser from tmux; cmux palette commands; a Claude
  Code skill (`install-skill`); shell completions (`completions`) and a man page (`man`).
- `--json` on every command, stable exit codes and error codes, `update` and `uninstall`.

[0.1.0]: https://github.com/pawanpaudel93/sessionsnap/releases/tag/v0.1.0
