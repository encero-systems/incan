#!/bin/zsh
# Check source interfaces for the standard declarations the runtime needs before compiled providers exist.
# These are checking inputs, outside the installed Loaf tree; no native library or governed body is fabricated.
set -eu

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
repo=$lane/incan
home_dir=${SELFBUILD_HOME:?set SELFBUILD_HOME to a fresh toolchain home}
toolchain=$home_dir/toolchains/0.6.0-dev.8
builder=${SELFBUILD_NATIVE_BUILDER:-$lane/out/native-builds/toolchain-native}
[[ -x $builder ]] || { print -u2 'build the native description helper first: scripts/build-native-support.sh --tool-only'; exit 1; }
export INCAN_HOME=$home_dir INCAN_STDLIB=${SELFBUILD_STDLIB:-$repo/loaves/stdlib}
driver=$(jq -er '.driver' "$toolchain/toolchain.json")
[[ $driver == /* ]] || driver=$toolchain/$driver
sysroot=$(jq -er '.sysroot' "$toolchain/toolchain.json")
[[ $sysroot == /* ]] || sysroot=$toolchain/$sysroot
interfaces=$home_dir/source-interfaces
mkdir -p "$interfaces"
print '[]' > "$interfaces/loaves.json"

for package in core async; do
    project=$repo/loaves/stdlib/$package
    name=$(sed -nE 's/^name = "([^"]+)"/\1/p' "$project/loaf.toml" | head -1)
    directory=$(mktemp -d "$interfaces/$package.XXXXXX")
    unit=$directory/unit.json
    # Bind the installed support generation before injecting checking-only source interfaces.
    SELFBUILD_LIBRARY_OUTPUT= "$builder" "$repo" "$package" release > "$unit.prepared"
    jq --slurpfile interfaces "$interfaces/loaves.json" \
        '.kind = "library" | .loaves += $interfaces[0]' "$unit.prepared" > "$unit"
    if ! "$driver" --source-interface "$unit" "$sysroot" "$directory" > "$directory/build.log" 2>&1; then
        tail -10 "$directory/build.log" | cut -c1-600
        print -u2 "source interface $name did not check; see $directory/build.log"
        exit 1
    fi
    jq --arg name "$name" --arg directory "$directory" \
        '. + [{key:$name, name:$name, dir:$directory, standard:true}]' \
        "$interfaces/loaves.json" > "$interfaces/loaves.json.next"
    mv "$interfaces/loaves.json.next" "$interfaces/loaves.json"
    shasum -a 256 "$directory/$name.incnlib"
done
