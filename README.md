# sessionsnap

Named, versioned snapshots of [cmux](https://cmux.com) workspaces and the Claude Code sessions in
their tabs. Save what is open, edit it, and later reopen only what is missing, with each Claude
tab resumed (`claude --resume <id>`) in its own folder.

Nothing runs in the background: every snapshot is one you ask for.

## Install

```sh
cargo install --path .
sessionsnap install-cmux     # optional: palette commands in cmux (asks first, backs up cmux.json)
sessionsnap install-skill    # optional: teaches Claude Code to use sessionsnap
```

## Use

```sh
sessionsnap save work                 # snapshot everything open (new version if "work" exists)
sessionsnap save work -w Cascadia     # update only one workspace in the snapshot
sessionsnap show work                 # workspaces and tabs
sessionsnap history work              # versions
sessionsnap diff work@3 work@4
sessionsnap restore work --plan       # what would be reopened
sessionsnap restore work              # plan, confirm, reopen what is missing
sessionsnap recent                    # Claude sessions from the last 24h not open in cmux
sessionsnap tab work Cascadia#4 --move "Cascadia Community"
sessionsnap tab work Cascadia --add-session <session-id>
```

Every command takes `--json` (for scripts and agents) and `--yes` (skip confirmations).
Exit codes: 0 ok, 1 error, 2 cmux unreachable, 3 nothing to do, 4 refused, 5 unknown cmux state
format, 6 plan stale, 7 not found.

## Web UI

`sessionsnap ui` serves a small page on `http://127.0.0.1:47317/` and shows it in the cmux Dock (the
right sidebar). It runs while its terminal is open and stops after 30 minutes without use; nothing
starts at login. From the palette: "sessionsnap: Open UI" (it runs in its own "sessionsnap UI" workspace).

- The link it opens carries a one-time key after `#` (`…/#t=<key>`). Browsers never send that
  part to a server; the page keeps the key for its own calls. A new start makes a new key.
- Flags: `--port <n>` (default 47317), `--idle-minutes <n>` (default 30), `--no-open` (serve
  without showing the page).
- If the page has no key or an old one, click **Reconnect**, or run `sessionsnap ui` again: either asks
  the running server to show its page again with its key.
- To stop it: Ctrl-C in the terminal running `sessionsnap ui`, or close the "sessionsnap UI" workspace.

- **Snapshots:** save what is open, update a snapshot from what is open, delete one.
- **Workspaces:** rename (double-click), reorder, remove, drag tabs to reorder or move them, add a
  Claude session or a shell tab, add a workspace from cmux or from another snapshot. Edits are
  previewed and saved together as one version.
- **History:** compare two versions; restore an older one.
- **Restore:** tick workspaces and tabs, see what would open, then restore.
- **Recent:** Claude sessions from the last days that are not open in cmux.

In cmux 0.64 the Dock is a beta feature (Settings → Beta Features → Dock). Without it the page
opens as a normal browser tab.

## Where things live

- Snapshots: `~/.sessionsnap/snapshots/<name>/<NNNN>-<time>.json` (`SESSIONSNAP_HOME` to change)
- Reads cmux's state file and `cmux --json tree`; reads Claude transcripts under `~/.claude/projects`
- Never stores environment variables from cmux's state

## Verified

On 2026-10-11 with cmux 0.64.22 (macOS):

- `save` captured 9 workspaces and 29 Claude tabs; every tab's session ID matched cmux's own record.
- `restore` reopened a closed workspace and resumed its Claude conversation, with cmux running.
- `install-cmux` added the palette commands to cmux's commented `cmux.json`; ⌘⇧P shows them.
- "sessionsnap: Open UI" showed the UI in the Dock and switched the right sidebar to it (cmux
  pauses pages in a hidden Dock); running it again pointed the open page at the new key.
- Not yet checked in cmux's own browser: dragging a tab between workspaces.
- Not yet checked: `restore` when it has to start cmux itself (after a reboot). Until then, open
  cmux first, wait for it to reopen your workspaces, then restore.
