# 4. Your first change: add a builtin

This chapter is a guided walkthrough for adding a **builtin**: a function that looks like a normal call in Incan, but
lowers/emits to a specific Rust pattern.

If you haven’t read it yet, start with: [Extending the language](../../how-to/extending_language.md)

## Decide: stdlib function vs compiler builtin

Use a **stdlib function** when the behavior can live entirely in runtime support code.

Use a **compiler builtin** when you need:

- special typing rules,
- special lowering/emission,
- or you want the surface syntax to stay “function-call-like” while generating nontrivial Rust.

## End-to-end checklist (compiler builtin)

Before you start, sanity-check which layer your change belongs in (to avoid language/tooling drift):

--8<-- "_snippets/contributing/compiler_pipeline_checklist.md"

### What you will usually touch

Builtin changes typically land in the compiler (`incan` crate), so you will usually touch:

1. **Frontend symbol table** (so it typechecks)
    - `src/frontend/symbols.rs` → builtin name + signature
2. **IR builtin enum** (so lowering can represent it explicitly)
    - `src/backend/ir/expr.rs` → `BuiltinFn` variant + name mapping
3. **Lowering** (so calls become `BuiltinCall`)
    - `src/backend/ir/lower/expr.rs`
4. **Emission** (so Rust output matches the intended pattern)
    - `src/backend/ir/emit/expressions/builtins.rs`
5. **Tests and docs**
    - add a regression test (parse/typecheck/codegen)
    - add/adjust docs if it changes user-visible behavior

## A suggested “first builtin” exercise

Pick a small builtin where you can clearly verify the generated Rust (and add a regression test):

- a builtin that maps to a single Rust stdlib call
- a builtin that needs a small helper function emitted

Keep it small: your goal is to learn the pipeline and leave the codebase in a better state.

## Running your feedback loop

After implementing the builtin, validate it through the toolchain:

- `make pre-commit` still passes (fast local gate: fmt-check + cargo check)
- `make pre-commit-full` passes before pushing (fmt-check + tests + clippy)
- `make smoke-test` still passes (end-to-end sanity check)
- the LSP still parses/diagnoses edited files (no syntax drift)

Optionally, consider adding a tiny example showing the new builtin in use, to lock in the intended behavior. Note that examples
are intended for user-visible behavior, not for internal implementation details.

## Next

Next chapter: [05. Your first syntax change](05_add_new_syntax.md).
