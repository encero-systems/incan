# Implementation Learnings

Reference document for AI agents. These are hard-won insights from past RFC implementations and issue resolutions. Read the relevant section before starting work on any RFC implementation or any change that touches the parser, typechecker, or lowering stages.

## General pipeline pitfalls

- **Async result passthrough should stay direct**: when an async helper already returns `Result`, prefer `await helper(...) ?` over matches that rewrap `Ok(_) => Ok(None)` / `Err(err) => Err(err)`; Incan lowers `await expr?` correctly, and the explicit rewrap just adds showcase-hostile noise. (InQL cleanup, April 2026)
- **Boolean equality checks are noise**: avoid `expr == false` / `expr == true` in Incan code; prefer `not expr` or `expr` directly so conditions read as logic instead of desugared comparison. This matters especially in planning/lowering code where visual noise compounds quickly. (InQL cleanup, April 2026)
- **Bridge modules need contract docs**: adapter/bridge files that translate between internal and public ASTs must document directionality, error behavior, and unsupported shapes up front; sparse rustdocs in these boundaries cause incorrect call-site assumptions and fragile follow-on changes (RFC 027 Phase 6).
- **Ceremony helpers need a job**: tiny wrappers like `empty_*()` or `_clone_*()` are anti-patterns when they only spell `[]` or a one-line copy the language already expresses clearly; keep helpers only when they carry real semantics such as generic type witnesses, cross-stage invariants, or non-obvious boundary contracts. (InQL showcase cleanup, April 2026)
- **Dependency policy changes need approval**: when `cargo-deny` surfaces new licenses or advisories, do not add global license allows or advisory ignores as a convenience step; first identify the exact dependency path and get explicit approval for any policy exception versus dependency upgrade. (Wasmtime/deny cleanup, April 2026)
- **Dual module paths can collide**: when Incan emits Rust modules, having both a file module and directory module for the same name (`foo.incn` and `foo/mod.incn`) can map to conflicting Rust paths (`foo.rs` and `foo/mod.rs`) and fail with module ambiguity (`E0761`); pick one module shape and keep it consistent across the package.
- **Duckborrowing is codegen policy**: when work touches lowering/emission, call arguments, collection literals, returns, match scrutinees, Rust interop, or generated `.clone()`s, route ownership through `src/backend/ir/ownership.rs` / `ValueUseSite` and update trait-bound inference/tests instead of adding local `.clone()`, `.as_ref()`, `str(...)`, or `.into()` workarounds. (Issue #121, April 2026)
- **Name repeated kind checks**: when the language lacks grouped pattern arms, do not duplicate long `kind == A or kind == B ...` chains across functions; hide the grouping behind one predicate/helper so later enum-surface changes do not drift between call sites. (Prism output-column cleanup, April 2026)
- **Never expose local paths**: Shareable artifacts must use repo-relative paths or plain command names; absolute workstation paths like `/Users/...` leak personal details and should be blocked in hooks and avoided in docs, issues, PR text, and examples.
- **New AST variants need full pipeline wiring**: adding a `Statement`/`Expr` variant is never parser-only; you must update formatter, feature scanners, typechecker, lowering, and any AST bridge layers in the same change or compilation/tests will break in scattered places (RFC 027 Phase 6).
- **String literal wrappers are showcase noise**: avoid `str("literal")` when a plain string literal communicates the same value; use explicit ownership conversion like `"literal".to_string()` only at Rust/container boundaries that currently require owned `str` values. (InQL Substrait cleanup, April 2026)
- **Typechecker passing does not mean lowering works.** A feature that typechecks correctly can still generate invalid Rust if the lowering stage doesn't handle the transformation. Always verify both stages independently. (Learned from RFC 021: field aliases typechecked but lowering didn't translate them, producing broken Rust.)
- **Reject out-of-scope features at the typechecker**, not silently. When an RFC says "X is not supported", add an explicit diagnostic in the typechecker. Don't leave partial support that passes typechecking but fails in lowering or emission.
- **Parser should own type-sugar desugaring**: If a surface spelling must never reach lowering/emission (e.g. function-type sugar vs. a dedicated AST node), desugar in the parser to the canonical `Type::*` shape instead of duplicating normalization in `resolve_type`/symbols—two sites drift, and the resolver-only path can lag so the sugar leaks into generated Rust.
- **`Program` struct stability**: adding fields to `Program` breaks all literal construction sites. Use `#[derive(Default)]` + `..Default::default()` in tests — never explicit field lists in test helpers.
- **Use `IrType::incan_name()` for user-facing type strings**, not `IrType::to_string()` (which returns Rust types like `String` instead of `str`).

## Rust: `unwrap` / `expect` ban vs. `unwrap_or`

- **`unwrap_or` is not banned like `unwrap`**: AGENTS forbids `.unwrap()` and `.expect()` because they panic on absent/error values; it does **not** forbid `.unwrap_or(...)`, `.unwrap_or_else`, or similar fallbacks. Converting `try_into().unwrap_or(saturate)` into a `match` to “avoid unwrap” is misguided and can fail `clippy::manual_unwrap_or` with `-D warnings`.

## Testing strategy

- **Always test both typechecker and codegen.** Typechecker unit tests validate semantics; codegen snapshot tests verify end-to-end output. Neither alone is sufficient.
- **Review closeout requires repo gates**: do not declare a review or implementation loop complete on targeted parser/typechecker/codegen checks alone; run `make pre-commit` (or the repo’s full gate) before closeout, because formatter/clippy/all-targets failures can still surface missing imports, feature-gated compile errors, or drift outside the exercised feature slice. (RFC 049 / issue #333)
- **Conformance fixtures belong in tests**: production conformance modules should define scenario contracts and validators only; synthetic fixture plans and hardcoded sample literals belong in test-local builders so contract APIs stay clean and reusable.
- **Extern fixture coverage must stay real**: when removing or renaming a Rust host-boundary symbol, update generic extern-delegation fixtures/snapshots (for example `rust_extern_delegation`) alongside feature-specific snapshots; otherwise test coverage keeps validating dead runtime paths instead of the current boundary shape. (Issues #301/#302)
- **Snapshot tests must exercise features in expressions**, not just declarations. A model that declares an alias but never uses it in an expression won't catch lowering bugs.
- **Test both `From` and `RustFrom` import forms** when changing import handling — they share `parse_import_items(rust_item_names)`; only `RustFrom` passes `true` so Rust symbols may be Incan keywords (e.g. `import type as proto_type`). Incan `from m import ...` keeps `rust_item_names=false`.

## Parser and lexer patterns

- **Parser warning infrastructure**: `Parser.warnings` stores non-fatal warnings as `Vec<CompileError>`. On success they move into `Program.warnings`; on error they fold into the error vec with `ErrorKind::Warning`. This is the canonical way to add syntax nudges without blocking compilation.
- **Lexer bracket depth handles multi-line for free**: `bracket_depth` suppresses `Newline`/`Indent`/`Dedent` inside `(...)`. Check lexer bracket tracking before adding complex multi-line parser state.
- **Soft keywords stay as identifiers in the lexer**: emit `Ident("async")` etc.; the parser promotes them to keywords only when the activating namespace is imported. Activation is tracked per-file via `active_soft_keywords`.

## Stdlib and registry patterns

- **`STDLIB_NAMESPACES` is the single source of truth** for which `std.*` modules exist, how stub paths resolve, and which imports activate soft keywords. Extend the registry rather than adding hardcoded special cases.
- **Stdlib stubs (`stdlib/*.incn`) are IDE-only** — the prelude isn't wired into compilation. For core surface types like `FieldInfo`, also register them in `incan_core::lang::surface::types` and map to the runtime Rust type in IR emission.
- **Metadata-driven decorators keep their extern shell**: `std.testing` markers (`skip`, `xfail`, `slow`, `fixture`, `parametrize`) cannot be converted to plain Incan wrappers just because runtime calls are rejected; `src/frontend/testing_markers.rs` extracts semantics from `@rust.extern(metadata={...})` in `stdlib/testing.incn`, so removing that shell breaks discovery even if normal execution still passes. (Issue #302)
- **Runtime facades must match generated imports**: if codegen emits `incan_stdlib::r#async`, the runtime crate must export that module tree.
- **Prefer `.incn` declarations over synthesized `FunctionInfo`**: if a public stdlib function exists, use a local `.incn` wrapper so the AST loader extracts its signature from source. Handwritten `FunctionInfo` drifts.

## Wiring: CLI and LSP

- **CLI wiring for warnings**: surface `ast.warnings` via `eprint!` in `common.rs`'s `collect_modules()` — this automatically covers all CLI commands.
- **LSP wiring for warnings**: after `parser::parse()` succeeds, loop `ast.warnings` and push each through `compile_error_to_diagnostic()` before typechecking.
- **LSP is feature-gated**: `cargo build --features lsp` (or `make build`, which enables it) produces `incan-lsp`. For local dev, `make build` symlinks `~/.cargo/bin/incan-lsp` to `target/debug/incan-lsp` unless CI / `INCAN_SKIP_CARGO_BIN_LINK=1`; use `make install-lsp` or `cargo install --path . --features lsp --bin incan-lsp --force` when you need a crates.io-style install instead.

## Docs and RFC tooling

- **Rust prose comments should stay paragraph-shaped**: when editing `///`, `//!`, or prose `//` comments, do not manually wrap to short line lengths or create staircase-shaped rustdoc. Agents do not need to optimize rustdoc width; `make fmt` handles formatting well enough, and hand-chopped prose creates review noise and cleanup work. Treat overly short rustdoc/comment wrapping as a documentation bug, not a style choice. (RFC 015 branch, April 2026)
- **Explanation comments are part of the surface**: in Incan-family codebases, especially planning/lowering/interop code written in Python-shaped syntax, short explanatory comments are not optional garnish; they reduce the “hidden magic” effect for readers who are seeing compiler- or systems-level work in an unfamiliar surface language. Remove stale comments, not merely comments that an expert finds obvious. (InQL readability policy, April 2026)
- **Implementation docs must be user-facing**: RFCs and release notes do not satisfy user documentation for a new language/compiler feature; when behavior is user-visible, update the authored explanation/how-to/tutorial/reference docs where users actually learn the surface, not just the RFC or changelog. (RFC 049 / issue #333)
- **RFC phase before code**: when using `ralph-loop` for an RFC implementation, move the RFC to `In Progress` and confirm the implementation plan/checklist before writing code; do not treat lifecycle edits and phase confirmation as a post-implementation cleanup step. (RFC 016 / issue #327)
- **North-star first for RFCs**: when a maintainer asks for an RFC, start from the desired end-state contract and only discuss incremental slices after that north-star is explicit; do not reflexively shrink RFC scope into the smallest implementable change unless the user asks for rollout planning.
- **RFCs are decision records, not diaries**: keep RFCs as moment-in-time intent/status documents, and move implementation details, drift notes, and current behavior into regular docs or release notes with issue links instead of rewriting RFC narrative in flight.
- **Implementation work must check dev version first**: before landing an implementation on the active dev line, verify the repo's actual source-of-truth version instead of assuming an older release train from stale docs or a worker worktree; at minimum, implementation work should bump `-dev.N` by one and update any versioned docs/release-note targets that track `main`. (Issue #333, April 2026)
- **Stdlib closeouts need reference-nav parity**: when a stdlib issue changes a module's implementation shape or canonical docs path, update the stdlib reference index, MkDocs nav, and any legacy standalone reference page together; otherwise modules like `std.testing` drift out of the `language/reference/stdlib/` structure even when release notes and how-to docs were updated. (Issues #301/#302)
- **RFC lifecycle edits need graph updates**: When an RFC is renamed, moved, split, or superseded, update inbound RFC references and regenerate `workspaces/docs-site/docs/_snippets/rfcs_refs.md` plus `workspaces/docs-site/docs/_snippets/tables/rfcs_index.md`; otherwise the docs graph silently points at stale RFC paths and statuses. (RFC 012/050/051 split)
- **Ralph worktrees live in encero/tmp**: for `ralph-loop`, every implementation must start in a fresh worktree under `/Users/danny/Development/encero/tmp`, not `/tmp` and not the primary checkout, so VS Code discovers the workspace and orchestration stays consistent. (RFC 016 / issue #327)

## Builtin trait stubs and stdlib method lookup (#193)

- **Vocabulary-only builtin traits ship with empty `methods` maps** in the symbol table. For instance method resolution, `trait_method_info_resolved` can fall back to `StdlibAstCache::lookup_trait` after `stdlib_module_segments_for_trait_methods` in `src/frontend/typechecker/check_decl.rs` maps the trait name to a stdlib module path.
- **That path map is limited to `std.derives.{copying,string,comparison}`** (Clone/Default/Debug/Eq/…). Traits defined under other stdlib prefixes—**example:** `Serialize` / `Deserialize` in `std.serde.json`—are **not** discovered by this fallback. Extending derive-backed instance methods for those traits means adding entries (short term) or a **registry-driven** trait→module mapping (long term), not duplicating path logic in scattered call sites.

## Generic bounds and extern functions

- **Store explicit generic bounds in frontend symbols**, not just type-parameter names. A per-parameter bounds map lets call checking enforce `with` contracts before lowering.
- **Extern diagnostics need CLI-level wrapping**: typechecker catches declaration-shape issues, but signature/path mismatches fail in Cargo. Map rustc stderr back to `@rust.extern` spans for actionable diagnostics.
- **Centralize shared typechecker heuristics immediately**: if a naming rule or predicate is used in more than one checker module, extract it to a shared helper to avoid silent drift.
- **Manifest-backed type conversion must have one source of truth**: keep `TypeRef` ↔ `ResolvedType` mapping in a shared helper/module and reuse it across typechecker/LSP/codegen consumers. Duplicated conversion logic drifts and causes hard-to-debug manifest import regressions.

## RFC 041 (first-class Rust interop) implementation notes

- **Guardrail tests reject hardcoded builtin trait literals in compiler layers**: even in tests, avoid exact string literals like `"Clone"` in assertions; use `incan_core::lang::traits::as_str(TraitId::Clone)` (or equivalent canonical vocabulary helpers).
- **Examples runner can pick a stale repo-local release binary when `CARGO_TARGET_DIR` is redirected**: in Cursor/CI-like environments, Cargo may build to a non-default target dir while `scripts/run_examples.sh` prefers `./target/release/incan`. If this mismatch appears, pass `INCAN_BIN="$CARGO_TARGET_DIR/release/incan"` or make the script fallback aware.
- **Metadata-backed interop needs default-path coverage**: `rust-metadata` is optional, so Rust interop fixes that rely on canonical metadata must also preserve the default build path when receiver provenance degrades or the feature is off. For method lowering/coercion bugs, test both the metadata-enhanced path and a plain default build of a real interop example. (Issue #236)
- **`rusttype` example checks should be feature-aware**: `examples/rust_interop_rusttype/main.incn` can fail default smoke checks when relying on metadata-backed/static rebinding behavior that is only validated under `rust-metadata` feature paths. Keep a clear skip reason in `scripts/run_examples.sh` until that path is fully on by default.
