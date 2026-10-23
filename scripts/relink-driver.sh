#!/bin/zsh
# Rebuild a dev.8 bootstrap lane's driver with its installed Oven and optional optimized local Rust units.
# Usage: DEV8_LANE=/path/to/lane scripts/relink-driver.sh [--lowering] [--release]
# INCAN_DRIVER_INSTALL=0 builds without replacing the lane's installed driver, for baseline comparisons.
set -e

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
repo=$lane/incan
bootstrap=$lane/bootstrap
toolchain=$lane/dev8-home/toolchains/0.6.0-dev.8
profile=debug
refresh=false

for argument in "$@"; do
    case $argument in
        --lowering) refresh=true ;;
        --release) profile=release ;;
        *) print -u2 "unknown option: $argument"; exit 2 ;;
    esac
done

if $refresh; then
    $bootstrap/scripts/refresh-lowering.sh
fi

sysroot=$(jq -er '.sysroot' "$toolchain/toolchain.json")
export RUSTC=$sysroot/bin/rustc INCAN_STDLIB=$repo/loaves/stdlib INCAN_HOME=$lane/dev8-home

if ! (cd "$repo" && "$lane/oven-bin/incan" build loaves/toolchain/incan-rustc-driver \
    --profile "$profile" --substitute "incan_mir_lowering=$bootstrap/loaves/incan_mir_lowering" \
    > "$bootstrap/driver-build-$profile.log" 2>&1); then
    tail -40 "$bootstrap/driver-build-$profile.log"
    exit 1
fi

built=$repo/loaves/toolchain/incan-rustc-driver/target/incan/rust/$profile/bin/incan-rustc-driver
[[ -f $built ]] || { print -u2 "missing $profile driver: $built"; exit 1; }
print -r -- "$profile driver built: $built"
[[ ${INCAN_DRIVER_INSTALL:-1} == 0 ]] && exit 0

# Replace only the driver and its ABI cache; the lane's selected runtime roots and native search paths stay authoritative.
mkdir -p "$toolchain/bin" "$toolchain/rust-abi"
digest=$(shasum -a 256 "$built" | cut -d ' ' -f1)
driver=$toolchain/bin/incan-rustc-driver-${digest[1,12]}
[[ -f $driver ]] || cp "$built" "$driver"
abi=$toolchain/rust-abi/std-${driver:t}.rustabi
[[ -f $abi ]] || env -u RUSTC_BOOTSTRAP "$driver" --rust-abi "$sysroot" "$abi" std core alloc

manifest=$(mktemp "$toolchain/toolchain.json.XXXXXX")
trap '[[ -n ${manifest:-} ]] && rm -f "$manifest"' EXIT
jq --arg driver "$driver" --arg abi "$abi" '.driver = $driver | .rust_abi = [$abi]' \
    "$toolchain/toolchain.json" > "$manifest"
mv "$manifest" "$toolchain/toolchain.json"
manifest=
print -r -- "$profile driver installed: $driver"
