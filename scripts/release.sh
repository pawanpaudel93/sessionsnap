#!/bin/sh
# Cut a sessionsnap release from a Mac: scripts/release.sh X.Y.Z [--dry-run] [-y]
#
# Bumps the version, runs the CI checks, builds dist/ (a universal macOS binary and static Linux
# builds for x86_64 and arm64, each as a tarball with a .sha256), then commits, tags, pushes main
# and the tag, and creates the GitHub Release with those assets. Uses your existing `gh` login.
# The Linux builds run in one Docker container of this Mac's own architecture
# (RELEASE_RUST_IMAGE, default rust:1.99.0-alpine), cross-linking the other with rust-lld.
#
#   --dry-run  build dist/ and show what would be published; commit, tag, push nothing
#   -y         don't ask before publishing
#
# RELEASE_ALLOW_DIRTY=1 lets --dry-run run on a dirty tree or another branch (for testing the
# script only).
set -eu

REPO="pawanpaudel93/sessionsnap"
MAC_TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
LINUX_TARGETS="x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
# Pinned to an exact Rust release so builds don't drift; bump it on purpose. (Its index digest
# was sha256:0cce0a5e0e8b…f1627 on 2026-10-12; a digest reference trips Docker Desktop when the
# same image is already pulled under another tag, so the version tag is used.)
RUST_IMAGE="${RELEASE_RUST_IMAGE:-rust:1.99.0-alpine}"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() {
  printf '\033[31merror:\033[0m %s\n' "$*" >&2
  exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

usage() {
  echo "usage: scripts/release.sh X.Y.Z [--dry-run] [-y]" >&2
  exit 2
}

VERSION=""
DRY_RUN=0
YES=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    -y | --yes) YES=1 ;;
    -h | --help) usage ;;
    -*) die "unknown option: $arg" ;;
    *)
      [ -z "$VERSION" ] || usage
      VERSION="${arg#v}"
      ;;
  esac
done
[ -n "$VERSION" ] || usage
printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' ||
  die "version must look like X.Y.Z (got $VERSION)."
TAG="v$VERSION"

cd "$(dirname "$0")/.."
ROOT=$(pwd)
DIST="$ROOT/dist"

# --- preconditions ---------------------------------------------------------
[ "$(uname -s)" = "Darwin" ] || die "releases are built on a Mac (lipo makes the universal binary)."
for tool in git cargo rustup lipo codesign docker shasum tar file node shellcheck; do
  has "$tool" || die "$tool is required."
done
installed=$(rustup target list --installed)
for t in $MAC_TARGETS; do
  printf '%s\n' "$installed" | grep -qx "$t" || die "missing Rust target $t; run: rustup target add $t"
done
docker info >/dev/null 2>&1 || die "Docker is not running; start it (the Linux builds run in containers)."
if [ "$DRY_RUN" = 0 ]; then
  has gh || die "gh is required."
  gh auth status >/dev/null 2>&1 || die "gh is not logged in; run: gh auth login"
fi

CURRENT=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$CURRENT" ] || die "no version in Cargo.toml."
# A release never goes backwards (the same version is the first release of it).
older=$(printf '%s %s\n' "${VERSION%%-*}" "${CURRENT%%-*}" | tr '.' ' ' |
  awk '{ for (i = 1; i <= 3; i++) { if ($i < $(i + 3)) { print 1; exit } if ($i > $(i + 3)) { print 0; exit } } print 0 }')
[ "$older" = 0 ] || die "$VERSION is older than the current version $CURRENT."

# Files that record the version: Cargo's, and on the site only the marked spots (the nav badge,
# and the home page's JSON-LD "version"), never examples in the text.
VERSION_FILES="Cargo.toml Cargo.lock CHANGELOG.md"
SITE_FILES=""
for f in site/index.html site/docs.html; do
  [ ! -f "$f" ] || SITE_FILES="$SITE_FILES $f"
done
VERSION_FILES="$VERSION_FILES$SITE_FILES"

TESTING=0
[ "$DRY_RUN" = 1 ] && [ "${RELEASE_ALLOW_DIRTY:-0}" = 1 ] && TESTING=1
if [ "$TESTING" = 1 ]; then
  say "RELEASE_ALLOW_DIRTY=1: skipping the branch, clean-tree and origin checks"
  # shellcheck disable=SC2086 # word-split on purpose
  [ -z "$(git status --porcelain -- $VERSION_FILES)" ] || die "the version files have uncommitted changes."
else
  [ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || die "not on main."
  [ -z "$(git status --porcelain)" ] || die "working tree is not clean; commit or stash first."
  say "Fetching origin"
  git fetch --quiet origin main
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
    die "main is not in sync with origin/main; pull or push first."
fi
# Publish only a commit CI passed (it also ran the Linux job).
if [ "$DRY_RUN" = 0 ]; then
  ci=$(gh run list --repo "$REPO" --workflow ci.yml --commit "$(git rev-parse HEAD)" \
    --json status,conclusion --jq '.[0] | "\(.status) \(.conclusion)"' 2>/dev/null || true)
  case "$ci" in
    "completed success") ;;
    "") die "CI hasn't run on $(git rev-parse --short HEAD) yet; push main and wait for it." ;;
    completed*) die "CI failed on $(git rev-parse --short HEAD) ($ci); fix it first." ;;
    *) die "CI is still running on $(git rev-parse --short HEAD); wait for it to pass." ;;
  esac
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  die "tag $TAG already exists locally."
fi
if [ "$TESTING" = 0 ] && git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
  die "tag $TAG already exists on origin."
fi

# Until the release commit exists, put the version files back on any exit.
RESTORE=1
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/sessionsnap-release.XXXXXX")
cleanup() {
  rm -rf "$STAGE"
  # From HEAD, index included: a failed commit leaves the bump staged.
  # shellcheck disable=SC2086
  [ "$RESTORE" = 0 ] || git checkout --quiet HEAD -- $VERSION_FILES
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# --- version ---------------------------------------------------------------
# subst FILE SED_EXPR: edit FILE in place (portable across BSD/GNU sed).
subst() {
  sed "$2" "$1" >"$1.tmp" && mv "$1.tmp" "$1"
}
# Releasing the version already in Cargo.toml (e.g. the first release) skips the bump.
if [ "$CURRENT" = "$VERSION" ]; then
  say "Version files already at $VERSION"
else
  say "Setting version $CURRENT -> $VERSION"
  OLD_RE=$(printf '%s' "$CURRENT" | sed 's/\./\\./g')
  subst Cargo.toml "1,/^version = /s/^version = \"$OLD_RE\"/version = \"$VERSION\"/"
  for f in $SITE_FILES; do
    subst "$f" "s#<span class=\"version\" translate=\"no\">v[0-9A-Za-z.+-]*</span>#<span class=\"version\" translate=\"no\">$TAG</span>#"
    grep -qF "<span class=\"version\" translate=\"no\">$TAG</span>" "$f" || die "$f: nav version badge not updated to $TAG."
  done
  if [ -f site/index.html ]; then
    subst site/index.html "s/^\( *\"version\": \"\)[0-9A-Za-z.+-]*\(\",\)\$/\1$VERSION\2/"
    grep -qF "\"version\": \"$VERSION\"," site/index.html || die "site/index.html: JSON-LD version not updated."
  fi
  cargo update --quiet --offline --workspace
fi
[ "$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)" = "$VERSION" ] ||
  die "Cargo.toml did not take the new version."
# The changelog describes every release: its "Unreleased" section for this version gets today's date.
grep -q "^## \[$VERSION\] - " CHANGELOG.md || die "CHANGELOG.md has no section for $VERSION; write one first."
VERSION_RE=$(printf '%s' "$VERSION" | sed 's/\./\\./g')
subst CHANGELOG.md "s/^## \[$VERSION_RE\] - Unreleased\$/## [$VERSION] - $(date +%Y-%m-%d)/"

# --- checks (the same script CI runs) ----------------------------------------
scripts/check.sh

rm -rf "$DIST"
mkdir -p "$DIST"
# No AppleDouble (._*) files in the archives.
export COPYFILE_DISABLE=1

# package TARGET BINARY: dist/sessionsnap-VERSION-TARGET.tar.gz (+ .sha256), with the binary
# at the archive's top level.
ASSETS=""
package() {
  name="sessionsnap-$VERSION-$1"
  rm -rf "$STAGE/pkg"
  mkdir -p "$STAGE/pkg"
  cp "$2" "$STAGE/pkg/sessionsnap"
  chmod 755 "$STAGE/pkg/sessionsnap"
  tar -C "$STAGE/pkg" -czf "$DIST/$name.tar.gz" sessionsnap
  (cd "$DIST" && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")
  ASSETS="$ASSETS dist/$name.tar.gz dist/$name.tar.gz.sha256"
  say "Packaged $name.tar.gz"
}

# --- macOS: one universal binary ---------------------------------------------
slices=""
for t in $MAC_TARGETS; do
  say "Building $t"
  cargo build --release --locked --target "$t"
  slices="$slices target/$t/release/sessionsnap"
done
# shellcheck disable=SC2086
lipo -create -output "$DIST/sessionsnap" $slices
# arm64 macOS refuses unsigned binaries; re-sign the merged file ad hoc.
codesign --force --sign - "$DIST/sessionsnap"
codesign --verify "$DIST/sessionsnap" || die "codesign verification failed for dist/sessionsnap"
lipo -info "$DIST/sessionsnap"
for a in arm64 x86_64; do
  lipo "$DIST/sessionsnap" -verify_arch "$a" || die "dist/sessionsnap has no $a slice."
done
GOT=$("$DIST/sessionsnap" --version)
[ "$GOT" = "sessionsnap $VERSION" ] || die "built binary reports '$GOT', expected 'sessionsnap $VERSION'."
package universal-apple-darwin "$DIST/sessionsnap"

# --- Linux: static musl builds in Docker -------------------------------------
# Docker on a Mac can't run the other architecture's containers without emulation, so one
# container of the native architecture builds both; musl ships its own libc, so rust-lld links
# the other target statically (only that one: build scripts and proc macros link natively).
NATIVE=$(docker version --format '{{.Server.Arch}}')
case "$NATIVE" in
  arm64 | aarch64) NATIVE=arm64 NATIVE_TARGET=aarch64-unknown-linux-musl CROSS_LINKER=CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER ;;
  amd64 | x86_64) NATIVE=amd64 NATIVE_TARGET=x86_64-unknown-linux-musl CROSS_LINKER=CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER ;;
  *) die "unexpected Docker architecture: $NATIVE" ;;
esac
say "Building $LINUX_TARGETS in Docker ($RUST_IMAGE, linux/$NATIVE)"
# Its own target dir, so the container never mixes artifacts with the Mac's builds.
docker run --rm --platform "linux/$NATIVE" \
  -v "$ROOT:/src" -w /src \
  -v "sessionsnap-cargo-registry:/usr/local/cargo/registry" \
  -e CARGO_TARGET_DIR=/src/target/docker \
  -e "$CROSS_LINKER=rust-lld" \
  "$RUST_IMAGE" \
  sh -c "apk add --no-cache musl-dev >/dev/null && rustup target add $LINUX_TARGETS >/dev/null &&
    cargo build --release --locked $(printf -- '--target %s ' $LINUX_TARGETS)"
for t in $LINUX_TARGETS; do
  bin="target/docker/$t/release/sessionsnap"
  [ -f "$bin" ] || die "the $t build left no binary at $bin."
  case "$t" in
    x86_64-*) want="x86-64" ;;
    aarch64-*) want="aarch64" ;;
  esac
  kind=$(file -b "$bin")
  case "$kind" in
    *ELF*"$want"*static*) echo "    $t: $kind" ;;
    *) die "$bin is not a static $want ELF binary: $kind" ;;
  esac
  if [ "$t" = "$NATIVE_TARGET" ]; then
    GOT=$(docker run --rm --platform "linux/$NATIVE" -v "$ROOT/$bin:/sessionsnap:ro" "$RUST_IMAGE" /sessionsnap --version)
    [ "$GOT" = "sessionsnap $VERSION" ] || die "$t binary reports '$GOT', expected 'sessionsnap $VERSION'."
  fi
  package "$t" "$bin"
done

# --- release notes ----------------------------------------------------------
# The repo commits straight to main, so the notes come from the Conventional Commit subjects
# since the previous tag (gh's --generate-notes lists merged PRs).
say "Writing dist/NOTES.md"
PREV=$(git describe --tags --abbrev=0 2>/dev/null || true)
RANGE=${PREV:+$PREV..}HEAD
section() { # section TITLE REGEX: one bullet per matching subject, prefix stripped
  # A breaking change (`type!:` or `type(scope)!:`) keeps a visible marker.
  lines=$(git log --no-merges --format=%s "$RANGE" | grep -E "$2" | grep -v '^chore: release' |
    sed -E -e 's/^[a-z]+(\([^)]*\))?!: /- **Breaking:** /' -e 's/^[a-z]+(\([^)]*\))?: /- /') || true
  [ -z "$lines" ] || printf '### %s\n\n%s\n\n' "$1" "$lines"
}
INSTALL='curl -fsSL https://sessionsnap.pawanpaudel.com.np/install.sh | sh'
{
  if [ -z "$PREV" ]; then
    # The first release: its history is the pre-release work, so a short summary instead.
    printf 'First release.\n\n'
    printf 'Save and resume your coding-agent sessions in cmux and tmux. sessionsnap saves named, versioned snapshots of your cmux workspaces or tmux sessions and the coding-agent sessions in their tabs (Claude Code, Codex and more), and reopens only what is missing, each agent tab resumed in its folder with its saved flags. See CHANGELOG.md for everything in this release.\n\n'
    printf 'Install (macOS and Linux):\n\n```sh\n%s\n```\n' "$INSTALL"
  else
    printf 'Install or upgrade (macOS and Linux):\n\n```sh\n%s\n```\n\n' "$INSTALL"
    printf 'Already installed? Run `sessionsnap update`.\n\n'
    section "Features" '^feat(\(|!|:)'
    section "Fixes" '^fix(\(|!|:)'
    section "Performance" '^perf(\(|!|:)'
    section "Docs" '^docs(\(|!|:)'
    section "Other" '^(refactor|build|ci|test|chore|style)(\(|!|:)'
    printf '**Full changelog**: https://github.com/%s/compare/%s...%s\n' "$REPO" "$PREV" "$TAG"
  fi
} >"$DIST/NOTES.md"

# --- publish ---------------------------------------------------------------
cat <<EOF

Ready to publish $TAG:
  commit  "chore: release $TAG"  ($VERSION_FILES; skipped if unchanged)
  tag     $TAG (annotated)
  push    git push origin main && git push origin $TAG
  release gh release create $TAG --repo $REPO --notes-file dist/NOTES.md
         $ASSETS
EOF

if [ "$DRY_RUN" = 1 ]; then
  echo
  # shellcheck disable=SC2086
  git --no-pager diff --stat -- $VERSION_FILES
  say "Dry run: nothing committed, tagged, pushed or released; version edits reverted. Assets are in dist/."
  exit 0
fi

if [ "$YES" != 1 ]; then
  printf '\nPublish %s? [y/N] ' "$TAG"
  read -r answer || answer=""
  case "$answer" in
    y | Y | yes | YES) ;;
    *) die "aborted; version edits reverted." ;;
  esac
fi

say "Committing and tagging $TAG"
# shellcheck disable=SC2086
git add -- $VERSION_FILES
git diff --cached --quiet || git commit --quiet -m "chore: release $TAG"
RESTORE=0
git tag -a "$TAG" -m "$TAG"

say "Pushing main and $TAG"
git push origin main || die "push of main failed; the commit and tag $TAG are local only."
git push origin "$TAG" || die "push of $TAG failed; retry: git push origin $TAG"

say "Creating GitHub Release $TAG"
# A pre-release (X.Y.Z-rc1) must not become `latest`, which install.sh and `sessionsnap update` take.
PRE=
case "$VERSION" in *-*) PRE=--prerelease ;; esac
# shellcheck disable=SC2086
gh release create "$TAG" --repo "$REPO" --verify-tag --title "$TAG" $PRE --notes-file "$DIST/NOTES.md" $ASSETS ||
  die "release failed; retry: gh release create $TAG --repo $REPO --verify-tag --title $TAG $PRE --notes-file dist/NOTES.md $ASSETS"

echo
say "Released $TAG: https://github.com/$REPO/releases/tag/$TAG"
