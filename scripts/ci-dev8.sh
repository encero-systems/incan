#!/usr/bin/env bash
# Install one host's dev.8 release bundle and verify the checkout's contracts.
# Usage: ci-dev8.sh ARCHIVE OVEN | ci-dev8.sh GHCR_TAG@sha256:DIGEST
# CI_DEV8_OUT selects the evidence directory; CI_DEV8_JOBS defaults to two.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export LC_ALL=C

fail() {
    echo "ci-dev8: $*" >&2
    exit 1
}

# Keep complete operation logs and show the first diagnostic when a prerequisite fails.
logged_step() {
    local name=$1 description=$2
    shift 2
    if "$@" > "$run/$name.log" 2>&1; then
        return
    fi
    head -c 6000 "$run/$name.log" >&2
    fail "$description failed; evidence: $run"
}

# Install missing Rust components without updating an already pinned installation.
install_rust() {
    command -v rustup >/dev/null || fail 'rustup is required'
    if ! rustup run 1.98.0 rustc --version >/dev/null 2>&1; then
        rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
    elif ! rustup component list --toolchain 1.98.0 --installed | grep -q '^rustc-dev-'; then
        rustup component add --toolchain 1.98.0 rustc-dev
    fi
    export RUSTUP_TOOLCHAIN=1.98.0
    export INCAN_SYSROOT
    INCAN_SYSROOT=$(rustup run 1.98.0 rustc --print sysroot)
    rustup run 1.98.0 rustc -vV > "$run/rust.txt"
    grep -qx 'release: 1.98.0' "$run/rust.txt" || fail 'Rust 1.98.0 is required'
    grep -qx "host: $target" "$run/rust.txt" || fail "Rust must match $target"
}

# Resolve only immutable GHCR references; the companion Oven shares the archive's digest pin.
resolve_bundle() {
    local input=$1
    if [[ -f "$input" ]]; then
        [[ $# == 2 && -x "$2" ]] || fail 'a local archive requires its matching Oven executable'
        archive=$(cd "$(dirname "$input")" && pwd)/$(basename "$input")
        oven=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
        return
    fi
    [[ $# == 1 ]] || fail 'an OCI bundle supplies its own Oven'
    local pin="^ghcr[.]io/encero-systems/incan[.]pub/toolchain/incan:[a-zA-Z0-9_.-]+-$target@sha256:[a-f0-9]{64}$"
    [[ "$input" =~ $pin ]] || fail "expected a GHCR tag for $target with a sha256 digest pin"
    command -v oras >/dev/null || fail 'oras 1.3 is required for a registry reference'
    mkdir "$run/download"
    logged_step pull 'bundle pull' oras pull "$input" --output "$run/download"
    archive=$run/download/toolchain.tar.zst
    oven=$run/download/incan
    [[ -f "$archive" && -f "$oven" ]] || fail 'bundle must contain toolchain.tar.zst and incan'
    chmod +x "$oven"
    printf '%s\n' "$input" > "$run/reference.txt"
}

# Expose only standard packages actually present in this installation, using checkout sources.
select_standard_packages() {
    local artifact package
    mkdir "$run/stdlib"
    for artifact in "$INCAN_HOME/toolchains/0.6.0-dev.8/loaves/"*; do
        [[ -d "$artifact" ]] || continue
        package=${artifact##*/}
        [[ "$package" == incan_stdlib_* ]] || fail "unknown standard package: $package"
        package=${package#incan_stdlib_}
        [[ -f "$ROOT/loaves/stdlib/$package/loaf.toml" ]] || fail "missing standard source: $package"
        ln -s "$ROOT/loaves/stdlib/$package" "$run/stdlib/$package"
        printf '%s\n' "$package" >> "$run/standard-packages.txt"
    done
    [[ -d "$run/stdlib/core" ]] || fail 'archive has no core standard package'
    export INCAN_STDLIB=$run/stdlib
}

# Build this checkout's runner with the installed compiler, preserving compiler diagnostics on failure.
build_runner() {
    mkdir "$run/runner"
    cp "$ROOT/workspaces/contracts/loaf.toml" "$run/runner/loaf.toml"
    cp -R "$ROOT/workspaces/contracts/src" "$run/runner/src"
    logged_step runner-build 'contract runner build' "$oven" build "$run/runner/src/main.incn"
    runner=$run/runner/target/incan/debug/bin/contracts
    [[ -x "$runner" ]] || fail "runner build produced no executable at $runner"
}

# Run all families under one worker budget. Existing failures are allowed; lost PASS keys are not.
run_contracts() {
    local status=0
    "$runner" "$oven" "$ROOT/workspaces/contracts/fixtures" \
        --scratch "$run/scratch" --jobs "${CI_DEV8_JOBS:-2}" --timeout "${CI_DEV8_TIMEOUT:-120}" \
        behavior check library interop testing project format graph > "$run/contracts.log" 2>&1 || status=$?
    [[ $status -le 1 ]] || fail "contract runner exited $status; evidence: $run"
    "$ROOT/scripts/check-dev8-baseline.sh" "$ROOT/workspaces/contracts/dev8-pass-baseline.txt" \
        "$run/contracts.log" | tee "$run/summary.txt"
}

[[ $# -ge 1 && $# -le 2 ]] || fail 'usage: ci-dev8.sh ARCHIVE OVEN | GHCR_TAG@sha256:DIGEST'
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    *) fail 'supported hosts: macOS arm64 and Linux x86_64' ;;
esac
mkdir -p "${CI_DEV8_OUT:-$ROOT/target/ci-dev8}"
run=$(mktemp -d "${CI_DEV8_OUT:-$ROOT/target/ci-dev8}/run.XXXXXX")
run=$(cd "$run" && pwd)
echo "dev.8 evidence: $run"
git -C "$ROOT" rev-parse HEAD > "$run/checkout.txt"
export INCAN_HOME=$run/home INCAN_NO_BANNER=1
unset RUSTC_BOOTSTRAP INCAN_STDLIB
install_rust
resolve_bundle "$@"
shasum -a 256 "$archive" > "$run/archive.sha256"
shasum -a 256 "$oven" > "$run/oven.sha256"
logged_step install 'toolchain installation' "$oven" toolchain install "$archive"
select_standard_packages
build_runner
run_contracts
