#!/usr/bin/env bash
set -euo pipefail

generated_at="${TOOLCHAIN_GENERATED_AT:-2026-06-06T00:00:00Z}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
dist_dir="${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}"
case "$dist_dir" in
  /*) ;;
  *) dist_dir="${root}/${dist_dir}" ;;
esac
incan_run_bin="${TOOLCHAIN_INCAN_BIN:-${root}/target/release/incan}"

usage() {
  cat <<'USAGE'
Smoke local toolchain release assets.

Usage:
  local_smoke.sh <package|assets|direct|npm|pip|homebrew|all>

Environment:
  TOOLCHAIN_DIST          Output directory for local release assets (default: /private/tmp/incan-local-test)
  TOOLCHAIN_HOST_TARGET   Host target override; auto-detected when omitted
  TOOLCHAIN_GENERATED_AT  Deterministic manifest timestamp (default: 2026-06-06T00:00:00Z)
  TOOLCHAIN_INCAN_BIN      Incan binary used to run prepare_assets.incn (default: target/release/incan)
USAGE
}

fail() {
  printf 'toolchain-local-smoke: %s\n' "$*" >&2
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

host_target="${TOOLCHAIN_HOST_TARGET:-$(detect_host_target)}"
[ -n "$host_target" ] || fail "TOOLCHAIN_HOST_TARGET must not be empty"

toolchain_version() {
  local version_file="${dist_dir}/toolchain-version.txt"
  [ -f "$version_file" ] || fail "missing toolchain version file: ${version_file}; run make toolchain-release-package first"
  sed -n '1p' "$version_file" | tr -d '\r\n'
}

toolchain_release() {
  local release_file="${dist_dir}/toolchain-release.txt"
  [ -f "$release_file" ] || fail "missing toolchain release file: ${release_file}; run make toolchain-release-package first"
  sed -n '1p' "$release_file" | tr -d '\r\n'
}

archive_path() {
  printf '%s/incan-%s-%s.tar.gz\n' "$dist_dir" "$(toolchain_release)" "$host_target"
}

npm_platform_package_path() {
  local platform
  case "$host_target" in
    x86_64-unknown-linux-gnu) platform="linux-x64" ;;
    x86_64-apple-darwin) platform="darwin-x64" ;;
    aarch64-apple-darwin) platform="darwin-arm64" ;;
    *) fail "unsupported npm platform smoke target: ${host_target}" ;;
  esac
  printf '%s/incan-toolchain-%s-%s.tgz\n' "$dist_dir" "$platform" "$(toolchain_version)"
}

require_archive() {
  local archive
  archive="$(archive_path)"
  [ -f "$archive" ] || fail "missing host archive: ${archive}; run make toolchain-release-package first"
  [ -f "${archive}.sha256" ] || fail "missing archive checksum: ${archive}.sha256"
}

require_incan_run_bin() {
  [ -x "$incan_run_bin" ] || fail "missing Incan runner: ${incan_run_bin}; run make toolchain-release-build first or set TOOLCHAIN_INCAN_BIN"
}

package_toolchain() {
  [ -x "${root}/target/release/incan" ] || fail "missing target/release/incan; run make toolchain-release-build first"
  [ -x "${root}/target/release/incan-lsp" ] || fail "missing target/release/incan-lsp; run make toolchain-release-build first"
  rm -rf "$dist_dir"
  mkdir -p "$dist_dir"
  printf 'Packaging toolchain for %s into %s\n' "$host_target" "$dist_dir"
  "${root}/workspaces/release/toolchain/package_archive.sh" "$host_target" --out-dir "$dist_dir"
}

write_assets() {
  require_archive
  require_incan_run_bin
  printf 'Writing toolchain manifest/install assets in %s\n' "$dist_dir"
  INCAN_REPO_ROOT="$root" \
    INCAN_TOOLCHAIN_DIST_DIR="$dist_dir" \
    INCAN_TOOLCHAIN_SKIP_HOMEBREW=1 \
    INCAN_TOOLCHAIN_GENERATED_AT="$generated_at" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_GENERATED_CARGO_TARGET_DIR="${root}/target/incan_generated_shared_target" \
    "$incan_run_bin" run "${root}/workspaces/release/toolchain/prepare_assets.incn"
}

smoke_direct() {
  require_archive
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make toolchain-release-assets first"
  rm -rf "${dist_dir}/install-home" "${dist_dir}/install-bin"
  bash "${dist_dir}/install.sh" \
    --manifest "${dist_dir}/manifest.json" \
    --target "$host_target" \
    --archive "$(archive_path)" \
    --incan-home "${dist_dir}/install-home" \
    --bin-dir "${dist_dir}/install-bin"
  "${dist_dir}/install-bin/incan" --version
  local installed_sdk_store
  installed_sdk_store="${dist_dir}/install-home/toolchains/$(toolchain_version)/share/incan/sdk"
  [ -d "$installed_sdk_store" ] || fail "installed toolchain is missing its compiled SDK provider seed"
  [ -f "$installed_sdk_store/sdk-inventory.json" ] || fail "installed toolchain is missing sdk-inventory.json"
  local component
  for component in \
    stdlib-core stdlib-system stdlib-codecs stdlib-compression stdlib-data \
    stdlib-async stdlib-observability stdlib-web stdlib-testing
  do
    [ -d "$installed_sdk_store/components/$component" ] \
      || fail "installed toolchain is missing SDK component $component"
  done
  [ ! -d "$installed_sdk_store/.cargo-target" ] \
    || fail "installed toolchain must not contain an SDK provider Cargo target"
  local sdk_payload_before
  sdk_payload_before="$(find "$installed_sdk_store" -type f -exec shasum -a 256 {} \; | sort)"
  # Exercise the user-facing symlink path, not the real toolchain binary path. Some hosts report the symlink path from
  # current_exe(), so stdlib/support-crate lookup must resolve the canonical target before walking toolchain ancestors.
  rm -rf "${dist_dir}/starter-smoke"
  mkdir -p "${dist_dir}/starter-smoke"
  (
    cd "${dist_dir}/starter-smoke"
    "${dist_dir}/install-bin/incan" new hello --yes
    cd hello
    "${dist_dir}/install-bin/incan" run
    "${dist_dir}/install-bin/incan" test
    "${dist_dir}/install-bin/incan" build --release
  )
  local sdk_payload_after
  sdk_payload_after="$(find "$installed_sdk_store" -type f -exec shasum -a 256 {} \; | sort)"
  [ "$sdk_payload_after" = "$sdk_payload_before" ] \
    || fail "installed compiler mutated or regenerated the shipped SDK provider seed"
  [ ! -d "$installed_sdk_store/.cargo-target" ] \
    || fail "installed compiler created a redundant SDK provider Cargo target"
}

# npm and Homebrew render metadata for every supported target, while a local smoke build produces only the current
# host binary. Reuse that host archive as a packaging-only fixture for missing foreign targets; the smoke never runs
# those foreign-labelled copies.
ensure_platform_archive_fixtures() {
  require_archive
  local release archive checksum target target_archive target_checksum
  release="$(toolchain_release)"
  archive="$(archive_path)"
  checksum="${archive}.sha256"
  for target in x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin; do
    if [ "$target" = "$host_target" ]; then
      continue
    fi
    target_archive="${dist_dir}/incan-${release}-${target}.tar.gz"
    target_checksum="${target_archive}.sha256"
    if [ -f "$target_archive" ] || [ -f "$target_checksum" ]; then
      [ -f "$target_archive" ] || fail "missing target archive while checksum exists: ${target_archive}"
      [ -f "$target_checksum" ] || fail "missing target archive checksum: ${target_checksum}"
      continue
    fi
    cp "$archive" "$target_archive"
    cp "$checksum" "$target_checksum"
  done
}

smoke_npm() {
  require_command node
  require_command npm
  ensure_platform_archive_fixtures
  npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    node "${root}/workspaces/release/npm/prepare_package.js" "$dist_dir"
  local npm_home="${dist_dir}/npm-home"
  local platform_package
  platform_package="$(npm_platform_package_path)"
  [ -f "$platform_package" ] || fail "missing npm platform package: ${platform_package}"
  rm -rf "$npm_home"
  mkdir -p "$npm_home"
  npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    npm_config_ignore_scripts=true \
    npm_config_audit=false \
    npm_config_fund=false \
    npm install -g --offline --omit=optional --ignore-scripts "$platform_package" "${dist_dir}/incan-toolchain-$(toolchain_version).tgz" --prefix "$npm_home"
  "${npm_home}/bin/incan" --version
  "${npm_home}/bin/incan-lsp" --help >/dev/null
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
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make toolchain-release-assets first"
  local python
  python="$(python_build_runner)"
  "$python" "${root}/workspaces/release/pip/prepare_package.py" "$dist_dir"
  local venv="${dist_dir}/pip-venv"
  rm -rf "$venv" "${dist_dir}/pip-toolchain-home" "${dist_dir}/pip-bin"
  python3 -m venv "$venv"
  PIP_CACHE_DIR="${dist_dir}/pip-cache" \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    "${venv}/bin/python" -m pip install "${dist_dir}/incan-$(toolchain_version | sed -E 's/-dev\./.dev/; s/-(a|b|rc)([0-9]+)$/\1\2/')-py3-none-any.whl"
  INCAN_TOOLCHAIN_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_PIP_TOOLCHAIN_HOME="${dist_dir}/pip-toolchain-home" \
    INCAN_PIP_BIN_DIR="${dist_dir}/pip-bin" \
    "${venv}/bin/install-incan" --archive "$(archive_path)" --target "$host_target"
  INCAN_TOOLCHAIN_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_PIP_TOOLCHAIN_HOME="${dist_dir}/pip-toolchain-home" \
    INCAN_PIP_BIN_DIR="${dist_dir}/pip-bin" \
    "${venv}/bin/incan" --version
}

smoke_homebrew() {
  require_command ruby
  require_incan_run_bin
  ensure_platform_archive_fixtures
  INCAN_REPO_ROOT="$root" \
    INCAN_TOOLCHAIN_DIST_DIR="$dist_dir" \
    INCAN_TOOLCHAIN_GENERATED_AT="$generated_at" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_GENERATED_CARGO_TARGET_DIR="${root}/target/incan_generated_shared_target" \
    "$incan_run_bin" run "${root}/workspaces/release/toolchain/prepare_assets.incn"
  ruby -c "${dist_dir}/incan.rb"
  if [ "${TOOLCHAIN_HOMEBREW_AUDIT:-0}" = "1" ]; then
    require_command brew
    mkdir -p "${dist_dir}/brew-cache" "${dist_dir}/brew-temp"
    HOMEBREW_CACHE="${dist_dir}/brew-cache" \
      HOMEBREW_TEMP="${dist_dir}/brew-temp" \
      HOMEBREW_NO_ANALYTICS=1 \
      HOMEBREW_NO_AUTO_UPDATE=1 \
      brew audit --strict --formula "${dist_dir}/incan.rb"
  else
    printf 'Skipped brew audit; set TOOLCHAIN_HOMEBREW_AUDIT=1 to run it.\n'
  fi
}

case "${1:-}" in
  package) package_toolchain ;;
  assets) write_assets ;;
  direct) smoke_direct ;;
  npm) smoke_npm ;;
  pip) smoke_pip ;;
  homebrew) smoke_homebrew ;;
  all)
    package_toolchain
    write_assets
    smoke_direct
    smoke_npm
    smoke_pip
    smoke_homebrew
    ;;
  -h|--help) usage ;;
  *) usage >&2; exit 2 ;;
esac
