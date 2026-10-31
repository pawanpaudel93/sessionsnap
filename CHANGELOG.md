# Changelog

All notable changes to sessionsnap are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

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
