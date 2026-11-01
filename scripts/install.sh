#!/bin/sh
# install.sh - install a published xerj release from the unsigned-gg/xerj fork.
#
# Fork-only. It is NOT the hosted installer behind https://xerj.org/get, which
# installs upstream releases. This one installs releases of THIS fork, and it
# verifies what it downloads before it puts anything on PATH.
#
# What it does:
#   1. resolves the tag: the one you pass, or the newest vX.Y.Z release
#   2. downloads the archive for this host, SHA256SUMS-<target> and the manifest
#   3. checks the archive against SHA256SUMS-<target> AND the manifest
#      (sha256 and byte size of the archive, and the manifest's tag)
#   4. runs `gh attestation verify` when gh is installed; with no gh it prints a
#      warning that provenance was NOT verified and carries on
#   5. unpacks, and runs the new binary with --version: it must report the tag
#   6. installs atomically: the new file is staged beside the target and moved
#      into place; an existing binary is kept as xerj.bak
#
# It never builds from source and never edits PATH or shell profiles.
#
# Usage:
#   scripts/install.sh                 # newest vX.Y.Z release
#   scripts/install.sh v1.0.0-rc.101   # a specific tag
#   scripts/install.sh --dir DIR       # install into DIR (default ~/.local/bin)
#
# Environment (all optional):
#   XERJ_INSTALL_DIR     same as --dir
#   XERJ_TARGET          override the host target triple (the version check is
#                        then skipped, since the binary may not run here)
#   XERJ_REPO            default unsigned-gg/xerj
#   XERJ_DOWNLOAD_BASE   default https://github.com/$XERJ_REPO/releases/download
#   XERJ_RELEASES_URL    default https://api.github.com/repos/$XERJ_REPO/releases?per_page=100
#
# Needs: curl, tar (with gzip), sha256sum or shasum, and optionally gh (2.49 or newer).
# Linux and macOS only; on Windows use the PowerShell installer.
set -eu

REPO="${XERJ_REPO:-unsigned-gg/xerj}"
DOWNLOAD_BASE="${XERJ_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}"
RELEASES_URL="${XERJ_RELEASES_URL:-https://api.github.com/repos/$REPO/releases?per_page=100}"
DEST_DIR="${XERJ_INSTALL_DIR:-${HOME:-}/.local/bin}"
TAG=""
WORK=""

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
usage_error() { printf 'error: %s\n' "$*" >&2; exit 2; }

cleanup() {
  if [ -n "$WORK" ] && [ -d "$WORK" ]; then rm -rf "$WORK"; fi
  return 0
}
trap cleanup EXIT INT TERM

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    --dir)     [ $# -ge 2 ] || usage_error "--dir needs a value"; DEST_DIR="$2"; shift ;;
    -*)        usage_error "unknown flag: $1" ;;
    *)         [ -z "$TAG" ] || usage_error "more than one tag given"; TAG="$1" ;;
  esac
  shift
done

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar  >/dev/null 2>&1 || die "tar is required"
[ -n "$DEST_DIR" ] || die "no install directory: set HOME, XERJ_INSTALL_DIR or --dir"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum  >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "sha256sum or shasum is required"
  fi
}

# --- host target --------------------------------------------------------------
HOST_RUNS=1
if [ -n "${XERJ_TARGET:-}" ]; then
  TARGET="$XERJ_TARGET"
  HOST_RUNS=0
else
  os="$(uname -s)"; arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64)  arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) die "no release build for CPU '$arch'" ;;
  esac
  case "$os" in
    # musl: the static build runs on any libc.
    Linux)  TARGET="$arch-unknown-linux-musl" ;;
    Darwin) TARGET="$arch-apple-darwin" ;;
    *) die "this installer supports Linux and macOS, not '$os'" ;;
  esac
fi

# --- tag ----------------------------------------------------------------------
TAG_PATTERN='^v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*[0-9A-Za-z.+-]*$'
if [ -z "$TAG" ]; then
  # The release list also holds corpus-pack releases, so take the first entry
  # whose tag looks like a version. The API lists newest first.
  listing="$(curl -fsSL "$RELEASES_URL")" || die "could not list releases at $RELEASES_URL"
  TAG="$(printf '%s\n' "$listing" | tr ',' '\n' \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
    | grep -E "$TAG_PATTERN" | head -n 1 || true)"
  [ -n "$TAG" ] || die "no vX.Y.Z release found in $REPO"
fi
printf '%s\n' "$TAG" | grep -Eq "$TAG_PATTERN" || usage_error "'$TAG' is not a vX.Y.Z tag"
VERSION="${TAG#v}"

ARCHIVE="xerj-$VERSION-$TARGET.tar.gz"
SUMS="SHA256SUMS-$TARGET"
MANIFEST="xerj-$VERSION-manifest.json"
say "xerj $TAG for $TARGET from $REPO"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/xerj-install-XXXXXX")" || die "could not create a temp directory"

fetch() {
  curl -fsSL -o "$WORK/$1" "$DOWNLOAD_BASE/$TAG/$1" || die "$1: download failed from $DOWNLOAD_BASE/$TAG/$1"
}
fetch "$ARCHIVE"
fetch "$SUMS"
fetch "$MANIFEST"

# --- SHA256SUMS ---------------------------------------------------------------
actual="$(sha256_of "$WORK/$ARCHIVE")"
want="$(awk -v n="$ARCHIVE" '$2 == n { print $1 }' "$WORK/$SUMS" | head -n 1)"
[ -n "$want" ] || die "$SUMS has no line for $ARCHIVE"
[ "$want" = "$actual" ] || die "$ARCHIVE: sha256 $actual does not match $SUMS ($want)"
say "ok   $ARCHIVE matches $SUMS"

# --- manifest -----------------------------------------------------------------
# One artifact is one flat JSON object, so split the file after each closing
# brace and read the object that names this archive.
flat="$(tr -d '\n\r' < "$WORK/$MANIFEST")"
m_tag="$(printf '%s' "$flat" | sed -n 's/.*"tag": *"\([^"]*\)".*/\1/p')"
[ "$m_tag" = "$TAG" ] || die "$MANIFEST: manifest is for tag '$m_tag', not $TAG"
entry="$(printf '%s' "$flat" | sed 's/}/}\n/g' | grep "\"archive\": *\"$ARCHIVE\"" | head -n 1 || true)"
[ -n "$entry" ] || die "$MANIFEST: no entry for $ARCHIVE"
m_sha="$(printf '%s' "$entry" | sed -n 's/.*"sha256": *"\([0-9a-f]*\)".*/\1/p')"
m_size="$(printf '%s' "$entry" | sed -n 's/.*"size": *\([0-9][0-9]*\).*/\1/p')"
size="$(wc -c < "$WORK/$ARCHIVE" | tr -d ' ')"
[ "$m_sha" = "$actual" ] || die "$ARCHIVE: manifest sha256 '$m_sha' does not match the archive ($actual)"
[ "$m_size" = "$size" ] || die "$ARCHIVE: manifest size '$m_size' does not match the archive ($size bytes)"
say "ok   $ARCHIVE matches $MANIFEST (sha256 and size)"

# --- provenance ---------------------------------------------------------------
if command -v gh >/dev/null 2>&1; then
  if gh attestation verify "$WORK/$ARCHIVE" --repo "$REPO" >"$WORK/attest.log" 2>&1; then
    say "ok   $ARCHIVE has a provenance attestation from $REPO"
  else
    cat "$WORK/attest.log" >&2
    die "$ARCHIVE: attestation verification failed for $REPO (if gh is not logged in, run 'gh auth login')"
  fi
else
  warn "gh is not installed: provenance NOT verified. The checksums matched, but nothing proved this archive was built by $REPO. Install gh 2.49 or newer to verify."
fi

# --- unpack and check the binary ----------------------------------------------
mkdir "$WORK/unpack"
tar -xzf "$WORK/$ARCHIVE" -C "$WORK/unpack" || die "$ARCHIVE: did not extract"
NEW_BIN="$WORK/unpack/xerj-$VERSION-$TARGET/xerj"
[ -f "$NEW_BIN" ] && [ ! -L "$NEW_BIN" ] || die "$ARCHIVE: no xerj binary at xerj-$VERSION-$TARGET/xerj"
chmod 755 "$NEW_BIN"
if [ "$HOST_RUNS" = 1 ]; then
  reported="$("$NEW_BIN" --version 2>&1 || true)"
  case "$reported" in
    "xerj v$VERSION"*) say "ok   binary --version: $reported" ;;
    *) die "the binary's --version says '$reported', expected 'xerj v$VERSION'; nothing was installed" ;;
  esac
else
  warn "XERJ_TARGET is set: the binary was not run, so its --version was NOT checked"
fi

# --- atomic install -----------------------------------------------------------
mkdir -p "$DEST_DIR" || die "could not create $DEST_DIR"
DEST="$DEST_DIR/xerj"
STAGED="$DEST_DIR/.xerj.new.$$"
cp "$NEW_BIN" "$STAGED" || die "could not stage the binary in $DEST_DIR"
chmod 755 "$STAGED"
if [ -e "$DEST" ]; then
  KEEP="$DEST_DIR/.xerj.bak.$$"
  cp -p "$DEST" "$KEEP" || { rm -f "$STAGED"; die "could not back up $DEST"; }
  mv -f "$KEEP" "$DEST.bak"
  say "kept the previous binary as $DEST.bak"
fi
mv -f "$STAGED" "$DEST" || { rm -f "$STAGED"; die "could not move the binary into place"; }
say "installed $DEST ($TAG)"

case ":${PATH:-}:" in
  *":$DEST_DIR:"*) ;;
  *) say "note: $DEST_DIR is not on your PATH; this installer does not edit it" ;;
esac
