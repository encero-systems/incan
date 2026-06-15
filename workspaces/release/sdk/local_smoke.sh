#!/usr/bin/env bash
set -euo pipefail

dist_dir="${SDK_DIST:-/private/tmp/incan-sdk-local-test}"
generated_at="${SDK_GENERATED_AT:-2026-06-06T00:00:00Z}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
incan_run_bin="${SDK_INCAN_BIN:-${root}/target/release/incan}"

usage() {
  cat <<'USAGE'
Smoke local SDK release assets.

Usage:
  local_smoke.sh <package|assets|direct|npm|pip|homebrew|all>

Environment:
  SDK_DIST          Output directory for local release assets (default: /private/tmp/incan-sdk-local-test)
  SDK_HOST_TARGET   Host target override; auto-detected when omitted
  SDK_GENERATED_AT  Deterministic manifest timestamp (default: 2026-06-06T00:00:00Z)
  SDK_INCAN_BIN      Incan binary used to run prepare_assets.incn (default: target/release/incan)
USAGE
}

fail() {
  printf 'sdk-local-smoke: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

detect_host_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Darwin:arm64|Darwin:aarch64) printf '%s\n' "aarch64-apple-darwin" ;;
    Darwin:x86_64) printf '%s\n' "x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) printf '%s\n' "x86_64-unknown-linux-gnu" ;;
    *) fail "unsupported local host: ${os} ${arch}" ;;
  esac
}

host_target="${SDK_HOST_TARGET:-$(detect_host_target)}"
[ -n "$host_target" ] || fail "SDK_HOST_TARGET must not be empty"

sdk_version() {
  local version_file="${dist_dir}/sdk-version.txt"
  [ -f "$version_file" ] || fail "missing SDK version file: ${version_file}; run make sdk-release-package first"
  sed -n '1p' "$version_file" | tr -d '\r\n'
}

sdk_release() {
  local release_file="${dist_dir}/sdk-release.txt"
  [ -f "$release_file" ] || fail "missing SDK release file: ${release_file}; run make sdk-release-package first"
  sed -n '1p' "$release_file" | tr -d '\r\n'
}

archive_path() {
  printf '%s/incan-%s-%s.tar.gz\n' "$dist_dir" "$(sdk_release)" "$host_target"
}

require_archive() {
  local archive
  archive="$(archive_path)"
  [ -f "$archive" ] || fail "missing host archive: ${archive}; run make sdk-release-package first"
  [ -f "${archive}.sha256" ] || fail "missing archive checksum: ${archive}.sha256"
}

require_incan_run_bin() {
  [ -x "$incan_run_bin" ] || fail "missing Incan runner: ${incan_run_bin}; run make sdk-release-build first or set SDK_INCAN_BIN"
}

package_sdk() {
  [ -x "${root}/target/release/incan" ] || fail "missing target/release/incan; run make sdk-release-build first"
  [ -x "${root}/target/release/incan-lsp" ] || fail "missing target/release/incan-lsp; run make sdk-release-build first"
  rm -rf "$dist_dir"
  mkdir -p "$dist_dir"
  printf 'Packaging SDK for %s into %s\n' "$host_target" "$dist_dir"
  "${root}/workspaces/release/sdk/package_archive.sh" "$host_target" --out-dir "$dist_dir"
}

write_assets() {
  require_archive
  require_incan_run_bin
  printf 'Writing SDK manifest/install assets in %s\n' "$dist_dir"
  INCAN_REPO_ROOT="$root" \
    INCAN_SDK_DIST_DIR="$dist_dir" \
    INCAN_SDK_SKIP_HOMEBREW=1 \
    INCAN_SDK_GENERATED_AT="$generated_at" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_GENERATED_CARGO_TARGET_DIR="${root}/target/incan_generated_shared_target" \
    "$incan_run_bin" run "${root}/workspaces/release/sdk/prepare_assets.incn"
}

smoke_direct() {
  require_archive
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make sdk-release-assets first"
  rm -rf "${dist_dir}/install-home" "${dist_dir}/install-bin"
  bash "${dist_dir}/install.sh" \
    --manifest "${dist_dir}/manifest.json" \
    --target "$host_target" \
    --archive "$(archive_path)" \
    --incan-home "${dist_dir}/install-home" \
    --bin-dir "${dist_dir}/install-bin"
  "${dist_dir}/install-bin/incan" --version
}

smoke_npm() {
  require_command node
  require_command npm
  require_archive
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make sdk-release-assets first"
  npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    node "${root}/workspaces/release/npm/prepare_package.js" "$dist_dir"
  local npm_home="${dist_dir}/npm-home"
  rm -rf "$npm_home"
  mkdir -p "$npm_home"
  INCAN_SDK_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_NPM_SDK_HOME="${npm_home}/sdk-home" \
    INCAN_NPM_BIN_DIR="${npm_home}/bin" \
    npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    npm install -g "${dist_dir}/incan-sdk-$(sdk_version).tgz" --prefix "$npm_home" --ignore-scripts
  INCAN_SDK_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_NPM_SDK_HOME="${npm_home}/sdk-home" \
    INCAN_NPM_BIN_DIR="${npm_home}/bin" \
    "${npm_home}/bin/install-incan-sdk" --archive "$(archive_path)" --target "$host_target"
  INCAN_SDK_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_NPM_SDK_HOME="${npm_home}/sdk-home" \
    INCAN_NPM_BIN_DIR="${npm_home}/bin" \
    "${npm_home}/bin/incan" --version
}

python_build_runner() {
  if python3 -m build --version >/dev/null 2>&1 && python3 -c 'import hatchling.build' >/dev/null 2>&1; then
    printf '%s\n' "python3"
    return
  fi

  local venv="${dist_dir}/_pip-build-venv"
  if [ ! -x "${venv}/bin/python" ]; then
    require_command python3
    python3 -m venv "$venv"
  fi
  if "${venv}/bin/python" -m build --version >/dev/null 2>&1 && "${venv}/bin/python" -c 'import hatchling' >/dev/null 2>&1; then
    printf '%s\n' "${venv}/bin/python"
    return
  fi
  PIP_CACHE_DIR="${dist_dir}/pip-cache" \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    "${venv}/bin/python" -m pip install build hatchling >&2
  printf '%s\n' "${venv}/bin/python"
}

smoke_pip() {
  require_command python3
  require_archive
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make sdk-release-assets first"
  local python
  python="$(python_build_runner)"
  "$python" "${root}/workspaces/release/pip/prepare_package.py" "$dist_dir"
  local venv="${dist_dir}/pip-venv"
  rm -rf "$venv" "${dist_dir}/pip-sdk-home" "${dist_dir}/pip-bin"
  python3 -m venv "$venv"
  PIP_CACHE_DIR="${dist_dir}/pip-cache" \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    "${venv}/bin/python" -m pip install "${dist_dir}/incan_sdk-$(sdk_version | sed -E 's/-dev\./.dev/; s/-(a|b|rc)([0-9]+)$/\1\2/')-py3-none-any.whl"
  INCAN_SDK_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_PIP_SDK_HOME="${dist_dir}/pip-sdk-home" \
    INCAN_PIP_BIN_DIR="${dist_dir}/pip-bin" \
    "${venv}/bin/install-incan-sdk" --archive "$(archive_path)" --target "$host_target"
  INCAN_SDK_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_PIP_SDK_HOME="${dist_dir}/pip-sdk-home" \
    INCAN_PIP_BIN_DIR="${dist_dir}/pip-bin" \
    "${venv}/bin/incan" --version
}

smoke_homebrew() {
  require_command ruby
  require_archive
  require_incan_run_bin
  local release archive checksum target
  release="$(sdk_release)"
  archive="$(archive_path)"
  checksum="${archive}.sha256"
  for target in x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
    if [ "$target" = "$host_target" ]; then
      continue
    fi
    cp "$archive" "${dist_dir}/incan-${release}-${target}.tar.gz"
    cp "$checksum" "${dist_dir}/incan-${release}-${target}.tar.gz.sha256"
  done
  INCAN_REPO_ROOT="$root" \
    INCAN_SDK_DIST_DIR="$dist_dir" \
    INCAN_SDK_GENERATED_AT="$generated_at" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_GENERATED_CARGO_TARGET_DIR="${root}/target/incan_generated_shared_target" \
    "$incan_run_bin" run "${root}/workspaces/release/sdk/prepare_assets.incn"
  ruby -c "${dist_dir}/incan.rb"
  if [ "${SDK_HOMEBREW_AUDIT:-0}" = "1" ]; then
    require_command brew
    mkdir -p "${dist_dir}/brew-cache" "${dist_dir}/brew-temp"
    HOMEBREW_CACHE="${dist_dir}/brew-cache" \
      HOMEBREW_TEMP="${dist_dir}/brew-temp" \
      HOMEBREW_NO_ANALYTICS=1 \
      HOMEBREW_NO_AUTO_UPDATE=1 \
      brew audit --strict --formula "${dist_dir}/incan.rb"
  else
    printf 'Skipped brew audit; set SDK_HOMEBREW_AUDIT=1 to run it.\n'
  fi
}

case "${1:-}" in
  package) package_sdk ;;
  assets) write_assets ;;
  direct) smoke_direct ;;
  npm) smoke_npm ;;
  pip) smoke_pip ;;
  homebrew) smoke_homebrew ;;
  all)
    package_sdk
    write_assets
    smoke_direct
    smoke_npm
    smoke_pip
    smoke_homebrew
    ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
