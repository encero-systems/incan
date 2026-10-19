#!/bin/sh
# Transport explicit Make inputs to the Incan-owned compiler development loop.
# Receipt selection, dependency preparation and compilation decisions stay in Incan and Oven; Cargo is never run.
set -eu

if [ "$#" -lt 7 ]; then
    echo "usage: compiler_development.sh ROOT TARGET_DIR STAGE_ZERO TOOLCHAIN STORE MODE INPUT [EXACT REPORT]" >&2
    exit 2
fi
source_root=$1
target_dir=$2
stage_zero=$3
toolchain=$4
store=$5
mode=$6
shift 6
case "$mode:$#" in
    build:1|quiet:1|test:3) ;;
    *) echo "invalid compiler-development mode or argument count" >&2; exit 2 ;;
esac

# Shell quoting preserves spaces in an output root; Make's abspath treats them as a list of paths.
absolute_path() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$source_root" "$1" ;;
    esac
}
target_dir=$(absolute_path "$target_dir")
store=$(absolute_path "$store")
mkdir -p "$target_dir/compiler-development/command-evidence"
evidence_name=$mode
if [ "$mode" = test ]; then evidence_name=test-one; fi
command_evidence=$(mktemp -d "$target_dir/compiler-development/command-evidence/$evidence_name.XXXXXX")
# The guard stands in for Cargo on PATH; any invocation fails the command.
cp "$source_root/scripts/cargo-guard/cargo" "$command_evidence/cargo"
: > "$command_evidence/cargo-invocations.log"
finish_command() {
    command_status=$?
    trap - EXIT
    if [ -s "$command_evidence/cargo-invocations.log" ]; then
        echo "Compiler development reached the Cargo guard; evidence: $command_evidence" >&2
        command_status=1
    fi
    if [ "$command_status" -ne 0 ]; then
        echo "Compiler-development evidence: $command_evidence" >&2
        if [ -f "$command_evidence/compiler-bootstrap.json" ]; then
            cat "$command_evidence/compiler-bootstrap.json" >&2
        fi
    fi
    exit "$command_status"
}
trap finish_command EXIT
export PATH="$command_evidence:$PATH"
export CARGO="$command_evidence/cargo"
export INCAN_OVEN_CARGO_GUARD_LOG="$command_evidence/cargo-invocations.log"
stage_zero=$(command -v "$stage_zero")
case "$stage_zero" in
    /*) ;;
    *) stage_zero="$(cd "$(dirname "$stage_zero")" && pwd)/$(basename "$stage_zero")" ;;
esac
RUSTC=$(rustup which --toolchain "$toolchain" rustc)
export RUSTC RUSTUP_TOOLCHAIN="$toolchain" INCAN_NO_BANNER=1

cd "$source_root/workspaces/compiler-bootstrap"
export INCAN_COMPILER_STAGE_ZERO="$stage_zero"
if [ "$mode" = test ]; then
    report=$3
    if [ -n "$report" ]; then report=$(absolute_path "$report"); fi
    export INCAN_TEST_OVEN_TEST_ONE_REPORT="$report"
    "$stage_zero" run "$source_root/workspaces/compiler-bootstrap/src/main.incn" -- \
        --test "$source_root" "$1" "$2" "$RUSTC" "$store"
else
    output=$(absolute_path "$1")
    "$stage_zero" run "$source_root/workspaces/compiler-bootstrap/src/main.incn" -- \
        "$source_root" "$output" "$RUSTC" "$store" > "$command_evidence/compiler-bootstrap.json"
    if [ "$mode" = build ]; then
        cat "$command_evidence/compiler-bootstrap.json"
        echo "Compiler-development evidence: $command_evidence"
    fi
fi
