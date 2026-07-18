#!/usr/bin/env bash
set -euo pipefail

default_manifest_url="https://github.com/encero-systems/incan/releases/latest/download/manifest.json"
manifest_ref="${INCAN_TOOLCHAIN_MANIFEST:-$default_manifest_url}"
incan_home="${INCAN_HOME:-$HOME/.incan}"
bin_dir="${INCAN_BIN_DIR:-$HOME/.local/bin}"
skip_rust_install="${INCAN_SKIP_RUST_INSTALL:-false}"
rustup_init_ref="${INCAN_RUSTUP_INIT:-https://sh.rustup.rs}"
target_override=""
archive_override=""
dry_run="false"

usage() {
  cat <<'USAGE'
Install the Incan toolchain from a versioned release manifest.

Usage:
  install-incan.sh [options]

Options:
  --manifest <URL|PATH>   Release manifest to use (default: GitHub Releases latest manifest)
  --target <TRIPLE>       Host target override for tests or cross-install staging
  --archive <PATH>        Use an already-downloaded archive while still verifying the manifest checksum
  --incan-home <PATH>     toolchain install root (default: $INCAN_HOME or ~/.incan)
  --bin-dir <PATH>        Directory where command symlinks are created (default: $INCAN_BIN_DIR or ~/.local/bin)
  --skip-rust             Do not install or update the Rust backend toolchain
  --dry-run               Resolve manifest and target without downloading, extracting, or writing files
  -h, --help              Show this help
USAGE
}

fail() {
  printf 'install-incan: %s\n' "$*" >&2
  exit 1
}

truthy() {
  case "${1:-}" in
    1|true|TRUE|yes|YES|on|ON) return 0 ;;
    *) return 1 ;;
  esac
}

has_command() {
  command -v "$1" >/dev/null 2>&1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --manifest)
      [ "$#" -ge 2 ] || fail "--manifest requires a value"
      manifest_ref="$2"
      shift 2
      ;;
    --target)
      [ "$#" -ge 2 ] || fail "--target requires a value"
      target_override="$2"
      shift 2
      ;;
    --archive)
      [ "$#" -ge 2 ] || fail "--archive requires a value"
      archive_override="$2"
      shift 2
      ;;
    --incan-home)
      [ "$#" -ge 2 ] || fail "--incan-home requires a value"
      incan_home="$2"
      shift 2
      ;;
    --bin-dir)
      [ "$#" -ge 2 ] || fail "--bin-dir requires a value"
      bin_dir="$2"
      shift 2
      ;;
    --skip-rust)
      skip_rust_install="true"
      shift
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

require_command() {
  has_command "$1" || fail "required command not found: $1"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Darwin:arm64|Darwin:aarch64) printf '%s\n' "aarch64-apple-darwin" ;;
    Darwin:x86_64) printf '%s\n' "x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) printf '%s\n' "x86_64-unknown-linux-gnu" ;;
    Linux:arm64|Linux:aarch64) fail "Linux arm64 toolchain archives are not shipped in 0.4; use a source build or x86_64 Linux host for now" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT:*) fail "native Windows is not supported by the 0.4 toolchain installer; use WSL2 for now" ;;
    *) fail "unsupported host: ${os} ${arch}" ;;
  esac
}

copy_or_download() {
  local ref="$1"
  local out="$2"
  case "$ref" in
    http://*|https://*)
      require_command curl
      curl -fL --progress-bar "$ref" -o "$out"
      ;;
    file://*)
      cp "${ref#file://}" "$out"
      ;;
    *)
      cp "$ref" "$out"
      ;;
  esac
}

json_top_value() {
  local file="$1"
  local field="$2"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg field "$field" '.[$field] // empty' "$file"
  else
    require_command python3
    python3 - "$file" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
value = payload.get(sys.argv[2], "")
if isinstance(value, (dict, list)):
    print(json.dumps(value, separators=(",", ":")))
else:
    print(value)
PY
  fi
}

json_host_value() {
  local file="$1"
  local target="$2"
  local field="$3"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg target "$target" --arg field "$field" '.hosts[$target][$field] // empty' "$file"
  else
    require_command python3
    python3 - "$file" "$target" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
value = payload.get("hosts", {}).get(sys.argv[2], {}).get(sys.argv[3], "")
if isinstance(value, (dict, list)):
    print(json.dumps(value, separators=(",", ":")))
else:
    print(value)
PY
  fi
}

json_command_path() {
  local file="$1"
  local target="$2"
  local command_name="$3"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg target "$target" --arg command "$command_name" '.hosts[$target].commands[$command] // empty' "$file"
  else
    require_command python3
    python3 - "$file" "$target" "$command_name" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
print(payload.get("hosts", {}).get(sys.argv[2], {}).get("commands", {}).get(sys.argv[3], ""))
PY
  fi
}

json_commands() {
  local file="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r '.commands[]?' "$file"
  else
    require_command python3
    python3 - "$file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
for command_name in payload.get("commands", []):
    print(command_name)
PY
  fi
}

json_hosts() {
  local file="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r '.hosts | keys[]?' "$file"
  else
    require_command python3
    python3 - "$file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
for host in payload.get("hosts", {}):
    print(host)
PY
  fi
}

json_rust_toolchain_value() {
  local file="$1"
  local field="$2"
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg field "$field" '.rust_toolchain[$field] // empty' "$file"
  else
    require_command python3
    python3 - "$file" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
value = payload.get("rust_toolchain", {}).get(sys.argv[2], "")
if isinstance(value, (dict, list)):
    print(json.dumps(value, separators=(",", ":")))
else:
    print(value)
PY
  fi
}

json_rust_targets() {
  local file="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r '.rust_toolchain.targets[]?' "$file"
  else
    require_command python3
    python3 - "$file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
for target in payload.get("rust_toolchain", {}).get("targets", []):
    print(target)
PY
  fi
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    fail "required command not found: sha256sum or shasum"
  fi
}

install_rustup() {
  local channel="$1"
  printf 'Installing Rust backend with rustup (%s)...\n' "$channel"
  case "$rustup_init_ref" in
    http://*|https://*)
      require_command curl
      curl --proto '=https' --tlsv1.2 -sSf "$rustup_init_ref" | sh -s -- -y --profile minimal --default-toolchain "$channel"
      ;;
    file://*)
      sh "${rustup_init_ref#file://}" -y --profile minimal --default-toolchain "$channel"
      ;;
    *)
      sh "$rustup_init_ref" -y --profile minimal --default-toolchain "$channel"
      ;;
  esac
  export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
}

ensure_rust_backend() {
  local manifest_file="$1"
  local channel
  channel="$(json_rust_toolchain_value "$manifest_file" "channel")"
  [ -n "$channel" ] || channel="stable"

  if truthy "$skip_rust_install"; then
    printf 'Rust backend provisioning skipped.\n'
    return 0
  fi

  if ! has_command rustup; then
    install_rustup "$channel"
  fi
  has_command rustup || fail "rustup was not available after Rust backend installation"

  if ! has_command cargo || ! has_command rustc; then
    printf 'Installing Rust toolchain %s...\n' "$channel"
    rustup toolchain install "$channel" --profile minimal
    rustup default "$channel"
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
  fi
  has_command cargo || fail "cargo was not available after Rust backend installation"
  has_command rustc || fail "rustc was not available after Rust backend installation"

  printf 'Rust backend:\n'
  printf '  rustc: %s\n' "$(rustc --version)"
  printf '  cargo: %s\n' "$(cargo --version)"
  while IFS= read -r rust_target; do
    [ -n "$rust_target" ] || continue
    printf '  target: %s\n' "$rust_target"
    rustup target add "$rust_target"
  done <<RUST_TARGETS
$(json_rust_targets "$manifest_file")
RUST_TARGETS
}

target="${target_override:-$(detect_target)}"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
manifest_file="${tmp_dir}/manifest.json"
copy_or_download "$manifest_ref" "$manifest_file"

schema_version="$(json_top_value "$manifest_file" "schema_version")"
[ "$schema_version" = "1" ] || fail "unsupported toolchain manifest schema_version: ${schema_version:-missing}"

toolchain_version="$(json_top_value "$manifest_file" "toolchain_version")"
[ -n "$toolchain_version" ] || fail "manifest is missing toolchain_version"

archive_url="$(json_host_value "$manifest_file" "$target" "archive_url")"
archive_sha256="$(json_host_value "$manifest_file" "$target" "archive_sha256")"
archive_format="$(json_host_value "$manifest_file" "$target" "archive_format")"
if [ -z "$archive_url" ] || [ -z "$archive_sha256" ]; then
  printf 'Available targets:\n' >&2
  json_hosts "$manifest_file" >&2
  fail "manifest does not contain an archive for target ${target}"
fi
[ "${archive_format:-tar.gz}" = "tar.gz" ] || fail "unsupported archive format for ${target}: ${archive_format}"

printf 'Incan toolchain %s\n' "$toolchain_version"
printf '  target:     %s\n' "$target"
printf '  archive:    %s\n' "$archive_url"
printf '  incan home: %s\n' "$incan_home"
printf '  bin dir:    %s\n' "$bin_dir"

if [ "$dry_run" = "true" ]; then
  printf 'Dry run only; no files were written.\n'
  exit 0
fi

ensure_rust_backend "$manifest_file"
require_command tar
archive_file="${archive_override:-${tmp_dir}/toolchain.tar.gz}"
if [ -n "$archive_override" ]; then
  [ -f "$archive_override" ] || fail "archive override does not exist: $archive_override"
else
  printf 'Downloading toolchain archive...\n'
  copy_or_download "$archive_url" "$archive_file"
fi

printf 'Verifying toolchain archive checksum...\n'
actual_sha256="$(sha256_file "$archive_file")"
[ "$actual_sha256" = "$archive_sha256" ] || fail "checksum mismatch for ${archive_file}: expected ${archive_sha256}, got ${actual_sha256}"

toolchain_dir="${incan_home}/toolchains/${toolchain_version}"
if [ -e "$toolchain_dir" ]; then
  fail "toolchain directory already exists: ${toolchain_dir}"
fi
extract_dir="${tmp_dir}/toolchain"
mkdir -p "$extract_dir"
printf 'Extracting toolchain archive...\n'
tar -xzf "$archive_file" -C "$extract_dir"

while IFS= read -r command_name; do
  [ -n "$command_name" ] || continue
  command_path="$(json_command_path "$manifest_file" "$target" "$command_name")"
  [ -n "$command_path" ] || command_path="bin/${command_name}"
  source_path="${extract_dir}/${command_path}"
  [ -f "$source_path" ] || fail "archive did not contain ${command_name} at ${command_path}"
done <<COMMANDS
$(json_commands "$manifest_file")
COMMANDS

[ -f "${extract_dir}/share/incan/sdk/sdk-inventory.json" ] \
  || fail "archive did not contain share/incan/sdk/sdk-inventory.json"
for support_crate in incan_core incan_derive incan_stdlib incan_vocab incan_web_macros; do
  [ -f "${extract_dir}/crates/${support_crate}/Cargo.toml" ] || fail "archive did not contain crates/${support_crate}/Cargo.toml"
done
[ -f "${extract_dir}/crates/Cargo.toml" ] || fail "archive did not contain crates/Cargo.toml"

mkdir -p "$(dirname "$toolchain_dir")" "$bin_dir"
mv "$extract_dir" "$toolchain_dir"

while IFS= read -r command_name; do
  [ -n "$command_name" ] || continue
  command_path="$(json_command_path "$manifest_file" "$target" "$command_name")"
  [ -n "$command_path" ] || command_path="bin/${command_name}"
  source_path="${toolchain_dir}/${command_path}"
  chmod +x "$source_path"
  link_path="${bin_dir}/${command_name}"
  if [ -e "$link_path" ] && [ ! -L "$link_path" ]; then
    fail "refusing to replace non-symlink command path: ${link_path}"
  fi
  ln -sfn "$source_path" "$link_path"
  printf 'Linked %s -> %s\n' "$link_path" "$source_path"
done <<COMMANDS
$(json_commands "$manifest_file")
COMMANDS

current_link="${incan_home}/current"
if [ -e "$current_link" ] && [ ! -L "$current_link" ]; then
  fail "refusing to replace non-symlink current toolchain path: ${current_link}"
fi
ln -sfn "$toolchain_dir" "$current_link"

printf 'Installed Incan toolchain %s into %s\n' "$toolchain_version" "$toolchain_dir"
