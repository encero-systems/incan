# Shared native compiler binary transport probe

The source compiler bootstrap in [#1698](https://github.com/encero-systems/incan/issues/1698) calls the existing `bake_trusted_direct_rustc_run` through Incan `rust::` interop. That executor reuses caller-local output receipts. Its public API has a store-backed generated-library counterpart, but no store-backed binary counterpart.

Run `incan run src/main.incn` in this probe with the same admitted native SDK and Oven store used by the compiler bootstrap. The called import fails with Rust E0432: `oven_rustc::rustc::bake_trusted_direct_rustc_run_in_store` does not exist. An unused-import probe is insufficient because code generation omits unused imports. The call is intentionally present to establish export resolution; its zero arguments do not specify a future API signature.

This is evidence of a missing exported transport, not proof that shared executable selection cannot be authored in Incan using other existing APIs. Check those APIs before introducing a Rust bridge. Any bridge must retain the existing receipt, source, compiler, linker, selected artifact and environment checks, plus an execution lease and writable executable projection. A missing function alone does not justify duplicating store selection policy in Rust.

The compatible-worktree control also exposed a separate initial identity-engine preparation problem: two completed executable outputs can have equal source, compiler, receipt and build-unit authority but different executable bytes. Preparing a fresh local output and selecting an older global output on the next call changes the enclosing compiler receipt. Fix initial shared-output selection before attributing that repeat compilation to the missing binary transport.
