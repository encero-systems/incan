#!/bin/zsh
# Bootstrap: link incan-rustc-driver through Oven against the bootstrap lowering, then reinstall the toolchain.
# Pass --lowering to re-emit the lowering first (after changes to loaves/compiler/incan_mir_lowering).
set -e
LANE=${DEV8_LANE:?set DEV8_LANE to the bootstrap lane directory}
B=$LANE/bootstrap
REPO=$LANE/incan
[ "${1:-}" = "--lowering" ] && $B/scripts/refresh-lowering.sh
export RUSTC=$(rustup which --toolchain 1.98.0 rustc) INCAN_STDLIB=$REPO/loaves/stdlib INCAN_HOME=$LANE/dev8-home
(cd $REPO && $LANE/oven-bin/incan build loaves/toolchain/incan-rustc-driver \
    --substitute incan_mir_lowering=$B/loaves/incan_mir_lowering > $B/driver-build.log 2>&1) \
    || { grep -A12 "^error" $B/driver-build.log | head -40; exit 1; }
$B/scripts/install-toolchain.sh | cut -c1-110
