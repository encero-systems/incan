#!/usr/bin/env bash
set -euo pipefail

generated_at="${TOOLCHAIN_GENERATED_AT:-2026-06-06T00:00:00Z}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
dist_dir="${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}"
case "$dist_dir" in
  /*) ;;
  *) dist_dir="${root}/${dist_dir}" ;;
esac
incan_run_bin_override="${TOOLCHAIN_INCAN_BIN:-}"
incan_run_bin=""
# A caller that is already isolating generated-project state (notably the parallel Oven compiler suite) remains
# authoritative.  Ordinary local release smoke keeps the historic task-local fallback.
generated_cargo_target_dir="${INCAN_GENERATED_CARGO_TARGET_DIR:-${root}/target/incan_generated_shared_target}"

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

toolchain_rust_channel() {
  local channel_file="${dist_dir}/rust-channel-${host_target}.txt"
  [ -f "$channel_file" ] || fail "missing packaged Rust channel: ${channel_file}"
  local channel
  channel="$(sed -n '1p' "$channel_file" | tr -d '\r\n')"
  [ -n "$channel" ] || fail "packaged Rust channel is empty: ${channel_file}"
  printf '%s\n' "$channel"
}

packaged_rustc() {
  local channel
  channel="$(toolchain_rust_channel)"
  if [ -n "${RUSTC:-}" ]; then
    local reported
    reported="$("$RUSTC" --version 2>/dev/null || true)"
    case "$reported" in
      "rustc ${channel}"*)
        printf '%s\n' "$RUSTC"
        return
        ;;
    esac
  fi
  require_command rustup
  local rustc
  rustc="$(rustup which --toolchain "$channel" rustc 2>/dev/null)" \
    || fail "Rust ${channel} sealed this archive, but rustup cannot locate its rustc"
  [ -x "$rustc" ] || fail "Rust ${channel} sealed this archive, but rustup reported no executable rustc"
  printf '%s\n' "$rustc"
}

archive_path() {
  printf '%s/incan-%s-%s.tar.gz\n' "$dist_dir" "$(toolchain_release)" "$host_target"
}

require_archive() {
  local archive
  archive="$(archive_path)"
  [ -f "$archive" ] || fail "missing host archive: ${archive}; run make toolchain-release-package first"
  [ -f "${archive}.sha256" ] || fail "missing archive checksum: ${archive}.sha256"
}

resolve_incan_run_bin() {
  if [ -n "$incan_run_bin_override" ]; then
    printf '%s\n' "$incan_run_bin_override"
    return
  fi
  if [ -f "${dist_dir}/toolchain-release.txt" ]; then
    local packaged_bin="${dist_dir}/dist/incan-$(toolchain_release)-${host_target}/bin/incan"
    if [ -x "$packaged_bin" ]; then
      printf '%s\n' "$packaged_bin"
      return
    fi
  fi
  printf '%s\n' "${root}/target/release/incan"
}

require_incan_run_bin() {
  incan_run_bin="$(resolve_incan_run_bin)"
  [ -x "$incan_run_bin" ] || fail "missing Incan runner: ${incan_run_bin}; run make toolchain-release-build first or set TOOLCHAIN_INCAN_BIN"
}

package_toolchain() {
  [ -x "${root}/target/release/incan" ] || fail "missing target/release/incan; run make toolchain-release-build first"
  [ -x "${root}/target/release/incan-lsp" ] || fail "missing target/release/incan-lsp; run make toolchain-release-build first"
  rm -rf "$dist_dir"
  mkdir -p "$dist_dir"
  printf 'Packaging toolchain for %s into %s\n' "$host_target" "$dist_dir"
  # Packaging records the compiler that sealed this archive's Loafs, and manifest preparation refuses anything
  # that is not a concrete Rust release. A developer whose default toolchain is nightly would otherwise package an
  # archive that cannot be described by a publishable manifest, so select the exact supported toolchain used by CI.
  if [ -z "${RUSTC:-}" ] && command -v rustup >/dev/null 2>&1; then
    local supported_rustc
    if supported_rustc="$(rustup which --toolchain 1.98.0 rustc 2>/dev/null)" && [ -x "$supported_rustc" ]; then
      export RUSTC="$supported_rustc"
      printf 'Using supported rustc for packaging: %s\n' "$("$supported_rustc" --version)"
    fi
  fi
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
    INCAN_HOME="$dist_dir/asset-home" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_SOURCE_ROOT="$root" \
    INCAN_STDLIB="$root/crates/incan_stdlib/stdlib" \
    INCAN_STDLIB_DIR="$root/crates/incan_stdlib/stdlib" \
    INCAN_GENERATED_CARGO_TARGET_DIR="$generated_cargo_target_dir" \
    "$incan_run_bin" run "${root}/workspaces/release/toolchain/prepare_assets.incn"
}

smoke_direct() {
  require_archive
  [ -f "${dist_dir}/manifest.json" ] || fail "missing manifest: ${dist_dir}/manifest.json; run make toolchain-release-assets first"
  rm -rf "${dist_dir}/install-home" "${dist_dir}/install-bin"
  # `--skip-rust` keeps the smoke about archive, link and shim mechanics. Provisioning a full Rust toolchain per
  # smoke stage would add hundreds of megabytes of download to every local run and to the CI publish job; that
  # behavior is proven instead by `make gate-cleanroom`, which installs into containers for real.
  bash "${dist_dir}/install.sh" \
    --manifest "${dist_dir}/manifest.json" \
    --target "$host_target" \
    --archive "$(archive_path)" \
    --incan-home "${dist_dir}/install-home" \
    --bin-dir "${dist_dir}/install-bin" \
    --skip-rust
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
    export INCAN_HOME="${dist_dir}/install-home"
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
  require_archive
  npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    node "${root}/workspaces/release/npm/prepare_package.js" "$dist_dir"
  local npm_home="${dist_dir}/npm-home"
  rm -rf "$npm_home"
  mkdir -p "$npm_home"
  npm_config_cache="${dist_dir}/npm-cache" \
    npm_config_logs_dir="${dist_dir}/npm-logs" \
    npm_config_ignore_scripts=true \
    npm_config_audit=false \
    npm_config_fund=false \
    npm install -g --offline --ignore-scripts "${dist_dir}/incan-toolchain-$(toolchain_version).tgz" --prefix "$npm_home"
  # The reference shim provisions the toolchain from the release manifest on first invocation. An offline smoke
  # cannot exercise that download, so it validates the installed shim against the locally packaged host archive
  # through the shim's explicit toolchain-directory override; installer-driven provisioning is covered by the
  # direct and pip smokes, which share the same install-incan.sh contract the shim bundles.
  local npm_toolchain_dir="${dist_dir}/npm-toolchain"
  rm -rf "$npm_toolchain_dir"
  mkdir -p "$npm_toolchain_dir"
  tar -xzf "$(archive_path)" -C "$npm_toolchain_dir"
  INCAN_NPM_TOOLCHAIN_DIR="$npm_toolchain_dir" "${npm_home}/bin/incan" --version
  INCAN_NPM_TOOLCHAIN_DIR="$npm_toolchain_dir" "${npm_home}/bin/incan-lsp" --help >/dev/null
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
    "${venv}/bin/install-incan" --archive "$(archive_path)" --target "$host_target" --skip-rust
  INCAN_TOOLCHAIN_MANIFEST="${dist_dir}/manifest.json" \
    INCAN_PIP_TOOLCHAIN_HOME="${dist_dir}/pip-toolchain-home" \
    INCAN_PIP_BIN_DIR="${dist_dir}/pip-bin" \
    "${venv}/bin/incan" --version
}

smoke_homebrew() {
  require_command ruby
  require_incan_run_bin
  ensure_platform_archive_fixtures
  # The packaged compiler's immutable Loafs are sealed against the Rust release recorded beside the archive. The
  # direct/npm/pip smokes intentionally use --skip-rust, so their ambient Rust may belong to a different release.
  # Formula rendering imports Rust APIs and must therefore select the archive's exact compiler. An explicit runner is
  # a test/developer authority and keeps its caller-supplied environment unchanged.
  if [ -z "$incan_run_bin_override" ]; then
    export RUSTC="$(packaged_rustc)"
  fi
  INCAN_REPO_ROOT="$root" \
    INCAN_TOOLCHAIN_DIST_DIR="$dist_dir" \
    INCAN_TOOLCHAIN_GENERATED_AT="$generated_at" \
    INCAN_HOME="$dist_dir/asset-home" \
    INCAN_NO_BANNER=1 \
    CARGO_NET_OFFLINE=true \
    INCAN_SOURCE_ROOT="$root" \
    INCAN_STDLIB="$root/crates/incan_stdlib/stdlib" \
    INCAN_STDLIB_DIR="$root/crates/incan_stdlib/stdlib" \
    INCAN_GENERATED_CARGO_TARGET_DIR="$generated_cargo_target_dir" \
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
