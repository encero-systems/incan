# Implementation Learnings

Reference document for AI agents. These are hard-won insights from past RFC implementations and issue resolutions. Read the relevant section before starting work on any RFC implementation or any change that touches the parser, typechecker, or lowering stages.

## General pipeline pitfalls

- **Bridge modules need contract docs**: adapter/bridge files that translate between internal and public ASTs must document directionality, error behavior, and unsupported shapes up front; sparse rustdocs in these boundaries cause incorrect call-site assumptions and fragile follow-on changes (RFC 027 Phase 6).
- **New AST variants need full pipeline wiring**: adding a `Statement`/`Expr` variant is never parser-only; you must update formatter, feature scanners, typechecker, lowering, and any AST bridge layers in the same change or compilation/tests will break in scattered places (RFC 027 Phase 6).
- **Typechecker passing does not mean lowering works.** A feature that typechecks correctly can still generate invalid Rust if the lowering stage doesn't handle the transformation. Always verify both stages independently. (Learned from RFC 021: field aliases typechecked but lowering didn't translate them, producing broken Rust.)
- **Reject out-of-scope features at the typechecker**, not silently. When an RFC says "X is not supported", add an explicit diagnostic in the typechecker. Don't leave partial support that passes typechecking but fails in lowering or emission.
- **`Program` struct stability**: adding fields to `Program` breaks all literal construction sites. Use `#[derive(Default)]` + `..Default::default()` in tests — never explicit field lists in test helpers.
- **Use `IrType::incan_name()` for user-facing type strings**, not `IrType::to_string()` (which returns Rust types like `String` instead of `str`).

## Testing strategy

- **Always test both typechecker and codegen.** Typechecker unit tests validate semantics; codegen snapshot tests verify end-to-end output. Neither alone is sufficient.
- **Snapshot tests must exercise features in expressions**, not just declarations. A model that declares an alias but never uses it in an expression won't catch lowering bugs.
- **Test both `From` and `RustFrom` import forms** when changing import handling — they share `parse_import_items()` but a test gap for one form can hide regressions.

## Parser and lexer patterns

- **Parser warning infrastructure**: `Parser.warnings` stores non-fatal warnings as `Vec<CompileError>`. On success they move into `Program.warnings`; on error they fold into the error vec with `ErrorKind::Warning`. This is the canonical way to add syntax nudges without blocking compilation.
- **Lexer bracket depth handles multi-line for free**: `bracket_depth` suppresses `Newline`/`Indent`/`Dedent` inside `(...)`. Check lexer bracket tracking before adding complex multi-line parser state.
- **Soft keywords stay as identifiers in the lexer**: emit `Ident("async")` etc.; the parser promotes them to keywords only when the activating namespace is imported. Activation is tracked per-file via `active_soft_keywords`.

## Stdlib and registry patterns

- **`STDLIB_NAMESPACES` is the single source of truth** for which `std.*` modules exist, how stub paths resolve, and which imports activate soft keywords. Extend the registry rather than adding hardcoded special cases.
- **Stdlib stubs (`stdlib/*.incn`) are IDE-only** — the prelude isn't wired into compilation. For core surface types like `FieldInfo`, also register them in `incan_core::lang::surface::types` and map to the runtime Rust type in IR emission.
- **Runtime facades must match generated imports**: if codegen emits `incan_stdlib::r#async`, the runtime crate must export that module tree.
- **Prefer `.incn` declarations over synthesized `FunctionInfo`**: if a public stdlib function exists, use a local `.incn` wrapper so the AST loader extracts its signature from source. Handwritten `FunctionInfo` drifts.

## Wiring: CLI and LSP

- **CLI wiring for warnings**: surface `ast.warnings` via `eprint!` in `common.rs`'s `collect_modules()` — this automatically covers all CLI commands.
- **LSP wiring for warnings**: after `parser::parse()` succeeds, loop `ast.warnings` and push each through `compile_error_to_diagnostic()` before typechecking.
- **LSP is feature-gated**: `make lsp` / `make install-lsp` require `--features lsp`. Always verify with a fresh `make install-lsp` after changing LSP-touching code.

## Generic bounds and extern functions

- **Store explicit generic bounds in frontend symbols**, not just type-parameter names. A per-parameter bounds map lets call checking enforce `with` contracts before lowering.
- **Extern diagnostics need CLI-level wrapping**: typechecker catches declaration-shape issues, but signature/path mismatches fail in Cargo. Map rustc stderr back to `@rust.extern` spans for actionable diagnostics.
- **Centralize shared typechecker heuristics immediately**: if a naming rule or predicate is used in more than one checker module, extract it to a shared helper to avoid silent drift.
- **Manifest-backed type conversion must have one source of truth**: keep `TypeRef` ↔ `ResolvedType` mapping in a shared helper/module and reuse it across typechecker/LSP/codegen consumers. Duplicated conversion logic drifts and causes hard-to-debug manifest import regressions.
