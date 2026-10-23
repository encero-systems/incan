# `incan-rustc-driver`

The compiler: a second front end to the pinned rustc (DD-0004). Oven runs it once per unit, from the unit description it wrote (DD-0008).

- `--unit UNIT CRATE OUTPUT SYSROOT [--extern CRATE=RLIB] [--search DIR] [--rust-abi CACHE] [--rustc-arg ARG] [--library]` checks the unit, lowers its Body IR through `incan_mir_lowering` and compiles it with rustc to `OUTPUT`. A library unit also writes its Loaf beside the rlib (DD-0007).
- `--check UNIT SYSROOT [--format text|json] [--extern …] [--search …] [--rust-abi …]` checks the unit and prints its report; JSON is the stable `incan check` report.
- `--identity SYSROOT` verifies the driver's identity against the sysroot, without compiling, and prints its host target.
- `--rust-abi SYSROOT OUTPUT CRATE... [--extern CRATE=RLIB] [--search DIR]` writes the ABI cache of the named crates: every public item's Rust facts, read from rustc's crate metadata (DD-0009).

Checking reads the Rust facts of `rust::` imports from the ABI caches it is given, and reads any path they do not cover from rustc's metadata of the unit's own crates, with the same extractor (`src/rust_facts.rs`).

The plan types and the lowering's entry points come from `incan_mir_lowering::caller::incan`; no Rust mirror of the plan is kept. Declarations are injected AST items with diverging placeholders, and bodies are supplied through `mir_built`.

The unit description's `kind` selects `program` (the default), `library` or `test`. Every kind uses the same checked-source and Body IR pipeline and the lowering's `unit_plan` entry. `--library` remains a bootstrap spelling for the library kind. Test collection uses the compilation session's marker semantics; lowering maps collected functions to native entries, which the adapter registers with rustc's test harness. The compiler writes `<OUTPUT>.tests.json` with each entry's source name, native name, skip reason and expected-failure reason for Oven to execute and report.

## Identity

The driver links the toolchain's own `rustc_driver` and verifies it before it compiles anything. Oven builds it with three inputs: `INCAN_DRIVER_RUSTC_IDENTITY` (the complete `rustc -vV` output), `INCAN_DRIVER_LIBRARY` (the library's path in the sysroot) and `INCAN_DRIVER_LIBRARY_DIGEST` (SHA-256 of its bytes), and gives the binary a runpath to that library through the `sysroot` link beside its `bin` directory, then to the building machine's sysroot. Identity is content, never location: the driver accepts any sysroot whose compiler output and loaded library bytes match, so an installed toolchain runs against the same rustc at any path. Before compilation it refuses another compiler, another loaded library and any `RUSTC_BOOTSTRAP`. `--identity SYSROOT` runs only this verification and prints the host target.

Each compilation verifies identity once, before checking, and passes the proven `DriverIdentity` to the native adapter. Every compilation invocation still checks the complete compiler identity, the actual loaded library path inside the sysroot and ambient bootstrap permission. The library's SHA-256 proof can be reused across processes for an unchanged file state.

An installed driver stores one optional `driver-library.proof` beside its toolchain's `toolchain.json`, above the executable's `bin` directory. The proof binds the canonical library path (losslessly encoded), device, inode, size, nanosecond modification and change times, and expected build digest. A miss hashes the opened library in bounded chunks; both the descriptor and path must retain the same state before the proof is published. Any changed key requires hashing again. Missing, malformed or unreadable proof data means a miss; failed writes leave compilation dependent on the successful hash. Publication uses a private temporary file and atomic rename, so concurrent driver starts cannot admit a partial proof. Uninstalled drivers hash without storing a proof.

The toolchain home is trusted installation state: its owner can replace the executable and its proof. The proof detects file-state changes; it is not an attestation against an actor able to forge toolchain-home files or filesystem metadata. `INCAN_TRACE_DIR` records one `verify_identity` span per compilation and a `cached` field on the loaded-library proof event.

The manifest declares `rustc_private`, `rustc-dev` and `rustc_driver` on the executable; Oven requires the pinned rustc 1.98.0 with rustc-dev installed.

`sh tests/verify_identity.sh DRIVER SYSROOT UNIT NEW_SCRATCH_DIRECTORY` exercises proof reuse, invalidation, storage failures, concurrent starts and startup refusals. Run it from this Loaf's directory with a program unit description produced by Oven. It copies the driver into its own scratch installation and leaves the supplied toolchain and sysroot unchanged. The test requires `jq` and `xxd`; supported loader tests run on macOS and Linux.
