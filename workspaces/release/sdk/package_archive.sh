#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Package the Incan SDK commands for one host target.

Usage:
  package_archive.sh <target> [--out-dir <dir>]

Environment:
  INCAN_BIN      Path to the built incan binary (default: target/release/incan)
  INCAN_LSP_BIN  Path to the built incan-lsp binary (default: target/release/incan-lsp)
  SDK_RELEASE    Release name override (default: tag name or v<workspace version>)
USAGE
}

fail() {
  printf 'package_archive: %s\n' "$*" >&2
  exit 1
}

if [ "$#" -lt 1 ]; then
  usage >&2
  exit 2
fi

target="$1"
shift
out_dir="."

[ -n "$target" ] || fail "target must not be empty"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --out-dir)
      [ "$#" -ge 2 ] || fail "--out-dir requires a value"
      out_dir="$2"
      shift 2
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

workspace_version() {
  awk '
    /^\[workspace.package\]/ { in_section=1; next }
    /^\[/ { in_section=0 }
    in_section && /^version = / {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
}

version="$(workspace_version)"
[ -n "$version" ] || fail "could not read workspace package version from Cargo.toml"

if [ -n "${SDK_RELEASE:-}" ]; then
  release="$SDK_RELEASE"
elif [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
  release="${GITHUB_REF_NAME}"
else
  release="v${version}"
fi

incan_bin="${INCAN_BIN:-target/release/incan}"
incan_lsp_bin="${INCAN_LSP_BIN:-target/release/incan-lsp}"
[ -x "$incan_bin" ] || fail "incan binary is not executable: $incan_bin"
[ -x "$incan_lsp_bin" ] || fail "incan-lsp binary is not executable: $incan_lsp_bin"

mkdir -p "$out_dir"
package_dir="$out_dir/dist/incan-${release}-${target}"
archive="$out_dir/incan-${release}-${target}.tar.gz"

rm -rf "$package_dir"
mkdir -p "$package_dir/bin"
cp "$incan_bin" "$package_dir/bin/incan"
cp "$incan_lsp_bin" "$package_dir/bin/incan-lsp"

tar -C "$package_dir" -czf "$archive" .
shasum -a 256 "$archive" | awk '{print $1}' > "${archive}.sha256"
printf '%s\n' "$version" > "$out_dir/sdk-version.txt"
printf '%s\n' "$release" > "$out_dir/sdk-release.txt"

printf 'Packaged %s\n' "$archive"
