---
name: ccsnap
description: Save, edit and restore named, versioned snapshots of cmux workspaces and their Claude Code sessions with the ccsnap CLI. Use when the user wants to snapshot what is open in cmux, bring back lost cmux tabs or Claude sessions, see what a snapshot contains or how it changed, or edit a saved snapshot.
---

# ccsnap

`ccsnap` saves what is open in cmux (workspaces, tabs, and the Claude session in each tab) as
named snapshots with versions, and reopens only what is missing.

## Rules

- Always pass `--json` and read the result; never parse the human output.
- Restoring is two steps. Run `ccsnap restore <name> --plan --json`, show the user what the plan
  creates or adds, what it skips, and its warnings. Run `ccsnap apply <planId> --json` only after
  the user agrees, unless they already told you to restore without asking.
- Never pass `--yes` to `delete`, `prune` or `install-cmux` unless the user asked for that exact action.
- Never schedule or automate saving (cron, loops, hooks) unless the user asks for it.
- `save` only reads cmux. If nothing changed it writes nothing and exits 3.

## Commands

| Goal | Command |
|---|---|
| Save / update everything open | `ccsnap save <name> --json` |
| Update only some workspaces | `ccsnap save <name> -w <ws> -w <ws> --json` |
| List snapshots | `ccsnap list --json` |
| Contents | `ccsnap show <name>[@v] --json` |
| Versions | `ccsnap history <name> --json` |
| Compare | `ccsnap diff <name>@a <name>@b --json` |
| Restore (plan) | `ccsnap restore <name>[@v] [-w <ws>] [--tab <ws>#<n>] --plan --json` |
| Apply a plan | `ccsnap apply <planId> --json` |
| Lost sessions not open in cmux | `ccsnap recent [--hours 24] --json`, then `ccsnap recent --plan --json` |
| Find a session | `ccsnap sessions --search "<text>" --json` |
| Edit a tab | `ccsnap tab <name> <ws>#<n> --rename/--move <ws>/--index <n>/--remove/--cwd/--session --json` |
| Add a session as a tab | `ccsnap tab <name> <ws> --add-session <id> --json` |
| Edit a workspace | `ccsnap ws <name> <ws> --rename/--color/--index/--remove --json` |
| Many edits as one version | `ccsnap edit <name> --batch - --json` with a JSON array on stdin, e.g. `[{"op":"tab-move","tab":"Alpha#2","to":"Beta"}]` |

Tabs are `<workspace>#<position>` (1-based) or `<workspace>:"<part of the title>"`.
Edit ops: `ws-rename {ws,to}`, `ws-color {ws,color}`, `ws-index {ws,index}`, `ws-remove {ws}`,
`ws-add-live {ws}`, `ws-from {from,ws}`, `tab-remove {tab}`, `tab-rename {tab,to}`,
`tab-index {tab,index}`, `tab-move {tab,to}`, `tab-add-session {ws,sessionId,title?}`,
`tab-add-terminal {ws,cwd,command?,title?}`, `tab-set-cwd {tab,cwd}`, `tab-set-session {tab,sessionId}`.

## Exit codes

`0` ok · `1` error · `2` cmux not running/reachable · `3` nothing to do · `4` refused (needs
confirmation, or the change would break resuming) · `5` unknown cmux state format · `6` plan
stale (make a new one) · `7` not found. With `--json`, errors print `{"error": {code, exit, message}}`.
