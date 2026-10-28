#!/bin/zsh
# Build and install checked runtime source, refusing a closure that imports a previous runtime.
# Usage: DEV8_LANE=/path/to/lane scripts/build-native-runtime.sh [--release]
# SELFBUILD_INSTALL_HOME may name a fresh destination distinct from the installed builder.
# SELFBUILD_SOURCE_INTERFACES may name build-native-interfaces.sh's checking-only source inventory.
set -eu

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
repo=$lane/incan
home_dir=${SELFBUILD_HOME:-$lane/dev8-home}
toolchain=$home_dir/toolchains/0.6.0-dev.8
installation=${SELFBUILD_INSTALL_HOME:-$home_dir}/toolchains/0.6.0-dev.8
[[ -f $installation/toolchain.json ]] || { print -u2 "missing destination toolchain: $installation"; exit 1; }
builder=${SELFBUILD_NATIVE_BUILDER:-$lane/out/native-builds/toolchain-native}
[[ -x $builder ]] || { print -u2 'build the native description helper first: scripts/build-native-support.sh --tool-only'; exit 1; }
profile=debug
if [[ ${1:-} == --release && $# == 1 ]]; then
    profile=release
elif (( $# != 0 )); then
    print -u2 'usage: build-native-runtime.sh [--release]'
    exit 2
fi

export INCAN_HOME=$home_dir INCAN_STDLIB=${SELFBUILD_STDLIB:-$repo/loaves/stdlib}
output=$repo/loaves/compiler/incan_native_runtime/target/incan/native/$profile
mkdir -p "$output"
unit=$output/incan_native_runtime.unit.json
manifest=$toolchain/toolchain.json
driver=$(jq -er '.driver' "$manifest")
[[ $driver == /* ]] || driver=$toolchain/$driver
sysroot=$(jq -er '.sysroot' "$manifest")
[[ $sysroot == /* ]] || sysroot=$toolchain/$sysroot

# The helper binds the installed support generation instead of rebuilding local Rust facets.
"$builder" "$repo" runtime "$profile" > "$unit.prepared"
jq --slurpfile interfaces "${SELFBUILD_SOURCE_INTERFACES:-/dev/null}" '
    .kind = "library" | .externs |= map(select(.name != "incan_native_runtime"
        and (.name | startswith("incan_stdlib_") | not)))
    | .loaves |= map(select(.name != "incan_native_runtime"))
    | if ($interfaces | length) > 0 then
        .loaves |= map(select(.standard | not))
        | .search = ([.externs[].path | sub("/[^/]+$"; "")] | unique)
      else . end
    | .loaves += ($interfaces[0] // [])' "$unit.prepared" > "$unit"
env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" incan_native_runtime \
    "$output/libincan_native_runtime.rlib" "$sysroot" --library > "$output/build.log" 2>&1 \
    || { tail -30 "$output/build.log"; exit 1; }

if ! jq -e 'all(.externs[]; .[0] != "incan_native_runtime"
    and (.[0] | startswith("incan_stdlib_") | not))' \
    "$output/incan_native_runtime.native.json" > /dev/null; then
    print -u2 'native runtime producer retained a prior runtime or compiled standard Loaf dependency'
    exit 1
fi
if [[ ${INCAN_RUNTIME_INSTALL:-1} == 0 ]]; then
    shasum -a 256 "$output/libincan_native_runtime.rlib"
    exit 0
fi

# Stage every runtime artifact before computing the generation identity.
mkdir -p "$installation/runtime"
staged=$(mktemp -d "$installation/runtime/.staged.XXXXXX")
temporary=
trap '[[ -z ${temporary:-} ]] || rm -f "$temporary"; [[ -z ${staged:-} ]] || rm -rf "$staged"' EXIT
artifacts=(libincan_native_runtime.rlib incan_native_runtime.incnlib incan_native_runtime.incnbody \
    incan_native_runtime.rustabi)
[[ ! -f $output/incan_native_runtime.incngraph ]] || artifacts+=(incan_native_runtime.incngraph)
for artifact in $artifacts; do
    cp "$output/$artifact" "$staged/$artifact"
done
# Native closure paths belong to the producer directory; moving the sidecar must retain those exact artifacts.
jq --arg directory "$output" '
    def absolute: if startswith("/") then . else $directory + "/" + . end;
    .externs |= map(.[1] |= absolute)
    | .macro_externs |= map(.[1] |= absolute)
    | .search |= map(absolute)' "$output/incan_native_runtime.native.json" \
    > "$staged/incan_native_runtime.native.json"
# Names and bytes both contribute; unchanged machine code cannot hide changed governed bodies or native facts.
(cd "$staged" && shasum -a 256 $artifacts incan_native_runtime.native.json) > "$staged/artifacts.sha256"
digest=$(shasum -a 256 "$staged/artifacts.sha256" | cut -d ' ' -f1)
installed=$installation/runtime/$digest
if [[ -e $installed ]]; then
    for artifact in $artifacts incan_native_runtime.native.json artifacts.sha256; do
        cmp "$staged/$artifact" "$installed/$artifact" \
            || { print -u2 "runtime generation $digest has conflicting $artifact bytes"; exit 1; }
    done
else
    mv "$staged" "$installed"
    staged=
fi
manifest=$installation/toolchain.json
temporary=$(mktemp "$installation/toolchain.json.XXXXXX")
jq --arg runtime "$installed/libincan_native_runtime.rlib" --arg directory "$installed" '
    [.externs[] | select(.name == "incan_native_runtime") | .path | sub("/[^/]+$"; "")] as $previous
    | .externs = ([.externs[] | select(.name != "incan_native_runtime")]
        + [{name: "incan_native_runtime", path: $runtime}])
    | .search = (([.search[] | select(. as $path | $previous | index($path) | not)] + [$directory]) | unique)' \
    "$manifest" > "$temporary"
mv "$temporary" "$manifest"
temporary=
shasum -a 256 "$installed/libincan_native_runtime.rlib"
