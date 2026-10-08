#!/bin/bash
# usage: native.sh <program.incn>  -- run the baked native driver's frontend+lowering on one program
export RUSTUP_TOOLCHAIN=1.98.0 INCAN_NO_BANNER=1 CARGO_BIN_EXE_incan=/root/incan-target/debug/incan
export INCAN_INTERNAL_SDK_PROVIDER_STORE=/root/incan-target/incan_test_sdk_provider_store
export INCAN_HOME=/root/incan-target/incan_test_oven_home
export INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE=/root/incan-target/incan_test_sdk_provider_path
export INCAN_SDK_INVENTORY="$(cat /root/incan-target/incan_test_sdk_provider_path)/sdk-inventory.json"
unset RUSTC_BOOTSTRAP
timeout 300 /root/incan-target/oven-explicit-bake-workspace/sha256-1e3465c33e8633d0ffe11778e8034bdf346781ea33f914986b3e6461e254e08b/loaves__compiler__incan_driver__tests__native_driver_project_tests/driver/target/rust/release/incan-rustc-driver --source "$1" native_corpus "${1%.incn}-native" /root/.rustup/toolchains/1.98.0-x86_64-unknown-linux-gnu 2>&1 | tail -4
