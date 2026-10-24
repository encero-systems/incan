#!/bin/zsh
# Build and install checked runtime source, refusing a closure that imports a previous runtime.
# Usage: DEV8_LANE=/path/to/lane scripts/build-native-runtime.sh [--release]
set -eu

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
repo=$lane/incan
home_dir=${SELFBUILD_HOME:-$lane/dev8-home}
toolchain=$home_dir/toolchains/0.6.0-dev.8
builder=${SELFBUILD_OVEN:-$lane/oven-bin/incan}
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

"$builder" describe-unit "$repo/loaves/compiler/incan_native_runtime/src/lib.incn" --profile "$profile" > "$unit.prepared"
jq '.kind = "library" | .externs |= map(select(.name != "incan_native_runtime"))
    | .loaves |= map(select(.name != "incan_native_runtime"))' "$unit.prepared" > "$unit"
env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" incan_native_runtime \
    "$output/libincan_native_runtime.rlib" "$sysroot" --library > "$output/build.log" 2>&1 \
    || { tail -30 "$output/build.log"; exit 1; }

if ! jq -e 'all(.externs[]; .[0] != "incan_native_runtime")' \
    "$output/incan_native_runtime.native.json" > /dev/null; then
    print -u2 'native runtime producer retained a prior runtime dependency'
    exit 1
fi
if [[ ${INCAN_RUNTIME_INSTALL:-1} == 0 ]]; then
    shasum -a 256 "$output/libincan_native_runtime.rlib"
    exit 0
fi

# Published generations stay immutable, including the governed facade beside the library.
digest=$(shasum -a 256 "$output/libincan_native_runtime.rlib" | cut -d ' ' -f1)
installed=$toolchain/runtime/$digest
mkdir -p "$installed"
for artifact in libincan_native_runtime.rlib incan_native_runtime.incnlib incan_native_runtime.incnbody incan_native_runtime.native.json; do
    cp "$output/$artifact" "$installed/$artifact"
done
temporary=$(mktemp "$toolchain/toolchain.json.XXXXXX")
trap '[[ -z ${temporary:-} ]] || rm -f "$temporary"' EXIT
jq --arg runtime "$installed/libincan_native_runtime.rlib" --arg directory "$installed" '
    .externs |= map(if .name == "incan_native_runtime" then .path = $runtime else . end)
    | .search = ((.search + [$directory]) | unique)' "$manifest" > "$temporary"
mv "$temporary" "$manifest"
temporary=
shasum -a 256 "$installed/libincan_native_runtime.rlib"
