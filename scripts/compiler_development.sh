#!/bin/sh
# Transport explicit Make inputs to the Incan-owned compiler development loop.
# Receipt selection, dependency preparation and compilation decisions stay in Incan and Oven.
set -eu

if [ "$#" -lt 8 ]; then
    echo "usage: compiler_development.sh ROOT TARGET_DIR STAGE_ZERO TOOLCHAIN SDK_PATH_FILE STORE MODE INPUT [EXACT REPORT]" >&2
    exit 2
fi
source_root=$1
target_dir=$2
stage_zero=$3
toolchain=$4
sdk_path_file=$5
store=$6
mode=$7
shift 7
case "$mode:$#" in
    build:1|quiet:1|test:3) ;;
    *) echo "invalid compiler-development mode or argument count" >&2; exit 2 ;;
esac

mkdir -p "$target_dir/compiler-development/command-evidence"
evidence_name=$mode
if [ "$mode" = test ]; then evidence_name=test-one; fi
command_evidence=$(mktemp -d "$target_dir/compiler-development/command-evidence/$evidence_name.XXXXXX")
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
        if [ -f "$command_evidence/bootstrap-preparation.stderr" ]; then
            cat "$command_evidence/bootstrap-preparation.stderr" >&2
        fi
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

if [ -z "${INCAN_SDK_INVENTORY:-}" ]; then
    INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE="$sdk_path_file" "$stage_zero" prepare-sdk
    INCAN_SDK_INVENTORY="$(cat "$sdk_path_file")/sdk-inventory.json"
    export INCAN_SDK_INVENTORY
fi
cd "$source_root/workspaces/compiler-bootstrap"
"$stage_zero" oven bake --project "$source_root/workspaces/compiler-bootstrap" --format json \
    > "$command_evidence/bootstrap-preparation.json" 2> "$command_evidence/bootstrap-preparation.stderr"
export INCAN_COMPILER_STAGE_ZERO="$stage_zero"
if [ "$mode" = test ]; then
    export INCAN_TEST_OVEN_TEST_ONE_REPORT="$3"
    "$stage_zero" run "$source_root/workspaces/compiler-bootstrap/src/main.incn" -- \
        --test "$source_root" "$1" "$2" "$RUSTC" "$store"
else
    "$stage_zero" run "$source_root/workspaces/compiler-bootstrap/src/main.incn" -- \
        "$source_root" "$1" "$RUSTC" "$store" > "$command_evidence/compiler-bootstrap.json"
    if [ "$mode" = build ]; then
        cat "$command_evidence/compiler-bootstrap.json"
        echo "Compiler-development evidence: $command_evidence"
    fi
fi
