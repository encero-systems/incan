# 2. Layering and boundaries

Incan is easiest to evolve when dependencies are clean. This chapter explains how to avoid “clever” shortcuts that create
cycles, leaky abstractions, or drift between tooling and the compiler.

## The principle

Each layer should depend “inward” on more stable layers, not sideways on more complex ones.

When in doubt:

- **Prefer data passing over calling across layers**
- **Prefer pure helpers over global state**
- **Prefer shared crates for shared policy** (instead of copy/paste)

## The practical rules

The canonical rules live here:

- [Layering rules](../../explanation/layering.md)
- [Readable, maintainable Rust](../../explanation/readable-maintainable-rust.md)

In practice, the patterns you want are:

- **`incan_syntax` is shared**: lexer/parser/AST/diagnostics are reused by compiler, formatter, and LSP.
- **`incan_core` is a semantic core**: shared, pure semantics and registries that must not drift.
- **`incan_semantics_core` is the descriptor contract**: semantics packs describe compiler actions without calling compiler internals.
- **`incan_semantics_stdlib` is implementation**: current stdlib packs are toolchain-locked, not general runtime APIs.
- **`rust_inspect` is staged interop**: prepare/prewarm Rust metadata explicitly, then use cache-oriented reads in semantic paths.
- **Compiler crates should not depend on runtime crates**: runtime is for generated programs, not for the compiler.

## A common failure mode

You add a feature, it “works” in the CLI, but:

- the formatter prints it differently,
- the LSP can’t parse it,
- or the language reference/keywords drift from implementation.

Another failure mode is treating runtime convenience as compiler policy:

- adding a helper to `incan_stdlib` and calling it from `incan`,
- putting stdlib-owned runtime types into `incan_core` without a clear language-policy reason,
- or making `rust_inspect` do hidden workspace loading from a semantic hot path.

When you feel tempted to “just implement it in the CLI”:

- stop and ask which layer actually owns the behavior, and
- keep the ownership boundary crisp.

## Next

Next chapter: [03. Proposals: issues vs RFCs](03_proposals_issues_vs_rfcs.md).
