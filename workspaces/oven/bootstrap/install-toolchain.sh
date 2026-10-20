#!/bin/zsh
# Bootstrap: install the dev.8 toolchain in the bootstrap home (incan.toolchain/1).
# The driver is a copy of the Oven-built incan-rustc-driver, named by its digest so a reinstall never replaces a binary
# a running build uses; the closure is the bootstrap runtime's, minus the dev.7-generated standard components (they
# share names with the standard Loaves the dev.8 compiler builds; search reaches them).
set -e
LANE=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
B=$LANE/bootstrap
HOME_DIR=$LANE/dev8-home/toolchains/0.6.0-dev.8
BUILT=$LANE/incan/loaves/toolchain/incan-rustc-driver/target/incan/rust/debug/bin/incan-rustc-driver
RT=$B/loaves/incan_native_runtime/target/incan/rust/debug
STORE=/Volumes/Builds/dev8-fix/closure/closure-store/store/entries
SYSROOT=$(rustup run 1.98.0 rustc --print sysroot)
mkdir -p $HOME_DIR/bin $B/stdlib-available
DRIVER=$HOME_DIR/bin/incan-rustc-driver-$(shasum -a 256 $BUILT | cut -c1-12)
[ -f $DRIVER ] || cp $BUILT $DRIVER
# The standard crates' ABI cache, written by this driver (its layout version is the driver's).
mkdir -p $HOME_DIR/rust-abi
STD_ABI=$HOME_DIR/rust-abi/std-$(basename $DRIVER).rustabi
[ -f $STD_ABI ] || env -u RUSTC_BOOTSTRAP $DRIVER --rust-abi $SYSROOT $STD_ABI std core alloc
externs=()
search=()
for dir in $RT/*(/); do
    for artifact in $dir/lib*.rlib(N) $dir/lib*.dylib(N); do
        name=${${artifact:t:r}#lib}
        name=${name%-*}
        case $name in
            incan_stdlib_*) ;;
            *) externs+=("$(jq -n --arg n "$name" --arg p "$artifact" '{name: $n, path: $p}')") ;;
        esac
    done
    search+=("$dir")
done
for dir in $STORE/*/artifacts(/); do
    search+=("$dir")
done
jq -n --arg driver "$DRIVER" --arg sysroot "$SYSROOT" --arg std_abi "$STD_ABI" \
    --arg registry_units /Volumes/Builds/dev8-fix/closure/debug.json \
    --argjson externs "$(printf '%s\n' "${externs[@]}" | jq -s .)" \
    --argjson search "$(printf '%s\n' "${search[@]}" | jq -R . | jq -s .)" \
    '{schema: "incan.toolchain/1", driver: $driver, sysroot: $sysroot, rust_abi: [$std_abi], registry_units: $registry_units, externs: $externs, search: $search}' \
    > $HOME_DIR/toolchain.json.new
mv $HOME_DIR/toolchain.json.new $HOME_DIR/toolchain.json
jq -c '{driver, externs: [.externs[].name], search: (.search | length)}' $HOME_DIR/toolchain.json
