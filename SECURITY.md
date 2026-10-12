# Security policy

## Reporting a vulnerability

Please report security problems privately through GitHub:
**Security → Report a vulnerability** on
<https://github.com/pawanpaudel93/sessionsnap/security/advisories/new>.
Do not open a public issue for them.

Include what you ran, what happened, and the sessionsnap version (`sessionsnap --version`). Leave
out real tokens, keys and session contents; a made-up value that shows the problem is enough.

You should get a reply within a week. Fixes ship in a new release, and the advisory is published
once the release is out.

## Supported versions

Only the latest release gets security fixes. Run `sessionsnap update` to get it.

## Scope

sessionsnap turns snapshot files into commands typed into cmux, so the most important area is
anything that could make a restore run something other than the saved agent or command: quoting,
validation of session IDs and flags, and control characters. The README's
[Security](README.md#security) section lists the current safeguards. The web UI listens on
localhost only and needs a per-run key; a way around either is in scope too.
