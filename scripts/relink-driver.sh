#!/bin/zsh
# Rebuild a dev.8 bootstrap lane's driver with its installed Oven and optional optimized local Rust units.
# Usage: DEV8_LANE=/path/to/lane scripts/relink-driver.sh [--lowering] [--release] [--refresh-build-toolchain]
# INCAN_DRIVER_INSTALL=0 builds without replacing the lane's installed driver, for baseline comparisons.
set -e
set -o pipefail
zmodload zsh/datetime
started=$EPOCHREALTIME

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
repo=$lane/incan
bootstrap=$lane/bootstrap
toolchain=$lane/dev8-home/toolchains/0.6.0-dev.8
profile=debug
refresh=false
refresh_toolchain=false

for argument in "$@"; do
    case $argument in
        --lowering) refresh=true ;;
        --release) profile=release ;;
        --refresh-build-toolchain) refresh_toolchain=true ;;
        *) print -u2 "unknown option: $argument"; exit 2 ;;
    esac
done

if $refresh; then
    zsh "$repo/scripts/refresh-lowering.sh"
fi
emitted=$EPOCHREALTIME

# Build with a pinned compiler rather than the output of the previous relink. Otherwise installing a changed
# driver invalidates every Rust library on the next build even though none of their sources changed.
build_home=$bootstrap/driver-build-home
build_toolchain=$build_home/toolchains/0.6.0-dev.8
mkdir -p "$build_toolchain"
if $refresh_toolchain || [[ ! -f $build_toolchain/toolchain.json ]]; then
    # Toolchain paths can be relative to their manifest. Keep their meaning when copying only the description.
    jq --arg base "$toolchain/" '
        def absolute: if startswith("/") then . else $base + . end;
        .driver |= absolute | .sysroot |= absolute | .rust_abi |= map(absolute) |
        .externs |= map(.path |= absolute) | .search |= map(absolute) |
        if (.registry_units // "") != "" then .registry_units |= absolute else . end
    ' "$toolchain/toolchain.json" > "$build_toolchain/toolchain.json.new"
    mv "$build_toolchain/toolchain.json.new" "$build_toolchain/toolchain.json"
fi
sysroot=$(jq -er '.sysroot' "$build_toolchain/toolchain.json")
export RUSTC=$sysroot/bin/rustc INCAN_STDLIB=$repo/loaves/stdlib INCAN_HOME=$build_home
prepared=$EPOCHREALTIME
build_log=${INCAN_RELINK_LOG:-$bootstrap/driver-build-$profile.log}

if ! (cd "$repo" && "$lane/oven-bin/incan" build loaves/toolchain/incan-rustc-driver \
    --profile "$profile" --substitute "incan_mir_lowering=$bootstrap/loaves/incan_mir_lowering" \
    2>&1 | while IFS= read -r line; do
        if [[ ${INCAN_RELINK_TRACE:-0} == 1 ]]; then
            print -r -- "$EPOCHREALTIME $line"
        else
            print -r -- "$line"
        fi
    done > "$build_log"); then
    tail -40 "$build_log"
    exit 1
fi
compiled=$EPOCHREALTIME

built=$repo/loaves/toolchain/incan-rustc-driver/target/incan/rust/$profile/bin/incan-rustc-driver
[[ -f $built ]] || { print -u2 "missing $profile driver: $built"; exit 1; }
print -r -- "$profile driver built: $built"
[[ ${INCAN_DRIVER_INSTALL:-1} == 0 ]] && exit 0

# Replace only the driver and its ABI cache; the lane's selected runtime roots and native search paths stay authoritative.
mkdir -p "$toolchain/bin" "$toolchain/rust-abi"
digest=$(shasum -a 256 "$built" | cut -d ' ' -f1)
driver=$toolchain/bin/incan-rustc-driver-${digest[1,12]}
[[ -f $driver ]] || cp "$built" "$driver"
installed=$EPOCHREALTIME
abi=$toolchain/rust-abi/std-${driver:t}.rustabi
[[ -f $abi ]] || env -u RUSTC_BOOTSTRAP "$driver" --rust-abi "$sysroot" "$abi" std core alloc
cached=$EPOCHREALTIME

manifest=$(mktemp "$toolchain/toolchain.json.XXXXXX")
trap '[[ -n ${manifest:-} ]] && rm -f "$manifest"' EXIT
jq --arg driver "$driver" --arg abi "$abi" '.driver = $driver | .rust_abi = [$abi]' \
    "$toolchain/toolchain.json" > "$manifest"
mv "$manifest" "$toolchain/toolchain.json"
manifest=
print -r -- "$profile driver installed: $driver"
if [[ ${INCAN_RELINK_TRACE:-0} == 1 ]]; then
    print -rl -- "relink timing emission: $((emitted-started))" \
        "relink timing setup: $((prepared-emitted))" "relink timing compilation: $((compiled-prepared))" \
        "relink timing install: $((installed-compiled))" "relink timing abi: $((cached-installed))" \
        "relink timing manifest: $((EPOCHREALTIME-cached))" "relink timing total: $((EPOCHREALTIME-started))"
fi
