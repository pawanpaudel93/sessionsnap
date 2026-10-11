#!/bin/sh
# Every check CI runs, in one place, so a release runs exactly the same ones:
#   scripts/check.sh
# Needs cargo, node and shellcheck; python3 for the site check once site/ exists. Runs on macOS
# and Linux.
set -eu
cd "$(dirname "$0")/.."
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

say "cargo fmt --check"
cargo fmt --check
say "cargo clippy"
cargo clippy --locked --all-targets -- -D warnings
say "cargo test"
cargo test --locked

say "web UI script"
node --check src/interfaces/web/assets/app.js

say "shell syntax"
for f in install.sh scripts/*.sh; do
  sh -n "$f"
done
say "shellcheck"
shellcheck -S warning install.sh scripts/*.sh
say "install.sh against fake releases (offline)"
sh scripts/test-install.sh

if [ -f scripts/test-site.py ]; then
  say "static site"
  python3 scripts/test-site.py
fi
