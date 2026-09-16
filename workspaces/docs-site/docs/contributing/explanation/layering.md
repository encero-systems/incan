# Layering Rules

This repository follows a strict dependency direction to keep semantics shared and prevent accidental drift between the compiler and the runtime:

- `incan` (compiler) may depend on `incan_lang`.
- `incan` may depend on `incan_syntax`, `incan_semantics_core`, `incan_semantics_stdlib`, and optional `rust_inspect` for compiler/toolchain work.
- `incan` must **not** depend on a standard library facet (`incan_std_core` and the other `incan_std_<component>` crates) except as a **dev-dependency** for parity tests.
- The facets depend on `incan_lang`; the optional facets depend on `incan_std_core`.
- Generated user programs depend on `incan_std_core` and on whichever other facets their namespaces reach.

```mermaid
flowchart TD
  incanCompiler["incan (compiler)"] --> incanLang["incan_lang"]
  incanCompiler --> incanSyntax["incan_syntax"]
  incanCompiler --> semanticsCore["incan_semantics_core"]
  incanCompiler --> semanticsStdlib["incan_semantics_stdlib"]
  incanCompiler -. optional CLI/LSP interop .-> rustInspect["rust_inspect"]
  incanSyntax --> incanLang
  semanticsStdlib --> semanticsCore
  stdCore["incan_std_core"] --> incanLang
  stdFacets["incan_std_data · incan_std_async · incan_std_web · incan_std_testing"] --> stdCore
  generatedProgram["generated program"] --> stdCore
  generatedProgram --> stdFacets
  generatedProgram --> incanDerive["incan_derive"]
  generatedProgram --> incanWebMacros["incan_web_macros"]
```

CI/Test guardrails enforce that `incan` keeps the facets out of its normal dependencies. If you need runtime helpers inside tests, add them under `[dev-dependencies]` only.

## Workspace crate categories

Use this policy when deciding where new code belongs:

- **Stable contracts**: `incan_lang`, `incan_syntax`, `incan_semantics_core`, and `incan_vocab`. Other layers build on these crates. Keep them deterministic, dependency-light, and free of runtime side effects.
- **Compiler/toolchain implementation**: `incan`, `incan_semantics_stdlib`, and `rust_inspect`. These crates are tied to the current compiler/tooling. They may depend on stable contracts but should not become runtime APIs.
- **Runtime-only implementation**: the standard library facets (`incan_std_core`, `incan_std_data`, `incan_std_async`, `incan_std_web`, `incan_std_testing`), `incan_derive`, and `incan_web_macros`. Generated Rust programs use these crates. The compiler may generate references to them but must not depend on them in normal builds.
- **Transitional runtime surfaces**: the current `incan_std_web` facet and related macro glue. This runtime code is not yet a stable long-term contract. Keep it quarantined and avoid treating it as compiler-owned policy.

## Why we do this

We want one “source of truth” for language behavior so the compiler and runtime don’t drift:

- **Semantics must match**: if const-eval validates something, runtime should do the same thing the same way (especially for Unicode-sensitive string operations and numeric edge cases).
- **Diagnostics/panics must stay aligned**: user-facing error messages should not diverge between compile-time and runtime.
- **Compiler stays lean**: the compiler shouldn’t accidentally pull in runtime-only APIs or heavy dependencies.

## What goes where (contracts vs implementations)

**`incan_lang`**:

- Pure helpers that define *meaning/policy* (e.g., string indexing/slicing rules, numeric promotion, canonical error message constants).
- Central registries for language vocabulary and stdlib wiring (for example `incan_lang::lang::stdlib::STDLIB_NAMESPACES` and keyword metadata used by the lexer/parser).
- Must be deterministic and side-effect free.
- Should not depend on compiler internals (AST, spans, lexer/parser state).
- Should not gain new stdlib-owned runtime surface types unless the type metadata is truly shared language policy.

**`incan_syntax`**:

- Lexer, parser, AST, and syntax diagnostics shared by compiler, formatter, LSP, and future tooling.
- May use language vocabulary from stable contract crates.
- Must not perform name resolution, typechecking, lowering, Rust interop loading, or runtime behavior.

**`incan_semantics_core`**:

- Stable action-descriptor and semantics-pack contracts that compiler stages can consume.
- Owns behavior descriptors, not compiler execution. Packs describe what to do; compiler stages decide how to do it.

**`incan_semantics_stdlib`**:

- Stdlib semantics-pack implementation for current built-in library surfaces.
- Toolchain-locked implementation crate, not a stable external API.
- Should return descriptors and canonical targets instead of reaching into compiler internals.

**`rust_inspect`**:

- Dedicated Rust metadata preparation, extraction, and caching subsystem.
- Allowed behind compiler/tooling features for Rust interop.
- Should remain explicit and staged: prepare/prewarm metadata at CLI/LSP/project boundaries, then read cached metadata in semantic paths.

**The standard library facets (`incan_std_core`, `incan_std_data`, `incan_std_async`, `incan_std_web`, `incan_std_testing`)**:

- Runtime helpers used by generated Rust code, one crate per stdlib component that has Rust beside its Incan sources (`loaves/stdlib/<component>/rust/`); `incan_std_core` is mandatory and serves the language itself, the others serve their component's namespaces.
- Should delegate behavior to `incan_lang` for policy/consistency, and implement runtime-only actions (like panicking) using the shared error messages/taxonomy.
- May contain transitional implementation modules, but those modules must not become compiler dependencies.

**`incan_derive` / `incan_web_macros`**:

- Runtime-side macro support for generated Rust programs.
- Must not become a backchannel for compiler logic.
- Web macro/runtime glue is transitional until the web surface has a stable long-term ownership model.

**`incan` (compiler)**:

- Parsing, typing, lowering, codegen, diagnostics.
- May use stable contract crates to implement checks/const-eval and to keep error text aligned.
- Must not use runtime-only crates in normal builds; only `incan_std_core` as a dev-dependency for parity tests.

## Allowed / forbidden dependencies

**Allowed**:

- `incan` → `incan_lang`, `incan_syntax`, `incan_semantics_core`, `incan_semantics_stdlib`, `incan_vocab` as normal compiler/toolchain dependencies.
- `incan` → `rust_inspect` behind the `rust_inspect`/CLI/LSP interop path.
- a facet → `incan_lang` (and an optional facet → `incan_std_core`) as normal dependencies.
- `incan` → `incan_std_core` as a dev-dependency only, for tests.

**Forbidden**:

- `incan` → any facet in `[dependencies]` (this breaks layering).
- `incan` → `incan_derive` or `incan_web_macros` in normal dependencies.
- `incan_lang`, `incan_syntax`, or `incan_semantics_core` → compiler/toolchain/runtime implementation crates.
- Runtime crates calling back into compiler crates.

## Common pitfalls

- Adding a “quick helper” in a facet and calling it from the compiler.
    - Fix: move the policy/logic to `incan_lang` and keep only runtime glue (panics, wrappers) in the facet.

- Adding another stdlib-specific surface type to `incan_lang` because similar metadata already exists there.
    - Fix: decide whether the type is true language policy or library-owned surface. Prefer library-defined ownership when possible, and document the exception when it must stay core-owned.

- Emitting direct Rust operations that bypass shared semantics (e.g., slicing Rust `String` by byte indices).
    - Fix: emit calls to `incan_std_core` wrappers which themselves delegate to `incan_lang`.

- Duplicating error messages as string literals in multiple places.
    - Fix: put canonical text in `incan_lang` and reuse it from both compiler and runtime.

- Loading Rust metadata opportunistically from typechecking or lowering.
    - Fix: prewarm through the explicit `rust_inspect` preparation path and keep semantic lookups cache-oriented.

## Guardrails (how it is enforced)

- **Dependency gate**: `loaves/toolchain/incan-cli/tests/layering_guard.rs` fails if a facet appears in the `[dependencies]` section of the root, compiler-ring or kernel-ring manifests (keeping one in `[dev-dependencies]` for parity tests is allowed), if the registry's facet facts disagree with `sdk-components.toml` and the crates on disk, or if the compiler ring spells a runtime crate the catalog does not know.

## How to add shared behavior safely

When you notice drift risk (compiler vs runtime):

1. Put the *policy* in `incan_lang` (pure function + typed error or canonical message).
2. Add a thin wrapper in the owning facet (`incan_std_core` for language runtime) that calls semantics and performs runtime-only behavior (panic, allocation, conversions).
3. Update compiler const-eval / typechecking to use the semantics helper directly (never stdlib).
4. Add a parity test in the owning ring's integration roots (the frontend's `semantic_core_parity` roots under `loaves/compiler/incan_frontend/tests/`, or `incan_emit`'s codegen roots) that compares compiler/semantics/runtime behavior for the edge case.
