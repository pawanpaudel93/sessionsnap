#!/bin/sh
# Runs what CI runs (.github/workflows/ci.yml), here: scripts/check.sh on this machine, the same in a
# Linux container as a normal user, and the tests on the minimum Rust version. Needs Docker.
# `scripts/release.sh --local-ci` runs it instead of waiting for CI.
#
#   scripts/ci-local.sh            all three
#   scripts/ci-local.sh linux      only the Linux check
#   scripts/ci-local.sh msrv       only the minimum-Rust tests
set -eu

cd "$(dirname "$0")/.."
# The images CI's jobs stand for: Debian with Rust, and the minimum Rust version (Cargo.toml).
LINUX_IMAGE=${CI_LOCAL_LINUX_IMAGE:-rust:1.99.0-bookworm}
MSRV=$(sed -n 's/^rust-version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
MSRV_IMAGE=${CI_LOCAL_MSRV_IMAGE:-rust:$MSRV-bookworm}

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || die "docker is required."
docker info >/dev/null 2>&1 || die "docker is not running."

# In a container: copy the repo (read-only mount) into a normal user's home, without target/.
in_container() { # IMAGE SETUP COMMAND
  docker run --rm -v "$PWD:/src:ro" "$1" sh -c "
    set -e
    $2
    useradd -m ci >/dev/null
    mkdir /home/ci/work && cd /src && tar --exclude=./target --exclude=./dist -cf - . | tar -xf - -C /home/ci/work
    chown -R ci /home/ci/work /usr/local/cargo /usr/local/rustup
    su ci -c 'export PATH=/usr/local/cargo/bin:\$PATH; cd /home/ci/work && $3'
  "
}

run_host() {
  say "check.sh on $(uname -s)"
  scripts/check.sh
}

run_linux() {
  say "check.sh on Linux ($LINUX_IMAGE)"
  in_container "$LINUX_IMAGE" \
    "apt-get update -qq >/dev/null && apt-get install -y -qq nodejs shellcheck python3 >/dev/null && rustup component add rustfmt clippy >/dev/null 2>&1" \
    "scripts/check.sh"
}

run_msrv() {
  say "tests on Rust $MSRV ($MSRV_IMAGE)"
  in_container "$MSRV_IMAGE" ":" "cargo test --locked -q"
}

case "${1:-all}" in
  all) run_host && run_linux && run_msrv ;;
  host) run_host ;;
  linux) run_linux ;;
  msrv) run_msrv ;;
  *) die "usage: scripts/ci-local.sh [all|host|linux|msrv]" ;;
esac
say "Local CI passed"
