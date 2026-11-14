#!/usr/bin/env bash
# Install an Incan 0.6 release: the `incan` program (which is also `oven`), the language server and the toolchain.
#
# A release is one directory, read from a URL or from disk, that holds for each host target:
#
#   SHA256SUMS                    one `<sha256>  <name>` line for each file below
#   incan-<target>                Oven, the program that answers to `incan` and `oven`
#   incan-lsp-<target>            the language server
#   toolchain-<target>.tar.zst    the toolchain archive `incan toolchain install` reads
#
# The installer downloads the three files for this host, checks each against `SHA256SUMS`, and runs the downloaded
# Oven's `toolchain install` on the archive. That command checks the local Rust prerequisite, verifies the toolchain,
# and puts `incan` and `oven` in `INCAN_HOME/bin`; the installer adds `incan-lsp` beside them.
set -euo pipefail

releases="https://github.com/encero-systems/incan/releases"
release="${INCAN_RELEASE:-$releases/latest/download}"
incan_home="${INCAN_HOME:-$HOME/.incan}"
target=""
dry_run="false"

usage() {
  cat <<'USAGE'
Install the Incan toolchain from a release.

Usage:
  install-incan.sh [options]

Options:
  --version <TAG>       Install the release with this tag instead of the latest one
  --release <URL|DIR>   Read the release from this URL or directory (default: $INCAN_RELEASE or the latest release)
  --incan-home <PATH>   Install into this home (default: $INCAN_HOME or ~/.incan)
  --target <TRIPLE>     Install this host target's files instead of the detected one
  --dry-run             Print what would be downloaded and where it would go, and write nothing
  -h, --help            Show this help
USAGE
}

fail() {
  printf 'install-incan: %s\n' "$*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--release|--incan-home|--target)
      [ "$#" -ge 2 ] || fail "$1 requires a value"
      case "$1" in
        --version) release="$releases/download/$2" ;;
        --release) release="${2%/}" ;;
        --incan-home) incan_home="$2" ;;
        --target) target="$2" ;;
      esac
      shift 2
      ;;
    --dry-run)
      dry_run="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Darwin:arm64|Darwin:aarch64) printf 'aarch64-apple-darwin\n' ;;
    Linux:x86_64|Linux:amd64) printf 'x86_64-unknown-linux-gnu\n' ;;
    *) fail "no Incan release is built for this host: ${os} ${arch}" ;;
  esac
}

# Copy one release file to `out`, from a URL or a directory.
fetch() {
  local name="$1" out="$2"
  case "$release" in
    http://*|https://*|file://*)
      command -v curl >/dev/null 2>&1 || fail "required command not found: curl"
      curl -fsSL "$release/$name" -o "$out" || fail "cannot download $release/$name"
      ;;
    *)
      cp "$release/$name" "$out" 2>/dev/null || fail "cannot read $release/$name"
      ;;
  esac
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "required command not found: sha256sum or shasum"
  fi
}

# The release-signature hook.
#
# `SHA256SUMS` is the one file every other download is checked against, so a signed release signs that file, and this
# is the one place the installer verifies it: before any checksum in it is believed. It receives the downloaded
# `SHA256SUMS` and must refuse (`fail`) a release whose signature does not verify against the release trust root.
# This installer checks checksums only, and says so.
verify_release_signature() {
  local sums="$1"
  [ -s "$sums" ] || fail "the release's SHA256SUMS is empty"
  printf 'release signature: not verified (this installer checks SHA-256 checksums only)\n'
}

# Download one release file and refuse it unless `SHA256SUMS` lists it with the digest it has.
fetch_verified() {
  local name="$1" out="$2" expected actual
  expected="$(awk -v name="$name" '$2 == name || $2 == "*" name { print $1 }' "$sums")"
  [ -n "$expected" ] || fail "SHA256SUMS does not list $name"
  printf 'downloading %s\n' "$name"
  fetch "$name" "$out"
  actual="$(sha256_file "$out")"
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $name: SHA256SUMS records $expected, the download is $actual"
}

# Put the earlier programs back after a refused installation.
restore_earlier() {
  for name in incan incan-lsp; do
    if [ -e "$bin_dir/.$name.earlier" ]; then
      rm -f "$bin_dir/$name"
      mv "$bin_dir/.$name.earlier" "$bin_dir/$name"
    fi
  done
}

# Say how a new shell finds the installed commands and their home.
path_advice() {
  local profile="your shell's startup file"
  case "${SHELL:-}" in
    */zsh) profile="~/.zshrc" ;;
    */bash) profile="~/.bashrc" ;;
  esac
  case ":${PATH}:" in
    *":${bin_dir}:"*) ;;
    *)
      printf '%s is not on PATH. Add this line to %s:\n' "$bin_dir" "$profile"
      printf '  export PATH="%s:$PATH"\n' "$bin_dir"
      ;;
  esac
  if [ "$incan_home" != "$HOME/.incan" ]; then
    printf 'This home is not the default ~/.incan. Add this line as well:\n'
    printf '  export INCAN_HOME="%s"\n' "$incan_home"
  fi
}

[ -n "$target" ] || target="$(detect_target)"
bin_dir="$incan_home/bin"
oven_name="incan-$target"
lsp_name="incan-lsp-$target"
archive_name="toolchain-$target.tar.zst"

printf 'Incan release %s\n' "$release"
printf '  target:     %s\n' "$target"
printf '  incan home: %s\n' "$incan_home"
if [ "$dry_run" = "true" ]; then
  for name in SHA256SUMS "$oven_name" "$lsp_name" "$archive_name"; do
    printf '  would read: %s/%s\n' "$release" "$name"
  done
  printf 'Dry run only; no files were written.\n'
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
sums="$work/SHA256SUMS"
fetch SHA256SUMS "$sums"
verify_release_signature "$sums"
fetch_verified "$oven_name" "$work/incan"
fetch_verified "$lsp_name" "$work/incan-lsp"
fetch_verified "$archive_name" "$work/$archive_name"
chmod 755 "$work/incan" "$work/incan-lsp"

# An Oven never replaces another Oven's entry point, so an earlier one is set aside until this one is installed.
mkdir -p "$bin_dir"
for name in incan incan-lsp; do
  if [ -e "$bin_dir/$name" ] || [ -L "$bin_dir/$name" ]; then
    mv "$bin_dir/$name" "$bin_dir/.$name.earlier"
  fi
done
if ! INCAN_HOME="$incan_home" "$work/incan" toolchain install "$work/$archive_name"; then
  restore_earlier
  fail "the toolchain was not installed"
fi
cp "$work/incan-lsp" "$bin_dir/incan-lsp"
rm -f "$bin_dir/.incan.earlier" "$bin_dir/.incan-lsp.earlier"

printf 'incan, oven and incan-lsp are in %s\n' "$bin_dir"
path_advice
