# Initial compiler library-entry result

**Follow-up:** [COMPILER_CALLBACK_RESULTS.md](COMPILER_CALLBACK_RESULTS.md) records working native and Incan-authored callbacks. This file preserves the prior library-entry stage and its then-open gaps.

The per-invocation compiler-private linkage patch builds successfully from the dev.8 seed. It compiles an independent Incan rustc enum consumer and a direct `rustc_driver.main()` consumer. The latter reports rustc target configuration without spawning a compiler subprocess. No Incan-authored callback or typed-body injection has executed.

The matching prebuilt runtime and standard-library providers were reused. The successful build used installed Oven with the copied ordinary SDK rather than the relink helper's compiler-build home: the helper first lacked relative native roots, then encountered incompatible installed/source-built incan_lang artifacts. No identity guard was relaxed. The experiment plan is retained unchanged; this file records the execution difference.

The standalone consumer repository is `/Volumes/Builds/incan-compiler-surface-spike`. Its `scripts/prepare-reduced.rb`, `scripts/reduced.sh`, and `scripts/verify-reduced.sh` preserve the commands and checks. The library entry supplies no Incan standard-library Loaves and explicitly binds only rustc_driver and the prebuilt incan_native_runtime, with the SDK still available for transitive native artifacts. Its assertion companion also requires the native incan_std_core helper. This is a reduced build/input closure, not a physically minimized distribution or Rust no_std result.

## Callback boundary remains open

CA-2 remains reproducible: source `class Hooks with Callbacks` passes checking but lowering refuses `NativeTraitObject`; rustc's own TimePassesCallbacks fails matching against `&mut dyn Callbacks + Send`. No partial callback workaround was committed.

Source inspection shows why a reference-layout-only patch is insufficient. `check_class_trait_adoptions` records local TraitInfo adoptions but skips the conformance branch for an imported symbol without TraitInfo. The plan's model bridges cover specific standard traits rather than carrying a general native trait implementation contract. `rust_type_implements_trait_object` also looks up the complete principal-plus-auto-traits spelling as one trait. The next coherent slice must prove native trait conformance, preserve every bound, and lower the borrow/unsizing with rustc validation. Hardcoding Hooks or omitting Send would conceal this gap.

## Scope and verification

Only CLI argument plumbing and the adapter's explicit experimental feature selection are changed. Ordinary invocations do not select compiler-private linkage. Existing bootstrap refusal and pinned compiler identity verification remain intact. This switch is local experimental authority, not a released capability or sandbox boundary. It deliberately explores an exception to DD-0004's driver-only restriction without changing the production design document.

The driver builds through the native release path, changed Rust sources pass rustfmt and diff checks, and the standalone integration script records both successes and expected refusals. No full SDK rebuild, full repository test suite, or release qualification is claimed. No public branch or PR is created for this experiment.
