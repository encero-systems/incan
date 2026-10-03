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
  name=$1 expected=$2
  shift 2
  actual=$("$out/$name" "$@")
  if [ "$actual" != "$expected" ]; then
    printf 'FAIL %s: expected "%s", got "%s"\n' "$name" "$expected" "$actual" >&2
    exit 1
  fi
  printf 'ok   %s -> %s\n' "$name" "$actual"
}

expect_panic_at() {
  if "$out/$1" >"$out/$1.stdout" 2>"$out/$1.stderr"; then
    printf "FAIL %s: expected a panic, but it exited cleanly\n" "$1" >&2
    exit 1
  fi
  if ! grep -qF "$2" "$out/$1.stderr" || ! grep -qF "$3" "$out/$1.stderr"; then
    printf "FAIL %s: expected a panic at %s (%s), got:\n" "$1" "$2" "$3" >&2
    cat "$out/$1.stderr" >&2
    exit 1
  fi
  printf "ok   %s -> panicked at %s %s\n" "$1" "$2" "$3"
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

build_driver step5_enum_adt
compile_with step5_enum_adt step5_program
expect_output step5_program "rect 3x3 | empty | [12, 20, 0]"

build_driver step6_generic_function
compile_with step6_generic_function step6_program
expect_output step6_program "incan 2.5 9"

build_driver step7_incn_spans
INCAN_SOURCE="$here/scores.incn" compile_with step7_incn_spans step7_program
expect_panic_at step7_program "scores.incn:3:12:" "attempt to add with overflow"

# Step 8 is a three-crate graph: Rust `host` <- Incan `policy` (no Rust source) <- Rust `app`.
build_driver step8_boundary_rows
rustc +"$toolchain" --edition 2024 --crate-type rlib --crate-name host "$here/step8_host.rs" -o "$out/libhost.rlib"
"$out/step8_boundary_rows" --sysroot "$sysroot" --edition 2024 --cap-lints allow --crate-type rlib --crate-name policy \
  --extern host="$out/libhost.rlib" -L "$out" -o "$out/libpolicy.rlib" - </dev/null 2>"$out/policy.log"
rustc +"$toolchain" --edition 2024 --extern policy="$out/libpolicy.rlib" -L "$out" "$here/step8_app.rs" -o "$out/step8_app"
expect_output step8_app "42 5 15 second"

# Step 9: Incan code drives the rustc seam. One driver builds every unit; `rustc_private` is a declared permission.
build_driver step9_rustc_seam
unset RUSTC_BOOTSTRAP
unit() { "$out/step9_rustc_seam" --sysroot "$sysroot" --edition 2024 --cap-lints allow -L "$out" "$@"; }
if "$out/step9_rustc_seam" --sysroot "$sysroot" --edition 2024 --crate-type rlib --crate-name mir_seam \
  "$here/step9_seam.rs" -o "$out/refused.rlib" 2>/dev/null; then
  printf 'FAIL step9: the seam built without the declared --incan-allow-rustc-private\n' >&2
  exit 1
fi
printf 'ok   step9_seam -> refused without --incan-allow-rustc-private\n'
unit --incan-allow-rustc-private --crate-type rlib --crate-name mir_seam "$here/step9_seam.rs" -o "$out/libmir_seam.rlib"
unit --incan-unit --crate-type rlib --crate-name planner --extern mir_seam="$out/libmir_seam.rlib" \
  -o "$out/libplanner.rlib" - </dev/null 2>"$out/planner.log"
unit --incan-allow-rustc-private --extern planner="$out/libplanner.rlib" "$here/step9_app.rs" -o "$out/step9_app"
expect_output step9_app "in-process rustc exit status 0" "$here/step9_target.rs" "$out/step9_target" "$sysroot"
expect_output step9_target "answer() = 42"

# Step 10: drops and unwinding. Unconditional drops plus one cleanup block; rustc's drop elaboration does the rest.
build_driver step10_drops_and_unwinding
compile_with step10_drops_and_unwinding step10_program
expect_output step10_program "10 7 unwound=true drops=1,2,3"

# Step 11: real Body IR, lowered natively. The driver links this checkout's Incan front end, built with the pinned
# rustc into its own target directory: CARGO_TARGET_DIR=<dir> cargo +1.98.0 build -p incan_frontend -p incan_std_core
# Set INCAN_SPIKE_FRONTEND_TARGET=<dir> to run it; without it the step is reported as skipped, never as passing.
if [ -n "${INCAN_SPIKE_FRONTEND_TARGET:-}" ]; then
  deps="$INCAN_SPIKE_FRONTEND_TARGET/debug/deps"
  lang=$(ls "$deps"/libincan_lang-*.rlib | head -1)
  core=$(ls "$deps"/libincan_semantics_core-*.rlib | head -1)
  RUSTC_BOOTSTRAP=1 rustc +"$toolchain" --edition 2024 -A warnings "$here/step11_body_ir_lowering.rs" -o "$out/step11_driver" \
    -L dependency="$deps" --extern incan_frontend="$INCAN_SPIKE_FRONTEND_TARGET/debug/libincan_frontend.rlib" \
    --extern incan_lang="$lang" --extern incan_semantics_core="$core"
  std_core="$INCAN_SPIKE_FRONTEND_TARGET/debug/libincan_std_core.rlib"
  "$out/step11_driver" --incan-source "$here/step11_kernels.incn" --sysroot "$sysroot" --edition 2024 --cap-lints allow \
    --crate-type rlib --crate-name kernels --extern incan_std_core="$std_core" -L dependency="$deps" -o "$out/libkernels.rlib" - </dev/null
  rustc +"$toolchain" --edition 2024 --extern kernels="$out/libkernels.rlib" -L "$out" -L dependency="$deps" "$here/step11_app.rs" -o "$out/step11_app"
  expect_output step11_app "$(printf 'fib(1000000) mod 1000000007 = 918091266\nTotal Collatz steps for 1..1000000: 131434424\nTotal iterations: 97631088')"
else
  printf 'skip step11 (set INCAN_SPIKE_FRONTEND_TARGET to a target dir holding incan_frontend and incan_std_core)\n'
fi
