#!/bin/zsh
# Bootstrap: re-emit incan_native_runtime's Rust from the dev.8 tree with the patched dev.7, rebuild its Rust Loaf
# through Oven, and reinstall the toolchain description (whose closure is the runtime's).
set -e
LANE=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
B=$LANE/bootstrap
REPO=$LANE/incan
S=$B/ambient2/incan_native_runtime/src
rm -rf "${S:?}"
cp -R $REPO/loaves/compiler/incan_native_runtime/src $S
rm -rf "$B/ambient2/incan_native_runtime/target/lib"
(cd $B/ambient2/incan_native_runtime && $B/scripts/stage0-boot.sh build --lib > $B/boot-runtime.log 2>&1) || true
errors=$(sed 's/\x1b\[[0-9;]*m//g' $B/boot-runtime.log | grep -c "^type error\|^syntax error" || true)
if [ "$errors" != "0" ] || [ ! -f $B/ambient2/incan_native_runtime/target/lib/src/lib.rs ]; then
    echo "emission failed ($errors errors); see $B/boot-runtime.log"
    exit 1
fi
R=$B/loaves/incan_native_runtime
rm -rf "${R:?}/src" "${R:?}/target"
cp -R $B/ambient2/incan_native_runtime/target/lib/src $R/src
sed -i '' 's/str_starts_with(actual, &prefix)/str_starts_with(\&actual, \&prefix)/' $R/src/raises_assertions.rs
export RUSTC=$(rustup which --toolchain 1.98.0 rustc) INCAN_STDLIB=$REPO/loaves/stdlib INCAN_HOME=$LANE/dev8-home
$LANE/oven-bin/incan build $R > $B/runtime-build.log 2>&1 \
    || { grep -A12 "^error" $B/runtime-build.log | head -40; exit 1; }
$B/scripts/install-toolchain.sh
