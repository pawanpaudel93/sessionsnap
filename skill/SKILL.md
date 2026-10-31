---
name: sessionsnap
description: Save, edit and restore named, versioned snapshots of cmux workspaces (or tmux sessions) and their agent sessions (Claude Code, Codex and other coding agents) with the sessionsnap CLI. Use when the user wants to snapshot what is open in cmux or tmux, bring back lost cmux or tmux tabs or agent sessions, see what a snapshot contains or how it changed, or edit a saved snapshot.
---

# sessionsnap

`sessionsnap` saves what is open in cmux or tmux (workspaces, tabs, and the agent session in each tab, with
the flags the agent was started with that are on its allow-list) as named snapshots with versions,
and reopens only what is missing. Claude Code and Codex are fully supported (their session
histories are read); 15 more agents are resumed with cmux's documented commands, only when started
without flags (`sessionsnap agents --json` lists them). Inside tmux it saves and restores tmux
sessions instead (it uses the terminal it runs in; `--terminal cmux|tmux` picks one); there, a pane whose agent session can't be told
is saved as a plain tab, and `save` says so.

## Rules

- Always pass `--json` and read the result; never parse the human output.
- Restoring is two steps. Run `sessionsnap restore <name> --plan --json`, show the user what the plan
  creates or adds, what it skips, and its warnings. Run `sessionsnap apply <planId> --json` only after
  the user agrees, unless they already told you to restore without asking. Prefer this to
  `restore --yes`, which applies without showing the plan. A plan expires after an hour and applies
  once; a second `apply` of it exits 3 or 7.
- If `restore … --plan` (or `recent --plan`) exits 2 because the terminal is not running, run
  `sessionsnap restore <name> --json` (or `sessionsnap recent --restore --json`) without `--yes`: it starts it
  (cmux; tmux only needs to be installed),
  saves the plan and does not apply it; show that plan, then `apply`.
- When some steps fail, `apply`/`restore` exit 1 with the normal result JSON (`failed: n`,
  `steps[].error`), not the `{"error": …}` shape; so does `uninstall` (a step with `status: "failed"`).
  A mistyped command exits 1 with `{"error": {"code": "usage", …}}`.
- Never pass `--yes` to `delete`, `prune`, `install-cmux` or `uninstall` unless the user asked for that
  exact action. An error with code `needs-confirmation` means: ask the user; never rerun with `--yes`
  on your own.
- `install-cmux` (the cmux palette) is macOS-only; on Linux it exits 4 and changes nothing.
- Run `update` (it replaces the sessionsnap binary) or `uninstall` only when the user asks; `update --check` is safe.
- Never start an automatic snapshot (`schedule add`) or automate saving any other way (cron, loops,
  hooks) unless the user asks for it.
- Do not start `sessionsnap ui` unless the user asks for the UI; it is for people.
- `sessionsnap ui` re-shows an already running UI (it asks that server to show its page) instead of starting a second one.
- `save` only reads the terminal. If nothing changed, or no tmux server runs, it writes nothing and exits 3. Its `warnings` name the
  flags it left out (only allow-listed flags are saved) and any agent session it left out (a
  restricting flag it could not save, a flag it doesn't know, flags of an untested agent, or a
  launch such as `claude -p` that is not a session); tell the user.
- A `list` row or `history` version with `"error"` (e.g. "unreadable: format 1, expected 2") is a
  file sessionsnap cannot read; tell the user, do not try to restore it.

## Commands

| Goal | Command |
|---|---|
| Save / update everything open | `sessionsnap save <name> --json` |
| Update only some workspaces | `sessionsnap save <name> -w <ws> -w <ws> --json` |
| List snapshots | `sessionsnap list [--cwd <dir>] --json` (rows have `workspaceNames`, `folders`, `agents`; `--cwd .` keeps the ones with something in this folder) |
| Contents | `sessionsnap show <name>[@v] --json` |
| Versions | `sessionsnap history <name> --json` |
| Compare | `sessionsnap diff <name>@a <name>@b --json` |
| Restore (plan) | `sessionsnap restore <name>[@v] [-w <ws>] [--tab <ws>#<n>] --plan --json` (a `--tab` tab is added to its open workspace, shells too; with no name, the most recently saved snapshot, named in `snapshot`) |
| Apply a plan | `sessionsnap apply <planId> --json` |
| Lost sessions not open in the terminal | `sessionsnap recent [--hours 24] [--agent <kind>] --json`, then `sessionsnap recent --plan --json` |
| Find a session | `sessionsnap sessions --search "<text>" [--agent <kind>] --json` |
| Edit a tab | `sessionsnap tab <name> <ws>#<n> --rename/--move <ws>/--index <n>/--remove/--cwd/--session [--agent <kind>] --json` |
| Add a session as a tab | `sessionsnap tab <name> <ws> --add-session <id> [--agent <kind>] --json` |
| Known agents | `sessionsnap agents --json` |
| Edit a workspace | `sessionsnap ws <name> <ws> --rename/--color/--index/--remove --json` |
| Many edits as one version | `sessionsnap edit <name> --batch - --json` with a JSON array on stdin, e.g. `[{"op":"tab-move","tab":"Alpha#2","to":"Beta"}]` |
| Workspaces and tabs open in the terminal now | `sessionsnap open --json` |
| Rename a snapshot | `sessionsnap rename <old> <new> --json` |
| Move a snapshot to another machine | `sessionsnap export <name>[@v] -o <file> --json`, there `sessionsnap import <file> [--as <name>] [--map <old>=<new>] --json` (only the snapshot; show the user the plan's tab commands before restoring a file from someone else) |
| Save automatically every interval (launchd on macOS, systemd on Linux) | `sessionsnap schedule add <name> --interval 30m --json` (a new name; saves once now) |
| Automatic snapshots | `sessionsnap schedule list --json` (`running`, `lastRun`); `list --json` rows of automatic ones have `schedule` |
| Pause / resume / change one | `sessionsnap schedule stop\|start <name> --json`, `sessionsnap schedule edit <name> --interval 2h --json` |
| Save one now | `sessionsnap schedule run <name> --json` |
| Make it a normal snapshot | `sessionsnap schedule remove <name> --json` (versions kept; `save` to an automatic one is refused, exit 4) |
| Preview edits without saving | `sessionsnap edit <name> --batch - --dry-run --json` (ops on stdin); save them with `--base <baseVersion>` from the preview, which exits 6 if the snapshot changed since |
| Newer release? | `sessionsnap update --check --json` (`updateAvailable`) |
| Install the latest release | `sessionsnap update --json` |
| Remove sessionsnap | `sessionsnap uninstall [--keep-data] --yes --json` (lists each step's `status`) |

Tabs are `<workspace>#<position>` (1-based) or `<workspace>:"<part of the title>"`.
Edit ops: `ws-rename {ws,to}`, `ws-color {ws,color}`, `ws-index {ws,index}`, `ws-remove {ws}`,
`ws-add-live {ws}`, `ws-from {from,ws}`, `tab-remove {tab}`, `tab-rename {tab,to}`,
`tab-index {tab,index}`, `tab-move {tab,to}`, `tab-add-session {ws,sessionId,agent?,title?}`,
`tab-add-terminal {ws,cwd,command?,title?}`, `tab-set-cwd {tab,cwd}`, `tab-set-session {tab,sessionId,agent?}`.
Without `agent`, a session ID is looked up in the Claude Code and Codex histories; pass `agent`
(a kind from `sessionsnap agents`) when it is in both, or for other agents.

Saved tabs are `{"type":"terminal",title,cwd,command?}` or
`{"type":"agent",title,cwd,agent,sessionId,args?}`. Session JSON (`sessions`, `recent`) is
`{agent,sessionId,title,firstPrompt,cwd,modified}`; `sessions` and `recent` take `--limit <n>` and
report `total`. A restore plan warns when an agent's program is not on PATH, and when a tab will open
as a plain shell (unknown agent, invalid ID, session gone, its folder gone); an applied restore
repeats those warnings. A `save` that warns "is not running" was read from what the terminal last
wrote to disk and may be out of date. Output is one compact JSON line when stdout is not a terminal.

## Exit codes

`0` ok · `1` error (`usage` for a mistyped command) · `2` terminal not reachable
(`terminal-unreachable`: cmux not running and no saved state, tmux not installed) · `3` nothing to do ·
`4` refused (`needs-confirmation`: ask the user; or `refused`: workspace name already exists,
session has no recorded folder, cmux.json is not plain JSON) · `5` unknown cmux state or snapshot format (snapshots must be format 2) · `6` plan
stale (make a new one) · `7` not found. With `--json`, errors print `{"error": {code, exit, message}}`.
