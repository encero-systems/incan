#!/bin/zsh
# Build the experiment through matching native tools and an isolated, coherent compiler build home.
set -eu
oven=${1:?usage: build.sh OVEN SPIKE_HOME}
spike_home=${2:?missing isolated compiler build home}
root=$(cd "$(dirname "$0")/../../.." && pwd)
toolchain=$spike_home/toolchains/0.6.0-dev.8
[[ -f $toolchain/toolchain.json ]] || { print -u2 'missing spike toolchain manifest'; exit 2; }
sysroot=$(jq -er '.sysroot' "$toolchain/toolchain.json")
[[ $sysroot == /* ]] || sysroot=$toolchain/$sysroot
env -u RUSTC_BOOTSTRAP -u INCAN_SOURCE_ROOT \
    INCAN_HOME="$spike_home" INCAN_STDLIB="$root/loaves/stdlib" RUSTC="$sysroot/bin/rustc" OVEN_OWNER=off \
    "$oven" build "$root/loaves/toolchain/incan-rustc-driver" --profile release
