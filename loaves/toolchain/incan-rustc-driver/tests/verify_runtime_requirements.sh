#!/bin/zsh
# Exercise runtime closure selection through the same direct-driver boundary as verify_identity.sh.
# Usage: verify_runtime_requirements.sh DRIVER SYSROOT NATIVE_BUILDER SOURCE_ROOT TEMPLATE_UNIT NEW_SCRATCH
# TEMPLATE_UNIT supplies the installed native support externs; its governed runtime Loaf must be present.
# No supplied toolchain, source fixture, or template is modified. Keep the scratch directory as the receipt.
set -eu
(( $# == 6 )) || { print -u2 'expected DRIVER SYSROOT NATIVE_BUILDER SOURCE_ROOT TEMPLATE_UNIT NEW_SCRATCH'; exit 2; }
driver=$1
sysroot=$2
builder=$3
source_root=$4
template=$5
scratch=$6
fixtures=${0:A:h}/fixtures/runtime_dependencies
mkdir "$scratch"
jq -e '.loaves | any(.name == "incan_native_runtime")' "$template" > /dev/null

# Every case is a fresh process. Retain the runtime provider but remove every ordinary std Loaf; native support
# remains unchanged. This isolates declaration reachability rather than pretending the native foundation is absent.
for name in empty identity bytes_loop hex; do
    project=$scratch/$name
    mkdir -p "$project/src"
    cp "$fixtures/$name.incn" "$project/src/lib.incn"
    unit=$project/unit.json
    jq --arg project "$project" --arg name "runtime_requirement_$name" '
        .project_root=$project | .source_root=($project+"/src") | .source=($project+"/src/lib.incn") |
        .package=$name | .loaves |= map(select(.name=="incan_native_runtime"))
    ' "$template" > "$unit"
    "$builder" "$source_root" identify "$unit" > "$project/identify.log" 2>&1
    flags=("${(@f)$(<"$unit.arguments")}")
    env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" "runtime_requirement_$name" \
        "$project/libruntime_requirement_$name.rlib" "$sysroot" --library "${flags[@]}" \
        > "$project/build.stdout" 2> "$project/build.stderr"
    test -s "$project/libruntime_requirement_$name.rlib"
    print -r -- "PASS runtime requirement: $name without ordinary core"
done

# A separate consumer must keep the same minimal closure; success cannot depend on retained compiler state.
project=$scratch/consumer
mkdir -p "$project/src"
cp "$fixtures/consumer.incn" "$project/src/main.incn"
unit=$project/unit.json
jq --arg project "$project" --arg provider "$scratch/hex" '
    .project_root=$project | .source_root=($project+"/src") | .source=($project+"/src/main.incn") |
    .package="runtime_requirement_consumer" | .kind="program" |
    .loaves |= map(select(.name=="incan_native_runtime")) |
    .loaves += [{dir:$provider,key:"runtime_requirement_hex",name:"runtime_requirement_hex",standard:false,toolchain_provided:false}] |
    .externs += [{name:"runtime_requirement_hex",path:($provider+"/libruntime_requirement_hex.rlib")}] |
    .search += [$provider]
' "$template" > "$unit"
"$builder" "$source_root" identify "$unit" > "$project/identify.log" 2>&1
flags=("${(@f)$(<"$unit.arguments")}")
env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" runtime_requirement_consumer "$project/consumer" "$sysroot" "${flags[@]}" \
    > "$project/build.stdout" 2> "$project/build.stderr"
"$project/consumer" > "$project/run.stdout" 2> "$project/run.stderr"
grep -qx 'core-free-consumer-ok' "$project/run.stdout"
print -r -- 'PASS runtime requirement: separately compiled core-free consumer'
