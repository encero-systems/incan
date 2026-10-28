#!/usr/bin/env bash
# Measure fixed source workloads in an explicitly reserved checkout and coherent installed home.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=${SCALING_SOURCE:?set SCALING_SOURCE to a dedicated, prepared source checkout}
output=${SCALING_OUTPUT:?set SCALING_OUTPUT to a new directory}
oven=${INCAN:?set INCAN to installed Oven}
installed=${INCAN_HOME:?set INCAN_HOME to the matching installed home}
support=${SCALING_SUPPORT:?set SCALING_SUPPORT to source-built toolchain-native}
[[ $(uname -s) == Darwin ]] || { echo 'Kernel footprint measurement requires macOS' >&2; exit 2; }
[[ ! -e $output ]] || { echo 'SCALING_OUTPUT must be new; completed receipts are never overwritten' >&2; exit 2; }
mkdir -p "$output/commands" "$output/lane/bootstrap" "$output/lane/out/native-builds" "$output/tmp"
output=$(cd "$output" && pwd)
export SCALING_OUTPUT=$output
cp "$here/run.sh" "$output/measure-run.sh"
cp "$here/report.rb" "$output/measure-report.rb"
export SCALING_DRIVER=$(jq -er .driver "$installed/toolchains/0.6.0-dev.8/toolchain.json")
[[ $SCALING_DRIVER == /* ]] || SCALING_DRIVER=$installed/toolchains/0.6.0-dev.8/$SCALING_DRIVER
export SCALING_DRIVER_SHA256=$(shasum -a 256 "$SCALING_DRIVER" | cut -d ' ' -f1)
git -C "$repo" rev-parse HEAD > "$output/source.sha"
git -C "$repo" diff HEAD -- loaves workspaces/oven scripts > "$output/source.patch"
ln -s "$repo" "$output/source"
ln -s "$repo" "$output/lane/incan"
experiment_home=$output/toolchain-home-for-isolated-single-process-measurements
cp -cR "$installed" "$experiment_home"
# The clone's compiler view is regenerable and must refer to this clone's Loaves.
if [[ -L $experiment_home/compiler-build/toolchains/0.6.0-dev.8/loaves ]]; then
    rm "$experiment_home/compiler-build/toolchains/0.6.0-dev.8/loaves"
fi
cp "$here/observe.rb" "$output/observe"
chmod +x "$output/observe"
clang -O2 -Wall -Wextra "$here/footprint.c" -o "$output/footprint"
manifest=$experiment_home/toolchains/0.6.0-dev.8/toolchain.json
jq --arg driver "$output/observe" '.driver=$driver' "$manifest" > "$manifest.next"
mv "$manifest.next" "$manifest"
cp -c "$support" "$output/lane/out/native-builds/toolchain-native"
export DEV8_LANE=$output/lane SELFBUILD_HOME=$experiment_home SELFBUILD_OVEN=$oven
export INCAN_HOME=$SELFBUILD_HOME INCAN_STDLIB=$output/source/loaves/stdlib
export SELFBUILD_STDLIB=$INCAN_STDLIB
export TMPDIR=$output/tmp OVEN_OWNER=off OVEN_TRACE=- INCAN_NATIVE_TIMING=1 RAYON_NUM_THREADS=2
unset INCAN_TRACE_DIR RUSTC_BOOTSTRAP
printf 'trial\twall_seconds\tstatus\n' > "$output/times.tsv"

admit() {
    local label=$1 load free disk
    while :; do
        { date -u +%FT%TZ; uptime; memory_pressure | tail -3; df -k "$output" | tail -1; } >> "$output/$label.host"
        load=$(LC_ALL=C uptime | awk -F 'load averages?: ' '{split($2, values, /[, ]+/); print values[1]}')
        free=$(memory_pressure | awk '/System-wide memory free percentage/ {gsub(/%/, "", $NF); print $NF}')
        disk=$(df -k "$output" | awk 'NR==2 {print $4}')
        if awk -v load="$load" -v free="$free" -v disk="$disk" 'BEGIN {exit !(load<=3 && free>=50 && disk>=58593750)}'; then return; fi
        echo "Waiting for quiet host and resource admission: $label" >&2
        sleep 30
    done
}

measure() {
    local label=$1 started finished code=0
    shift
    admit "$label"
    export SCALING_TRIAL=$label
    started=$(ruby -e 'puts Process.clock_gettime(Process::CLOCK_MONOTONIC)')
    "$@" > "$output/$label.log" 2>&1 || code=$?
    finished=$(ruby -e 'puts Process.clock_gettime(Process::CLOCK_MONOTONIC)')
    ruby -e 'puts [ARGV[0],ARGV[2].to_f-ARGV[1].to_f,ARGV[3]].join("\t")' "$label" "$started" "$finished" "$code" >> "$output/times.tsv"
    echo "$label: status $code"
    (( code == 0 )) || { tail -12 "$output/$label.log" >&2; return "$code"; }
}

build() {
    case $1 in
        lowering)
            "$oven" build "$output/source/loaves/toolchain/incan-rustc-driver" --profile release ;;
        data)
            SELFBUILD_PROFILE=release zsh "$output/source/scripts/build-native-stdlib.sh" data ;;
        oven) "$oven" build "$output/source/workspaces/oven" --profile release ;;
    esac
}

cd "$output/source"
for subject in ${SCALING_SUBJECTS:-lowering data oven}; do
    case $subject in
        lowering) file=loaves/compiler/incan_mir_lowering/src/lowering.incn
            stamp=loaves/toolchain/incan-rustc-driver/target/incan/loaves/incan_mir_lowering/release/.oven-inputs.sha256 ;;
        data) file=loaves/stdlib/data/src/datetime/civil/naive.incn
            stamp=$experiment_home/toolchains/0.6.0-dev.8/loaves/incan_stdlib_data/release/.oven-inputs.sha256 ;;
        oven) file=workspaces/oven/src/main.incn
            stamp=workspaces/oven/target/incan/release/bin/oven.oven-inputs ;;
        *) echo "Unknown subject: $subject" >&2; exit 2 ;;
    esac
    git diff --exit-code HEAD -- "$file" > "$output/$subject-source.diff"
    shasum -a 256 "$file" > "$output/$subject-before.sha256"
    # Cold means no retained output proof for the subject, with dependencies already built.
    [[ -z $stamp || ! -f $stamp ]] || rm "$stamp"
    measure "$subject-cold" build "$subject"
    [[ ${SCALING_COLD_ONLY:-0} != 1 ]] || continue
    measure "$subject-unchanged" build "$subject"
    patch -V none -p1 --batch < "$here/edits/$subject.patch" > "$output/$subject-edit.log"
    measure "$subject-edited" build "$subject"
    patch -V none -R -p1 --batch < "$here/edits/$subject.patch" >> "$output/$subject-edit.log"
    shasum -a 256 -c "$output/$subject-before.sha256" > "$output/$subject-restored.log"
done
ruby "$output/measure-report.rb" "$output"
