#!/usr/bin/env bash
# Rehearse one host's source release without publishing anything.
# Usage: INCAN_HOME=SEED_HOME RELEASE_DEV8_OVEN=/path/incan release-dev8.sh SHA NEW_OUT
# DEV8_LANE selects the frozen bootstrap inputs until compiler self-hosting is available (DD-0010).
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

# Write a dependency only into this rehearsal's generated bootstrap manifest. JSON strings are also valid TOML
# basic strings, so spaces and quotes in the clean checkout's path cannot change the dependency declaration.
bootstrap_dependency() {
    printf '%s = { loaf = "%s", path = %s }\n' "$1" "$2" "$(jq -n --arg path "$3" '$path')"
}

# Bind emitted Rust to this SHA's Rust facets, rather than the bootstrap machine's original checkout paths.
bootstrap_manifest() {
    local component=$1 directory=$bootstrap_lane/bootstrap/loaves/$1 project crate
    if [[ $component == mir_lowering || $component == native_runtime ]]; then
        directory=$bootstrap_lane/bootstrap/loaves/incan_$component
    fi
    project=incan_$component
    crate=${project%_component}
    mkdir -p "$directory"
    {
        printf '[project]\nname = "%s"\nversion = "0.6.0-dev.7"\n\n' "$project"
        printf '[rust]\nname = "%s"\ntype = "lib"\nedition = "2024"\n\n[dependencies]\n' "$crate"
        bootstrap_dependency incan_std_core incan_stdlib_core "$source/loaves/stdlib/core"
        bootstrap_dependency incan_derive incan_derive "$source/loaves/stdlib/derive/incan_derive"
        if [[ $component != stdlib_core_component ]]; then
            bootstrap_dependency incan_stdlib_core incan_stdlib_core_component \
                "$bootstrap_lane/bootstrap/loaves/stdlib_core_component"
        fi
        case $component in
            stdlib_async_component|native_runtime)
                bootstrap_dependency incan_std_async incan_stdlib_async "$source/loaves/stdlib/async"
                ;;
        esac
        case $component in
            stdlib_testing_component|native_runtime)
                bootstrap_dependency incan_std_testing incan_stdlib_testing "$source/loaves/stdlib/testing"
                ;;
        esac
        case $component in
            mir_lowering|native_runtime)
                bootstrap_dependency incan_lang incan_lang "$source/loaves/kernel/incan_lang"
                ;;
        esac
        if [[ $component == mir_lowering ]]; then
            bootstrap_dependency incan_semantics_core incan_semantics_core "$source/loaves/kernel/incan_semantics_core"
        elif [[ $component == native_runtime ]]; then
            bootstrap_dependency incan_std_data incan_stdlib_data "$source/loaves/stdlib/data"
            bootstrap_dependency incan_stdlib_async incan_stdlib_async_component \
                "$bootstrap_lane/bootstrap/loaves/stdlib_async_component"
            bootstrap_dependency incan_stdlib_testing incan_stdlib_testing_component \
                "$bootstrap_lane/bootstrap/loaves/stdlib_testing_component"
            printf 'serde = { loaf = "crates-io/serde", version = "1.0", features = ["derive"] }\n'
        elif [[ $component == stdlib_async_component ]]; then
            printf 'tokio = { loaf = "crates-io/tokio", version = "1", features = ["macros", "net", "rt-multi-thread", "sync", "time"] }\n'
        fi
    } > "$directory/loaf.toml"
}

# Re-emit this clean SHA's lowering and runtime with the configured frozen builder. Its generated standard support
# is a recorded seed input; the release's ordinary standard Loaves are compiled afresh below.
prepare_bootstrap() {
    local seed_lane=${DEV8_LANE:?set DEV8_LANE for the frozen builder} component
    compiler_build=frozen-stage-zero
    [[ -x "$seed_lane/bootstrap/scripts/stage0-boot.sh" ]] || fail 'frozen stage-zero wrapper is absent'
    bootstrap_lane=$out/bootstrap-lane
    mkdir -p "$bootstrap_lane/bootstrap/ambient2" "$bootstrap_lane/bootstrap/scripts" "$bootstrap_lane/oven-bin"
    ln -s "$source" "$bootstrap_lane/incan"
    ln -s "$INCAN_HOME" "$bootstrap_lane/dev8-home"
    ln -s "$seed_lane/stage0-home" "$bootstrap_lane/stage0-home"
    cp -R "$seed_lane/bootstrap/kernel" "$bootstrap_lane/bootstrap/kernel"
    cp "$seed_lane/bootstrap/scripts/stage0-boot.sh" "$bootstrap_lane/bootstrap/scripts/"
    cp "$seed_oven" "$bootstrap_lane/oven-bin/incan"
    for component in incan_mir_lowering incan_native_runtime; do
        mkdir "$bootstrap_lane/bootstrap/ambient2/$component"
        cp "$seed_lane/bootstrap/ambient2/$component/loaf.toml" \
            "$bootstrap_lane/bootstrap/ambient2/$component/loaf.toml"
        if [[ -f "$seed_lane/bootstrap/ambient2/$component/oven.lock" ]]; then
            cp "$seed_lane/bootstrap/ambient2/$component/oven.lock" \
                "$bootstrap_lane/bootstrap/ambient2/$component/oven.lock"
        fi
    done
    for component in stdlib_core_component stdlib_async_component stdlib_testing_component; do
        bootstrap_manifest "$component"
        cp -R "$seed_lane/bootstrap/loaves/$component/src" "$bootstrap_lane/bootstrap/loaves/$component/src"
    done
    bootstrap_manifest mir_lowering
    bootstrap_manifest native_runtime
    # The shared runtime helper's final installer belongs to an ordinary lane, not to this isolated build home.
    printf '#!/bin/sh\nexit 0\n' > "$bootstrap_lane/bootstrap/scripts/install-toolchain.sh"
    chmod +x "$bootstrap_lane/bootstrap/scripts/install-toolchain.sh"
    step lowering-emission env DEV8_LANE="$bootstrap_lane" zsh \
        "${RELEASE_DEV8_LOWERING_EMITTER:-$source/scripts/refresh-lowering.sh}"
    cp "$bootstrap_lane/bootstrap/lowering-emission.sha256" "$out/lowering-emission.sha256"
    (
        cd "$bootstrap_lane/bootstrap/loaves"
        find stdlib_core_component/src stdlib_async_component/src stdlib_testing_component/src -type f \
            -exec shasum -a 256 {} \;
    ) > "$out/frozen-support.sha256"
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

# Replace the seed runtime with this tree's native library and its published exact dependency closure.
install_runtime() {
    local directory native library
    if [[ $compiler_build == frozen-stage-zero ]]; then
        local units=$bootstrap_lane/bootstrap/loaves/incan_native_runtime/target/incan/rust/debug/units.json
        directory=$(jq -er '[.units[] | select(.crate=="incan_native_runtime")] | select(length==1) | .[0].directory' "$units")
        library=$(jq -er '.units[] | select(.crate=="incan_native_runtime") | .artifact' "$units")
        native=$directory/native.json
    else
        directory=$source/loaves/compiler/incan_native_runtime/target/incan/loaves/incan_native_runtime/debug
        library=$directory/libincan_native_runtime.rlib
        native=$directory/incan_native_runtime.native.json
    fi
    [[ -f "$library" && -f "$native" ]] || fail 'runtime artifacts or graph-selected native inputs are incomplete'
    jq --slurpfile native "$native" --arg base "$directory/" --arg library "$library" '
        def absolute: if startswith("/") then . else $base + . end;
        .externs = ([$native[0].externs[], ($native[0].macro_externs // [])[]]
            | map(.[1] |= absolute)
            | map(select(.[0] | startswith("incan_stdlib_") | not) | {name:.[0], path:.[1]}))
            + [{name:"incan_native_runtime", path:$library}] |
        .search = ($native[0].search | map(absolute) | unique)
    ' "$toolchain/toolchain.json" > "$toolchain/toolchain.json.next"
    jq -e '.externs | group_by(.name) | all(length==1)' "$toolchain/toolchain.json.next" >/dev/null \
        || fail 'native runtime closure has conflicting direct roots'
    mv "$toolchain/toolchain.json.next" "$toolchain/toolchain.json"
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
for command in jq shasum rustup git tar; do command -v "$command" >/dev/null || fail "$command is required"; done
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
step conductor-build "$seed_oven" build "$source/workspaces/oven" --profile release
seed_oven=$source/workspaces/oven/target/incan/release/bin/oven

# The candidate compiler must come from the clean tree. A seed archive is never relabeled as its output.
if [[ -n ${DEV8_LANE:-} ]]; then
    prepare_bootstrap
    step driver-build "$seed_oven" build "$source/loaves/toolchain/incan-rustc-driver" --profile release \
        --substitute "incan_mir_lowering=$bootstrap_lane/bootstrap/loaves/incan_mir_lowering"
else
    step driver-build "$seed_oven" build "$source/loaves/toolchain/incan-rustc-driver" --profile release
fi
driver=$source/loaves/toolchain/incan-rustc-driver/target/incan/rust/release/bin/incan-rustc-driver
[[ -x "$driver" ]] || fail 'driver build produced no executable'
sysroot=$(rustup run 1.98.0 rustc --print sysroot)
mkdir -p "$toolchain/bin" "$toolchain/rust-abi"
cp "$driver" "$toolchain/bin/incan-rustc-driver"
step rust-abi "$driver" --rust-abi "$sysroot" "$toolchain/rust-abi/std.rustabi" std core alloc
jq --arg driver "$toolchain/bin/incan-rustc-driver" --arg abi "$toolchain/rust-abi/std.rustabi" \
    '.driver=$driver | .rust_abi=[$abi]' "$toolchain/toolchain.json" > "$toolchain/toolchain.json.next"
mv "$toolchain/toolchain.json.next" "$toolchain/toolchain.json"
if [[ $compiler_build == frozen-stage-zero ]]; then
    step runtime-build env DEV8_LANE="$bootstrap_lane" INCAN_RUNTIME_BUILD_HOME="$INCAN_HOME" \
        zsh "${RELEASE_DEV8_RUNTIME_EMITTER:-$DEV8_LANE/bootstrap/scripts/refresh-runtime.sh}"
else
    step runtime-build "$seed_oven" build "$source/loaves/compiler/incan_native_runtime"
fi
install_runtime
build_standard_packages
step oven-build "$seed_oven" build "$source/workspaces/oven" --profile release
cp "$source/workspaces/oven/target/incan/release/bin/oven" "$out/bundle/incan"
oven=$out/bundle/incan
step lsp-build "$oven" build "$source/loaves/toolchain/incan-lsp" --profile release
cp "$source/loaves/toolchain/incan-lsp/target/incan/rust/release/bin/incan-lsp" "$out/bundle/incan-lsp"
step runner-build "$oven" build "$source/workspaces/contracts" --profile release
runner=$source/workspaces/contracts/target/incan/release/bin/contracts
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
