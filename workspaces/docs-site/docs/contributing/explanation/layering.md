# Layer boundaries

The dev.8 build has one semantic authority per fact. Oven owns projects and compilation requests. The checker owns resolved language meaning. Body IR carries those facts into Incan lowering. The native plan and Rust adapter translate the checked result into the pinned Rust compiler's MIR.

## Project boundary

`workspaces/oven` reads `loaf.toml`, selects features and dependencies, prepares artifacts and describes a unit. The compiler session reads that description. It does not independently reconstruct project selection.

## Semantic boundary

Kernel packages define shared syntax, identities and language policy. The frontend resolves names, types, ownership and native storage facts. Lowering consumes checked facts; a missing fact is not permission to guess a type or repeat resolution.

## Native boundary

`incan_mir_lowering` is authored in Incan. It fills the plan defined by `incan_mir_plan`. The Rust adapter in `incan-rustc-driver` maps that plan into rustc APIs. Rust-specific compiler integration stays in this adapter; application semantics and lowering decisions do not move there for convenience.

## Library boundary

Standard packages and user libraries publish checked metadata and native artifacts together. Consumers depend on public exports through declared package identities. A source dependency and its compiled public namespace must agree.

Runtime behavior belongs in `incan_native_runtime` and standard packages. Shared language policy belongs in kernel packages, where it can be consumed without pulling runtime behavior into checking.

## Platform boundary

`incan_platform` in `loaves/platform` is the OS-facing layer below the standard packages. It wraps OS and Rust facilities thinly through interop and imports no compiler, kernel or standard Loaf; standard packages wrap it, never the other way round. Its IPC carries bytes and peer identity only: the owner and engine protocols that encode those bytes stay in Oven, and the compiler worker's side of `incan.compiler-worker/1` is the worker host `incan_worker_host` in `loaves/compiler`.

## Verification boundary

Contract families test observable behavior through the installed toolchain. Compiler setup or a successful relink is not runtime proof. Use a focused case while changing behavior and whole affected families at commit points.
