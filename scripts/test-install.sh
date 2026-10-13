#!/bin/sh
# Runs install.sh offline against fake releases: scripts/test-install.sh
# Fake `curl` and `uname` on PATH serve assets from a temp folder and pretend to be each
# platform, so no network is used and nothing outside the temp folder is touched.
set -eu
cd "$(dirname "$0")/.."
ROOT=$(pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/sessionsnap-test-install.XXXXXX")
trap 'rm -rf "$T"' EXIT INT TERM

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  [ ! -f "$T/out" ] || sed 's/^/    /' "$T/out" >&2
  exit 1
}
sha() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi; }

# --- fake tools and releases -----------------------------------------------
mkdir -p "$T/bin" "$T/dist"
cat >"$T/bin/curl" <<'EOF'
#!/bin/sh
out=""
url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -w | --retry | -H | --connect-timeout | --max-time) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
f="$FAKE_DIST/${url##*/}"
[ -f "$f" ] || exit 22
if [ -n "$out" ]; then cp "$f" "$out"; else cat "$f"; fi
EOF
cat >"$T/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo "$FAKE_OS" ;;
  -m) echo "$FAKE_ARCH" ;;
esac
EOF
# A cmux on PATH: the Linux installs must still not offer the palette step.
printf '#!/bin/sh\nexit 0\n' >"$T/bin/cmux"
chmod 755 "$T/bin/curl" "$T/bin/uname" "$T/bin/cmux"

# release VERSION TARGET BODY: a tarball whose `sessionsnap` is a script with BODY.
release() {
  mkdir -p "$T/pkg"
  printf '#!/bin/sh\n%s\n' "$3" >"$T/pkg/sessionsnap"
  chmod 755 "$T/pkg/sessionsnap"
  name="sessionsnap-$1-$2.tar.gz"
  tar -C "$T/pkg" -czf "$T/dist/$name" sessionsnap
  (cd "$T/dist" && sha "$name" >"$name.sha256")
}
GOOD='[ "$1" = --version ] && echo "sessionsnap 9.9.9"'
for t in universal-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  release 9.9.9 "$t" "$GOOD"
done
release 8.8.8 x86_64-unknown-linux-musl 'exit 1'
release 7.7.7 x86_64-unknown-linux-musl "$GOOD"
# Runs, but is not the version asked for.
release 6.6.6 x86_64-unknown-linux-musl "$GOOD"
echo "0000000000000000000000000000000000000000000000000000000000000000  x" \
  >"$T/dist/sessionsnap-7.7.7-x86_64-unknown-linux-musl.tar.gz.sha256"

# run OS ARCH SHELL [VAR=value...]: install.sh in a fresh HOME; output in $T/out.
run() {
  os=$1 arch=$2 shell=$3
  shift 3
  rm -rf "${T:?}/home"
  mkdir -p "$T/home"
  env -i PATH="$T/bin:/usr/bin:/bin" HOME="$T/home" TMPDIR="$T" SHELL="/bin/$shell" \
    FAKE_DIST="$T/dist" FAKE_OS="$os" FAKE_ARCH="$arch" SESSIONSNAP_VERSION=9.9.9 "$@" \
    sh "$ROOT/install.sh" >"$T/out" 2>&1
}
has_out() { grep -qF -- "$1" "$T/out"; }

# --- cases -----------------------------------------------------------------
run Darwin arm64 zsh || fail "macOS install"
has_out "(universal-apple-darwin)" || fail "macOS picks the universal build"
has_out "sessionsnap 9.9.9 is installed." || fail "macOS reports the version"
has_out "install-cmux" || fail "macOS mentions the palette step"
[ -x "$T/home/.local/bin/sessionsnap" ] || fail "binary in ~/.local/bin"
grep -qxF "export PATH=\"$T/home/.local/bin:\$PATH\"" "$T/home/.zshrc" || fail "zsh PATH line"
# Re-running upgrades in place: one PATH line, no temp files left.
env -i PATH="$T/bin:/usr/bin:/bin" HOME="$T/home" TMPDIR="$T" SHELL=/bin/zsh FAKE_DIST="$T/dist" \
  FAKE_OS=Darwin FAKE_ARCH=arm64 SESSIONSNAP_VERSION=9.9.9 sh "$ROOT/install.sh" >"$T/out" 2>&1 || fail "re-run"
[ "$(grep -c PATH "$T/home/.zshrc")" = 1 ] || fail "PATH line added twice"
[ "$(ls -A "$T/home/.local/bin")" = sessionsnap ] || fail "temp files left in the install dir"

run Linux x86_64 bash || fail "Linux x86_64 install"
has_out "(x86_64-unknown-linux-musl)" || fail "Linux x86_64 asset"
grep -qF "$T/home/.local/bin" "$T/home/.bashrc" || fail "bash PATH line in .bashrc on Linux"
! has_out "install-cmux" || fail "Linux must not offer the cmux palette"

run Linux aarch64 fish XDG_CONFIG_HOME="$T/home/xdg" || fail "Linux aarch64 install"
has_out "(aarch64-unknown-linux-musl)" || fail "Linux aarch64 asset"
grep -qxF "fish_add_path \"$T/home/.local/bin\"" "$T/home/xdg/fish/config.fish" || fail "fish PATH line"

run Linux riscv64 bash && fail "riscv64 must be refused"
has_out "no sessionsnap build for Linux on riscv64" || fail "riscv64 message"

run Linux x86_64 bash SESSIONSNAP_VERSION=7.7.7 && fail "a checksum mismatch must be refused"
has_out "checksum mismatch" || fail "checksum message"
[ ! -e "$T/home/.local/bin/sessionsnap" ] || fail "installed despite a checksum mismatch"

run Linux x86_64 bash SESSIONSNAP_VERSION=8.8.8 && fail "a binary that does not run must be refused"
has_out "does not run" || fail "broken binary message"
[ ! -e "$T/home/.local/bin/sessionsnap" ] || fail "installed a binary that does not run"

# over OS VERSION: install.sh again into the same HOME (after a good install).
over() {
  env -i PATH="$T/bin:/usr/bin:/bin" HOME="$T/home" TMPDIR="$T" SHELL=/bin/bash FAKE_DIST="$T/dist" \
    FAKE_OS=Linux FAKE_ARCH=x86_64 SESSIONSNAP_VERSION="$1" sh "$ROOT/install.sh" >"$T/out" 2>&1
}
# A new binary that does not run, or reports another version, never replaces a working one.
run Linux x86_64 bash || fail "good install before a bad upgrade"
for bad in 8.8.8 6.6.6; do
  over "$bad" && fail "upgrading to the bad $bad must fail"
  has_out "does not run" || has_out "reports" || fail "message for the bad $bad"
  [ "$("$T/home/.local/bin/sessionsnap" --version)" = "sessionsnap 9.9.9" ] || fail "the working binary was replaced by $bad"
  [ "$(ls -A "$T/home/.local/bin")" = sessionsnap ] || fail "temp files left after the bad $bad"
done


run Linux x86_64 bash 'SESSIONSNAP_INSTALL_DIR=~/tools' || fail "a quoted ~ install dir"
[ -x "$T/home/tools/sessionsnap" ] || fail "~ expanded to HOME"
grep -qF "export PATH=\"$T/home/tools:\$PATH\"" "$T/home/.bashrc" || fail "expanded dir in the PATH line"

for bad in 'tools' '/tmp/a"b' '/tmp/$(id)' '/tmp/a`id`' '/tmp/a\b'; do
  run Linux x86_64 bash SESSIONSNAP_INSTALL_DIR="$bad" && fail "install dir '$bad' must be refused"
  has_out "SESSIONSNAP_INSTALL_DIR" || fail "message for '$bad'"
done
run Linux x86_64 bash SESSIONSNAP_INSTALL_DIR="/tmp/a
b" && fail "a newline in the install dir must be refused"
has_out "control characters" || fail "newline message"

mkdir -p "$T/occupied/sessionsnap"
run Linux x86_64 bash SESSIONSNAP_INSTALL_DIR="$T/occupied" && fail "a folder named sessionsnap must be refused"
has_out "is a folder" || fail "folder message"

echo "install.sh: all cases passed"
