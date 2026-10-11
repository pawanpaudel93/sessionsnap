---
name: sessionsnap
description: Save, edit and restore named, versioned snapshots of cmux workspaces and their Claude Code sessions with the sessionsnap CLI. Use when the user wants to snapshot what is open in cmux, bring back lost cmux tabs or Claude sessions, see what a snapshot contains or how it changed, or edit a saved snapshot.
---

# sessionsnap

`sessionsnap` saves what is open in cmux (workspaces, tabs, and the Claude session in each tab) as
named snapshots with versions, and reopens only what is missing.

## Rules

- Always pass `--json` and read the result; never parse the human output.
- Restoring is two steps. Run `sessionsnap restore <name> --plan --json`, show the user what the plan
  creates or adds, what it skips, and its warnings. Run `sessionsnap apply <planId> --json` only after
  the user agrees, unless they already told you to restore without asking.
- If `restore … --plan` (or `recent --plan`) exits 2 because cmux is not running, run
  `sessionsnap restore <name> --json` (or `sessionsnap recent --restore --json`) without `--yes`: it starts cmux,
  saves the plan and does not apply it; show that plan, then `apply`.
- When some steps fail, `apply`/`restore` exit 1 with the normal result JSON (`failed: n`,
  `steps[].error`), not the `{"error": …}` shape. Argument (usage) errors print plain text and exit 1.
- Never pass `--yes` to `delete`, `prune` or `install-cmux` unless the user asked for that exact action.
- Never schedule or automate saving (cron, loops, hooks) unless the user asks for it.
- Do not start `sessionsnap ui` unless the user asks for the UI; it is for people.
- `sessionsnap ui` re-shows an already running UI (it asks that server to show its page) instead of starting a second one.
- `save` only reads cmux. If nothing changed it writes nothing and exits 3.

## Commands

| Goal | Command |
|---|---|
| Save / update everything open | `sessionsnap save <name> --json` |
| Update only some workspaces | `sessionsnap save <name> -w <ws> -w <ws> --json` |
| List snapshots | `sessionsnap list --json` |
| Contents | `sessionsnap show <name>[@v] --json` |
| Versions | `sessionsnap history <name> --json` |
| Compare | `sessionsnap diff <name>@a <name>@b --json` |
| Restore (plan) | `sessionsnap restore <name>[@v] [-w <ws>] [--tab <ws>#<n>] --plan --json` |
| Apply a plan | `sessionsnap apply <planId> --json` |
| Lost sessions not open in cmux | `sessionsnap recent [--hours 24] --json`, then `sessionsnap recent --plan --json` |
| Find a session | `sessionsnap sessions --search "<text>" --json` |
| Edit a tab | `sessionsnap tab <name> <ws>#<n> --rename/--move <ws>/--index <n>/--remove/--cwd/--session --json` |
| Add a session as a tab | `sessionsnap tab <name> <ws> --add-session <id> --json` |
| Edit a workspace | `sessionsnap ws <name> <ws> --rename/--color/--index/--remove --json` |
| Many edits as one version | `sessionsnap edit <name> --batch - --json` with a JSON array on stdin, e.g. `[{"op":"tab-move","tab":"Alpha#2","to":"Beta"}]` |
| Workspaces and tabs open in cmux now | `sessionsnap open --json` |
| Preview edits without saving | `sessionsnap edit <name> --batch - --dry-run --json` (ops on stdin) |

Tabs are `<workspace>#<position>` (1-based) or `<workspace>:"<part of the title>"`.
Edit ops: `ws-rename {ws,to}`, `ws-color {ws,color}`, `ws-index {ws,index}`, `ws-remove {ws}`,
`ws-add-live {ws}`, `ws-from {from,ws}`, `tab-remove {tab}`, `tab-rename {tab,to}`,
`tab-index {tab,index}`, `tab-move {tab,to}`, `tab-add-session {ws,sessionId,title?}`,
`tab-add-terminal {ws,cwd,command?,title?}`, `tab-set-cwd {tab,cwd}`, `tab-set-session {tab,sessionId}`.

## Exit codes

`0` ok · `1` error · `2` cmux not running/reachable · `3` nothing to do · `4` refused (needs
confirmation with `--yes`, workspace name already exists, session has no recorded folder, or
cmux.json is not plain JSON) · `5` unknown cmux state format · `6` plan
stale (make a new one) · `7` not found. With `--json`, errors print `{"error": {code, exit, message}}`.
