# ccsnap

Named, versioned snapshots of [cmux](https://cmux.com) workspaces and the Claude Code sessions in
their tabs. Save what is open, edit it, and later reopen only what is missing, with each Claude
tab resumed (`claude --resume <id>`) in its own folder.

Nothing runs in the background: every snapshot is one you ask for.

## Install

```sh
cargo install --path .
ccsnap install-cmux     # optional: palette commands in cmux (asks first, backs up cmux.json)
ccsnap install-skill    # optional: teaches Claude Code to use ccsnap
```

## Use

```sh
ccsnap save work                 # snapshot everything open (new version if "work" exists)
ccsnap save work -w Cascadia     # update only one workspace in the snapshot
ccsnap show work                 # workspaces and tabs
ccsnap history work              # versions
ccsnap diff work@3 work@4
ccsnap restore work --plan       # what would be reopened
ccsnap restore work              # plan, confirm, reopen what is missing
ccsnap recent                    # Claude sessions from the last 24h not open in cmux
ccsnap tab work Cascadia#4 --move "Cascadia Community"
ccsnap tab work Cascadia --add-session <session-id>
```

Every command takes `--json` (for scripts and agents) and `--yes` (skip confirmations).
Exit codes: 0 ok, 1 error, 2 cmux unreachable, 3 nothing to do, 4 refused, 5 unknown cmux state
format, 6 plan stale, 7 not found.

## Where things live

- Snapshots: `~/.ccsnap/snapshots/<name>/<NNNN>-<time>.json` (`CCSNAP_HOME` to change)
- Reads cmux's state file and `cmux --json tree`; reads Claude transcripts under `~/.claude/projects`
- Never stores environment variables from cmux's state
