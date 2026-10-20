# `incan-rustc-driver`

The compiler: a second front end to the pinned rustc (DD-0004). Oven runs it once per unit, from the unit description it wrote (DD-0008).

- `--unit UNIT CRATE OUTPUT SYSROOT [--extern CRATE=RLIB] [--search DIR] [--rust-abi CACHE] [--rustc-arg ARG] [--library]` checks the unit, lowers its Body IR through `incan_mir_lowering` and compiles it with rustc to `OUTPUT`. A library unit also writes its Loaf beside the rlib (DD-0007).
- `--check UNIT SYSROOT [--format text|json] [--extern …] [--search …] [--rust-abi …]` checks the unit and prints its report; JSON is the stable `incan check` report.
- `--rust-abi SYSROOT OUTPUT CRATE... [--extern CRATE=RLIB] [--search DIR]` writes the ABI cache of the named crates: every public item's Rust facts, read from rustc's crate metadata (DD-0009).

Checking reads the Rust facts of `rust::` imports from the ABI caches it is given, and reads any path they do not cover from rustc's metadata of the unit's own crates, with the same extractor (`src/rust_facts.rs`).

The plan types and the lowering's entry points come from `incan_mir_lowering::caller::incan`; no Rust mirror of the plan is kept. Declarations are injected AST items with diverging placeholders, and bodies are supplied through `mir_built`.

The unit description's `kind` selects `program` (the default), `library` or `test`. Every kind uses the same checked-source and Body IR pipeline and the lowering's `unit_plan` entry. `--library` remains a bootstrap spelling for the library kind. Test collection uses the compilation session's marker semantics; lowering maps collected functions to native entries, which the adapter registers with rustc's test harness. The compiler writes `<OUTPUT>.tests.json` with each entry's source name, native name, skip reason and expected-failure reason for Oven to execute and report.

## Identity

The driver links the toolchain's own `rustc_driver` and verifies it before it compiles anything. Oven builds it with four inputs: `INCAN_DRIVER_SYSROOT`, `INCAN_DRIVER_RUSTC_IDENTITY` (the complete `rustc -vV` output), `INCAN_DRIVER_LIBRARY` (the library's path in the sysroot) and `INCAN_DRIVER_LIBRARY_DIGEST` (SHA-256 of its bytes), and gives the binary a runpath to that library. At startup the driver refuses another sysroot, another compiler, another loaded library and any `RUSTC_BOOTSTRAP`.

The manifest declares `rustc_private`, `rustc-dev` and `rustc_driver` on the executable; Oven requires the pinned rustc 1.98.0 with rustc-dev installed.
