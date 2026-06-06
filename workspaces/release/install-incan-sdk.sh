#!/usr/bin/env bash
set -euo pipefail

default_manifest_url="https://github.com/dannys-code-corner/incan/releases/latest/download/manifest.json"
manifest_ref="${INCAN_SDK_MANIFEST:-$default_manifest_url}"
incan_home="${INCAN_HOME:-$HOME/.incan}"
bin_dir="${INCAN_BIN_DIR:-$HOME/.local/bin}"
target_override=""
archive_override=""
dry_run="false"

usage() {
  cat <<'USAGE'
Install the Incan SDK from a versioned release manifest.

Usage:
  install-incan-sdk.sh [options]

Options:
  --manifest <URL|PATH>   Release manifest to use (default: GitHub Releases latest manifest)
  --target <TRIPLE>       Host target override for tests or cross-install staging
  --archive <PATH>        Use an already-downloaded archive while still verifying the manifest checksum
  --incan-home <PATH>     SDK install root (default: $INCAN_HOME or ~/.incan)
  --bin-dir <PATH>        Directory where command symlinks are created (default: $INCAN_BIN_DIR or ~/.local/bin)
  --dry-run               Resolve manifest and target without downloading, extracting, or writing files
  -h, --help              Show this help
USAGE
}

fail() {
  printf 'install-incan-sdk: %s\n' "$*" >&2
  exit 1
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
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Darwin:arm64|Darwin:aarch64) printf '%s\n' "aarch64-apple-darwin" ;;
    Darwin:x86_64) printf '%s\n' "x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) printf '%s\n' "x86_64-unknown-linux-gnu" ;;
    Linux:arm64|Linux:aarch64) fail "Linux arm64 SDK archives are not shipped in 0.4; use a source build or x86_64 Linux host for now" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT:*) fail "native Windows is not supported by the 0.4 SDK installer; use WSL2 for now" ;;
    *) fail "unsupported host: ${os} ${arch}" ;;
  esac
}

copy_or_download() {
  local ref="$1"
  local out="$2"
  case "$ref" in
    http://*|https://*)
      require_command curl
      curl -fsSL "$ref" -o "$out"
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

target="${target_override:-$(detect_target)}"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
manifest_file="${tmp_dir}/manifest.json"
copy_or_download "$manifest_ref" "$manifest_file"

schema_version="$(json_top_value "$manifest_file" "schema_version")"
[ "$schema_version" = "1" ] || fail "unsupported SDK manifest schema_version: ${schema_version:-missing}"

sdk_version="$(json_top_value "$manifest_file" "sdk_version")"
[ -n "$sdk_version" ] || fail "manifest is missing sdk_version"

archive_url="$(json_host_value "$manifest_file" "$target" "archive_url")"
archive_sha256="$(json_host_value "$manifest_file" "$target" "archive_sha256")"
archive_format="$(json_host_value "$manifest_file" "$target" "archive_format")"
if [ -z "$archive_url" ] || [ -z "$archive_sha256" ]; then
  printf 'Available targets:\n' >&2
  json_hosts "$manifest_file" >&2
  fail "manifest does not contain an archive for target ${target}"
fi
[ "${archive_format:-tar.gz}" = "tar.gz" ] || fail "unsupported archive format for ${target}: ${archive_format}"

printf 'Incan SDK %s\n' "$sdk_version"
printf '  target:     %s\n' "$target"
printf '  archive:    %s\n' "$archive_url"
printf '  incan home: %s\n' "$incan_home"
printf '  bin dir:    %s\n' "$bin_dir"

if [ "$dry_run" = "true" ]; then
  printf 'Dry run only; no files were written.\n'
  exit 0
fi

require_command tar
archive_file="${archive_override:-${tmp_dir}/sdk.tar.gz}"
if [ -n "$archive_override" ]; then
  [ -f "$archive_override" ] || fail "archive override does not exist: $archive_override"
else
  copy_or_download "$archive_url" "$archive_file"
fi

actual_sha256="$(sha256_file "$archive_file")"
[ "$actual_sha256" = "$archive_sha256" ] || fail "checksum mismatch for ${archive_file}: expected ${archive_sha256}, got ${actual_sha256}"

sdk_dir="${incan_home}/sdks/${sdk_version}"
if [ -e "$sdk_dir" ]; then
  fail "SDK directory already exists: ${sdk_dir}"
fi
extract_dir="${tmp_dir}/sdk"
mkdir -p "$extract_dir"
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

mkdir -p "$(dirname "$sdk_dir")" "$bin_dir"
mv "$extract_dir" "$sdk_dir"

while IFS= read -r command_name; do
  [ -n "$command_name" ] || continue
  command_path="$(json_command_path "$manifest_file" "$target" "$command_name")"
  [ -n "$command_path" ] || command_path="bin/${command_name}"
  source_path="${sdk_dir}/${command_path}"
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
  fail "refusing to replace non-symlink current SDK path: ${current_link}"
fi
ln -sfn "$sdk_dir" "$current_link"

printf 'Installed Incan SDK %s into %s\n' "$sdk_version" "$sdk_dir"
