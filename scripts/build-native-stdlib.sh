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
driver=$(jq -er '.driver' "$toolchain/toolchain.json")
[[ $driver == /* ]] || driver=$toolchain/$driver
sysroot=$(jq -er '.sysroot' "$toolchain/toolchain.json")
[[ $sysroot == /* ]] || sysroot=$toolchain/$sysroot

for package in "$@"; do
    project=$repo/loaves/stdlib/$package
    [[ -f $project/loaf.toml ]] || { print -u2 "unknown standard package: $package"; exit 2; }
    name=$(sed -nE 's/^name = "([^"]+)"/\1/p' "$project/loaf.toml" | head -1)
    output=$toolchain/loaves/$name/$profile
    mkdir -p "${output:h}"
    staged=$(mktemp -d "$output.XXXXXX")
    unit=$staged/$name.unit.json
    SELFBUILD_LIBRARY_OUTPUT=$staged "$builder" "$repo" "$package" "$profile" > "$unit"
    env -u RUSTC_BOOTSTRAP "$driver" --unit "$unit" "$name" "$staged/lib$name.rlib" "$sysroot" --library \
        > "$staged/build.log" 2>&1 || { tail -20 "$staged/build.log" | cut -c1-600; exit 1; }
    # Preserve the current package until its complete replacement is available to dependent source units.
    previous=
    if [[ -e $output || -L $output ]]; then
        previous=$(mktemp -d "$output.previous.XXXXXX")
        rmdir "$previous"
        mv "$output" "$previous"
    fi
    mv "$staged" "$output"
    [[ -z $previous ]] || rm -rf -- "$previous"
    if [[ $profile == release ]]; then
        default=$toolchain/loaves/$name/debug
        previous=
        if [[ -e $default || -L $default ]]; then
            previous=$(mktemp -d "$default.previous.XXXXXX")
            rmdir "$previous"
            mv "$default" "$previous"
        fi
        ln -s release "$default"
        [[ -z $previous ]] || rm -rf -- "$previous"
    fi
    if [[ -n ${SELFBUILD_ADMITTED_STDLIB:-} ]]; then
        print -r -- "$name" >> "$SELFBUILD_ADMITTED_STDLIB"
    fi
    shasum -a 256 "$output/lib$name.rlib"
done
