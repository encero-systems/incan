#!/bin/zsh
# Build the runtime's Rust support facets into a fresh home's single release generation.
# SELFBUILD_BUILDER_HOME selects the installed compiler that builds the Incan orchestration tool.
set -eu

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
repo=$lane/incan
if (( $# > 1 )) || [[ -n ${1:-} && $1 != --tool-only ]]; then
    print -u2 'usage: build-native-support.sh [--tool-only]'
    exit 2
fi
builder=${SELFBUILD_OVEN:-$lane/oven-bin/incan}
export INCAN_STDLIB=${SELFBUILD_STDLIB:-$repo/loaves/stdlib}
mkdir -p "$lane/out/native-builds"
log=$lane/out/native-builds/support.log
INCAN_HOME=${SELFBUILD_BUILDER_HOME:-$lane/dev8-home} "$builder" build \
    "$repo/workspaces/oven/src/toolchain_native.incn" --profile release > "$log" 2>&1 \
    || { tail -20 "$log" | cut -c1-600; exit 1; }
helper=$lane/out/native-builds/toolchain-native
cp "$repo/workspaces/oven/target/incan/release/bin/oven" "$helper.next"
mv "$helper.next" "$helper"
[[ ${1:-} != --tool-only ]] || exit 0
home_dir=${SELFBUILD_HOME:?set SELFBUILD_HOME to a fresh toolchain home}
INCAN_HOME=$home_dir "$helper" "$repo"
