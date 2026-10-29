#!/bin/sh
# Usage: INCAN_HOME=SOURCE_HOME verify_std_reproducibility.sh OVEN CONTRACTS REPOSITORY NEW_SCRATCH_DIRECTORY
# Build standard Incan Loaves twice without package artifacts; retain byte digests and every contract outcome.
set -eu
oven=$1
contracts=$2
repo=$3
scratch=$4
source=${INCAN_HOME:?set INCAN_HOME to the built toolchain}
source_home=$(cd "$source" && pwd -P)
source=$(cd "$source/toolchains/0.6.0-dev.8" && pwd -P)
mkdir "$scratch"
scratch=$(cd "$scratch" && pwd -P)
export INCAN_STDLIB=$repo/loaves/stdlib
status=0

# Pin one native closure for both rounds even if another build installs a newer toolchain meanwhile.
jq --arg source "$source" '
    def located: if startswith("/") then . else $source + "/" + . end;
    .driver |= located | .sysroot |= located | .registry_units |= located |
    .rust_abi |= map(located) | .externs |= map(.path |= located) | .search |= map(located)
' "$source/toolchain.json" > "$scratch/toolchain.json"

for round in first second; do
    export INCAN_HOME=$scratch/$round
    toolchain=$INCAN_HOME/toolchains/0.6.0-dev.8
    logs=$INCAN_HOME/logs
    mkdir -p "$toolchain" "$logs"
    cp "$scratch/toolchain.json" "$toolchain/toolchain.json"
    driver=$(jq -r .driver "$toolchain/toolchain.json")
    sysroot=$(jq -r .sysroot "$toolchain/toolchain.json")
    jq -r '.driver,.rust_abi[],.externs[].path' "$toolchain/toolchain.json" |
        LC_ALL=C sort -u | while IFS= read -r artifact; do
            shasum -a 256 "$artifact"
        done > "$INCAN_HOME/native-inputs.sha256"
    for package in core testing interop async system codecs compression data observability web; do
        root=$repo/loaves/stdlib/$package
        name=$(sed -n 's/^name = "\([^"]*\)"/\1/p' "$root/loaf.toml" | head -1)
        directory=$toolchain/loaves/$name/debug
        mkdir -p "$directory"
        unit=$logs/$package.unit.json
        "$oven" describe-unit "$root/src/lib.incn" > "$unit.full" 2> "$logs/$package.describe.log"
        # Bootstrap exposes only manifests that an earlier package in this round actually published.
        jq -r '.loaves[] | select(.standard) | [.name,.dir] | @tsv' "$unit.full" |
            while IFS="$(printf '\t')" read -r identity artifact_dir; do
                if test -f "$artifact_dir/$identity.incnlib"; then
                    printf '%s\n' "$identity"
                fi
            done > "$logs/available"
        jq --rawfile available "$logs/available" '
            ($available | split("\n")) as $keep |
            .loaves |= map(select((.standard | not) or (.name as $name | $keep | index($name))))
        ' "$unit.full" > "$unit"
        if ! env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" "$name" "$directory/lib$name.rlib" \
            "$sysroot" --library > "$logs/$package.log" 2>&1; then
            echo "$round/$package did not compile; see $logs/$package.log" >&2
            exit 1
        fi
        echo "$round/$package compiled"
    done
    (
        cd "$toolchain/loaves"
        find . -type f -print | LC_ALL=C sort | while IFS= read -r artifact; do
            shasum -a 256 "$artifact"
        done
    ) > "$INCAN_HOME/artifacts.sha256"
done

for evidence in artifacts native-inputs; do
    if ! diff -u "$scratch/first/$evidence.sha256" "$scratch/second/$evidence.sha256" \
        > "$scratch/$evidence.diff"; then
        echo "standard $evidence differ; see $scratch/$evidence.diff" >&2
        status=1
    fi
done

for round in first second; do
    export INCAN_HOME=$scratch/$round
    # Partition hints hold case names only. Both rounds still compile their programs and execute every case.
    hints=$source_home/cache/contract-groups-v1
    if test "$round" = second; then
        hints=$scratch/first/cache/contract-groups-v1
    fi
    if test -d "$hints"; then
        mkdir -p "$INCAN_HOME/cache"
        cp -R "$hints" "$INCAN_HOME/cache/contract-groups-v1"
    fi
    if ! "$contracts" "$oven" "$repo/workspaces/contracts/fixtures" \
        --scratch "$INCAN_HOME/contracts" --jobs 2 \
        behavior check library interop testing project format graph > "$INCAN_HOME/contracts.log" 2>&1; then
        status=1
    fi
    sed -n '/^PASS /p; /^FAIL /p; /^PEND /p' "$INCAN_HOME/contracts.log" |
        LC_ALL=C sort > "$INCAN_HOME/outcomes.txt"
    tail -2 "$INCAN_HOME/contracts.log"
done

if ! diff -u "$scratch/first/outcomes.txt" "$scratch/second/outcomes.txt" > "$scratch/outcomes.diff"; then
    echo "contract outcomes differ; see $scratch/outcomes.diff" >&2
    status=1
fi
exit "$status"
