# Test corpus inventory

!!! warning "Generated control-plane reference"

    Do not edit this page by hand. Regenerate it with `make test-inventory` from the tests in the tree and `scripts/test_inventory/dispositions.json`; `make test-inventory-check` fails when it is stale.

This is the control plane for the slice-7 cutover (issue [#1561](https://github.com/encero-systems/incan/issues/1561), test corpus). Every Rust test under `loaves/` and `workspaces/` has a disposition by what it proves and how, not by where it lives; every `.incn` fixture root is listed with its case count. A disposition is a recorded decision, the lane signals beside it are the mechanical evidence, and a `retire` row may be deleted only once it names a twin. How to classify a test, record a twin, or plan a split is in [Work the test inventory](../how-to/work_the_test_inventory.md).

## Summary

| Disposition | Tests | Files | Fixture cases |
|---|---:|---:|---:|
| keep | 3079 | 140 | 7 |
| re-point | 529 | 41 | 368 |
| retire | 1103 | 72 | 0 |
| unaffected | 1393 | 131 | 4 |
| unreviewed | 0 | 0 | 0 |
| **Total** | **6104** | **384** | **379** |

- Retire-class tests with a named twin: 12/1103.
- Retire-class files with no twin at all: 78 (the `Twins` column reads `0/n`).
- Files whose test region exceeds the split threshold of 1500 lines: 28, of which 15 in the durable corpus (keep or re-point).
- Unreviewed files: 0.

## Dispositions

| Disposition | Meaning |
|---|---|
| `keep` | Asserts source meaning through the parser, typechecker, Body IR, lowering facts, formatter, LSP or semantics core. Survives the slice-7 cutover untouched. |
| `re-point` | Asserts program behaviour (output, exit code, diagnostics of a run) but proves it by building or running generated Rust. The assertion stays; slice 7 changes the route. |
| `retire` | Asserts the shape of the generated Rust itself: snapshot text, `contains("fn ...")` on emitted source, emitter unit tests. Dies with #654, and only after its twin exists. |
| `unaffected` | Oven, store, rustc, installer, stdlib runtime, formatter internals and other tests the cutover does not touch. Listed so the total reconciles. |
| `unreviewed` | Nobody has read the file yet. The mechanical proposal is recorded in the notes when there is one; the maintainer works these rows through. |

## Lane signals

The collector counts these in the text of each test function and of the file-local helpers it calls. They are evidence, not the verdict: the disposition column is what the reviewer recorded.

| Signal | Fires when the test |
|---|---|
| `codegen` | calls a codegen API (`IrCodegen`, `try_generate`, `generate_rust`, `emit_program`, `read_generated_rust`) |
| `snapshot` | asserts an `insta` snapshot |
| `generated_text` | asserts on generated Rust text (`contains("fn ")`, `contains("impl ")` and the like) |
| `build_run` | builds or runs a project (`run_incan`, `incan_command`, `run_explicit_oven_bake`, project runner helpers) |
| `replacement` | uses the replacement route or Body IR (`replacement::`, `shadow_support`, `body_ir`, `lower_typed_body_ir`, `execute_free_function`) |
| `checker` | typechecks or reads diagnostics (`TypeChecker`, `check_str`, `CompileError`, `CompilationSession`) |
| `parser` | lexes or parses (`parser::parse`, `parse_str`, `lexer::lex`) |
| `legacy_ir` | lowers through the Rust-source backend's own IR (`AstLowering`, `lower_program`, `IrProgram`, `IrType`), which #654 removes with the emitter |
| `formatter` | formats source (`format_source`, `incan_format`) |
| `lsp` | drives the language server (`incan_lsp::`, `tower_lsp`, `lsp_types::`, hover and completion params) |

## Fixture roots

`.incn` fixtures are programs; the compiler suite, the example runner and the verified documentation examples run them as programs, so they are inventoried by root rather than per file.

| Root | Pattern | Cases | Disposition | Owner | Notes |
|---|---|---:|---|---|---|
| `examples` | `**/*.incn` | 91 | re-point | #1561 | typechecked and run by scripts/run_examples.sh (`make examples`); vocab and library examples are baked. |
| `loaves/compiler/incan_driver/tests/fixtures` | `**/*.incn` | 20 | re-point | #1561 | driver integration fixtures (generated_rust_* artifact projects, callability, native consumer); their owner tests are retire-class. |
| `loaves/compiler/incan_emit/tests/codegen_snapshots` | `**/*.incn` | 180 | re-point | #1561 | snapshot corpus inputs considered as programs; the .snap outputs retire with codegen_snapshot_tests.rs. |
| `loaves/compiler/incan_test_support/fixtures` | `*.incn` | 12 | re-point | #1561 | top-level regression programs run by CLI integration tests (rfc023/rfc030/rfc064/rfc088 behaviour, reflection, model traits). |
| `loaves/compiler/incan_test_support/fixtures/invalid` | `**/*.incn` | 7 | keep | #1561 | diagnostics through the checker only (test_invalid_fixtures). |
| `loaves/compiler/incan_test_support/fixtures/oven_project_bake` | `**/*.incn` | 1 | re-point | #1561 | Incan project baked under Oven by the compiler suite; the program's route changes, the bake harness does not. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_app_bake` | `**/*.incn` | 1 | re-point | #1561 | release-profile Oven bake of an Incan app. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_bytes_io` | `**/*.incn` | 2 | re-point | #1561 | release-profile Oven bake exercising bytes I/O. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_file_lock` | `**/*.incn` | 2 | re-point | #1561 | release-profile Oven bake exercising file locks. |
| `loaves/compiler/incan_test_support/fixtures/valid` | `**/*.incn` | 28 | re-point | #1561 | typechecked in bulk by test_valid_fixtures (keep) and run one by one as std surface programs by CLI tests (re-point). |
| `loaves/oven/oven_rustc/src/fixtures` | `**/*.incn` | 4 | unaffected | #1561 | Oven rustc fixtures. |
| `loaves/toolchain/incan-cli/tests/fixtures` | `**/*.incn` | 8 | re-point | #1561 | CLI integration fixtures (layering, package boundary facade, pub union consumer, vocab guardrails). |
| `workspaces/docs-site/docs/_snippets/language/examples` | `verified_*.incn` | 8 | re-point | #1561 | verified documentation examples checked by scripts/check_docs_examples.sh. |
| `workspaces/oven/src` | `test_*.incn` | 11 | re-point | #1561 | Incan tests of the Oven release-policy project (workspaces/oven), run through `incan test`. |
| `workspaces/oven/tests/fixtures` | `**/*.incn` | 4 | re-point | #1561 | fixtures of the Oven release-policy project's tests. |

## Test files by crate

`Lines` is the file length; `Test lines` is the test region (the whole file for a test file, the `#[cfg(test)]` modules for a source file) that the split threshold applies to. `Twins` is `named/retire-class` for files with retire-class tests. Per-test rows follow a file only when it carries per-test overrides.

### `loaves/compiler/incan_driver` (745 tests in 98 files: keep 390, re-point 109, retire 103, unaffected 143)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_driver/src/backend/c_abi.rs` | 11 | 1074 | 333 | unaffected | - | - | #1561 | - | clang verification of checked C signatures; not the Rust backend. |
| `loaves/compiler/incan_driver/src/backend/project/cargo_toml.rs` | 22 | 1056 | 675 | retire | 0/22 | - | #1561 | run 20 | generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/backend/project/generator.rs` | 22 | 3250 | 1373 | retire | 0/22 | - | #1561 | codegen 5, text 9, run 20, checker 4 | generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/backend/project/lock_projection.rs` | 9 | 816 | 349 | retire | 0/9 | - | #1561 | - | generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/backend/project/plan.rs` | 2 | 229 | 23 | retire | 0/2 | - | #1561 | - | generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/backend/project/runner.rs` | 18 | 1031 | 449 | re-point | - | - | #1561 | codegen 2, run 14 | project runner: builds and runs the generated project; the run contract (profiles, rebuild triggers) is re-pointed at the replacement route. |
| `loaves/compiler/incan_driver/src/backend/project/tests/codegen_generator.rs` | 4 | 327 | 327 | retire | 0/4 | - | #1561 | codegen 4, text 1, run 4, checker 1, parser 3, legacy_ir 1 | codegen into a generated project; asserts generated Rust text. |
| `loaves/compiler/incan_driver/src/backend/project/tests/lock_payload.rs` | 1 | 26 | 26 | retire | 0/1 | - | #1561 | codegen 1, run 1 | generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/backend/shadow/abs_sum_profile_tests.rs` | 1 | 128 | 128 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/enumerate_zip_tests.rs` | 5 | 164 | 164 | re-point | - | - | #1561 | run 3, replacement 4, checker 4, parser 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/json_stringify_tests.rs` | 3 | 219 | 219 | re-point | - | - | #1561 | run 3, replacement 3 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/legacy_oven.rs` | 1 | 669 | 59 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/len_string_tests.rs` | 2 | 82 | 82 | re-point | - | - | #1561 | run 2, replacement 2, checker 2 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/tests.rs` | 47 | 1253 | 1253 | re-point (re-point 46, retire 1) | 0/1 | - | #1561 | run 3, replacement 30 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. Result-report transport tests are the comparison harness itself. |
| `loaves/compiler/incan_driver/src/build/bake.rs` | 5 | 1137 | 172 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/caller_owned.rs` | 7 | 1044 | 315 | unaffected | - | - | #1561 | checker 4 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/inline_command.rs` | 5 | 129 | 75 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/library_exports.rs` | 12 | 933 | 559 | keep | - | - | #1561 | checker 8, parser 9 | library re-export resolution and Rust ABI query paths from checked declarations. |
| `loaves/compiler/incan_driver/src/build/library_outputs.rs` | 3 | 253 | 59 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/library_publication.rs` | 6 | 609 | 218 | unaffected | - | - | #1561 | checker 6 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/mod.rs` | 3 | 938 | 79 | unaffected | - | - | #1561 | replacement 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_materialization.rs` | 2 | 1273 | 411 | unaffected | - | - | #1561 | replacement 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_paths.rs` | 5 | 971 | 228 | unaffected | - | - | #1561 | codegen 1, run 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_selection.rs` | 1 | 1091 | 358 | unaffected | - | - | #1561 | replacement 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/oven_project.rs` | 6 | 1326 | 148 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/package_loafs.rs` | 2 | 1014 | 435 | unaffected | - | - | #1561 | text 1, checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/plan_authority.rs` | 4 | 1062 | 191 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/plan_selection.rs` | 6 | 1022 | 469 | unaffected | - | - | #1561 | codegen 1, run 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/prepare_project.rs` | 1 | 497 | 74 | retire | 0/1 | - | #1561 | codegen 1, run 1, checker 1 | prunes the generated project's Cargo dependencies; generated Cargo project shape; dies with the generated-project route unless #654 keeps generated projects for inspection. |
| `loaves/compiler/incan_driver/src/build/provider_compilation.rs` | 6 | 1010 | 529 | unaffected | - | - | #1561 | checker 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/provider_metadata.rs` | 5 | 1111 | 266 | keep | - | - | #1561 | checker 3, parser 3 | provider operation metadata projected from checked declaration facts. |
| `loaves/compiler/incan_driver/src/build/publication.rs` | 1 | 798 | 63 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/replacement.rs` | 5 | 533 | 161 | keep | - | - | #1561 | replacement 4, checker 4, parser 1 | replacement build pipeline (session projection, exact numeric report). |
| `loaves/compiler/incan_driver/src/build/reuse.rs` | 1 | 679 | 35 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/source_authority.rs` | 19 | 1846 | 1171 | unaffected | - | - | #1561 | checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build_unit.rs` | 1 | 202 | 27 | unaffected | - | - | #1561 | checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/cargo_policy.rs` | 4 | 270 | 92 | retire | 0/4 | - | #1561 | - | Cargo flag policy of the generated-project route. |
| `loaves/compiler/incan_driver/src/generated_cache.rs` | 18 | 1510 | 447 | retire | 0/18 | - | #1561 | - | generated-project cache identity and pruning; dies with the generated-project route. |
| `loaves/compiler/incan_driver/src/inspect/closure.rs` | 8 | 409 | 224 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/inspect/codegraph.rs` | 13 | 4549 | 849 | keep | - | - | #1561 | replacement 3, checker 9, parser 8 | codegraph projection from checked facts. |
| `loaves/compiler/incan_driver/src/lock/mod.rs` | 2 | 558 | 159 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/registry_sources.rs` | 3 | 517 | 48 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/resolution.rs` | 3 | 751 | 85 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/rust_inspect.rs` | 2 | 536 | 97 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/test_inputs.rs` | 2 | 200 | 91 | unaffected | - | - | #1561 | checker 2, parser 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/workspace.rs` | 1 | 587 | 45 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/modules.rs` | 23 | 2041 | 1166 | keep | - | - | #1561 | replacement 1, checker 19, parser 20 | module collection and Rust dependency use discovery through the parser. |
| `loaves/compiler/incan_driver/src/project.rs` | 9 | 407 | 105 | unaffected | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/replacement_compatibility.rs` | 12 | 3938 | 310 | keep | - | - | #1561 | replacement 12, formatter 10 | replacement compatibility inventory collector and validator. |
| `loaves/compiler/incan_driver/src/rust_inspect_workspace.rs` | 19 | 1885 | 867 | unaffected | - | - | #1561 | codegen 5, text 1, run 9, checker 1 | generated rust-inspect Cargo workspace that feeds the checker with Rust metadata; not the Rust backend. |
| `loaves/compiler/incan_driver/src/session.rs` | 4 | 911 | 175 | keep | - | - | #1561 | checker 4, parser 2 | CompilationSession analysis (provider plan reuse, feature projection). |
| `loaves/compiler/incan_driver/src/shadow_support.rs` | 2 | 252 | 28 | re-point | - | - | #1561 | checker 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/testing/discovery.rs` | 33 | 1973 | 829 | keep | - | - | #1561 | checker 32, parser 32 | test discovery through the parser. |
| `loaves/compiler/incan_driver/src/testing/module_graph.rs` | 3 | 412 | 128 | keep | - | - | #1561 | checker 1, parser 3 | test-runner module graph through the parser. |
| `loaves/compiler/incan_driver/src/tests/executable_session.rs` | 1 | 81 | 81 | keep | - | - | #1561 | replacement 1, checker 1 | canonical frames through a checked cycle. |
| `loaves/compiler/incan_driver/src/typecheck.rs` | 3 | 356 | 73 | keep | - | - | #1561 | checker 2 | typecheck over the import graph. |
| `loaves/compiler/incan_driver/tests/duration_artifact_tests.rs` | 1 | 67 | 67 | re-point | - | - | #1561 | run 1 | runs a program and checks the duration artifact. |
| `loaves/compiler/incan_driver/tests/emitted_symbol_projection_tests.rs` | 1 | 38 | 38 | retire | 0/1 | - | #1561 | - | RFC 120 emitted symbol projection and demangling; generated Rust shape. |
| `loaves/compiler/incan_driver/tests/fixtures/generated_rust_native_consumer/consumer/src/lib.rs` | 1 | 47 | 47 | retire | 0/1 | - | #1561 | - | fixture consumer of generated_rust_native_consumer_tests; follows its owner. |
| `loaves/compiler/incan_driver/tests/generated_cache_integration.rs` | 4 | 468 | 468 | retire | 0/4 | - | #1561 | run 4 | generated-project cache across builds; dies with the generated-project route. |
| `loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs` | 5 | 567 | 567 | retire | 0/5 | - | #1561 | run 5, checker 2 | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit); survives only if #654 keeps generated artifacts as inspection output. |
| `loaves/compiler/incan_driver/tests/generated_rust_audit_tests.rs` | 4 | 193 | 193 | retire | 0/4 | - | #1561 | - | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit); survives only if #654 keeps generated artifacts as inspection output. |
| `loaves/compiler/incan_driver/tests/generated_rust_callability_artifact_tests.rs` | 1 | 258 | 258 | retire | 0/1 | - | #1561 | text 1, run 1, checker 1 | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit); survives only if #654 keeps generated artifacts as inspection output. |
| `loaves/compiler/incan_driver/tests/generated_rust_native_consumer_tests.rs` | 1 | 367 | 367 | retire | 0/1 | - | #1561 | run 1 | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit); survives only if #654 keeps generated artifacts as inspection output. |
| `loaves/compiler/incan_driver/tests/parity_corpus_tests.rs` | 28 | 5792 | 5792 | keep | - | required | #1561 | replacement 22 | the parity corpus: slice 7's own measurement instrument (RFC 120 coverage, replacement receipts, corpus validation). |
| `loaves/compiler/incan_driver/tests/protected_builtin_binding_tests.rs` | 3 | 141 | 141 | keep | - | - | #1561 | checker 3, parser 3 | lex/parse/typecheck only. |
| `loaves/compiler/incan_driver/tests/protected_generic_binding_tests.rs` | 2 | 106 | 106 | keep | - | - | #1561 | checker 2, parser 2 | lex/parse/typecheck only. |
| `loaves/compiler/incan_driver/tests/replacement_abs_sum_profile_shadow_tests.rs` | 1 | 154 | 154 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_abs_sum_profile_tests.rs` | 4 | 199 | 199 | keep | - | - | #1561 | run 1, replacement 3, checker 3, parser 3 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests.rs` | 118 | 5699 | 5699 | keep (keep 117, retire 1) | 0/1 | required | #1561 | codegen 1, run 37, replacement 82, checker 81, parser 81 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. Two tests compare against the legacy backend. |
| `loaves/compiler/incan_driver/tests/replacement_bool_truthiness_shadow_tests.rs` | 1 | 77 | 77 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_bool_truthiness_tests.rs` | 6 | 198 | 198 | keep | - | - | #1561 | run 1, replacement 5, checker 5, parser 5 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_collection_len_shadow_tests.rs` | 1 | 77 | 77 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_collection_len_tests.rs` | 4 | 153 | 153 | keep | - | - | #1561 | run 1, replacement 3, checker 3, parser 3 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_compatibility_registry_tests.rs` | 6 | 360 | 360 | keep | - | - | #1561 | - | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_boundary_tests.rs` | 10 | 728 | 728 | keep | - | - | #1561 | replacement 10, checker 10, parser 10 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_parity_cases.rs` | 4 | 162 | 162 | re-point | - | - | #1561 | run 4, replacement 4 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_shadow_tests.rs` | 1 | 128 | 128 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_tests.rs` | 15 | 552 | 552 | keep | - | - | #1561 | run 2, replacement 13, checker 13, parser 13 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_example_coverage.rs` | 1 | 215 | 215 | keep | - | - | #1561 | replacement 1, checker 1, parser 1 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_container_boundary_tests.rs` | 1 | 48 | 48 | keep | - | - | #1561 | replacement 1, checker 1, parser 1 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_execution_tests.rs` | 12 | 260 | 260 | keep | - | - | #1561 | run 1, replacement 11, checker 10, parser 10 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_shadow_tests.rs` | 1 | 81 | 81 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_isinstance_shadow_tests.rs` | 1 | 75 | 75 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_isinstance_tests.rs` | 8 | 334 | 334 | keep | - | - | #1561 | replacement 8, checker 8, parser 8 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_program_io_tests.rs` | 5 | 144 | 144 | keep | - | - | #1561 | run 5 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_scalar_conversion_shadow_tests.rs` | 7 | 358 | 358 | re-point | - | - | #1561 | run 7, replacement 7 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_scalar_conversion_tests.rs` | 16 | 803 | 803 | keep | - | - | #1561 | run 1, replacement 15, checker 15, parser 15 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_sorted_int_list_shadow_tests.rs` | 1 | 78 | 78 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_sorted_int_list_tests.rs` | 7 | 223 | 223 | keep | - | - | #1561 | run 1, replacement 6, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_stream_writer_tests.rs` | 7 | 239 | 239 | keep | - | - | #1561 | replacement 6, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_helper_execution_tests.rs` | 7 | 238 | 238 | keep | - | - | #1561 | run 1, replacement 7, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_helper_shadow_tests.rs` | 2 | 96 | 96 | re-point | - | - | #1561 | run 2, replacement 2 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_string_len_execution_tests.rs` | 4 | 96 | 96 | keep | - | - | #1561 | replacement 4, checker 4, parser 4 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_len_shadow_tests.rs` | 1 | 50 | 50 | re-point | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_typed_numeric_tests.rs` | 13 | 581 | 581 | keep | - | - | #1561 | replacement 13, checker 13, parser 13 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/shadow_comparison_tests.rs` | 9 | 451 | 451 | re-point | - | - | #1561 | run 9, replacement 9 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/stdlib_version_artifact_tests.rs` | 1 | 132 | 132 | retire | 0/1 | - | #1561 | codegen 1, parser 1 | asserts the stdlib version check inside the generated artifact. |

Per-test overrides in `loaves/compiler/incan_driver/src/backend/shadow/tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `the_generated_entrypoint_writes_a_typed_result_without_touching_program_streams` | retire | - | replacement | asserts generated Rust text |

Per-test overrides in `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `replacement_refuses_a_nominal_pattern_after_its_exact_target_identity_is_removed` | keep | - | replacement, checker, parser | generated-text hit is a diagnostic string |
| `both_backends_render_a_multi_argument_print_the_same_way` | retire | - | codegen, replacement, checker, parser | compares replacement output with IrCodegen output; the replacement-only assertion is the twin |

### `loaves/compiler/incan_emit` (922 tests in 54 files: keep 100, retire 822)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_emit/src/checked_program/borrowed_rust_enum.rs` | 3 | 181 | 181 | retire | 0/3 | - | #1561 | codegen 1, checker 3, parser 3 | checked-program codegen of borrowed Rust enums. |
| `loaves/compiler/incan_emit/src/checked_program/embedded_fragment.rs` | 1 | 130 | 130 | retire | 0/1 | - | #1561 | codegen 1, checker 1, parser 1, legacy_ir 1 | checked-program codegen of embedded fragments. |
| `loaves/compiler/incan_emit/src/checked_program/rust_supertrait_codegen.rs` | 2 | 76 | 76 | retire | 0/2 | - | #1561 | codegen 2, text 1, checker 1, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/src/checked_program/rust_trait_receiver_codegen.rs` | 2 | 196 | 196 | retire | 0/2 | - | #1561 | codegen 2, text 1, checker 2, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/src/checked_program/sdk_module_derives.rs` | 5 | 257 | 257 | retire (keep 2, retire 3) | 0/3 | - | #1561 | codegen 3, checker 5, parser 5 | SDK module derive requirements flowing into codegen; two tests assert the metadata round trip only. |
| `loaves/compiler/incan_emit/src/checked_program/tests.rs` | 5 | 495 | 495 | keep (keep 4, retire 1) | 0/1 | - | #1561 | codegen 1, replacement 1, checker 4, parser 4, legacy_ir 1 | checked-program facts (callable shapes, forwarding metadata, vocab refusal); one test drives codegen. |
| `loaves/compiler/incan_emit/src/codegen.rs` | 129 | 8039 | 5500 | retire | 0/129 | required | #1561 | codegen 122, text 52, checker 112, parser 82, legacy_ir 115 | IrCodegen entry point and emitter-side metadata; every test drives IrCodegen or emitter-owned merges (manifest type refs, native-union capture). Split with the retirement, not before. |
| `loaves/compiler/incan_emit/src/codegen/capability_bridge.rs` | 1 | 187 | 18 | retire | 0/1 | - | #1561 | parser 1 | codegen capability activation for generated projects. |
| `loaves/compiler/incan_emit/src/codegen/dependency_metadata.rs` | 6 | 1086 | 167 | retire | 0/6 | - | #1561 | checker 1, parser 5, legacy_ir 1 | which stdlib and provider items the generated project links; generated-project shape. |
| `loaves/compiler/incan_emit/src/conversions.rs` | 79 | 2805 | 1578 | retire | 0/79 | required | #1561 | legacy_ir 74 | Rust conversion policy (to_string/borrow/clone plans) for emission; the duckborrower facts in Body IR are the twin surface. |
| `loaves/compiler/incan_emit/src/emit/decls/functions.rs` | 3 | 1997 | 115 | retire | 0/3 | - | #1561 | codegen 3, legacy_ir 3 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/decls/mod.rs` | 5 | 1225 | 237 | retire | 0/5 | - | #1561 | codegen 2, checker 2, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/decls/structures.rs` | 8 | 1212 | 244 | retire | 0/8 | - | #1561 | codegen 8, text 7, legacy_ir 8, formatter 8 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/errors.rs` | 1 | 45 | 15 | retire | 0/1 | - | #1561 | - | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/builtins.rs` | 5 | 1087 | 174 | retire | 0/5 | - | #1561 | codegen 3, legacy_ir 4 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/calls.rs` | 29 | 2805 | 1278 | retire | 0/29 | - | #1561 | codegen 29, legacy_ir 29 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/indexing.rs` | 2 | 474 | 68 | retire | 0/2 | - | #1561 | codegen 2, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/mod.rs` | 52 | 4409 | 2798 | retire | 0/52 | required | #1561 | codegen 52, legacy_ir 52 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/structs_enums.rs` | 1 | 145 | 20 | retire | 0/1 | - | #1561 | codegen 1, legacy_ir 1 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/mod.rs` | 20 | 4605 | 952 | retire | 0/20 | - | #1561 | codegen 15, checker 4, legacy_ir 18 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/native_unions.rs` | 8 | 2072 | 1082 | retire | 0/8 | - | #1561 | codegen 6, checker 7, parser 7, legacy_ir 6 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/program.rs` | 10 | 4635 | 348 | retire | 0/10 | - | #1561 | codegen 10, text 2, checker 6, legacy_ir 10 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/statements.rs` | 8 | 1942 | 315 | retire | 0/8 | - | #1561 | codegen 7, text 4, legacy_ir 8 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/types.rs` | 3 | 689 | 61 | retire | 0/3 | - | #1561 | codegen 3, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/ownership.rs` | 45 | 1947 | 853 | retire | 0/45 | - | #1561 | legacy_ir 45 | emitter-side ownership and argument plans (borrow/clone/move for generated Rust); twins belong to Body IR ownership facts. |
| `loaves/compiler/incan_emit/src/reference_shape.rs` | 3 | 107 | 65 | retire | 0/3 | - | #1561 | legacy_ir 3 | Rust reference-shape helper for emitted callbacks. |
| `loaves/compiler/incan_emit/src/replacement/executable_resolution_tests.rs` | 17 | 1102 | 1102 | keep | - | - | #1561 | codegen 1, replacement 16, checker 17, parser 17 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/hashed/tests.rs` | 15 | 314 | 314 | keep | - | - | #1561 | replacement 15 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/mod.rs` | 4 | 8103 | 262 | keep | - | - | #1561 | replacement 4 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/provider/tests.rs` | 15 | 1004 | 1004 | keep | - | - | #1561 | replacement 15, checker 15, parser 15 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/provider/tests/host_preflight_tests.rs` | 13 | 589 | 589 | keep | - | - | #1561 | replacement 13 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/source_profile.rs` | 4 | 272 | 128 | keep | - | - | #1561 | replacement 3, parser 3 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/selection.rs` | 13 | 970 | 323 | keep | - | - | #1561 | replacement 13 | backend selection policy (replacement vs legacy); survives as the replacement route's own selection. |
| `loaves/compiler/incan_emit/src/tests/lowering_through_emission.rs` | 2 | 331 | 331 | retire | 0/2 | - | #1561 | codegen 1, checker 2, parser 2, legacy_ir 2 | lowering through emission end to end. |
| `loaves/compiler/incan_emit/src/trait_bound_inference.rs` | 15 | 4401 | 703 | retire | 0/15 | - | #1561 | codegen 5, legacy_ir 15 | infers Rust trait bounds for generated generics; Rust-shape concern. |
| `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs` | 14 | 675 | 675 | retire (keep 5, retire 9) | 1/9 | - | #1561 | codegen 9, snapshot 1, text 8, replacement 4, checker 14, parser 14 | empty-collection constructors: generated-text and snapshot assertions retire; the checked-type and Body IR aggregate assertions stay. |
| `loaves/compiler/incan_emit/tests/closure_local_call_codegen_tests.rs` | 4 | 208 | 208 | retire | 0/4 | - | #1561 | codegen 4, text 1, checker 4, parser 4 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/codegen_snapshot_tests.rs` | 289 | 6771 | 6771 | retire | 0/289 | required | #1561 | codegen 289, snapshot 1, text 31, checker 7, parser 289 | insta snapshot corpus of generated Rust; the .incn inputs under loaves/compiler/incan_emit/tests/codegen_snapshots are inventoried as a fixture root (re-point) and are the twin surface once run as programs. |
| `loaves/compiler/incan_emit/tests/construction_diagnostics_tests.rs` | 2 | 52 | 52 | keep | - | - | #1561 | checker 2, parser 2 | typechecker diagnostics for model construction. |
| `loaves/compiler/incan_emit/tests/constructor_argument_order_tests.rs` | 4 | 161 | 161 | retire | 4/4 | - | #1561 | codegen 1, checker 3, parser 4, legacy_ir 3 | argument sequencing asserted through the legacy IR and generated Rust; the CLI regression for issue 1462 runs the same program. |
| `loaves/compiler/incan_emit/tests/empty_list_comparison_operand_tests.rs` | 5 | 143 | 143 | keep (keep 3, retire 2) | 1/2 | - | #1561 | codegen 1, text 1, checker 4, parser 5, legacy_ir 1 | checker facts for empty-list operands; the lowered/generated assertions retire. |
| `loaves/compiler/incan_emit/tests/enumerate_value_codegen_tests.rs` | 1 | 71 | 71 | retire | 0/1 | - | #1561 | codegen 1, parser 1 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/fallible_entrypoint_codegen_tests.rs` | 1 | 50 | 50 | retire | 0/1 | - | #1561 | codegen 1, snapshot 1, parser 1 | snapshot of the generated entrypoint. |
| `loaves/compiler/incan_emit/tests/generated_stdlib_version_tests.rs` | 2 | 24 | 24 | retire | 0/2 | - | #1561 | codegen 1 | the stdlib line the emitter declares in generated code; the version contract moves with the emitter. |
| `loaves/compiler/incan_emit/tests/implicit_borrowing_codegen_tests.rs` | 21 | 719 | 719 | retire | 0/21 | - | #1561 | codegen 21, text 1, checker 7, parser 21, legacy_ir 7 | implicit borrowing asserted on generated Rust; duckborrower facts are the twin surface. |
| `loaves/compiler/incan_emit/tests/lowering_error_propagation.rs` | 1 | 33 | 33 | retire | 0/1 | - | #1561 | legacy_ir 1 | legacy IR lowering error propagation. |
| `loaves/compiler/incan_emit/tests/match_arm_ownership_codegen_tests.rs` | 4 | 151 | 151 | retire | 0/4 | - | #1561 | codegen 4, parser 4 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/nested_list_loop_tests.rs` | 5 | 156 | 156 | keep (keep 3, retire 2) | 1/2 | - | #1561 | codegen 1, checker 4, parser 5, legacy_ir 1 | checker facts for nested empty lists (issue 1471); one generated-text and one legacy IR assertion retire. |
| `loaves/compiler/incan_emit/tests/qualified_type_annotation_codegen_tests.rs` | 3 | 167 | 167 | retire | 0/3 | - | #1561 | codegen 3, parser 3 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/return_operand_ownership_tests.rs` | 5 | 205 | 205 | retire | 5/5 | - | #1561 | checker 5, parser 5, legacy_ir 5 | return-operand move/read decisions asserted through the legacy IR; the CLI regression for issue 1489 runs the loop-variable case, the other cases still need Body IR ownership twins. |
| `loaves/compiler/incan_emit/tests/stdlib_generated_rust_snapshot_tests.rs` | 14 | 434 | 434 | retire | 0/14 | - | #1561 | codegen 14, snapshot 13, parser 14 | insta snapshots of generated stdlib Rust. |
| `loaves/compiler/incan_emit/tests/string_helper_codegen_tests.rs` | 1 | 45 | 45 | retire | 0/1 | - | #1561 | codegen 1, snapshot 1, text 1, parser 1 | snapshot and generated text. |
| `loaves/compiler/incan_emit/tests/string_len_codegen_tests.rs` | 2 | 43 | 43 | retire | 0/2 | - | #1561 | codegen 2, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/zip_alias_codegen_tests.rs` | 10 | 345 | 345 | retire | 0/10 | - | #1561 | codegen 7, parser 7, legacy_ir 3 | generated clone placement for zip aliases; three assignment-plan unit tests are emitter ownership plans and retire with them. |

Per-test overrides in `loaves/compiler/incan_emit/src/checked_program/sdk_module_derives.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `sdk_module_derives_metadata_round_trip_preserves_exact_membership` | keep | - | checker, parser | metadata round trip through the checker, no codegen |
| `sdk_module_derives_missing_membership_stays_rejected` | keep | - | checker, parser | checker rejection, no codegen |

Per-test overrides in `loaves/compiler/incan_emit/src/checked_program/tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `rust_trait_associated_call_uses_callable_shape_from_dependency_namespace_source` | retire | - | codegen, checker, parser | drives IrCodegen directly |

Per-test overrides in `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `checked_empty_collection_constructors_lower_to_existing_aggregate_shapes_issue1247` | keep | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `checked_empty_list_constructor_lowers_to_the_list_aggregate_issue1464` | keep | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `mismatched_typed_empty_collection_constructor_contexts_remain_errors_issue1247` | keep | - | checker, parser | checked result type or Body IR aggregate shape |
| `shadowed_collection_constructors_do_not_gain_aggregate_lowering_issue1247` | keep | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `missing_checked_collection_constructor_facts_do_not_guess_aggregate_lowering_issue1247` | keep | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `checked_typed_empty_dict_reaches_existing_aggregate_emission_issue1247` | retire | `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs::checked_empty_collection_constructors_lower_to_existing_aggregate_shapes_issue1247` | codegen, snapshot, checker, parser | snapshot of the emitted aggregate |

Per-test overrides in `loaves/compiler/incan_emit/tests/empty_list_comparison_operand_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `lowered_empty_list_operand_carries_the_element_type` | retire | - | checker, parser, legacy_ir | legacy IR shape |
| `generated_rust_names_the_element_type_on_the_empty_operand` | retire | `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs::empty_list_equality_operands_build_with_json_cohort_issue1476` | codegen, generated_text, parser | asserts generated Rust text |

Per-test overrides in `loaves/compiler/incan_emit/tests/nested_list_loop_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `nested_list_empty_holes_retain_checked_string_types` | retire | - | checker, parser, legacy_ir | legacy IR shape |
| `nested_list_loop_emits_owned_strings_without_caller_annotation` | retire | `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs::nested_empty_first_list_runs_without_a_caller_annotation_issue1471` | codegen, parser | asserts generated Rust text |

### `loaves/compiler/incan_format` (191 tests in 5 files: keep 191)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_format/src/config.rs` | 24 | 275 | 206 | keep | - | - | #1561 | - | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/formatter/tests.rs` | 11 | 327 | 327 | keep | - | - | #1561 | parser 7, formatter 11 | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/lib.rs` | 112 | 2852 | 2623 | keep | - | required | #1561 | checker 1, parser 110, formatter 110 | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/writer.rs` | 37 | 565 | 389 | keep | - | - | #1561 | - | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/tests/property_tests.rs` | 7 | 411 | 411 | keep | - | - | #1561 | parser 4, formatter 6 | formatter; no emit/driver dependency. Reviewed at crate level. |

### `loaves/compiler/incan_frontend` (1707 tests in 42 files: keep 1707)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_frontend/src/api_metadata.rs` | 17 | 3762 | 944 | keep | - | - | #1561 | checker 17, parser 17 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/body_ir.rs` | 2 | 1094 | 49 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/body_ir/tests.rs` | 247 | 8012 | 8012 | keep | - | required | #1561 | replacement 238, checker 241, parser 244 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/compiler_stack.rs` | 3 | 113 | 41 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/contract_metadata.rs` | 5 | 517 | 89 | keep | - | - | #1561 | parser 2, formatter 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/hir.rs` | 7 | 540 | 328 | keep | - | - | #1561 | checker 7, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_exports.rs` | 6 | 2104 | 179 | keep | - | - | #1561 | checker 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/artifact.rs` | 16 | 1677 | 937 | keep | - | - | #1561 | checker 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/published_layout.rs` | 2 | 287 | 47 | keep | - | - | #1561 | checker 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests.rs` | 75 | 4445 | 4445 | keep | - | required | #1561 | checker 72, parser 1 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/type_projection.rs` | 1 | 502 | 69 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest_index.rs` | 10 | 1387 | 468 | keep | - | - | #1561 | checker 8 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/module.rs` | 33 | 1659 | 913 | keep | - | - | #1561 | parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/features.rs` | 11 | 1919 | 578 | keep | - | - | #1561 | checker 10 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/plan.rs` | 15 | 2789 | 697 | keep | - | - | #1561 | checker 13 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/sdk.rs` | 13 | 1305 | 270 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/stdlib_sources.rs` | 2 | 223 | 71 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/resolved_type_subst.rs` | 1 | 217 | 45 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/rust_type_display.rs` | 7 | 754 | 171 | keep | - | - | #1561 | parser 6 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/surface_semantics.rs` | 3 | 138 | 48 | keep | - | - | #1561 | parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/symbols.rs` | 16 | 3053 | 462 | keep | - | - | #1561 | checker 10 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/testing_markers.rs` | 7 | 871 | 164 | keep | - | - | #1561 | checker 1, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/canonical_identity_tests.rs` | 75 | 3233 | 3233 | keep | - | required | #1561 | checker 75, parser 75 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/check_expr/calls/rust_boundary.rs` | 43 | 2524 | 1425 | keep | - | - | #1561 | text 2, checker 43 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/collect/stdlib_imports.rs` | 1 | 4825 | 32 | keep | - | - | #1561 | checker 1 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/identity_surface_tests.rs` | 8 | 284 | 284 | keep | - | - | #1561 | checker 8, parser 8 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/stdlib_loader.rs` | 31 | 3072 | 1113 | keep | - | - | #1561 | parser 30 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/tests.rs` | 952 | 25942 | 25942 | keep | - | required | #1561 | checker 944, parser 898 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_supertraits.rs` | 7 | 164 | 164 | keep | - | - | #1561 | checker 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_trait_import_candidates.rs` | 6 | 155 | 155 | keep | - | - | #1561 | checker 6, parser 6 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_trait_qualified_calls.rs` | 9 | 325 | 325 | keep | - | - | #1561 | checker 8, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/validate_rust_module.rs` | 2 | 270 | 30 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_ast_bridge.rs` | 12 | 1896 | 491 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_desugar_pass/helper_bindings.rs` | 15 | 829 | 455 | keep | - | - | #1561 | checker 11 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_desugar_pass/runtime.rs` | 4 | 860 | 78 | keep | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/checked_string_helper_identity_tests.rs` | 5 | 389 | 389 | keep | - | - | #1561 | replacement 4, checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/checked_string_len_identity_tests.rs` | 5 | 158 | 158 | keep | - | - | #1561 | replacement 4, checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/declaration_identity_corpus.rs` | 3 | 348 | 348 | keep | - | - | #1561 | replacement 2, checker 3, parser 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_core_parity.rs` | 6 | 125 | 125 | keep | - | - | #1561 | checker 2, parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_core_parity_strings.rs` | 10 | 203 | 203 | keep | - | - | #1561 | checker 2, parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_digest_invariants.rs` | 9 | 293 | 293 | keep | - | - | #1561 | replacement 9, checker 9, parser 9 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/stdlib_module_trait_tests.rs` | 5 | 132 | 132 | keep | - | - | #1561 | checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |

### `loaves/compiler/incan_ir` (146 tests in 11 files: retire 146)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_ir/src/decl.rs` | 1 | 711 | 11 | retire | 0/1 | - | #1561 | - | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/expr.rs` | 3 | 1165 | 81 | retire | 0/3 | - | #1561 | legacy_ir 3 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lib.rs` | 5 | 793 | 136 | retire | 0/5 | - | #1561 | legacy_ir 5 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/helpers.rs` | 3 | 883 | 150 | retire | 0/3 | - | #1561 | legacy_ir 3 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/methods.rs` | 1 | 2233 | 78 | retire | 0/1 | - | #1561 | checker 1, parser 1, legacy_ir 1 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/traits.rs` | 3 | 279 | 45 | retire | 0/3 | - | #1561 | legacy_ir 3 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/expr/calls.rs` | 20 | 5372 | 1079 | retire | 0/20 | - | #1561 | checker 9, parser 4, legacy_ir 18 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/expr/mod.rs` | 15 | 3135 | 351 | retire | 0/15 | - | #1561 | checker 3, legacy_ir 15 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/mod.rs` | 30 | 5189 | 1095 | retire | 0/30 | - | #1561 | checker 25, parser 26, legacy_ir 30 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/types.rs` | 13 | 1864 | 343 | retire | 0/13 | - | #1561 | checker 1, legacy_ir 13 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/types.rs` | 52 | 1305 | 464 | retire | 0/52 | - | #1561 | legacy_ir 52 | Rust-source backend lowering (AstLowering, IrProgram, IrType, Rust name spellings); #654 removes it with the emitter. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests.rs). Reviewed at crate level. |

### `loaves/compiler/incan_test_support` (4 tests in 2 files: retire 3, unaffected 1)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/compiler/incan_test_support/src/builtin_stdlib.rs` | 1 | 75 | 16 | unaffected | - | - | #1561 | - | test-support helper for the builtin stdlib inventory. |
| `loaves/compiler/incan_test_support/src/emitted_symbol_artifact.rs` | 3 | 421 | 50 | retire | 0/3 | - | #1561 | - | helpers over emitted symbol projections (RFC 120 physical names). |

### `loaves/kernel/incan_lang` (106 tests in 20 files: keep 106)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/kernel/incan_lang/src/bin/generate_vscode_grammar_keywords.rs` | 2 | 161 | 35 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/errors.rs` | 6 | 327 | 85 | keep | - | - | #1561 | checker 1 | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/interop/coercions.rs` | 6 | 448 | 84 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/interop/metadata.rs` | 21 | 2022 | 396 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/builtins.rs` | 1 | 339 | 12 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/c_abi.rs` | 1 | 465 | 74 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/callables.rs` | 1 | 90 | 14 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/conventions.rs` | 1 | 55 | 13 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/highlighting.rs` | 4 | 217 | 56 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/keywords.rs` | 3 | 939 | 25 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/stdlib.rs` | 11 | 1452 | 473 | keep | - | - | #1561 | checker 1 | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/text_codecs.rs` | 2 | 95 | 36 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/traits.rs` | 1 | 373 | 24 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/types/numerics.rs` | 4 | 469 | 79 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lib.rs` | 5 | 331 | 99 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/numeric_strings.rs` | 6 | 204 | 113 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/numeric_values.rs` | 3 | 222 | 53 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/strings.rs` | 2 | 366 | 30 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/tests/lang_registry_guardrails.rs` | 25 | 671 | 671 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/tests/string_len_semantics.rs` | 1 | 12 | 12 | keep | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |

### `loaves/kernel/incan_semantics_core` (128 tests in 13 files: keep 128)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/kernel/incan_semantics_core/src/authority.rs` | 9 | 344 | 177 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/body_ir.rs` | 32 | 4463 | 1080 | keep | - | - | #1561 | replacement 1 | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/closure_digest.rs` | 12 | 430 | 226 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/dependencies.rs` | 4 | 317 | 155 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/emitted_symbol.rs` | 7 | 559 | 177 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/executable_representation.rs` | 13 | 1308 | 583 | keep | - | - | #1561 | replacement 13 | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/facts.rs` | 12 | 1494 | 408 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/hir.rs` | 2 | 242 | 66 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/namespace.rs` | 7 | 148 | 78 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/receipts.rs` | 12 | 1014 | 431 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/semantic_digest.rs` | 4 | 502 | 84 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/stable_identity.rs` | 7 | 444 | 245 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/types.rs` | 7 | 435 | 95 | keep | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |

### `loaves/kernel/incan_syntax` (307 tests in 6 files: keep 307)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/kernel/incan_syntax/src/ast/types.rs` | 1 | 252 | 39 | keep | - | - | #1561 | - | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/diagnostics/base.rs` | 2 | 396 | 39 | keep | - | - | #1561 | checker 1 | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/diagnostics/stable.rs` | 5 | 605 | 121 | keep | - | - | #1561 | checker 5 | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/lexer/mod.rs` | 20 | 909 | 400 | keep | - | - | #1561 | checker 20, parser 20 | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/embedded/tests.rs` | 28 | 914 | 914 | keep | - | - | #1561 | checker 24, parser 4 | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests.rs` | 251 | 6673 | 6673 | keep | - | required | #1561 | checker 226, parser 251, formatter 2 | lexer, parser and diagnostics catalogue; below the emitter, cannot reach codegen. Reviewed at crate level. |

### `loaves/toolchain/incan-cli` (617 tests in 38 files: keep 58, re-point 420, retire 28, unaffected 111)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/toolchain/incan-cli/src/bin/generate_feature_inventory.rs` | 1 | 85 | 23 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/build.rs` | 11 | 1317 | 620 | keep (keep 8, retire 3) | 0/3 | - | #1561 | replacement 6, checker 3, parser 1 | build command over checked facts (contract step, library publication); the rustc-failure classification tests belong to the generated build. |
| `loaves/toolchain/incan-cli/src/commands/cache.rs` | 2 | 131 | 17 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/debug.rs` | 1 | 178 | 22 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/init.rs` | 16 | 755 | 316 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/lifecycle.rs` | 4 | 924 | 104 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/representation_inspect.rs` | 4 | 503 | 126 | keep | - | - | #1561 | checker 4 | representation inspection from checked facts. |
| `loaves/toolchain/incan-cli/src/commands/tools_boundary_tests.rs` | 1 | 116 | 116 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/workspace.rs` | 1 | 428 | 45 | unaffected | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/lib.rs` | 36 | 3253 | 1050 | unaffected | - | - | #1561 | replacement 9 | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/test_runner/execution.rs` | 21 | 3539 | 618 | keep (keep 10, retire 11) | 0/11 | - | #1561 | text 1, checker 3, parser 3 | `incan test` today lowers Incan tests into a Rust libtest harness; the harness-shape tests retire with it, the discovery and session tests stay. |
| `loaves/toolchain/incan-cli/src/test_runner/mod.rs` | 15 | 2148 | 492 | keep | - | - | #1561 | checker 3 | test collection, parametrize expansion, marker selection and scheduling. |
| `loaves/toolchain/incan-cli/tests/canonical_item_imports.rs` | 3 | 229 | 229 | re-point | - | - | #1561 | run 3 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_catalogue_forms_tests.rs` | 1 | 113 | 113 | re-point | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_codegraph_and_inspection_tests.rs` | 16 | 2016 | 2016 | re-point | - | required | #1561 | run 16 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_decorator_and_partial_tests.rs` | 11 | 1113 | 1113 | re-point | - | - | #1561 | run 11 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_float_display_tests.rs` | 1 | 86 | 86 | re-point | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_interop_target_tests.rs` | 9 | 868 | 868 | re-point | - | - | #1561 | run 9 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_issue1370_phantom_type_param_tests.rs` | 1 | 93 | 93 | re-point | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_issue1668_stdlib_gaps_tests.rs` | 2 | 235 | 235 | re-point | - | - | #1561 | run 2 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs` | 25 | 2101 | 2101 | re-point | - | required | #1561 | codegen 1, run 25 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/cli_layering_guardrails.rs` | 2 | 189 | 189 | unaffected | - | - | #1561 | codegen 2, legacy_ir 2 | layering baseline of what the CLI may reach; names codegen types without using them. |
| `loaves/toolchain/incan-cli/tests/cli_provider_boundary_tests.rs` | 19 | 1343 | 1343 | re-point | - | - | #1561 | codegen 2, run 19 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/cli_rust_interop_tests.rs` | 13 | 1504 | 1504 | re-point (re-point 12, retire 1) | 0/1 | required | #1561 | codegen 1, text 1, run 13 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/cli_std_environ_tests.rs` | 4 | 473 | 473 | re-point | - | - | #1561 | run 4 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_surface_tests.rs` | 26 | 1432 | 1432 | re-point | - | - | #1561 | run 25 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/cli_workspace_and_lock_tests.rs` | 23 | 1795 | 1795 | re-point | - | required | #1561 | run 22 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/example_capability_coverage.rs` | 1 | 279 | 279 | keep | - | - | #1561 | replacement 1 | examples cover the stable capability registry. |
| `loaves/toolchain/incan-cli/tests/integration_tests.rs` | 203 | 12563 | 12563 | re-point (keep 20, re-point 174, retire 9) | 0/9 | required | #1561 | codegen 8, text 7, run 178, checker 14, parser 29 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Checker-only and lexer tests stay; generated-text and IrCodegen tests retire. |
| `loaves/toolchain/incan-cli/tests/layering_guard.rs` | 9 | 359 | 359 | unaffected | - | - | #1561 | - | crate and stdlib layering guards. |
| `loaves/toolchain/incan-cli/tests/package_boundary_facade_tests.rs` | 7 | 646 | 646 | re-point | - | - | #1561 | run 7, checker 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/package_executable_representation.rs` | 6 | 724 | 724 | re-point | - | - | #1561 | run 6, checker 3 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/repository_path_tests.rs` | 5 | 61 | 61 | unaffected | - | - | #1561 | run 1 | repository path resolution for the compiler command. |
| `loaves/toolchain/incan-cli/tests/rfc031_pub_import_integration_tests.rs` | 82 | 8051 | 8051 | re-point (re-point 78, retire 4) | 0/4 | required | #1561 | codegen 1, text 4, run 82, checker 33, parser 12 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/script_target_diagnostics.rs` | 1 | 44 | 44 | re-point | - | - | #1561 | - | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` | 1 | 128 | 128 | re-point | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/toolchain_installer_tests.rs` | 30 | 2728 | 2728 | unaffected | - | required | #1561 | run 6 | installer, archive packager and release manifest. |
| `loaves/toolchain/incan-cli/tests/vocab_guardrails.rs` | 3 | 575 | 575 | unaffected | - | - | #1561 | text 1 | source audits (semantic string audit, stringly vocab checks). |

Per-test overrides in `loaves/toolchain/incan-cli/src/commands/build.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `classify_signature_mismatch_for_rust_extern_context` | retire | - | - | maps a rustc failure of the generated build back to source |
| `classify_unresolved_backing_item_for_rust_extern_context` | retire | - | - | maps a rustc failure of the generated build back to source |
| `wraps_rust_extern_failure_back_to_incan_declaration_span` | retire | - | - | maps a rustc failure of the generated build back to source |

Per-test overrides in `loaves/toolchain/incan-cli/src/test_runner/execution.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `merge_test_runner_dependencies_promotes_dev_deps_into_dependencies` | retire | - | - | libtest harness of the generated test crate |
| `merge_test_runner_dependencies_unifies_normal_and_dev_features` | retire | - | - | libtest harness of the generated test crate |
| `parse_libtest_outcomes_detects_ok_and_failed` | retire | - | - | libtest harness of the generated test crate |
| `parse_libtest_outcomes_normalizes_prefixed_names` | retire | - | - | libtest harness of the generated test crate |
| `successful_native_batch_preserves_a_passing_harness_result_issue996` | retire | - | - | libtest harness of the generated test crate |
| `extracts_unindented_libtest_panic_payloads_from_a_batch` | retire | - | - | libtest harness of the generated test crate |
| `runner_crate_name_is_derived_from_batch_suffix` | retire | - | - | libtest harness of the generated test crate |
| `native_test_outputs_do_not_alias_distinct_execution_groups` | retire | - | - | libtest harness of the generated test crate |
| `batch_suffix_is_path_independent_but_content_sensitive` | retire | - | - | libtest harness of the generated test crate |
| `inject_file_test_harness_emits_tests_module` | retire | - | generated_text | libtest harness of the generated test crate |
| `inject_file_test_harness_wraps_async_tests_and_fixtures` | retire | - | - | libtest harness of the generated test crate |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_rust_interop_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `cold_library_build_preserves_rust_string_compound_assignment_issue896` | retire | - | generated_text, build_run | asserts generated Rust text |

Per-test overrides in `loaves/toolchain/incan-cli/tests/integration_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `build_explicit_mutable_rust_generic_reaches_codegen_through_normal_cli_path` | retire | - | codegen, generated_text, build_run | generated text or IrCodegen |
| `decorated_method_explicit_mutable_rust_generic_keeps_static_and_wrapper_abi` | retire | - | codegen, generated_text, build_run | generated text or IrCodegen |
| `test_compound_assign_float_with_int_rhs` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_valid_fixtures` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_invalid_fixtures` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_imported_static_initializer_does_not_deadlock_issue680` | retire | - | generated_text, build_run | generated text or IrCodegen |
| `lexer_token_surface_cases` | keep | - | - | lexer only |
| `test_python_like_numeric_ops_compile` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_hello_world_codegen` | retire | - | codegen, generated_text, checker, parser | generated text or IrCodegen |
| `test_method_alias_codegen_rewrites_to_target_method` | retire | - | codegen, parser | generated text or IrCodegen |
| `test_rfc041_rusttype_interop_typechecks_end_to_end` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_rfc041_rusttype_with_methods_typechecks` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_rfc041_rust_coercion_codegen_smoke` | retire | - | codegen, checker, parser | generated text or IrCodegen |
| `test_rfc041_structural_coercion_codegen_smoke` | retire | - | codegen, generated_text, checker, parser | generated text or IrCodegen |
| `test_rfc009_numeric_resize_and_decimal_codegen_smoke` | retire | - | codegen, generated_text, checker, parser | generated text or IrCodegen |
| `build_lib_imported_static_decorator_receiver_materializes_string_arg_issue671` | retire | - | generated_text, build_run | generated text or IrCodegen |
| `test_model_with_decorator` | keep | - | parser | lex/parse/typecheck only |
| `test_class_with_traits` | keep | - | parser | lex/parse/typecheck only |
| `test_trait_supertraits_compile_source` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_trait_constructor_rejected_in_full_pipeline` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_method_with_mut_self` | keep | - | parser | lex/parse/typecheck only |
| `test_generic_instance_method_full_pipeline` | keep | - | checker, parser | lex/parse/typecheck only |
| `test_match_with_case` | keep | - | parser | lex/parse/typecheck only |
| `test_list_comprehension` | keep | - | parser | lex/parse/typecheck only |
| `test_generic_type` | keep | - | parser | lex/parse/typecheck only |
| `test_yield_expression` | keep | - | parser | lex/parse/typecheck only |
| `test_fixture_decorator` | keep | - | parser | lex/parse/typecheck only |
| `test_rust_crate_import` | keep | - | parser | lex/parse/typecheck only |
| `test_rust_from_import` | keep | - | parser | lex/parse/typecheck only |

Per-test overrides in `loaves/toolchain/incan-cli/tests/rfc031_pub_import_integration_tests.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `compiled_provider_preserves_shared_rust_interop_contracts_issues834_835_961` | retire | - | codegen, generated_text, build_run, checker | asserts generated Rust text |
| `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` | retire | - | generated_text, build_run | asserts generated Rust text |
| `consumer_build_plans_source_backed_vocab_helper_calls_with_defaults_and_unions_issue729` | retire | - | generated_text, build_run, checker | asserts generated Rust text |
| `consumer_build_plans_source_backed_pub_helper_calls_with_defaults_and_unions_issue729` | retire | - | generated_text, build_run, checker | asserts generated Rust text |

### `loaves/toolchain/incan-lsp` (93 tests in 6 files: keep 92, retire 1)

| File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---|---|---|---|
| `loaves/toolchain/incan-lsp/src/backend.rs` | 56 | 9413 | 2077 | keep | - | required | #1561 | checker 21, parser 33, lsp 3 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/call_site_type_args.rs` | 4 | 747 | 48 | keep | - | - | #1561 | parser 1 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/diagnostics.rs` | 7 | 395 | 168 | keep | - | - | #1561 | checker 3 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/main.rs` | 3 | 98 | 30 | keep | - | - | #1561 | - | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/semantic_tokens.rs` | 14 | 1394 | 284 | keep | - | - | #1561 | parser 13 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/tests/rfc081_embedded_conformance.rs` | 9 | 730 | 730 | keep (keep 8, retire 1) | 0/1 | - | #1561 | codegen 1, checker 2, parser 1, formatter 3, lsp 1 | RFC 081 embedded-fragment conformance through desugar, typecheck and formatter; one test asserts the emitter refuses. |

Per-test overrides in `loaves/toolchain/incan-lsp/tests/rfc081_embedded_conformance.rs`:

| Test | Disposition | Twin | Lanes | Notes |
|---|---|---|---|---|
| `every_submode_survives_desugar_typecheck_and_lowering_then_refuses_emission` | retire | - | codegen, checker | asserts an emitter refusal; the refusal moves to the replacement route's source profile |

??? note "Unaffected crates (1138 tests in 89 files)"

    Every test in these crates is `unaffected`: the cutover does not touch them. They are listed so the summary reconciles to the whole tree.

    #### `loaves/compiler/incan_oven_facet` (6 tests in 2 files: unaffected 6)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/compiler/incan_oven_facet/src/lib.rs` | 4 | 465 | 235 | unaffected | - | - | #1561 | - | Oven facet of the compiler; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_oven_facet/tests/oven_pr_regressions.rs` | 2 | 210 | 210 | unaffected | - | - | #1561 | - | Oven facet of the compiler; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/compiler/incan_provider` (97 tests in 9 files: unaffected 97)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/compiler/incan_provider/src/compiled_sdk.rs` | 1 | 74 | 16 | unaffected | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/dependency_resolver.rs` | 21 | 1161 | 533 | unaffected | - | - | #1561 | checker 20 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/inventory.rs` | 7 | 1036 | 590 | unaffected | - | - | #1561 | checker 3 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/lock_semantics.rs` | 14 | 2063 | 1320 | unaffected | - | - | #1561 | checker 12 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/requirements.rs` | 11 | 1002 | 275 | unaffected | - | - | #1561 | checker 8 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/sdk_build.rs` | 8 | 869 | 253 | unaffected | - | - | #1561 | run 1, checker 1 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/sdk_store.rs` | 11 | 1178 | 511 | unaffected | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/vocab_extraction.rs` | 14 | 1713 | 296 | unaffected | - | - | #1561 | run 3 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/tests/stdlib_effect_digest.rs` | 10 | 313 | 313 | unaffected | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/compiler/rust_inspect` (103 tests in 7 files: unaffected 103)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/compiler/rust_inspect/src/cache_tests.rs` | 41 | 2610 | 2610 | unaffected | - | required | #1561 | replacement 1 | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/digest/tests.rs` | 17 | 634 | 634 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/extractor.rs` | 20 | 3243 | 1279 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/lib.rs` | 2 | 206 | 149 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/loader.rs` | 15 | 2105 | 630 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/mir_digest.rs` | 5 | 455 | 235 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/receiver_contract.rs` | 3 | 101 | 43 | unaffected | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/kernel/incan_codegraph` (5 tests in 1 file: unaffected 5)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/kernel/incan_codegraph/src/lib.rs` | 5 | 1312 | 174 | unaffected | - | - | #1561 | - | codegraph record format. Reviewed at crate level. |

    #### `loaves/kernel/incan_vocab` (6 tests in 1 file: unaffected 6)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/kernel/incan_vocab/src/lib.rs` | 6 | 514 | 205 | unaffected | - | - | #1561 | checker 6 | vocab registration contract crate. Reviewed at crate level. |

    #### `loaves/oven/oven_cargo_compat` (143 tests in 10 files: unaffected 143)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/oven/oven_cargo_compat/src/cargo_process.rs` | 3 | 209 | 72 | unaffected | - | - | #1561 | run 1 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/compiler_suite_foundation.rs` | 7 | 1542 | 978 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/harvest.rs` | 13 | 1671 | 807 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/lib.rs` | 68 | 9360 | 4749 | unaffected | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/loaf_bake.rs` | 10 | 1592 | 750 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/loaf_bake/vocab_support.rs` | 3 | 1023 | 290 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/registry_sources.rs` | 2 | 699 | 129 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/rustc_trace.rs` | 5 | 494 | 103 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/selected_graph_projection.rs` | 21 | 3696 | 1385 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/selected_unit_capture.rs` | 11 | 2361 | 783 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_interop` (20 tests in 1 file: unaffected 20)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/oven/oven_interop/src/lib.rs` | 20 | 4394 | 2720 | unaffected | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_model` (159 tests in 9 files: unaffected 159)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/oven/oven_model/src/loaf_registry.rs` | 5 | 685 | 245 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/lock.rs` | 24 | 1893 | 994 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/manifest.rs` | 56 | 3646 | 1120 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/oven_interop.rs` | 8 | 2098 | 603 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/env.rs` | 18 | 860 | 476 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/toolchain.rs` | 4 | 295 | 58 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/version.rs` | 10 | 418 | 104 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/toolchain_layout.rs` | 15 | 875 | 353 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/workspace.rs` | 19 | 2065 | 604 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_rustc` (291 tests in 16 files: unaffected 291)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/oven/oven_rustc/src/loaf.rs` | 28 | 4707 | 1338 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/loaf_mirror.rs` | 9 | 758 | 429 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test.rs` | 34 | 2881 | 2881 | unaffected | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test/case_slice.rs` | 5 | 174 | 87 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test/evidence.rs` | 4 | 258 | 74 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/plan/composition.rs` | 8 | 1496 | 686 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/plan/selection.rs` | 1 | 518 | 51 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc.rs` | 94 | 11409 | 6517 | unaffected | - | required | #1561 | run 3 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/compiled_unit.rs` | 7 | 844 | 476 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/inspection.rs` | 31 | 2617 | 1951 | unaffected | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_closure.rs` | 8 | 1087 | 509 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_executor.rs` | 9 | 1341 | 759 | unaffected | - | - | #1561 | run 4 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_foundation.rs` | 26 | 2152 | 1473 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/selected_unit.rs` | 13 | 1793 | 717 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/substitution.rs` | 10 | 708 | 335 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/toolchain.rs` | 4 | 604 | 51 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_store` (94 tests in 6 files: unaffected 94)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/oven/oven_store/src/closure_proof.rs` | 1 | 107 | 27 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/lib.rs` | 13 | 2285 | 550 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/process.rs` | 10 | 769 | 769 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/progress.rs` | 6 | 343 | 99 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/store.rs` | 55 | 6391 | 2080 | unaffected | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/store_mirror.rs` | 9 | 635 | 373 | unaffected | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/stdlib/async` (27 tests in 6 files: unaffected 27)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/async/rust/src/channel.rs` | 8 | 540 | 115 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/race.rs` | 5 | 157 | 58 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/runtime.rs` | 2 | 104 | 33 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/sync.rs` | 4 | 663 | 125 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/task.rs` | 3 | 178 | 46 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/time.rs` | 5 | 184 | 85 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/core` (88 tests in 9 files: unaffected 88)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/core/rust/src/collections.rs` | 28 | 605 | 242 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/conversions.rs` | 6 | 73 | 41 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/frozen.rs` | 1 | 445 | 16 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/iter.rs` | 11 | 365 | 96 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/num.rs` | 26 | 1008 | 290 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/strings.rs` | 5 | 405 | 42 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/version.rs` | 9 | 249 | 89 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/tests/errors.rs` | 1 | 32 | 32 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/tests/string_len.rs` | 1 | 10 | 10 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/data` (8 tests in 2 files: unaffected 8)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/data/rust/src/collections.rs` | 1 | 144 | 17 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/data/rust/src/json.rs` | 7 | 708 | 108 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/derive` (8 tests in 2 files: unaffected 8)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/derive/incan_web_macros/src/lib.rs` | 6 | 451 | 133 | unaffected | - | - | #1561 | parser 6 | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/derive/incan_web_macros/tests/route_runtime.rs` | 2 | 197 | 197 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/interop` (1 test in 1 file: unaffected 1)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/interop/vocab_companion/src/lib.rs` | 1 | 188 | 17 | unaffected | - | - | #1561 | checker 1 | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/testing` (3 tests in 1 file: unaffected 3)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/stdlib/testing/rust/src/lib.rs` | 3 | 275 | 80 | unaffected | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/third_party/ra_ap_proc_macro_api` (2 tests in 1 file: unaffected 2)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/third_party/ra_ap_proc_macro_api/src/legacy_protocol/msg.rs` | 2 | 430 | 244 | unaffected | - | - | #1561 | - | vendored crate. Reviewed at crate level. |

    #### `loaves/toolchain/oven-cli` (77 tests in 5 files: unaffected 77)

    | File | Tests | Lines | Test lines | Disposition | Twins | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---|---|---|---|
    | `loaves/toolchain/oven-cli/src/commands/oven.rs` | 58 | 6609 | 3307 | unaffected | - | required | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/case_partition.rs` | 6 | 379 | 171 | unaffected | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/harvest.rs` | 3 | 437 | 69 | unaffected | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/suite_environment.rs` | 2 | 899 | 32 | unaffected | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/tools.rs` | 8 | 1863 | 414 | unaffected | - | - | #1561 | run 1 | Oven tools command; the build_run hit is a Cargo config hint, not an Incan build. |
