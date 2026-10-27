#!/bin/zsh
# Rebuild a dev.8 lane's driver and Incan lowering with its installed toolchain.
# Usage: DEV8_LANE=/path/to/lane scripts/relink-driver.sh [--lowering] [--release]
# INCAN_DRIVER_INSTALL=0 builds without replacing the lane's installed driver, for baseline comparisons.
set -e
set -o pipefail
zmodload zsh/datetime
started=$EPOCHREALTIME

lane=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
repo=$lane/incan
bootstrap=$lane/bootstrap
toolchain_home=${SELFBUILD_HOME:-$lane/dev8-home}
toolchain=$toolchain_home/toolchains/0.6.0-dev.8
builder=${SELFBUILD_OVEN:-$lane/oven-bin/incan}
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
    # Withdraw only the generated library stamp; Oven recompiles the lowering through the installed driver.
    stamp=$repo/loaves/toolchain/incan-rustc-driver/target/incan/loaves/incan_mir_lowering/$profile/.oven-inputs.sha256
    [[ ! -f $stamp ]] || rm "$stamp"
fi
emitted=$EPOCHREALTIME

sysroot=$(jq -er '.sysroot' "$toolchain/toolchain.json")
[[ $sysroot == /* ]] || sysroot=$toolchain/$sysroot
# Compiler source owns incan_lang; the installed runtime/std generation remains the authority for every other root.
compiler_home=$toolchain_home/compiler-build
zsh "$repo/scripts/prepare-compiler-home.sh" "$toolchain_home" "$compiler_home"
export RUSTC=$sysroot/bin/rustc INCAN_STDLIB=${SELFBUILD_STDLIB:-$repo/loaves/stdlib} INCAN_HOME=$compiler_home
prepared=$EPOCHREALTIME
build_log=${INCAN_RELINK_LOG:-$bootstrap/driver-build-$profile.log}

if ! (cd "$repo" && OVEN_TRACE=- "$builder" build loaves/toolchain/incan-rustc-driver \
    --profile "$profile" \
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
