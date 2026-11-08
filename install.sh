#!/bin/sh
# sessionsnap installer: curl -fsSL https://sessionsnap.pawanpaudel.com.np/install.sh | sh
#
# Environment:
#   SESSIONSNAP_VERSION      version to install, e.g. 0.1.0 (default: latest release)
#   SESSIONSNAP_INSTALL_DIR  where the binary goes (default: $HOME/.local/bin); an absolute path
#   SESSIONSNAP_NO_CMUX=1    don't offer to add the sessionsnap commands to the cmux palette (macOS)
#
# Runs on macOS (Apple Silicon or Intel) and Linux (x86_64 or arm64). Re-running upgrades in place.
set -eu

# Everything runs from main, called on the last line, so a download cut short runs nothing: a
# partial file only defines part of a function (a syntax error), never runs half the steps.
main() {
  REPO="pawanpaudel93/sessionsnap"
  INSTALL_DIR="${SESSIONSNAP_INSTALL_DIR:-$HOME/.local/bin}"

  say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
  warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
  die() {
    printf '\033[31merror:\033[0m %s\n' "$*" >&2
    exit 1
  }
  has() { command -v "$1" >/dev/null 2>&1; }

  # The folder ends up in a PATH line in the user's shell startup file, so it must be a plain
  # absolute path: a quoted "~/bin" is expanded, and quotes, $, `, \ or control characters refused.
  case "$INSTALL_DIR" in
    \~) INSTALL_DIR="$HOME" ;;
    \~/*) INSTALL_DIR="$HOME/${INSTALL_DIR#??}" ;; # drop the literal ~/
  esac
  case "$INSTALL_DIR" in
    /*) ;;
    *) die "SESSIONSNAP_INSTALL_DIR must be an absolute path (got: $INSTALL_DIR)." ;;
  esac
  case "$INSTALL_DIR" in
    *[\"\'\$\`\\]*) die "SESSIONSNAP_INSTALL_DIR may not contain quotes, \$, \` or backslashes." ;;
  esac
  [ "$(printf '%s' "$INSTALL_DIR" | tr -d '\001-\037\177')" = "$INSTALL_DIR" ] ||
    die "SESSIONSNAP_INSTALL_DIR may not contain control characters or newlines."

  # --- platform --------------------------------------------------------------
  OS=$(uname -s)
  ARCH=$(uname -m)
  case "$OS" in
    Darwin) TARGET="universal-apple-darwin" ;; # one binary for arm64 and x86_64
    Linux)
      case "$ARCH" in
        x86_64 | amd64) TARGET="x86_64-unknown-linux-musl" ;;
        aarch64 | arm64) TARGET="aarch64-unknown-linux-musl" ;;
        *) die "no sessionsnap build for Linux on $ARCH; build from source: cargo install --git https://github.com/$REPO" ;;
      esac
      ;;
    *) die "sessionsnap runs on macOS and Linux (found $OS)." ;;
  esac
  has curl || die "curl is required."
  has tar || die "tar is required."
  if has shasum; then
    sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
  elif has sha256sum; then
    sha256() { sha256sum "$1" | awk '{print $1}'; }
  else
    die "shasum or sha256sum is required."
  fi

  # --- version ---------------------------------------------------------------
  if [ -n "${SESSIONSNAP_VERSION:-}" ]; then
    VERSION="${SESSIONSNAP_VERSION#v}"
  else
    say "Looking up the latest release"
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null |
      sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) || TAG=""
    if [ -z "$TAG" ]; then
      # API rate-limited or unreachable: follow the releases/latest redirect instead.
      TAG=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null |
        sed -n 's#.*/releases/tag/##p') || TAG=""
    fi
    if [ -z "$TAG" ]; then
      die "no sessionsnap release is published yet (or GitHub is unreachable).
  Build from source instead:  cargo install --git https://github.com/$REPO
  Or, if you know a published version:  SESSIONSNAP_VERSION=x.y.z curl ... | sh"
    fi
    VERSION="${TAG#v}"
  fi
  printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' ||
    die "unexpected version: $VERSION"

  ASSET="sessionsnap-$VERSION-$TARGET.tar.gz"
  URL="https://github.com/$REPO/releases/download/v$VERSION/$ASSET"

  # --- download + verify -----------------------------------------------------
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/sessionsnap-install.XXXXXX")
  trap 'rm -rf "$TMP" ${NEW:+"$NEW"}' EXIT INT TERM

  say "Downloading sessionsnap $VERSION ($TARGET)"
  curl -fSL --retry 3 --progress-bar -o "$TMP/$ASSET" "$URL" || die "download failed: $URL"
  curl -fsSL --retry 3 -o "$TMP/$ASSET.sha256" "$URL.sha256" || die "download failed: $URL.sha256"

  say "Verifying checksum"
  EXPECTED=$(awk '{print $1; exit}' "$TMP/$ASSET.sha256")
  ACTUAL=$(sha256 "$TMP/$ASSET")
  [ -n "$EXPECTED" ] && [ "$EXPECTED" = "$ACTUAL" ] ||
    die "checksum mismatch for $ASSET (expected $EXPECTED, got $ACTUAL)."

  tar -xzf "$TMP/$ASSET" -C "$TMP"
  SRC="$TMP/sessionsnap"
  [ -f "$SRC" ] || die "the archive does not contain a sessionsnap binary."

  # --- install ---------------------------------------------------------------
  mkdir -p "$INSTALL_DIR"
  [ ! -d "$INSTALL_DIR/sessionsnap" ] || die "$INSTALL_DIR/sessionsnap is a folder; move it away first."
  # Copy then rename, so a running sessionsnap (e.g. an open UI) keeps its old file.
  # A temp name of its own, so two installers at once can't write into each other's copy.
  NEW=$(mktemp "$INSTALL_DIR/.sessionsnap.XXXXXX")
  cp "$SRC" "$NEW"
  chmod 755 "$NEW"
  # Check the new binary where it will run (a temp folder may be noexec) before it replaces the old
  # one, so a broken download never leaves you without a working sessionsnap.
  VER=$("$NEW" --version 2>/dev/null) || die "the downloaded binary does not run; $INSTALL_DIR/sessionsnap was left as it was."
  [ "$VER" = "sessionsnap $VERSION" ] ||
    die "the downloaded binary reports '$VER', not sessionsnap $VERSION; $INSTALL_DIR/sessionsnap was left as it was."
  mv -f "$NEW" "$INSTALL_DIR/sessionsnap"
  NEW=""
  BIN="$INSTALL_DIR/sessionsnap"
  say "Installed $BIN"

  # --- PATH ------------------------------------------------------------------
  shell_rc() {
    case "$(basename "${SHELL:-sh}")" in
      bash)
        # macOS login shells read .bash_profile; Linux terminals read .bashrc.
        if [ "$OS" = "Darwin" ]; then echo "$HOME/.bash_profile"; else echo "$HOME/.bashrc"; fi
        ;;
      fish) echo "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
      zsh) echo "${ZDOTDIR:-$HOME}/.zshrc" ;;
      *) echo "$HOME/.profile" ;;
    esac
  }

  # add_line FILE LINE: append LINE unless FILE already has it.
  add_line() {
    mkdir -p "$(dirname "$1")"
    touch "$1"
    grep -qxF "$2" "$1" || printf '\n%s\n' "$2" >>"$1"
  }

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
      RC=$(shell_rc)
      case "$RC" in
        *.fish) add_line "$RC" "fish_add_path \"$INSTALL_DIR\"" ;;
        *) add_line "$RC" "export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
      esac
      say "Added $INSTALL_DIR to PATH in $RC (open a new terminal to pick it up)"
      PATH="$INSTALL_DIR:$PATH"
      ;;
  esac

  # Another sessionsnap earlier on PATH (an old `cargo install` in ~/.cargo/bin, say) would run
  # instead of this one. Compare folders physically, so a symlinked or trailing-/ path still matches.
  FOUND=$(command -v sessionsnap 2>/dev/null) || FOUND=""
  if [ -n "$FOUND" ] &&
    [ "$(cd "$(dirname "$FOUND")" 2>/dev/null && pwd -P)" != "$(cd "$INSTALL_DIR" && pwd -P)" ]; then
    warn "$FOUND comes first on PATH, so \`sessionsnap\` runs that one, not $BIN.
  Remove it (rm \"$FOUND\"), or put $INSTALL_DIR before $(dirname "$FOUND") in PATH."
  fi

  # --- cmux palette (macOS) --------------------------------------------------
  # The palette commands are for cmux on macOS; cmux on Linux is a different app.
  # install-cmux asks before it changes cmux.json, so it needs the terminal even under curl | sh.
  if [ "$OS" = "Darwin" ]; then
    if [ "${SESSIONSNAP_NO_CMUX:-0}" != "1" ] && has cmux && (: </dev/tty) 2>/dev/null; then
      say "Adding sessionsnap to the cmux command palette"
      "$BIN" install-cmux </dev/tty || warn "sessionsnap install-cmux did not finish; run it again later."
    else
      say "To add sessionsnap to the cmux command palette later, run: sessionsnap install-cmux"
    fi
  fi

  if [ "$OS" = "Darwin" ]; then
    NEXT="  sessionsnap save work      # snapshot the cmux workspaces and agent sessions open now
  sessionsnap ui             # the web UI in the cmux Dock
  sessionsnap install-skill  # teach Claude Code, Pi and other agents to use sessionsnap (optional)"
  else
    NEXT="  Inside tmux:
  sessionsnap save work      # save your tmux sessions and their agents
  sessionsnap restore work   # reopen what is missing
  (cmux on Linux is not supported yet.)"
  fi

  cat <<EOF

$VER is installed.

Next steps:
$NEXT

Update with: sessionsnap update      Remove with: sessionsnap uninstall
Docs: https://sessionsnap.pawanpaudel.com.np
EOF
}

{ main "$@"; }
