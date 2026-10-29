#!/usr/bin/env bash
# Resolve the installed CLI and the Rust compiler selected by its toolchain manifest.
benchmark_toolchain() {
    INCAN="${INCAN:-$(command -v incan || true)}"
    [[ -x "$INCAN" ]] || { echo 'Set INCAN to an installed incan executable' >&2; return 2; }
    local selected_home="${INCAN_HOME:-$HOME/.incan}"
    TOOLCHAIN_NAME=0.6.0-dev.8
    local root="$selected_home/toolchains/$TOOLCHAIN_NAME"
    local manifest="$root/toolchain.json" sysroot
    [[ -f "$manifest" ]] || { echo "Missing installed toolchain manifest: $manifest" >&2; return 2; }
    sysroot=$(jq -er '.sysroot' "$manifest")
    DRIVER=$(jq -er '.driver' "$manifest")
    [[ "$sysroot" == /* ]] || sysroot="$root/$sysroot"
    [[ "$DRIVER" == /* ]] || DRIVER="$root/$DRIVER"
    PINNED_RUSTC="$sysroot/bin/rustc"
    [[ -x "$PINNED_RUSTC" && -x "$DRIVER" ]] || { echo 'Installed toolchain is incomplete' >&2; return 2; }
    mkdir -p "${BENCH_SCRATCH:-$SCRIPT_DIR/target}"
}
