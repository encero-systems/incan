#!/bin/sh
# Rebuild and check every step of the #1337 front-end spike with plain rustc on the pinned toolchain. No Cargo.
# Needs: rustup component add rustc-dev --toolchain 1.98.0
set -eu

toolchain=1.98.0
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
out=$(mktemp -d "${TMPDIR:-/tmp}/incan-1337-spike.XXXXXX")
trap 'rm -rf "$out"' EXIT INT TERM

sysroot=$(rustc +"$toolchain" --print sysroot)
export DYLD_LIBRARY_PATH="$sysroot/lib" LD_LIBRARY_PATH="$sysroot/lib"

build_driver() {
  RUSTC_BOOTSTRAP=1 rustc +"$toolchain" --edition 2024 -A warnings "$here/$1.rs" -o "$out/$1"
}

compile_with() {
  "$out/$1" --sysroot "$sysroot" --edition 2024 --cap-lints allow "$here/$2.rs" -o "$out/$2" 2>"$out/$2.log"
}

expect_output() {
  actual=$("$out/$1")
  if [ "$actual" != "$2" ]; then
    printf 'FAIL %s: expected "%s", got "%s"\n' "$1" "$2" "$actual" >&2
    exit 1
  fi
  printf 'ok   %s -> %s\n' "$1" "$actual"
}

build_driver step0_driver
compile_with step0_driver step0_program
expect_output step0_program "hello from a program compiled by the Incan driver"

build_driver step2_mir_body
compile_with step2_mir_body step2_program
expect_output step2_program "answer() = 42"

build_driver step3_injected_declaration
compile_with step3_injected_declaration step3_program
expect_output step3_program "answer() = 42"

build_driver step4_model_adt
compile_with step4_model_adt step4_program
expect_output step4_program "7 5 10"
