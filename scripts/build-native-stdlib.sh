#!/bin/zsh
# Compile standard package source into the selected installed toolchain, in the order supplied.
# Usage: DEV8_LANE=/path/to/lane scripts/build-native-stdlib.sh core testing interop ...
set -eu

lane=${DEV8_LANE:?set DEV8_LANE to the lane directory}
repo=$lane/incan
home_dir=${SELFBUILD_HOME:-$lane/dev8-home}
toolchain=$home_dir/toolchains/0.6.0-dev.8
builder=${SELFBUILD_NATIVE_BUILDER:-$lane/out/native-builds/toolchain-native}
profile=${SELFBUILD_PROFILE:-release}
[[ -x $builder ]] || { print -u2 'build native support first: scripts/build-native-support.sh'; exit 1; }
export INCAN_HOME=$home_dir INCAN_STDLIB=${SELFBUILD_STDLIB:-$repo/loaves/stdlib}

for package in "$@"; do
    project=$repo/loaves/stdlib/$package
    [[ -f $project/loaf.toml ]] || { print -u2 "unknown standard package: $package"; exit 2; }
    name=$(sed -nE 's/^name = "([^"]+)"/\1/p' "$project/loaf.toml" | head -1)
    output=$toolchain/loaves/$name/$profile
    mkdir -p "${output:h}"
    # Debug initially aliases release; a real debug producer must own a separate artifact set and stamp.
    if [[ -L $output ]]; then
        previous=$(mktemp -d "$output.previous.XXXXXX")
        rmdir "$previous"
        mv "$output" "$previous"
    fi
    # Oven retains the complete library key and stages replacement only when an input or output changed.
    "$builder" "$repo" "$package" "$profile" "$output"
    if [[ $profile == release ]]; then
        default=$toolchain/loaves/$name/debug
        # Supply a default only until debug has its own build; never evict another profile on an unchanged run.
        if [[ ! -e $default && ! -L $default ]]; then
            ln -s release "$default"
        fi
    fi
    if [[ -n ${SELFBUILD_ADMITTED_STDLIB:-} ]]; then
        print -r -- "$name" >> "$SELFBUILD_ADMITTED_STDLIB"
    fi
    shasum -a 256 "$output/lib$name.rlib"
done
