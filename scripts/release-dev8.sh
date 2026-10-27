#!/usr/bin/env bash
# Rehearse one host's source release without publishing anything.
# Usage: INCAN_HOME=SEED_HOME RELEASE_DEV8_OVEN=/path/incan release-dev8.sh SHA NEW_OUT
# The clean checkout builds natively from its own source with the seed toolchain, through the checkout's native build
# scripts (zsh): Oven, then the driver with its Incan lowering, the runtime, the standard packages and the bundled tools.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export LC_ALL=C

fail() {
    echo "release-dev8: $*" >&2
    exit 1
}

# Retain the complete failing build; a refusal never becomes a successful receipt.
step() {
    phase=$1
    shift
    local started=$SECONDS
    if "$@" > "$out/$phase.log" 2>&1; then
        printf '%s\t%s\n' "$phase" "$((SECONDS - started))" >> "$out/timings.tsv"
        return
    fi
    tail -30 "$out/$phase.log" >&2
    fail "$phase failed; see $out/$phase.log"
}

# Capture partial evidence on failure too; absent counts and artifacts stay null.
receipt() {
    local status=$? file
    trap - EXIT
    local artifacts='{}'
    for file in toolchain.tar.zst incan incan-lsp; do
        if [[ -f "$out/bundle/$file" ]]; then
            artifacts=$(jq --arg name "$file" \
                --arg digest "$(shasum -a 256 "$out/bundle/$file" | cut -d ' ' -f1)" \
                '. + {($name): $digest}' <<< "$artifacts")
        fi
    done
    jq -n --arg sha "$sha" --arg host "$target" --arg phase "$phase" --argjson status "$status" \
        --argjson artifacts "$artifacts" --argjson elapsed "$((SECONDS - began))" \
        --argjson counts "$(cat "$out/counts.json" 2>/dev/null || echo null)" \
        --argjson oven_tests "$(cat "$out/oven-tests.json" 2>/dev/null || echo null)" \
        --arg compiler_build "$compiler_build" \
        --argjson parity "$parity" \
        '{schema:"incan.release-rehearsal/1", source_sha:$sha, host:$host,
          outcome:(if $status==0 then "passed" else "refused" end), phase:$phase,
          exit_code:$status, elapsed_seconds:$elapsed, sha256:$artifacts,
          compiler_build:$compiler_build, contracts:$counts, oven_tests:$oven_tests,
          baseline_parity:$parity}' > "$out/receipt.json"
    echo "release receipt: $out/receipt.json"
    exit "$status"
}

# Borrow immutable seed artifacts by absolute identity while building into a disposable home. Packing later closes
# every selected path into the candidate archive; no seed archive is relabeled as the candidate.
prepare_build_home() {
    local seed_directory=$seed_home/toolchains/0.6.0-dev.8
    mkdir -p "$toolchain"
    jq --arg base "$seed_directory/" '
        def absolute: if startswith("/") then . else $base + . end;
        .driver |= absolute | .sysroot |= absolute | .rust_abi |= map(absolute) |
        .externs |= map(.path |= absolute) | .search |= map(absolute) |
        if (.registry_units // "") != "" then .registry_units |= absolute else . end
    ' "$seed_directory/toolchain.json" > "$toolchain/toolchain.json"
    ln -s "$(jq -er '.sysroot' "$toolchain/toolchain.json")" "$toolchain/sysroot"
    ln -s "$seed_directory/loaves" "$toolchain/loaves"
    shasum -a 256 "$seed_oven" "$seed_directory/toolchain.json" > "$out/seed.sha256"
}

# Lay out the lane the checkout's native build scripts expect over this rehearsal: the clean checkout as its
# `incan`, with the scripts' Oven, runner, LSP and log directories inside this output.
prepare_native_lane() {
    lane=$out/lane
    mkdir -p "$lane/out"
    ln -s "$source" "$lane/incan"
}

# Run one of the checkout's native build scripts on the rehearsal's lane and build home, with `builder` as its Oven.
native() {
    local phase=$1 script=$2
    shift 2
    step "$phase" env DEV8_LANE="$lane" SELFBUILD_HOME="$INCAN_HOME" SELFBUILD_OVEN="$builder" \
        SELFBUILD_STDLIB="$INCAN_STDLIB" INCAN_RELINK_LOG="$out/$phase-oven.log" zsh "$source/scripts/$script" "$@"
}

# Compile standard packages in dependency order and publish only complete artifact sets into this build home.
build_standard_packages() {
    local package name directory profile file
    mkdir "$out/stdlib"
    export INCAN_STDLIB=$out/stdlib
    # These are installed seed artifacts in this script's disposable home, never user-owned sources.
    rm -r -- "$toolchain/loaves"
    for package in core testing interop async system codecs compression web data observability; do
        name=incan_stdlib_$package
        step "std-$package" "$seed_oven" build "$source/loaves/stdlib/$package/src/lib.incn"
        directory=$source/loaves/stdlib/$package/target/incan/loaves/$name/debug
        for file in "lib$name.rlib" "$name.incnlib" "$name.incnbody" "$name.native.json"; do
            [[ -f "$directory/$file" ]] || fail "missing standard artifact: $directory/$file"
        done
        for profile in debug release; do
            mkdir -p "$toolchain/loaves/$name/$profile"
            cp -R "$directory/." "$toolchain/loaves/$name/$profile/"
        done
        ln -s "$source/loaves/stdlib/$package" "$out/stdlib/$package"
    done
}

# The installed runtime must be one root: two externs with one name would make every unit's closure ambiguous.
check_runtime_roots() {
    jq -e '.externs | group_by(.name) | all(length==1)' "$toolchain/toolchain.json" >/dev/null \
        || fail 'native runtime closure has conflicting direct roots'
}

# Count a complete census and compare all case outcomes, beyond the committed PASS floor.
compare_censuses() {
    local installed_log=$1 log
    [[ $(cat "$reference_log.source-sha") == "$sha" ]] || fail 'integration census names a different source SHA'
    step integration-baseline "$source/scripts/check-dev8-baseline.sh" \
        "$source/workspaces/contracts/dev8-pass-baseline.txt" "$reference_log"
    cp "$reference_log" "$out/integration-contracts.log"
    shasum -a 256 "$out/integration-contracts.log" > "$out/integration-contracts.sha256"
    for log in "$out/integration-contracts.log" "$out/baseline-contracts.log" "$installed_log"; do
        awk '/^(PASS|FAIL|PEND) / {print $2 "\t" $1}' "$log" | sort > "$log.keys"
    done
    diff -u "$out/integration-contracts.log.keys" "$out/baseline-contracts.log.keys" \
        > "$out/integration-parity.diff" || fail 'candidate differs from the integration census'
    diff -u "$out/baseline-contracts.log.keys" "$installed_log.keys" > "$out/parity.diff" || fail 'baseline outcomes differ'
    parity=true
    awk '/^total:/ {printf "{\"pass\":%d,\"fail\":%d,\"pending\":%d}\n",$2,$4,$6}' \
        "$installed_log" > "$out/counts.json"
}

# With no supplied same-SHA census, measure the integration seed against the clean checkout's cases first.
prepare_reference() {
    if [[ -n ${RELEASE_DEV8_BASELINE_LOG:-} ]]; then
        reference_log=$RELEASE_DEV8_BASELINE_LOG
        return
    fi
    reference_log=$out/reference-contracts.log
    step reference-runner-build env INCAN_HOME="$seed_home" "$original_oven" \
        build "$source/workspaces/contracts" --profile release
    local reference_runner=$source/workspaces/contracts/target/incan/release/bin/contracts status=0
    phase=reference-contracts
    env INCAN_HOME="$seed_home" "$reference_runner" "$original_oven" "$source/workspaces/contracts/fixtures" \
        --scratch "$out/reference-scratch" --jobs "${CI_DEV8_JOBS:-2}" --timeout "${CI_DEV8_TIMEOUT:-120}" \
        behavior check library interop testing project format graph > "$reference_log" 2>&1 || status=$?
    [[ $status -le 1 ]] || fail "integration runner exited $status"
    printf '%s\n' "$sha" > "$reference_log.source-sha"
}

[[ $# == 2 ]] || fail 'usage: release-dev8.sh SHA NEW_OUT'
sha=$(git -C "$ROOT" rev-parse --verify "$1^{commit}")
[[ "$1" == "$sha" ]] || fail 'SHA must be a full source commit'
[[ ! -e "$2" ]] || fail "output already exists: $2"
seed_oven=${RELEASE_DEV8_OVEN:-$(command -v incan || true)}
[[ -n "$seed_oven" && -x "$seed_oven" ]] || fail 'set RELEASE_DEV8_OVEN to an installed dev.8 Oven'
seed_oven=$(cd "$(dirname "$seed_oven")" && pwd)/$(basename "$seed_oven")
original_oven=$seed_oven
[[ -n ${INCAN_HOME:-} ]] || fail 'INCAN_HOME must select the installed seed toolchain'
seed_home=$(cd "$INCAN_HOME" && pwd)
for command in jq shasum rustup git tar zsh; do command -v "$command" >/dev/null || fail "$command is required"; done
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    *) fail 'supported hosts: macOS arm64 and Linux x86_64' ;;
esac
mkdir -p "$2"
out=$(cd "$2" && pwd)
source=$out/source
began=$SECONDS
phase=checkout
parity=null
compiler_build=native
trap receipt EXIT
mkdir "$out/bundle" "$out/tmp"
export TMPDIR=$out/tmp
step clone git clone --shared --no-checkout "$ROOT" "$source"
step checkout git -C "$source" checkout --detach "$sha"
[[ -z $(git -C "$source" status --porcelain) ]] || fail 'source checkout is dirty'
unset RUSTC_BOOTSTRAP INCAN_LSP INCAN_STDLIB
export INCAN_NO_BANNER=1 OVEN_OWNER=off RUSTUP_TOOLCHAIN=1.98.0
rustup run 1.98.0 rustc -vV > "$out/rust.txt"
export INCAN_HOME=$out/build-home
toolchain=$INCAN_HOME/toolchains/0.6.0-dev.8
prepare_build_home
export INCAN_STDLIB=$source/loaves/stdlib
prepare_reference
prepare_native_lane
builder=$seed_oven
native conductor-build build-native-tools.sh oven
seed_oven=$lane/oven-bin/incan
builder=$seed_oven

# The candidate compiler must come from the clean tree: Oven compiles the Incan lowering with the seed's driver, then
# the driver, and installs it with its Rust ABI cache into the build home. A seed archive is never relabeled as its
# output.
native driver-build relink-driver.sh --release
native runtime-build build-native-runtime.sh
check_runtime_roots
build_standard_packages
native oven-build build-native-tools.sh oven --rebuild
cp "$lane/oven-bin/incan" "$out/bundle/incan"
oven=$out/bundle/incan
builder=$oven
native lsp-build build-native-tools.sh lsp
cp "$lane/lsp-bin/incan-lsp" "$out/bundle/incan-lsp"
native runner-build build-native-tools.sh contracts
runner=$lane/contracts-bin/contracts
step pack "$oven" toolchain pack "$out/bundle/toolchain.tar.zst"

# Measure this exact fixture tree directly before installing the bundle into a second fresh home.
phase=baseline-contracts
export INCAN_LSP=$out/bundle/incan-lsp
status=0
"$runner" "$oven" "$source/workspaces/contracts/fixtures" --scratch "$out/baseline-scratch" \
    --jobs "${CI_DEV8_JOBS:-2}" --timeout "${CI_DEV8_TIMEOUT:-120}" \
    behavior check library interop testing project format graph > "$out/baseline-contracts.log" 2>&1 || status=$?
[[ $status -le 1 ]] || fail "baseline runner exited $status"
step baseline "$source/scripts/check-dev8-baseline.sh" "$source/workspaces/contracts/dev8-pass-baseline.txt" \
    "$out/baseline-contracts.log"
export CI_DEV8_OUT=$out/ci
step installed-ci "$source/scripts/ci-dev8.sh" "$out/bundle/toolchain.tar.zst" "$oven" "$out/bundle/incan-lsp"
installed=($out/ci/run.*/contracts.log)
[[ ${#installed[@]} == 1 && -f ${installed[0]} ]] || fail 'expected one installed census'
compare_censuses "${installed[0]}"
cp "$(dirname "${installed[0]}")/oven-tests.json" "$out/oven-tests.json"
phase=complete
