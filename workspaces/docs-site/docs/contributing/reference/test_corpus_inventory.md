# Test corpus inventory

!!! warning "Generated control-plane reference"

    Do not edit this page by hand. Regenerate it with `make test-inventory` from the tests in the tree and `scripts/test_inventory/dispositions.json`; `make test-inventory-check` fails when it is stale.

    Temporary. This page, the dispositions record and the `test-inventory` gate exist for the v0.6 cutover only: they retire with [#654](https://github.com/encero-systems/incan/issues/654) once no `retire` row remains and the durable corpus is frozen. Nothing here is a lasting contributor contract.

This is the control plane for the slice-7 cutover (issue [#1561](https://github.com/encero-systems/incan/issues/1561), test corpus). Every Rust test under `loaves/` and `workspaces/` has a disposition by what it proves and how, not by where it lives; every `.incn` fixture root is listed with its case count. A disposition is a recorded decision, the lane signals beside it are the mechanical evidence, and a `retire` row may be deleted only once it names a twin or records `dies`. How to classify a test, record a twin, or plan a split is recorded on the owning issue: [working the test corpus inventory](https://github.com/encero-systems/incan/issues/1561#issuecomment-5750194193).

## Summary

| Disposition | Tests | Files | Fixture cases |
|---|---:|---:|---:|
| keep | 3110 | 196 | 7 |
| re-point | 488 | 49 | 401 |
| retire | 1108 | 72 | 0 |
| unaffected | 1430 | 133 | 5 |
| unreviewed | 0 | 0 | 0 |
| **Total** | **6136** | **450** | **413** |

- Retire-class tests: 1108, of which twinned 33, dies 115, open 960 (neither yet).
- Retire-class files with open rows: 60 (a file whose retire tests are all twinned or recorded `dies` is done).
- Files whose test region exceeds the split threshold of 1500 lines: 20, of which 8 in the durable corpus (keep or re-point).
- Unreviewed files: 0.

## Dispositions

| Disposition | Meaning |
|---|---|
| `keep` | Asserts source meaning through the parser, typechecker, Body IR, formatter, LSP or semantics core, and never touches generated Rust. Survives the slice-7 cutover untouched. |
| `re-point` | Asserts program behavior (output, exit code, diagnostics of a run) but proves it by building or running generated Rust. The assertion stays; slice 7 changes the route. |
| `retire` | Asserts the shape of the generated Rust itself: snapshot text, `contains("fn ...")` on emitted source, emitter unit tests. Dies with #654, and only after its row names a twin or records `dies` with the reason. |
| `unaffected` | Oven, store, rustc, installer, stdlib runtime, layering guards and other tests the cutover does not touch. Listed so the total reconciles. |
| `unreviewed` | Nobody has read the file yet. The mechanical proposal is recorded in the notes when there is one; the maintainer works these rows through. |

## Twins and `dies`

A `retire` row leaves the corpus by naming what proves the behavior after the cutover in its `twin` field, or by recording that nothing user-observable is lost. `Twins` counts the first kind against the file's retire-class tests; `Dies` counts the second.

| `twin` | Meaning |
|---|---|
| `path::fn` | a `keep` or `re-point` test that proves the same behavior. |
| a fixture root | a declared `.incn` fixture root, by its bare path, when running that root's programs proves the behavior. |
| a behavior fixture | a file or directory under `loaves/compiler/incan_test_support/fixtures/behavior/<area>/`: an Incan program whose header declares its expected observables and names the tests it retires in `# retires:` lines. The gate refuses a fixture and a row that do not name each other. |
| `dies` | the test has no user-observable behavior to twin; the reason is recorded in a `dies` field beside it and the page shows it. Generated projects, `inspect rust` output and the build-report Cargo fields die with #654 (no Rust is generated at all any more); a data-structure invariant of a dying crate dies with the crate. |

## Lane signals

The collector counts these in the text of each test function and of the file-local helpers it calls. They are evidence, not the verdict: the disposition column is what the reviewer recorded. Helpers that live in a `#[path = "support/..."]` module outside the file are invisible to the scanner, so a test that drives the shadow comparison through such a helper shows only the lanes its own text carries. A `#[cfg_attr(..., test)]` attribute and a one-line `#[test] fn ...` are not counted; the tree has neither.

| Signal | Fires when the test |
|---|---|
| `codegen` | calls a codegen API (`IrCodegen`, `try_generate`, `generate_rust`, `emit_program`, `read_generated_rust`) or reads generated Rust (`target/incan/<project>/src/*.rs`, `incan --emit-rust`) |
| `snapshot` | asserts an `insta` snapshot |
| `generated_text` | asserts on generated Rust text (`contains("fn ")`, `contains("impl ")` and the like) |
| `build_run` | builds or runs generated Rust (`run_explicit_oven_bake`, `compare_source_observable`, a `cargo`/`rustc` command, or a CLI invocation such as `run_incan` / `incan_command` beside a `build`, `run`, `test` or `bake` subcommand or a generated-target read; `incan fmt`, `incan check`, `--help` and `--version` alone do not count) |
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
| `loaves/compiler/incan_driver/src/replacement_compatibility/migration_baselines` | `**/*.incn` | 1 | unaffected | #1561 | frozen v0.5.0 migration baseline, decoded through the checked capability-metadata path (no generated Rust); retained only until the v0.5 replacement migration closes, per the compatibility inventory's retirement line, then removed from the collector. |
| `loaves/compiler/incan_driver/tests/fixtures` | `**/*.incn` | 20 | re-point | #1561 | driver integration fixtures (generated_rust_* artifact projects, callability, native consumer); their owner tests are retire-class. |
| `loaves/compiler/incan_emit/tests/codegen_snapshots` | `**/*.incn` | 180 | re-point | #1561 | snapshot corpus inputs considered as programs; the .snap outputs retire with codegen_snapshot_tests.rs. |
| `loaves/compiler/incan_test_support/fixtures` | `*.incn` | 12 | re-point | #1561 | top-level regression programs run by CLI integration tests (rfc023/rfc030/rfc064/rfc088 behavior, reflection, model traits). |
| `loaves/compiler/incan_test_support/fixtures/behavior/cli` | `<name>.incn or <name>/` | 5 | re-point | #1561 | behavior fixtures twinning the retire-class tests under loaves/toolchain/incan-cli (generated-text assertions after a build, IrCodegen smoke tests, an `--emit-rust`-only test), run by behavior_cli_tests.rs. A fixture is proved by running a program, so it is re-point today and stays valid after the route flips; each names the retire test it twins in `# retires:` lines. |
| `loaves/compiler/incan_test_support/fixtures/behavior/cli_dependencies` | `<name>.incn or <name>/` | 3 | re-point | #1561 | behavior fixtures that are projects with in-fixture path dependencies (`[dependencies] <name> = { path = "deps/<name>" }`, reached through `pub::<name>`): the runner bakes every provider in dependency order before the run, with no Cargo authority, so a twin can prove what a consumer prints (or which diagnostic refuses it) across a package boundary. Run by behavior_cli_dependencies_tests.rs; a provider that would need Cargo (one that itself declares `[dependencies]`) fails its fixture at the suite's Cargo guard, so such fixtures stay parked. Each names the retire tests it twins in `# retires:` lines. |
| `loaves/compiler/incan_test_support/fixtures/behavior/driver` | `<name>.incn or <name>/` | 3 | re-point | #1561 | behavior fixtures twinning incan_driver retire tests whose surviving observable is a program's output or a check-time diagnostic rather than the generated project they inspected; run by behavior_driver_tests.rs. Each names the retire tests it twins in `# retires:` lines. |
| `loaves/compiler/incan_test_support/fixtures/behavior/harness` | `<name>.incn or <name>/` | 8 | re-point | #1561 | the behavior-fixture harness proving itself: one fixture per shape of the format (refused program with one code and with two, non-zero exit, exit code as the only observable, empty stdout, contained lines, module directory, project directory), run by behavior_harness_tests.rs. They twin nothing; they exist so a runner or route change is caught here before it is caught in a twin. Header refusals are unit tests of parse_header, not fixtures. |
| `loaves/compiler/incan_test_support/fixtures/behavior/smoke` | `<name>.incn or <name>/` | 5 | re-point | #1561 | behavior fixtures: programs with their expected observables in the header, run by behavior_smoke_tests.rs. A fixture is proved by running a program, so it is re-point today and stays valid after the route flips; each names the retire tests it twins in `# retires:` lines. |
| `loaves/compiler/incan_test_support/fixtures/invalid` | `**/*.incn` | 7 | keep | #1561 | diagnostics through the checker only (test_invalid_fixtures). |
| `loaves/compiler/incan_test_support/fixtures/oven_project_bake` | `**/*.incn` | 1 | re-point | #1561 | Incan project baked under Oven by the compiler suite; the program's route changes, the bake harness does not. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_app_bake` | `**/*.incn` | 1 | re-point | #1561 | release-profile Oven bake of an Incan app. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_bytes_io` | `**/*.incn` | 2 | re-point | #1561 | release-profile Oven bake exercising bytes I/O. |
| `loaves/compiler/incan_test_support/fixtures/oven_release_file_lock` | `**/*.incn` | 2 | re-point | #1561 | release-profile Oven bake exercising file locks. |
| `loaves/compiler/incan_test_support/fixtures/valid` | `**/*.incn` | 28 | re-point | #1561 | typechecked in bulk by test_valid_fixtures (keep) and run one by one as std surface programs by CLI tests (re-point). |
| `loaves/oven/oven_rustc/src/fixtures` | `**/*.incn` | 4 | unaffected | #1561 | Oven rustc fixtures. |
| `loaves/toolchain/incan-cli/tests/fixtures` | `**/*.incn` | 8 | re-point | #1561 | CLI integration fixtures (layering, package boundary facade, pub union consumer, vocab guardrails). |
| `workspaces/benchmarks` | `**/*.incn` | 9 | re-point | #1561 | benchmark programs built and timed by workspaces/benchmarks/run_all.sh (`make benchmarks`); the route changes, the timing harness does not. |
| `workspaces/docs-site/docs/_snippets/language/examples` | `verified_*.incn` | 8 | re-point | #1561 | verified documentation examples checked by scripts/check_docs_examples.sh. |
| `workspaces/oven/src` | `test_*.incn` | 11 | re-point | #1561 | Incan tests of the Oven release-policy project (workspaces/oven), run through `incan test`. |
| `workspaces/oven/tests/fixtures` | `**/*.incn` | 4 | re-point | #1561 | fixtures of the Oven release-policy project's tests. |

## Test files by crate

`Lines` is the file length; `Test lines` is the test region the split threshold applies to: the `#[cfg(test)]` modules when the file has any, otherwise the whole file. `Twins` is `twinned/retire-class` and `Dies` the number recorded `dies`, for files with retire-class tests. Per-test rows follow a file only when it carries per-test overrides.

### `loaves/compiler/incan_driver` (745 tests in 107 files: keep 362, re-point 138, retire 102, unaffected 143)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_driver/src/backend/c_abi.rs` | 11 | 1074 | 333 | unaffected | - | - | - | #1561 | - | clang verification of checked C signatures; not the Rust backend. |
| `loaves/compiler/incan_driver/src/backend/project/cargo_toml.rs` | 22 | 1056 | 675 | retire | 0/22 | 22 | - | #1561 | run 20 | generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/backend/project/generator.rs` | 22 | 3250 | 1373 | retire (retire 22) | 3/22 | 19 | - | #1561 | codegen 5, text 9, run 20, checker 4 | generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/backend/project/lock_projection.rs` | 9 | 816 | 349 | retire | 0/9 | 9 | - | #1561 | - | generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/backend/project/plan.rs` | 2 | 229 | 23 | retire | 0/2 | 2 | - | #1561 | - | generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/backend/project/runner.rs` | 18 | 1031 | 449 | re-point | - | - | - | #1561 | codegen 2, run 14 | project runner: builds and runs the generated project; the run contract (profiles, rebuild triggers) is re-pointed at the replacement route. |
| `loaves/compiler/incan_driver/src/backend/project/tests/codegen_generator.rs` | 4 | 327 | 327 | retire | 0/4 | 4 | - | #1561 | codegen 4, text 1, run 4, checker 1, parser 3, legacy_ir 1 | codegen into a generated project; asserts generated Rust text. |
| `loaves/compiler/incan_driver/src/backend/project/tests/lock_payload.rs` | 1 | 26 | 26 | retire | 0/1 | 1 | - | #1561 | codegen 1, run 1 | generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/backend/shadow/abs_sum_profile_tests.rs` | 1 | 128 | 128 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/enumerate_zip_tests.rs` | 5 | 164 | 164 | re-point | - | - | - | #1561 | run 3, replacement 4, checker 4, parser 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/json_stringify_tests.rs` | 3 | 219 | 219 | re-point | - | - | - | #1561 | run 3, replacement 3 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/legacy_oven.rs` | 1 | 669 | 59 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/len_string_tests.rs` | 2 | 82 | 82 | re-point | - | - | - | #1561 | run 2, replacement 2, checker 2 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/backend/shadow/tests.rs` | 47 | 1253 | 1253 | re-point | - | - | - | #1561 | run 3, replacement 30 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. Result-report transport tests are the comparison harness itself. |
| `loaves/compiler/incan_driver/src/build/bake.rs` | 5 | 1137 | 172 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/caller_owned.rs` | 7 | 1044 | 315 | unaffected | - | - | - | #1561 | checker 4 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/inline_command.rs` | 5 | 129 | 75 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/library_exports.rs` | 12 | 933 | 559 | keep | - | - | - | #1561 | checker 8, parser 9 | library re-export resolution and Rust ABI query paths from checked declarations. |
| `loaves/compiler/incan_driver/src/build/library_outputs.rs` | 3 | 253 | 59 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/library_publication.rs` | 6 | 609 | 218 | unaffected | - | - | - | #1561 | checker 6 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/mod.rs` | 3 | 938 | 79 | unaffected | - | - | - | #1561 | replacement 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_materialization.rs` | 2 | 1273 | 411 | unaffected | - | - | - | #1561 | replacement 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_paths.rs` | 5 | 971 | 228 | unaffected | - | - | - | #1561 | codegen 1, run 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/output_selection.rs` | 1 | 1091 | 358 | unaffected | - | - | - | #1561 | replacement 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/oven_project.rs` | 6 | 1326 | 148 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/package_loafs.rs` | 2 | 1014 | 435 | unaffected | - | - | - | #1561 | text 1, checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/plan_authority.rs` | 4 | 1062 | 191 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/plan_selection.rs` | 6 | 1022 | 469 | unaffected | - | - | - | #1561 | codegen 1, run 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/prepare_project.rs` | 1 | 497 | 74 | retire | 0/1 | 1 | - | #1561 | codegen 1, run 1, checker 1 | prunes the generated project's Cargo dependencies; generated Cargo project shape; dies with the generated-project route |
| `loaves/compiler/incan_driver/src/build/provider_compilation.rs` | 6 | 1010 | 529 | unaffected | - | - | - | #1561 | checker 2 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/provider_metadata.rs` | 5 | 1111 | 266 | keep | - | - | - | #1561 | checker 3, parser 3 | provider operation metadata projected from checked declaration facts. |
| `loaves/compiler/incan_driver/src/build/publication.rs` | 1 | 798 | 63 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/replacement.rs` | 5 | 533 | 161 | keep | - | - | - | #1561 | replacement 4, checker 4, parser 1 | replacement build pipeline (session projection, exact numeric report). |
| `loaves/compiler/incan_driver/src/build/reuse.rs` | 1 | 679 | 35 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build/source_authority.rs` | 19 | 1846 | 1171 | unaffected | - | - | - | #1561 | codegen 1, checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/build_unit.rs` | 1 | 202 | 27 | unaffected | - | - | - | #1561 | checker 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/cargo_policy.rs` | 4 | 270 | 92 | retire (retire 4) | 2/4 | 2 | - | #1561 | - | Two tests die with the Cargo command line (arg ordering, `INCAN_CARGO_ARGS`); the `INCAN_LOCKED/FROZEN/OFFLINE` env defaults, the `--no-locked/--no-offline/--no-frozen` negations and frozen ⇒ locked+offline survive as Oven lock-policy inputs (maintainer ruling, 2026-09-20) and are twinned by re-point CLI tests. |
| `loaves/compiler/incan_driver/src/generated_cache.rs` | 18 | 1510 | 447 | retire | 0/18 | 18 | - | #1561 | - | generated-project cache identity and pruning; dies with the generated-project route. |
| `loaves/compiler/incan_driver/src/inspect/closure.rs` | 8 | 409 | 224 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/inspect/codegraph.rs` | 13 | 4549 | 849 | keep | - | - | - | #1561 | replacement 3, checker 9, parser 8 | codegraph projection from checked facts. |
| `loaves/compiler/incan_driver/src/lock/mod.rs` | 2 | 558 | 159 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/registry_sources.rs` | 3 | 517 | 48 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/resolution.rs` | 3 | 751 | 85 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/rust_inspect.rs` | 2 | 536 | 97 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/test_inputs.rs` | 2 | 200 | 91 | unaffected | - | - | - | #1561 | checker 2, parser 1 | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/lock/workspace.rs` | 1 | 587 | 45 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/modules.rs` | 23 | 2041 | 1166 | keep | - | - | - | #1561 | replacement 1, checker 19, parser 20 | module collection and Rust dependency use discovery through the parser. |
| `loaves/compiler/incan_driver/src/project.rs` | 9 | 407 | 105 | unaffected | - | - | - | #1561 | - | build orchestration over Oven (loafs, providers, publication, locks); not the Rust backend. |
| `loaves/compiler/incan_driver/src/replacement_compatibility.rs` | 12 | 3938 | 310 | keep | - | - | - | #1561 | replacement 12, formatter 10 | replacement compatibility inventory collector and validator. |
| `loaves/compiler/incan_driver/src/rust_inspect_workspace.rs` | 19 | 1885 | 867 | unaffected | - | - | - | #1561 | codegen 5, text 1, run 9, checker 1 | generated rust-inspect Cargo workspace that feeds the checker with Rust metadata; not the Rust backend. |
| `loaves/compiler/incan_driver/src/session.rs` | 4 | 911 | 175 | keep | - | - | - | #1561 | checker 4, parser 2 | CompilationSession analysis (provider plan reuse, feature projection). |
| `loaves/compiler/incan_driver/src/shadow_support.rs` | 2 | 252 | 28 | re-point | - | - | - | #1561 | checker 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/src/testing/discovery.rs` | 33 | 1973 | 829 | keep | - | - | - | #1561 | checker 32, parser 32 | test discovery through the parser. |
| `loaves/compiler/incan_driver/src/testing/module_graph.rs` | 3 | 412 | 128 | keep | - | - | - | #1561 | checker 1, parser 3 | test-runner module graph through the parser. |
| `loaves/compiler/incan_driver/src/tests/executable_session.rs` | 1 | 81 | 81 | keep | - | - | - | #1561 | replacement 1, checker 1 | canonical frames through a checked cycle. |
| `loaves/compiler/incan_driver/src/typecheck.rs` | 3 | 356 | 73 | keep | - | - | - | #1561 | checker 2 | typecheck over the import graph. |
| `loaves/compiler/incan_driver/tests/duration_artifact_tests.rs` | 1 | 67 | 67 | re-point | - | - | - | #1561 | run 1 | runs a program and checks the duration artifact. |
| `loaves/compiler/incan_driver/tests/emitted_symbol_projection_tests.rs` | 1 | 38 | 38 | retire | 0/1 | 1 | - | #1561 | - | RFC 120 emitted symbol projection and demangling; generated Rust shape. |
| `loaves/compiler/incan_driver/tests/fixtures/generated_rust_native_consumer/consumer/src/lib.rs` | 1 | 47 | 47 | retire | 0/1 | 1 | - | #1561 | - | fixture consumer of generated_rust_native_consumer_tests; follows its owner. |
| `loaves/compiler/incan_driver/tests/generated_cache_integration.rs` | 4 | 468 | 468 | retire (retire 4) | 0/4 | 4 | - | #1561 | run 4 | generated-project cache across builds; dies with the generated-project route. |
| `loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs` | 5 | 567 | 567 | retire (retire 5) | 2/5 | 3 | - | #1561 | run 5, checker 2 | public generated-Rust artifact contract (RFC 120 projections, baseline generated projects); three rows die with the generated project, one is twinned by a refused-program project fixture, one stays open on a question the maintainer decides (the `.incnlib` manifest's fate; the provider bake its consumer-run twin would need now exists in the fixture runner). |
| `loaves/compiler/incan_driver/tests/generated_rust_audit_tests.rs` | 4 | 193 | 193 | retire | 0/4 | 4 | - | #1561 | - | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit) |
| `loaves/compiler/incan_driver/tests/generated_rust_callability_artifact_tests.rs` | 1 | 258 | 258 | retire | 1/1 | 0 | - | #1561 | text 1, run 1, checker 1 | The generated Cargo manifests and the `transforms.rs`/`main.rs` text (fn-pointer parameters, qualified provider calls) die with #654; the surviving observable, the consumer run printing `2 3 4` (a `Callable` value passed across a `pub::` package boundary, which no other test runs), is the twin: a project fixture carrying the producer under `deps/callability_core`, which the runner bakes before `incan run` (harness-deps). |
| `loaves/compiler/incan_driver/tests/generated_rust_native_consumer_tests.rs` | 1 | 367 | 367 | retire | 0/1 | 1 | - | #1561 | run 1 | public generated-Rust artifact contract (RFC 120 projections, native consumers, audit) |
| `loaves/compiler/incan_driver/tests/parity_corpus_tests/corpus_proofs.rs` | 7 | 456 | 456 | re-point | - | - | - | #1561 | - | split of parity_corpus_tests.rs (the parity corpus: slice 7's own measurement instrument; the corpus support helper lives outside the file in `loaves/compiler/incan_driver/tests/support/parity_corpus.rs`, so the scanner sees only the `replacement` lane); corpus validation: the red-state schema proof, the green-state structural and behavioral proofs, the replacement rows' receipt-bound evidence and the CI summary. |
| `loaves/compiler/incan_driver/tests/parity_corpus_tests/paired_row_proofs.rs` | 15 | 857 | 857 | re-point | - | - | - | #1561 | replacement 6 | split of parity_corpus_tests.rs (the parity corpus: slice 7's own measurement instrument; the corpus support helper lives outside the file in `loaves/compiler/incan_driver/tests/support/parity_corpus.rs`, so the scanner sees only the `replacement` lane); the paired-comparison rows: these tests assert a green (or explicitly unavailable) legacy-vs-replacement shadow comparison through `shadow_support::compare_source_observable`, the same evidence class as `backend/shadow/**`: slice 7 (#1675) re-points that baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/parity_corpus_tests/rfc_120_proofs.rs` | 6 | 334 | 334 | re-point | - | - | - | #1561 | - | split of parity_corpus_tests.rs (the parity corpus: slice 7's own measurement instrument; the corpus support helper lives outside the file in `loaves/compiler/incan_driver/tests/support/parity_corpus.rs`, so the scanner sees only the `replacement` lane); the RFC 120 coverage and evidence proofs. |
| `loaves/compiler/incan_driver/tests/protected_builtin_binding_tests.rs` | 3 | 141 | 141 | keep | - | - | - | #1561 | checker 3, parser 3 | lex/parse/typecheck only. |
| `loaves/compiler/incan_driver/tests/protected_generic_binding_tests.rs` | 2 | 106 | 106 | keep | - | - | - | #1561 | checker 2, parser 2 | lex/parse/typecheck only. |
| `loaves/compiler/incan_driver/tests/replacement_abs_sum_profile_shadow_tests.rs` | 1 | 154 | 154 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_abs_sum_profile_tests.rs` | 4 | 199 | 199 | keep | - | - | - | #1561 | run 1, replacement 3, checker 3, parser 3 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/async_tasks_and_race.rs` | 10 | 467 | 467 | keep | - | - | - | #1561 | run 4, replacement 6 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); async tasks and `race`, direct and through the CLI. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/bodies_bindings_and_scalars.rs` | 16 | 546 | 546 | keep | - | - | - | #1561 | replacement 16 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); core body execution, bindings, projections and scalars. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/callables_and_generators.rs` | 19 | 772 | 772 | keep | - | - | - | #1561 | replacement 19 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); named calls, closures, partials, callable defaults and generators. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/cli_nominal_and_enum_values.rs` | 9 | 703 | 703 | keep | - | - | - | #1561 | run 9 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); CLI-driven nominal and enum programs with their receipts and refusals. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/cli_refusals_shadow_and_boundaries.rs` | 13 | 815 | 815 | keep | - | - | - | #1561 | run 13, replacement 1 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); CLI-driven refusals without a receipt, shadow requests that stay explicitly non-green, module and Rust interop boundaries. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/collections_strings_and_print.rs` | 18 | 497 | 497 | keep (keep 17, retire 1) | 1/1 | 0 | - | #1561 | codegen 1, run 1, replacement 17, parser 1 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); collections, strings, scalar JSON and `print`; one test compares against the legacy backend and retires to its behavior fixture. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/modules_and_sessions.rs` | 14 | 921 | 921 | keep | - | - | - | #1561 | run 10, replacement 4, checker 4, parser 4 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); the execution graph's cross-module resolution and the CLI-driven module, facade and session tests. |
| `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/nominal_and_enum_values.rs` | 19 | 995 | 995 | keep (keep 19) | - | - | - | #1561 | replacement 19 | split of replacement_backend_execution_tests.rs (Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep); source-local nominal models and enums through a direct callable, with the identity and layout refusals. |
| `loaves/compiler/incan_driver/tests/replacement_bool_truthiness_shadow_tests.rs` | 1 | 77 | 77 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_bool_truthiness_tests.rs` | 6 | 198 | 198 | keep | - | - | - | #1561 | run 1, replacement 5, checker 5, parser 5 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_collection_len_shadow_tests.rs` | 1 | 77 | 77 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_collection_len_tests.rs` | 4 | 153 | 153 | keep | - | - | - | #1561 | run 1, replacement 3, checker 3, parser 3 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_compatibility_registry_tests.rs` | 6 | 360 | 360 | keep | - | - | - | #1561 | - | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_boundary_tests.rs` | 10 | 728 | 728 | keep | - | - | - | #1561 | replacement 10, checker 10, parser 10 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_parity_cases.rs` | 4 | 162 | 162 | re-point | - | - | - | #1561 | run 4, replacement 4 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_shadow_tests.rs` | 1 | 128 | 128 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_enumerate_zip_tests.rs` | 15 | 552 | 552 | keep | - | - | - | #1561 | run 2, replacement 13, checker 13, parser 13 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_example_coverage.rs` | 1 | 215 | 215 | keep | - | - | - | #1561 | replacement 1, checker 1, parser 1 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_container_boundary_tests.rs` | 1 | 48 | 48 | keep | - | - | - | #1561 | replacement 1, checker 1, parser 1 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_execution_tests.rs` | 12 | 260 | 260 | keep | - | - | - | #1561 | run 1, replacement 11, checker 10, parser 10 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_hashed_shadow_tests.rs` | 1 | 81 | 81 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_isinstance_shadow_tests.rs` | 1 | 75 | 75 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_isinstance_tests.rs` | 8 | 334 | 334 | keep | - | - | - | #1561 | replacement 8, checker 8, parser 8 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_program_io_tests.rs` | 5 | 144 | 144 | keep | - | - | - | #1561 | run 5 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_scalar_conversion_shadow_tests.rs` | 7 | 358 | 358 | re-point | - | - | - | #1561 | run 7, replacement 7 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_scalar_conversion_tests.rs` | 16 | 803 | 803 | keep | - | - | - | #1561 | run 1, replacement 15, checker 15, parser 15 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_sorted_int_list_shadow_tests.rs` | 1 | 78 | 78 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_sorted_int_list_tests.rs` | 7 | 223 | 223 | keep | - | - | - | #1561 | run 1, replacement 6, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_stream_writer_tests.rs` | 7 | 239 | 239 | keep | - | - | - | #1561 | replacement 6, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_helper_execution_tests.rs` | 7 | 238 | 238 | keep | - | - | - | #1561 | run 1, replacement 7, checker 6, parser 6 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_helper_shadow_tests.rs` | 2 | 96 | 96 | re-point | - | - | - | #1561 | run 2, replacement 2 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_string_len_execution_tests.rs` | 4 | 96 | 96 | keep | - | - | - | #1561 | replacement 4, checker 4, parser 4 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/replacement_string_len_shadow_tests.rs` | 1 | 50 | 50 | re-point | - | - | - | #1561 | run 1, replacement 1 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/replacement_typed_numeric_tests.rs` | 13 | 581 | 581 | keep | - | - | - | #1561 | replacement 13, checker 13, parser 13 | Body IR lowering and replacement execution; the CLI-driven tests run the replacement route and stay keep. |
| `loaves/compiler/incan_driver/tests/shadow_comparison_tests.rs` | 9 | 451 | 451 | re-point | - | - | - | #1561 | run 9, replacement 9 | shadow comparison against the legacy Oven baseline; slice 7 (#1675) re-points the baseline to the frozen corpus receipts or retires the comparison with the legacy route. |
| `loaves/compiler/incan_driver/tests/stdlib_version_artifact_tests.rs` | 1 | 132 | 132 | retire | 0/1 | 1 | - | #1561 | codegen 1, parser 1 | asserts the stdlib version check inside the generated artifact. |

Per-test overrides in `loaves/compiler/incan_driver/src/backend/project/generator.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `generated_lock_is_discarded_when_its_manifest_no_longer_matches` | retire | `dies` | the generated project's Cargo.lock and the manifest witness that decides whether Cargo's `--locked` may reuse it; the generated project's Cargo.lock dies with #654. | build_run | - |
| `generated_consumer_rebinds_absent_private_sdk_cache_root_issue911` | retire | `dies` | rebinds a compiled artifact's Cargo path dependencies from an absent SDK cache root to the active one, asserts the projected Cargo.toml paths, and links the rebound Rust with direct rustc (#911); the compiled artifact's Cargo graph and the generated consumer that projects it die with #654. | codegen, generated_text, build_run, checker | - |
| `nested_compiled_artifacts_propagate_sdk_projection_issue911` | retire | `dies` | asserts that a nested compiled artifact's SDK Cargo path dependencies are re-projected through its parent's shadow (`.incan-sdk-rebound-ready`, artifact digest) and that the rebound Rust links with direct rustc (#911); the compiled artifact's Cargo graph dies with #654. | codegen, generated_text, build_run, checker | - |
| `sdk_projection_rejects_cargo_source_mismatch_before_cargo_issue911` | retire | `dies` | asserts that a compiled artifact whose Cargo.toml path or `default-features` disagrees with its checked `.incnlib` descriptor is refused before Cargo runs, and that the projected Cargo.toml normalizes `default-features = false` (#911); a Cargo-manifest consistency check of the compiled artifact, which dies with #654. | codegen, generated_text, build_run, checker | - |
| `test_is_stdlib_path` | retire | `dies` | predicate of the generator's `std` -> `__incan_std` Rust module renaming; a data-structure invariant of the dying generated-project crate. | - | - |
| `test_transform_stdlib_path` | retire | `dies` | the generator's `std` -> `__incan_std` Rust module renaming, which exists only to keep generated `mod std;` from shadowing Rust's `std`; a data-structure invariant of the dying generated-project crate. | - | - |
| `test_generate_multi_escapes_keyword_module_names` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/driver/module_named_like_host_keyword` | - | generated_text, build_run | asserted the `r#async` raw-identifier escape and `#[path]` attribute in generated module declarations; the twin imports a top-level and a nested module named `async` (soft keyword in Incan, hard in the host language) and reads their output. The retired test's `type` module is unreachable from source: `type` is a hard Incan keyword and the parser refuses it as a module path segment. |
| `test_generate_nested_escapes_keyword_submodule_names` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/driver/module_named_like_host_keyword` | - | generated_text, build_run | asserted the `r#async` raw-identifier escape and `#[path]` attribute in generated module declarations; the twin imports a top-level and a nested module named `async` (soft keyword in Incan, hard in the host language) and reads their output. The retired test's `type` module is unreachable from source: `type` is a hard Incan keyword and the parser refuses it as a module path segment. |
| `checked_namespace_facades_skip_children_fully_bound_by_the_parent_issue948` | retire | `dies` | asserts the generator's `public_namespace_facades` map, the plan for which `pub use child::*` lines a generated `mod.rs` carries so rustc sees no ambiguous glob re-export (#948); a generated-Rust facade plan. The user-facing namespace behavior is covered by the typechecker's pub_imports_namespaces_and_fields.rs (keep) and rfc031_pub_import_integration_tests.rs (re-point). | build_run, checker | - |
| `test_generate_nested_avoids_cargo_root_filenames_for_top_level_modules` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/driver/module_named_like_host_root_file` | - | build_run | asserted the `__incan_mod_main.rs`/`__incan_mod_lib.rs` renames that keep top-level modules named `main`/`lib` out of the host project's root filenames; the twin imports a sibling module named `lib` beside the entrypoint and reads its output. A module named `main` beside `main.incn` is the entrypoint itself, so only `lib` is reachable from a script. |

Per-test overrides in `loaves/compiler/incan_driver/src/cargo_policy.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `cargo_policy_resolves_env_defaults_and_frozen_implication` | retire | `loaves/toolchain/incan-cli/tests/cli_workspace_and_lock_tests.rs::build_lock_policy_env_defaults_refuse_a_missing_lock` | - | - | `INCAN_LOCKED=1` and `INCAN_FROZEN=1` (frozen ⇒ locked) refuse a project without `oven.lock` and create none; the `INCAN_CARGO_ARGS` half dies with the Cargo command line. |
| `cargo_policy_uses_cli_extra_args_before_env_extra_args` | retire | `dies` | `INCAN_CARGO_ARGS` ordering against CLI extra args on the Cargo command line; dies with #654. | - | - |
| `cargo_policy_cli_disable_flags_override_env_defaults` | retire | `loaves/toolchain/incan-cli/tests/cli_workspace_and_lock_tests.rs::build_no_flags_override_lock_policy_env_defaults` | - | - | `--no-locked` / `--no-frozen` negate the environment defaults on the command line. |
| `cargo_command_flags_order_policy_features_then_extra_args` | retire | `dies` | Cargo argument ordering (policy flags, then features, then extra args) on the generated Cargo command line; dies with #654. | - | - |

Per-test overrides in `loaves/compiler/incan_driver/tests/generated_cache_integration.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `explicitly_baked_project_reuses_release_json_authority_without_cargo` | retire | `dies` | the `sealed project output … no longer matches this source tree` warning after an edit dies (maintainer ruling, 2026-09-20: a stale bake is re-baked transparently, no explicit rebake — the DX requirement is recorded on #1142, slice 8, with its re-point CLI test). The other observables are carried: `reused sealed project Loaf` on `build --locked` after a bake by `rfc031_pub_import_integration_tests.rs`; `oven.lock is out of date` after a manifest edit by `cli_workspace_and_lock_tests.rs` and `cli_interop_target_tests.rs`; `incan test tests --fail-on-empty` on a baked `std.json` project (#1056) by no other test — folded into the #1142 requirement as well. | build_run | - |

Per-test overrides in `loaves/compiler/incan_driver/tests/generated_rust_artifact_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `generated_application_artifact_matches_baseline` | retire | `dies` | the generated application project compared with a baseline: required files (`Cargo.toml` and a `main.rs` under `src`), no Cargo.lock, `main.rs` fragments, and the Cargo `[package]` table (name, version, edition, license; `license-files` not becoming Cargo's `license-file`) with `incan_std_core`/`incan_derive` dependencies. Generated projects die with #654. | build_run | - |
| `generated_application_without_package_metadata_uses_compiler_defaults` | retire | `dies` | the generated Cargo.toml `[package]` version defaulting to the compiler's and carrying no license when `loaf.toml` declares none; a generated-manifest assertion, dies with #654. | build_run | - |
| `generated_library_and_pub_dependency_consumer_artifacts_match_baseline` | retire | `loaves/toolchain/incan-cli/tests/rfc031_pub_import_integration_tests.rs::private_pub_model_survives_facade_library_and_test_batch_boundaries_issue884` | - | build_run, checker | The generated `lib.rs`/`widgets.rs`/consumer `main.rs` fragments and both Cargo manifests die with #654. The `.incnlib` fields are pinned by re-point rows (`cli_workspace_and_lock_tests.rs`, `cli_provider_boundary_tests.rs`, `package_boundary_facade_tests.rs`, `cli_decorator_and_partial_tests.rs`, `package_executable_representation.rs`); the consumer run (a provider baked, a consumer importing a `pub model`, stdout asserted) is carried by the named re-point test. |
| `path_dependency_artifact_rebuilds_for_a_b_a_feature_projections` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli_dependencies/refused_feature_gated_export_of_dependency` | - | build_run, checker | twinned by a refused-program project fixture: a consumer importing a feature-gated `pub::` export of its in-fixture provider while the package feature is off is refused with INCAN-I0103 (`incan check` prepares an unbaked provider's metadata itself). The alpha/beta/alpha re-bake sequence and the generated `lib.rs` half die with #654; the `.incnlib` `active_features` and `fact_requirements` fields are pinned by re-point rows (`cli_codegraph_and_inspection_tests.rs`, `cli_workspace_and_lock_tests.rs`, `package_executable_representation.rs`). |
| `a_package_projects_a_call_into_its_own_sibling_module` | retire | `dies` | asserts the RFC 120 `__incan_v1_` projected wrapper name, not the source spelling `c.bumped()`, in a package's generated `lib.rs` (#1174 emitted-name recoverability); the emitted-name projection and the legacy `incan_ir` lowering that required it die with generated Rust. The package-calls-its-own-sibling behavior is reachable only through a `--lib`/SDK component build, which is not a run the fixture format can express. The surviving observable — a package whose library calls a sibling module's method builds — is carried by the SDK provider prewarm every suite root performs. | build_run | - |

Per-test overrides in `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/collections_strings_and_print.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `both_backends_render_a_multi_argument_print_the_same_way` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/driver/println_multi_argument.incn` | - | codegen, replacement, parser | compared the replacement executor's rendering of `println("count", 3, true)` with the IrCodegen placeholder count; the twin runs the program and reads `count 3 true` from its stdout, and the replacement-only half stays in the keep sibling replacement_executes_print_by_recording_its_output. |

Per-test overrides in `loaves/compiler/incan_driver/tests/replacement_backend_execution_tests/nominal_and_enum_values.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `replacement_refuses_a_nominal_pattern_after_its_exact_target_identity_is_removed` | keep | - | - | replacement | generated-text hit is a diagnostic string |

### `loaves/compiler/incan_emit` (924 tests in 55 files: keep 100, retire 822, unaffected 2)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_emit/src/checked_program/borrowed_rust_enum.rs` | 3 | 181 | 181 | retire | 0/3 | 0 | - | #1561 | codegen 1, checker 3, parser 3 | checked-program codegen of borrowed Rust enums. |
| `loaves/compiler/incan_emit/src/checked_program/embedded_fragment.rs` | 1 | 130 | 130 | retire | 0/1 | 0 | - | #1561 | codegen 1, checker 1, parser 1, legacy_ir 1 | checked-program codegen of embedded fragments. |
| `loaves/compiler/incan_emit/src/checked_program/rust_supertrait_codegen.rs` | 2 | 76 | 76 | retire | 0/2 | 0 | - | #1561 | codegen 2, text 1, checker 1, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/src/checked_program/rust_trait_receiver_codegen.rs` | 2 | 196 | 196 | retire | 0/2 | 0 | - | #1561 | codegen 2, text 1, checker 2, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/src/checked_program/sdk_module_derives.rs` | 5 | 257 | 257 | retire (keep 2, retire 3) | 0/3 | 0 | - | #1561 | codegen 3, checker 5, parser 5 | SDK module derive requirements flowing into codegen; two tests assert the metadata round trip only. |
| `loaves/compiler/incan_emit/src/checked_program/tests.rs` | 5 | 495 | 495 | keep (keep 4, retire 1) | 0/1 | 0 | - | #1561 | codegen 1, replacement 1, checker 4, parser 4, legacy_ir 1 | checked-program facts (callable shapes, forwarding metadata, vocab refusal); one test drives codegen. |
| `loaves/compiler/incan_emit/src/codegen.rs` | 129 | 8039 | 5500 | retire (retire 129) | 6/129 | 0 | required | #1561 | codegen 122, text 52, checker 112, parser 82, legacy_ir 115 | IrCodegen entry point and emitter-side metadata; every test drives IrCodegen or emitter-owned merges (manifest type refs, native-union capture). Split with the retirement, not before. |
| `loaves/compiler/incan_emit/src/codegen/capability_bridge.rs` | 1 | 187 | 18 | retire | 0/1 | 0 | - | #1561 | parser 1 | codegen capability activation for generated projects. |
| `loaves/compiler/incan_emit/src/codegen/dependency_metadata.rs` | 6 | 1086 | 167 | retire | 0/6 | 0 | - | #1561 | checker 1, parser 5, legacy_ir 1 | which stdlib and provider items the generated project links; generated-project shape. |
| `loaves/compiler/incan_emit/src/conversions.rs` | 79 | 2805 | 1578 | retire | 0/79 | 0 | required | #1561 | legacy_ir 74 | Rust conversion policy (to_string/borrow/clone plans) for emission; the duckborrower facts in Body IR are the twin surface. |
| `loaves/compiler/incan_emit/src/emit/decls/functions.rs` | 3 | 1997 | 115 | retire | 0/3 | 0 | - | #1561 | codegen 3, legacy_ir 3 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/decls/mod.rs` | 5 | 1225 | 237 | retire | 0/5 | 0 | - | #1561 | codegen 2, checker 2, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/decls/structures.rs` | 8 | 1212 | 244 | retire | 0/8 | 0 | - | #1561 | codegen 8, text 7, legacy_ir 8, formatter 8 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/errors.rs` | 1 | 45 | 15 | retire | 0/1 | 0 | - | #1561 | - | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/builtins.rs` | 5 | 1087 | 174 | retire | 0/5 | 0 | - | #1561 | codegen 3, legacy_ir 4 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/calls.rs` | 29 | 2805 | 1278 | retire | 0/29 | 0 | - | #1561 | codegen 29, legacy_ir 29 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/indexing.rs` | 2 | 474 | 68 | retire | 0/2 | 0 | - | #1561 | codegen 2, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/mod.rs` | 52 | 4409 | 2798 | retire | 0/52 | 0 | required | #1561 | codegen 52, legacy_ir 52 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/expressions/structs_enums.rs` | 1 | 145 | 20 | retire | 0/1 | 0 | - | #1561 | codegen 1, legacy_ir 1 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/mod.rs` | 20 | 4605 | 952 | retire | 0/20 | 0 | - | #1561 | codegen 15, checker 4, legacy_ir 18 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/native_unions.rs` | 8 | 2072 | 1082 | retire | 0/8 | 0 | - | #1561 | codegen 6, checker 7, parser 7, legacy_ir 6 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/program.rs` | 10 | 4635 | 348 | retire | 0/10 | 0 | - | #1561 | codegen 10, text 2, checker 6, legacy_ir 10 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/statements.rs` | 8 | 1942 | 315 | retire | 0/8 | 0 | - | #1561 | codegen 7, text 4, legacy_ir 8 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/emit/types.rs` | 3 | 689 | 61 | retire | 0/3 | 0 | - | #1561 | codegen 3, legacy_ir 2 | emitter unit tests; frozen with the emitter under #1561, deleted by #654 once twinned. |
| `loaves/compiler/incan_emit/src/ownership.rs` | 45 | 1947 | 853 | retire | 0/45 | 0 | - | #1561 | legacy_ir 45 | emitter-side ownership and argument plans (borrow/clone/move for generated Rust); twins belong to Body IR ownership facts. |
| `loaves/compiler/incan_emit/src/reference_shape.rs` | 3 | 107 | 65 | retire | 0/3 | 0 | - | #1561 | legacy_ir 3 | Rust reference-shape helper for emitted callbacks. |
| `loaves/compiler/incan_emit/src/replacement/executable_resolution_tests.rs` | 17 | 1102 | 1102 | keep | - | - | - | #1561 | codegen 1, replacement 16, checker 17, parser 17 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/hashed/tests.rs` | 15 | 314 | 314 | keep | - | - | - | #1561 | replacement 15 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/mod.rs` | 4 | 8103 | 262 | keep | - | - | - | #1561 | replacement 4 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/provider/tests.rs` | 15 | 1004 | 1004 | keep | - | - | - | #1561 | replacement 15, checker 15, parser 15 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/provider/tests/host_preflight_tests.rs` | 13 | 589 | 589 | keep | - | - | - | #1561 | replacement 13 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/replacement/source_profile.rs` | 4 | 272 | 128 | keep | - | - | - | #1561 | replacement 3, parser 3 | replacement route (executable resolution, provider preflight, source profile). |
| `loaves/compiler/incan_emit/src/selection.rs` | 13 | 970 | 323 | keep | - | - | - | #1561 | replacement 13 | backend selection policy (replacement vs legacy); survives as the replacement route's own selection. |
| `loaves/compiler/incan_emit/src/tests/lowering_through_emission.rs` | 2 | 331 | 331 | retire | 0/2 | 0 | - | #1561 | codegen 1, checker 2, parser 2, legacy_ir 2 | lowering through emission end to end. |
| `loaves/compiler/incan_emit/src/trait_bound_inference.rs` | 15 | 4401 | 703 | retire | 0/15 | 0 | - | #1561 | codegen 5, legacy_ir 15 | infers Rust trait bounds for generated generics; Rust-shape concern. |
| `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs` | 14 | 675 | 675 | retire (keep 5, retire 9) | 1/9 | 0 | - | #1561 | codegen 9, snapshot 1, text 8, replacement 4, checker 14, parser 14 | empty-collection constructors: generated-text and snapshot assertions retire; the checked-type and Body IR aggregate assertions stay. |
| `loaves/compiler/incan_emit/tests/closure_local_call_codegen_tests.rs` | 4 | 208 | 208 | retire | 0/4 | 0 | - | #1561 | codegen 4, text 1, checker 4, parser 4 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/codegen_snapshot_tests.rs` | 289 | 6771 | 6771 | retire | 0/289 | 0 | required | #1561 | codegen 289, snapshot 1, text 31, checker 7, parser 289 | insta snapshot corpus of generated Rust; the .incn inputs under loaves/compiler/incan_emit/tests/codegen_snapshots are inventoried as a fixture root (re-point) and are the twin surface once run as programs. |
| `loaves/compiler/incan_emit/tests/construction_diagnostics_tests.rs` | 2 | 52 | 52 | keep | - | - | - | #1561 | checker 2, parser 2 | typechecker diagnostics for model construction. |
| `loaves/compiler/incan_emit/tests/constructor_argument_order_tests.rs` | 4 | 161 | 161 | retire | 4/4 | 0 | - | #1561 | codegen 1, checker 3, parser 4, legacy_ir 3 | argument sequencing asserted through the legacy IR and generated Rust; the CLI regression for issue 1462 runs the same program. |
| `loaves/compiler/incan_emit/tests/emitter_freeze_tests.rs` | 2 | 103 | 103 | unaffected | - | - | - | #1561 | - | emitter freeze gate (#1687); runs the fingerprint checker and goes with the emitter tree. |
| `loaves/compiler/incan_emit/tests/empty_list_comparison_operand_tests.rs` | 5 | 143 | 143 | keep (keep 3, retire 2) | 1/2 | 0 | - | #1561 | codegen 1, text 1, checker 4, parser 5, legacy_ir 1 | checker facts for empty-list operands; the lowered/generated assertions retire. |
| `loaves/compiler/incan_emit/tests/enumerate_value_codegen_tests.rs` | 1 | 71 | 71 | retire | 0/1 | 0 | - | #1561 | codegen 1, parser 1 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/fallible_entrypoint_codegen_tests.rs` | 1 | 50 | 50 | retire | 0/1 | 0 | - | #1561 | codegen 1, snapshot 1, parser 1 | snapshot of the generated entrypoint. |
| `loaves/compiler/incan_emit/tests/generated_stdlib_version_tests.rs` | 2 | 24 | 24 | retire | 0/2 | 0 | - | #1561 | codegen 1 | the stdlib line the emitter declares in generated code; the version contract moves with the emitter. |
| `loaves/compiler/incan_emit/tests/implicit_borrowing_codegen_tests.rs` | 21 | 719 | 719 | retire | 0/21 | 0 | - | #1561 | codegen 21, text 1, checker 7, parser 21, legacy_ir 7 | implicit borrowing asserted on generated Rust; duckborrower facts are the twin surface. |
| `loaves/compiler/incan_emit/tests/lowering_error_propagation.rs` | 1 | 33 | 33 | retire | 0/1 | 0 | - | #1561 | legacy_ir 1 | legacy IR lowering error propagation. |
| `loaves/compiler/incan_emit/tests/match_arm_ownership_codegen_tests.rs` | 4 | 151 | 151 | retire | 0/4 | 0 | - | #1561 | codegen 4, parser 4 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/nested_list_loop_tests.rs` | 5 | 156 | 156 | keep (keep 3, retire 2) | 1/2 | 0 | - | #1561 | codegen 1, checker 4, parser 5, legacy_ir 1 | checker facts for nested empty lists (issue 1471); one generated-text and one legacy IR assertion retire. |
| `loaves/compiler/incan_emit/tests/qualified_type_annotation_codegen_tests.rs` | 3 | 167 | 167 | retire | 0/3 | 0 | - | #1561 | codegen 3, parser 3 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/return_operand_ownership_tests.rs` | 5 | 205 | 205 | retire | 5/5 | 0 | - | #1561 | checker 5, parser 5, legacy_ir 5 | return-operand move/read decisions asserted through the legacy IR; the CLI regression for issue 1489 runs the loop-variable case, the other cases still need Body IR ownership twins. |
| `loaves/compiler/incan_emit/tests/stdlib_generated_rust_snapshot_tests.rs` | 14 | 434 | 434 | retire | 0/14 | 0 | - | #1561 | codegen 14, snapshot 13, parser 14 | insta snapshots of generated stdlib Rust. |
| `loaves/compiler/incan_emit/tests/string_helper_codegen_tests.rs` | 1 | 45 | 45 | retire | 0/1 | 0 | - | #1561 | codegen 1, snapshot 1, text 1, parser 1 | snapshot and generated text. |
| `loaves/compiler/incan_emit/tests/string_len_codegen_tests.rs` | 2 | 43 | 43 | retire | 0/2 | 0 | - | #1561 | codegen 2, parser 2 | asserts generated Rust text. |
| `loaves/compiler/incan_emit/tests/zip_alias_codegen_tests.rs` | 10 | 345 | 345 | retire | 0/10 | 0 | - | #1561 | codegen 7, parser 7, legacy_ir 3 | generated clone placement for zip aliases; three assignment-plan unit tests are emitter ownership plans and retire with them. |

Per-test overrides in `loaves/compiler/incan_emit/src/checked_program/sdk_module_derives.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `sdk_module_derives_metadata_round_trip_preserves_exact_membership` | keep | - | - | checker, parser | metadata round trip through the checker, no codegen |
| `sdk_module_derives_missing_membership_stays_rejected` | keep | - | - | checker, parser | checker rejection, no codegen |

Per-test overrides in `loaves/compiler/incan_emit/src/checked_program/tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `rust_trait_associated_call_uses_callable_shape_from_dependency_namespace_source` | retire | - | - | codegen, checker, parser | drives IrCodegen directly |

Per-test overrides in `loaves/compiler/incan_emit/src/codegen.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `string_membership_probe_borrows_loop_binding_used_later_in_branch_issue1057` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/membership_loop_binding.incn` | - | codegen, generated_text, checker, legacy_ir | twinned by the #1057 program run to completion: the loop binding is used after the membership probe, which is the observable half. Of the retired test's three assertions, the one a run cannot see, `!code.contains("name.clone()")` (the probe borrows rather than clones), is not user-observable and dies with #654; the twin stays. |
| `test_simple_function` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/simple_function_add.incn` | - | codegen, checker, legacy_ir | twinned by a program that calls the function and prints the sum. |
| `test_model_generation` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/model_fields.incn` | - | codegen, checker, legacy_ir | twinned by a program that builds the model and prints its fields. |
| `test_fstring_generation` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/fstring_greeting.incn` | - | codegen, checker, legacy_ir | twinned by a program that prints the interpolated greeting. |
| `test_struct_instantiation` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/model_fields.incn` | - | codegen, checker, legacy_ir | twinned by a program that builds the model with named arguments and prints each field. |
| `test_enum_generation` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/smoke/enum_variants.incn` | - | codegen, checker, legacy_ir | twinned by a program that matches each variant and prints its name. |

Per-test overrides in `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `checked_empty_collection_constructors_lower_to_existing_aggregate_shapes_issue1247` | keep | - | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `checked_empty_list_constructor_lowers_to_the_list_aggregate_issue1464` | keep | - | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `mismatched_typed_empty_collection_constructor_contexts_remain_errors_issue1247` | keep | - | - | checker, parser | checked result type or Body IR aggregate shape |
| `shadowed_collection_constructors_do_not_gain_aggregate_lowering_issue1247` | keep | - | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `missing_checked_collection_constructor_facts_do_not_guess_aggregate_lowering_issue1247` | keep | - | - | replacement, checker, parser | checked result type or Body IR aggregate shape |
| `checked_typed_empty_dict_reaches_existing_aggregate_emission_issue1247` | retire | `loaves/compiler/incan_emit/tests/checked_empty_collection_constructor_tests.rs::checked_empty_collection_constructors_lower_to_existing_aggregate_shapes_issue1247` | - | codegen, snapshot, checker, parser | snapshot of the emitted aggregate |

Per-test overrides in `loaves/compiler/incan_emit/tests/empty_list_comparison_operand_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `lowered_empty_list_operand_carries_the_element_type` | retire | - | - | checker, parser, legacy_ir | legacy IR shape |
| `generated_rust_names_the_element_type_on_the_empty_operand` | retire | `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs::empty_list_equality_operands_build_with_json_cohort_issue1476` | - | codegen, generated_text, parser | asserts generated Rust text |

Per-test overrides in `loaves/compiler/incan_emit/tests/nested_list_loop_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `nested_list_empty_holes_retain_checked_string_types` | retire | - | - | checker, parser, legacy_ir | legacy IR shape; the checked-type assertions (`List[List[str]]` on the empty holes) are the keep-class content the twin must carry. |
| `nested_list_loop_emits_owned_strings_without_caller_annotation` | retire | `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs::nested_empty_first_list_runs_without_a_caller_annotation_issue1471` | - | codegen, parser | asserts generated Rust text |

### `loaves/compiler/incan_format` (191 tests in 5 files: keep 191)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_format/src/config.rs` | 24 | 275 | 206 | keep | - | - | - | #1561 | - | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/formatter/tests.rs` | 11 | 327 | 327 | keep | - | - | - | #1561 | parser 7, formatter 11 | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/lib.rs` | 112 | 2852 | 2623 | keep | - | - | required | #1561 | checker 1, parser 110, formatter 110 | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/src/writer.rs` | 37 | 565 | 389 | keep | - | - | - | #1561 | - | formatter; no emit/driver dependency. Reviewed at crate level. |
| `loaves/compiler/incan_format/tests/property_tests.rs` | 7 | 411 | 385 | keep | - | - | - | #1561 | parser 4, formatter 6 | formatter; no emit/driver dependency. Reviewed at crate level. |

### `loaves/compiler/incan_frontend` (1707 tests in 83 files: keep 1707)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_frontend/src/api_metadata.rs` | 17 | 3762 | 944 | keep | - | - | - | #1561 | checker 17, parser 17 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/body_ir.rs` | 2 | 1094 | 49 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/async_and_race.rs` | 14 | 390 | 390 | keep | - | - | - | #1561 | replacement 1 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/calls_and_arguments.rs` | 37 | 996 | 996 | keep | - | - | - | #1561 | parser 1 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/closures_comprehensions_and_generators.rs` | 28 | 797 | 797 | keep | - | - | - | #1561 | parser 1 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/control_flow_and_loops.rs` | 21 | 670 | 670 | keep | - | - | - | #1561 | replacement 1 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/for_patterns.rs` | 16 | 482 | 482 | keep | - | - | - | #1561 | replacement 3, checker 6, parser 7 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/identities_and_imports.rs` | 17 | 1027 | 1027 | keep | - | - | - | #1561 | replacement 12, checker 12, parser 12 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/input_contract_and_refusals.rs` | 7 | 298 | 298 | keep | - | - | - | #1561 | replacement 5, checker 5, parser 5 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/methods_and_defaults.rs` | 31 | 1005 | 1005 | keep | - | - | - | #1561 | replacement 1, checker 1, parser 1 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/operators_literals_and_assignment.rs` | 38 | 859 | 859 | keep | - | - | - | #1561 | - | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/patterns_and_assertions.rs` | 24 | 803 | 803 | keep | - | - | - | #1561 | replacement 7, checker 2, parser 2 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/body_ir/tests/provider_plans.rs` | 14 | 627 | 627 | keep | - | - | - | #1561 | replacement 13, checker 13, parser 13 | split of body_ir/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/compiler_stack.rs` | 3 | 113 | 41 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/contract_metadata.rs` | 5 | 517 | 89 | keep | - | - | - | #1561 | parser 2, formatter 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/hir.rs` | 7 | 540 | 328 | keep | - | - | - | #1561 | checker 7, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_exports.rs` | 6 | 2104 | 179 | keep | - | - | - | #1561 | checker 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/artifact.rs` | 16 | 1677 | 937 | keep | - | - | - | #1561 | checker 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/published_layout.rs` | 2 | 287 | 47 | keep | - | - | - | #1561 | checker 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests/export_round_trips.rs` | 22 | 1086 | 1086 | keep | - | - | - | #1561 | checker 20 | split of library_manifest/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests/formats_and_metadata.rs` | 18 | 572 | 572 | keep | - | - | - | #1561 | checker 17 | split of library_manifest/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests/identity_graph.rs` | 7 | 926 | 926 | keep | - | - | - | #1561 | checker 7 | split of library_manifest/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests/identity_validation.rs` | 9 | 1084 | 1084 | keep | - | - | - | #1561 | checker 9, parser 1 | split of library_manifest/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/library_manifest/tests/vocab_and_scoped_surfaces.rs` | 19 | 732 | 732 | keep | - | - | - | #1561 | checker 19 | split of library_manifest/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/library_manifest/type_projection.rs` | 1 | 502 | 69 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/library_manifest_index.rs` | 10 | 1387 | 468 | keep | - | - | - | #1561 | checker 8 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/module.rs` | 33 | 1659 | 913 | keep | - | - | - | #1561 | parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/features.rs` | 11 | 1919 | 578 | keep | - | - | - | #1561 | checker 10 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/plan.rs` | 15 | 2789 | 697 | keep | - | - | - | #1561 | checker 13 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/sdk.rs` | 13 | 1305 | 270 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/provider/stdlib_sources.rs` | 2 | 223 | 71 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/resolved_type_subst.rs` | 1 | 217 | 45 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/rust_type_display.rs` | 7 | 754 | 171 | keep | - | - | - | #1561 | parser 6 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/surface_semantics.rs` | 3 | 138 | 48 | keep | - | - | - | #1561 | parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/symbols.rs` | 16 | 3053 | 462 | keep | - | - | - | #1561 | checker 10 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/testing_markers.rs` | 7 | 871 | 164 | keep | - | - | - | #1561 | checker 1, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/check_expr/calls/rust_boundary.rs` | 43 | 2524 | 1425 | keep | - | - | - | #1561 | text 2, checker 43 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/collect/stdlib_imports.rs` | 1 | 4825 | 32 | keep | - | - | - | #1561 | checker 1 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/identity_surface_tests.rs` | 8 | 284 | 284 | keep | - | - | - | #1561 | checker 8, parser 8 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/stdlib_loader.rs` | 31 | 3072 | 1113 | keep | - | - | - | #1561 | parser 30 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/async_and_iteration.rs` | 51 | 957 | 957 | keep | - | - | - | #1561 | checker 37 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/bounds_and_derives.rs` | 39 | 1122 | 1122 | keep | - | - | - | #1561 | checker 21, parser 10 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/calls_decorators_and_builtins.rs` | 50 | 1153 | 1153 | keep | - | - | - | #1561 | checker 27, parser 9 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/canonical_identity/declarations_and_scopes.rs` | 26 | 995 | 995 | keep | - | - | - | #1561 | checker 5 | split of typechecker/canonical_identity_tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/canonical_identity/duplicates_and_collisions.rs` | 12 | 605 | 605 | keep | - | - | - | #1561 | checker 11 | split of typechecker/canonical_identity_tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/canonical_identity/imports_and_dependencies.rs` | 19 | 818 | 818 | keep | - | - | - | #1561 | checker 18 | split of typechecker/canonical_identity_tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/canonical_identity/references_calls_and_patterns.rs` | 18 | 758 | 758 | keep | - | - | - | #1561 | checker 9 | split of typechecker/canonical_identity_tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/capabilities.rs` | 20 | 553 | 553 | keep | - | - | - | #1561 | checker 20, parser 9 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/checked_facts_and_registries.rs` | 21 | 986 | 986 | keep | - | - | - | #1561 | checker 20, parser 2 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/collections_strings_and_bytes.rs` | 46 | 984 | 984 | keep | - | - | - | #1561 | checker 42, parser 4 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/extern_and_c_bindings.rs` | 22 | 825 | 825 | keep | - | - | - | #1561 | checker 16, parser 3 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/fields_and_members.rs` | 51 | 1037 | 1037 | keep | - | - | - | #1561 | checker 47, parser 5 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/generics_and_type_tokens.rs` | 35 | 932 | 932 | keep | - | - | - | #1561 | checker 22, parser 9 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/imports_and_stdlib_modules.rs` | 42 | 1155 | 1155 | keep | - | - | - | #1561 | checker 31, parser 7 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/models_enums_and_newtypes.rs` | 51 | 963 | 963 | keep | - | - | - | #1561 | checker 37, parser 2 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/narrowing_and_matching.rs` | 53 | 1011 | 1011 | keep | - | - | - | #1561 | checker 53, parser 1 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/numerics_const_and_static.rs` | 62 | 983 | 983 | keep | - | - | - | #1561 | checker 61 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/partials_and_callable_aliases.rs` | 33 | 1117 | 1117 | keep | - | - | - | #1561 | checker 33, parser 12 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/pub_imports_namespaces_and_fields.rs` | 21 | 819 | 819 | keep | - | - | - | #1561 | checker 21, parser 16 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/pub_imports_symbols_and_identity.rs` | 30 | 1038 | 1038 | keep | - | - | - | #1561 | checker 29, parser 5 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/pub_imports_trait_adoptions.rs` | 8 | 700 | 700 | keep | - | - | - | #1561 | checker 8, parser 2 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_constructors_and_fields.rs` | 15 | 974 | 974 | keep | - | - | - | #1561 | checker 14, parser 11 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_generics_and_traits.rs` | 19 | 1163 | 1163 | keep | - | - | - | #1561 | checker 18, parser 12 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_imports_and_types.rs` | 55 | 1051 | 1051 | keep | - | - | - | #1561 | checker 41, parser 5 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_metadata_and_methods.rs` | 27 | 1317 | 1317 | keep | - | - | - | #1561 | checker 23, parser 11 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_supertraits.rs` | 7 | 164 | 164 | keep | - | - | - | #1561 | checker 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_trait_import_candidates.rs` | 6 | 155 | 155 | keep | - | - | - | #1561 | checker 6, parser 6 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/rust_trait_qualified_calls.rs` | 9 | 325 | 325 | keep | - | - | - | #1561 | checker 8, parser 7 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. asserts checker facts about `rust::` imports; moves with #1337's interop spec in slice 7, not with the route. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/statements_and_bindings.rs` | 69 | 1175 | 1175 | keep | - | - | - | #1561 | checker 66, parser 1 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/stdlib_surfaces.rs` | 44 | 1168 | 1168 | keep | - | - | - | #1561 | checker 31, parser 12 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/trait_instantiation_and_operators.rs` | 42 | 1363 | 1363 | keep | - | - | - | #1561 | checker 41, parser 11 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/tests/traits.rs` | 46 | 1057 | 1057 | keep | - | - | - | #1561 | checker 42, parser 22 | split of typechecker/tests.rs; typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. |
| `loaves/compiler/incan_frontend/src/typechecker/validate_rust_module.rs` | 2 | 270 | 30 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_ast_bridge.rs` | 12 | 1896 | 491 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_desugar_pass/helper_bindings.rs` | 15 | 829 | 455 | keep | - | - | - | #1561 | checker 11 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/src/vocab_desugar_pass/runtime.rs` | 4 | 860 | 78 | keep | - | - | - | #1561 | - | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/checked_string_helper_identity_tests.rs` | 5 | 389 | 389 | keep | - | - | - | #1561 | replacement 4, checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/checked_string_len_identity_tests.rs` | 5 | 158 | 158 | keep | - | - | - | #1561 | replacement 4, checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/declaration_identity_corpus.rs` | 3 | 348 | 348 | keep | - | - | - | #1561 | replacement 2, checker 3, parser 3 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_core_parity.rs` | 6 | 125 | 125 | keep | - | - | - | #1561 | checker 2, parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_core_parity_strings.rs` | 10 | 203 | 203 | keep | - | - | - | #1561 | checker 2, parser 2 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/semantic_digest_invariants.rs` | 9 | 293 | 293 | keep | - | - | - | #1561 | replacement 9, checker 9, parser 9 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |
| `loaves/compiler/incan_frontend/tests/stdlib_module_trait_tests.rs` | 5 | 132 | 132 | keep | - | - | - | #1561 | checker 5, parser 5 | typechecker, Body IR, library manifests, provider plans; no emit/driver dependency. Reviewed at crate level; generated-text hits are Incan source or diagnostics. |

### `loaves/compiler/incan_ir` (146 tests in 11 files: retire 146)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_ir/src/decl.rs` | 1 | 711 | 11 | retire | 0/1 | 0 | - | #1561 | - | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/expr.rs` | 3 | 1165 | 81 | retire | 0/3 | 0 | - | #1561 | legacy_ir 3 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lib.rs` | 5 | 793 | 136 | retire | 0/5 | 0 | - | #1561 | legacy_ir 5 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/helpers.rs` | 3 | 883 | 150 | retire | 0/3 | 0 | - | #1561 | legacy_ir 3 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/methods.rs` | 1 | 2233 | 78 | retire | 0/1 | 0 | - | #1561 | checker 1, parser 1, legacy_ir 1 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/decl/traits.rs` | 3 | 279 | 45 | retire | 0/3 | 0 | - | #1561 | legacy_ir 3 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/expr/calls.rs` | 20 | 5372 | 1079 | retire | 0/20 | 0 | - | #1561 | checker 9, parser 4, legacy_ir 18 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/expr/mod.rs` | 15 | 3135 | 351 | retire | 0/15 | 0 | - | #1561 | checker 3, legacy_ir 15 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/mod.rs` | 30 | 5189 | 1095 | retire | 0/30 | 0 | - | #1561 | checker 25, parser 26, legacy_ir 30 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/lower/types.rs` | 13 | 1864 | 343 | retire | 0/13 | 0 | - | #1561 | checker 1, legacy_ir 13 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |
| `loaves/compiler/incan_ir/src/types.rs` | 52 | 1305 | 464 | retire | 0/52 | 0 | - | #1561 | legacy_ir 52 | the Rust-source backend's own lowering (`AstLowering`, `IrProgram`, `IrType`, Rust name spellings). Retire because its only consumers are `incan_emit` and the driver's `backend/ir` re-export, and `replacement/**` and `shadow/**` import nothing from it; flips to keep if #654 keeps the generated-project inspection path or the keep definition is read to include this lowering. Twins belong in Body IR (loaves/compiler/incan_frontend/src/body_ir/tests/). Reviewed at crate level. |

### `loaves/compiler/incan_test_support` (22 tests in 3 files: retire 3, unaffected 19)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/compiler/incan_test_support/src/behavior_fixtures.rs` | 18 | 1671 | 596 | unaffected | - | - | - | #1561 | run 1 | test-support helper for the behavior-fixture family: header grammar, discovery, materialization and observable comparison. The module drives `incan check` / `incan run` through `run_incan`; its unit tests exercise the grammar, discovery and the comparison rules with no compiler call. |
| `loaves/compiler/incan_test_support/src/builtin_stdlib.rs` | 1 | 75 | 16 | unaffected | - | - | - | #1561 | - | test-support helper for the builtin stdlib inventory. |
| `loaves/compiler/incan_test_support/src/emitted_symbol_artifact.rs` | 3 | 421 | 50 | retire | 0/3 | 0 | - | #1561 | - | helpers over emitted symbol projections (RFC 120 physical names). |

### `loaves/kernel/incan_lang` (106 tests in 20 files: keep 106)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/kernel/incan_lang/src/bin/generate_vscode_grammar_keywords.rs` | 2 | 161 | 35 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/errors.rs` | 6 | 327 | 85 | keep | - | - | - | #1561 | checker 1 | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/interop/coercions.rs` | 6 | 448 | 84 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/interop/metadata.rs` | 21 | 2022 | 396 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/builtins.rs` | 1 | 339 | 12 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/c_abi.rs` | 1 | 465 | 74 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/callables.rs` | 1 | 90 | 14 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/conventions.rs` | 1 | 55 | 13 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/highlighting.rs` | 4 | 217 | 56 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/keywords.rs` | 3 | 939 | 25 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/stdlib.rs` | 11 | 1452 | 473 | keep | - | - | - | #1561 | checker 1 | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/text_codecs.rs` | 2 | 95 | 36 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/traits.rs` | 1 | 373 | 24 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lang/types/numerics.rs` | 4 | 469 | 79 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/lib.rs` | 5 | 331 | 99 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/numeric_strings.rs` | 6 | 204 | 113 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/numeric_values.rs` | 3 | 222 | 53 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/src/strings.rs` | 2 | 366 | 30 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/tests/lang_registry_guardrails.rs` | 25 | 671 | 671 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_lang/tests/string_len_semantics.rs` | 1 | 12 | 12 | keep | - | - | - | #1561 | - | language registry (stdlib inventory, interop metadata, version); below the emitter. Reviewed at crate level. |

### `loaves/kernel/incan_semantics_core` (128 tests in 13 files: keep 128)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/kernel/incan_semantics_core/src/authority.rs` | 9 | 344 | 177 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/body_ir.rs` | 32 | 4463 | 1080 | keep | - | - | - | #1561 | replacement 1 | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/closure_digest.rs` | 12 | 430 | 226 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/dependencies.rs` | 4 | 317 | 155 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/emitted_symbol.rs` | 7 | 559 | 177 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/executable_representation.rs` | 13 | 1308 | 583 | keep | - | - | - | #1561 | replacement 13 | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/facts.rs` | 12 | 1494 | 408 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/hir.rs` | 2 | 242 | 66 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/namespace.rs` | 7 | 148 | 78 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/receipts.rs` | 12 | 1014 | 431 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/semantic_digest.rs` | 4 | 502 | 84 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/stable_identity.rs` | 7 | 444 | 245 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |
| `loaves/kernel/incan_semantics_core/src/types.rs` | 7 | 435 | 95 | keep | - | - | - | #1561 | - | semantics core (Body IR, receipts, authority); below the emitter. Reviewed at crate level. |

### `loaves/kernel/incan_syntax` (307 tests in 15 files: keep 307)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/kernel/incan_syntax/src/ast/types.rs` | 1 | 252 | 39 | keep | - | - | - | #1561 | - | lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/diagnostics/base.rs` | 2 | 396 | 39 | keep | - | - | - | #1561 | checker 1 | lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/diagnostics/stable.rs` | 5 | 605 | 121 | keep | - | - | - | #1561 | checker 5 | lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/lexer/mod.rs` | 20 | 909 | 400 | keep | - | - | - | #1561 | checker 20, parser 20 | lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/embedded/tests.rs` | 28 | 915 | 907 | keep | - | - | - | #1561 | checker 24, parser 4 | lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/expressions.rs` | 26 | 769 | 769 | keep | - | - | - | #1561 | checker 20, parser 26 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/fstrings.rs` | 7 | 365 | 365 | keep | - | - | - | #1561 | checker 7, parser 7 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/functions_decorators_and_bindings.rs` | 23 | 556 | 556 | keep | - | - | - | #1561 | checker 16, parser 23 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/modules_and_imports.rs` | 47 | 883 | 883 | keep | - | - | - | #1561 | checker 30, parser 47 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/patterns_and_matching.rs` | 16 | 555 | 555 | keep | - | - | - | #1561 | checker 12, parser 16 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/soft_keywords_and_vocab_blocks.rs` | 20 | 655 | 655 | keep | - | - | - | #1561 | checker 5, parser 20 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/statements_and_blocks.rs` | 19 | 426 | 426 | keep | - | - | - | #1561 | checker 17, parser 19 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/type_declarations.rs` | 58 | 1134 | 1134 | keep | - | - | - | #1561 | checker 35, parser 58, formatter 2 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/types_and_bounds.rs` | 21 | 412 | 412 | keep | - | - | - | #1561 | checker 13, parser 21 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |
| `loaves/kernel/incan_syntax/src/parser/tests/vocab_scoped_symbols.rs` | 14 | 824 | 824 | keep | - | - | - | #1561 | parser 14 | split of parser/tests.rs; lexer, parser and diagnostics catalog; below the emitter, cannot reach codegen. Reviewed at crate level. |

### `loaves/toolchain/incan-cli` (626 tests in 43 files: keep 117, re-point 350, retire 34, unaffected 125)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/toolchain/incan-cli/src/bin/generate_feature_inventory.rs` | 1 | 85 | 23 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/build.rs` | 11 | 1317 | 620 | keep (keep 8, retire 3) | 0/3 | 3 | - | #1561 | replacement 6, checker 3, parser 1 | build command over checked facts (contract step, library publication); the rustc-failure classification tests belong to the generated build. |
| `loaves/toolchain/incan-cli/src/commands/cache.rs` | 2 | 131 | 17 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/debug.rs` | 1 | 178 | 22 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/init.rs` | 16 | 755 | 316 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/lifecycle.rs` | 4 | 924 | 104 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/representation_inspect.rs` | 4 | 503 | 126 | keep | - | - | - | #1561 | checker 4 | representation inspection from checked facts. |
| `loaves/toolchain/incan-cli/src/commands/tools_boundary_tests.rs` | 1 | 116 | 116 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/commands/workspace.rs` | 1 | 428 | 45 | unaffected | - | - | - | #1561 | - | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/lib.rs` | 37 | 3389 | 1069 | unaffected | - | - | - | #1561 | codegen 1, replacement 9 | CLI surface (argument parsing, scaffolding, lifecycle, cache); no compiler semantics. |
| `loaves/toolchain/incan-cli/src/test_runner/execution.rs` | 21 | 3539 | 618 | keep (keep 10, retire 11) | 0/11 | 11 | - | #1561 | text 1, checker 3, parser 3 | `incan test` today lowers Incan tests into a Rust libtest harness; the harness-shape tests retire with it, the discovery and session tests stay. |
| `loaves/toolchain/incan-cli/src/test_runner/mod.rs` | 15 | 2148 | 492 | keep | - | - | - | #1561 | checker 3 | test collection, parametrize expansion, marker selection and scheduling. |
| `loaves/toolchain/incan-cli/tests/behavior_cli_dependencies_tests.rs` | 1 | 22 | 22 | re-point | - | - | - | #1561 | - | runs every behavior fixture of the cli_dependencies area as a program, baking its in-fixture providers first with no Cargo authority (the root is deliberately not registered in OvenCompilerSuiteTargetCapabilities), and compares its observables; the route changes under it, the fixtures do not. |
| `loaves/toolchain/incan-cli/tests/behavior_cli_tests.rs` | 1 | 17 | 17 | re-point | - | - | - | #1561 | - | runs every behavior fixture of the cli area as a program and compares its observables; the route changes under it, the fixtures do not. |
| `loaves/toolchain/incan-cli/tests/behavior_driver_tests.rs` | 1 | 18 | 18 | re-point | - | - | - | #1561 | - | runs every behavior fixture of the driver area as a program and compares its observables; the route changes under it, the fixtures do not. |
| `loaves/toolchain/incan-cli/tests/behavior_harness_tests.rs` | 1 | 21 | 21 | re-point | - | - | - | #1561 | - | runs the harness area's self-proof fixtures as programs; the route changes under it, the fixtures do not. |
| `loaves/toolchain/incan-cli/tests/behavior_smoke_tests.rs` | 1 | 16 | 16 | re-point | - | - | - | #1561 | - | runs every behavior fixture of the smoke area as a program and compares its observables; the route changes under it, the fixtures do not. |
| `loaves/toolchain/incan-cli/tests/canonical_item_imports.rs` | 3 | 229 | 229 | re-point (keep 1, re-point 2) | - | - | - | #1561 | run 2 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7; the one `check`- or `inspect`-only test is keep (override). |
| `loaves/toolchain/incan-cli/tests/cli_catalog_forms_tests.rs` | 1 | 113 | 113 | re-point | - | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_codegraph_and_inspection_tests.rs` | 16 | 2016 | 2016 | re-point (keep 8, re-point 7, retire 1) | 0/1 | 1 | required | #1561 | run 7 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. `inspect codegraph`/`inspect bindings`/`check`-only tests are keep; `inspect rust` asserts the generated-project report and generated Rust text and retires unless #654 keeps that inspection path (overrides). |
| `loaves/toolchain/incan-cli/tests/cli_decorator_and_partial_tests.rs` | 11 | 1113 | 1113 | re-point (re-point 10, retire 1) | 1/1 | 0 | - | #1561 | codegen 1, run 11 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_float_display_tests.rs` | 1 | 86 | 86 | re-point | - | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_interop_target_tests.rs` | 9 | 868 | 868 | re-point (keep 1, re-point 8) | - | - | - | #1561 | run 8 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7; the one `check`- or `inspect`-only test is keep (override). |
| `loaves/toolchain/incan-cli/tests/cli_issue1370_phantom_type_param_tests.rs` | 1 | 93 | 93 | re-point | - | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_issue1668_stdlib_gaps_tests.rs` | 2 | 235 | 235 | re-point | - | - | - | #1561 | run 2 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs` | 25 | 2101 | 2101 | re-point (re-point 25) | - | - | required | #1561 | codegen 5, run 25 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that also read generated Rust (`target/incan/<project>/src/main.rs`, `--emit-rust`) stay re-point on their build or run assertion and lose the generated-text assertion in slice 7 (overrides). |
| `loaves/toolchain/incan-cli/tests/cli_layering_guardrails.rs` | 2 | 189 | 189 | unaffected | - | - | - | #1561 | codegen 2, legacy_ir 2 | layering baseline of what the CLI may reach; names codegen types without using them. |
| `loaves/toolchain/incan-cli/tests/cli_provider_boundary_tests.rs` | 19 | 1343 | 1343 | re-point | - | - | - | #1561 | codegen 2, run 19 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/cli_rust_interop_tests.rs` | 13 | 1504 | 1504 | re-point (re-point 12, retire 1) | 0/1 | 0 | required | #1561 | codegen 7, text 1, run 13 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that also read generated Rust (`target/incan/<project>/src/main.rs`) stay re-point on their build or run assertion and lose the generated-text assertion in slice 7 (overrides); the one library-build test that only asserts generated text retires. |
| `loaves/toolchain/incan-cli/tests/cli_std_environ_tests.rs` | 4 | 473 | 473 | re-point | - | - | - | #1561 | run 4 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/cli_surface_tests.rs` | 26 | 1432 | 1432 | re-point (keep 10, re-point 12, unaffected 4) | - | - | - | #1561 | run 12 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. `check`/`explain`/`tools metadata api` tests are keep; `init`, `tools doctor`, `lock` and the fixture-handoff helper test are unaffected (overrides). |
| `loaves/toolchain/incan-cli/tests/cli_workspace_and_lock_tests.rs` | 26 | 1905 | 1905 | re-point (keep 2, re-point 19, unaffected 5) | - | - | required | #1561 | codegen 1, run 20 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. `workspace check`/`workspace fmt` tests are keep; `lock`/`workspace inspect`-only tests are unaffected; one build test also reads generated Rust and loses that assertion in slice 7 (overrides). |
| `loaves/toolchain/incan-cli/tests/example_capability_coverage.rs` | 1 | 279 | 279 | keep | - | - | - | #1561 | replacement 1 | examples cover the stable capability registry. |
| `loaves/toolchain/incan-cli/tests/integration_tests.rs` | 203 | 12563 | 12563 | re-point (keep 30, re-point 161, retire 8, unaffected 4) | 4/8 | 1 | required | #1561 | codegen 8, text 7, run 164, checker 14, parser 29 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Checker-only and lexer tests stay; generated-text and IrCodegen tests retire; `fmt`- and `check`-only CLI tests are keep and `--help`/`--version`/`lock`-only ones unaffected (overrides). |
| `loaves/toolchain/incan-cli/tests/layering_guard.rs` | 9 | 359 | 359 | unaffected | - | - | - | #1561 | - | crate and stdlib layering guards. |
| `loaves/toolchain/incan-cli/tests/package_boundary_facade_tests.rs` | 7 | 646 | 646 | re-point | - | - | - | #1561 | run 7, checker 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/package_executable_representation.rs` | 6 | 724 | 724 | re-point | - | - | - | #1561 | run 6, checker 3 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that read the generated .rs retire. |
| `loaves/toolchain/incan-cli/tests/repository_path_tests.rs` | 5 | 61 | 61 | unaffected | - | - | - | #1561 | - | repository path resolution for the compiler command. |
| `loaves/toolchain/incan-cli/tests/rfc031_pub_import_integration_tests.rs` | 82 | 8051 | 8051 | re-point (keep 27, re-point 46, retire 9) | 1/9 | 6 | required | #1561 | codegen 9, text 4, run 49, checker 33, parser 12 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. Tests that build and also read generated Rust stay re-point and lose the generated-text assertion in slice 7; tests whose only backend assertion is generated text (`--emit-rust` after `incan check`, a planned build's Rust) retire; `check`-only and `fmt`-only tests are keep (overrides). |
| `loaves/toolchain/incan-cli/tests/script_target_diagnostics.rs` | 1 | 44 | 44 | re-point | - | - | - | #1561 | - | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/std_encoding_algorithm_modules.rs` | 1 | 128 | 128 | re-point | - | - | - | #1561 | run 1 | runs `incan` and asserts output, exit code or diagnostics; the route changes in slice 7. |
| `loaves/toolchain/incan-cli/tests/toolchain_installer_tests.rs` | 30 | 2728 | 2728 | unaffected | - | - | required | #1561 | run 4 | installer, archive packager and release manifest. |
| `loaves/toolchain/incan-cli/tests/vocab_guardrails.rs` | 3 | 575 | 575 | unaffected | - | - | - | #1561 | text 1 | source audits (semantic string audit, stringly vocab checks). |

Per-test overrides in `loaves/toolchain/incan-cli/src/commands/build.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `classify_signature_mismatch_for_rust_extern_context` | retire | `dies` | classifies a rustc `error[E0308]` line in the generated Cargo build's stderr back to the `@rust.extern` item; there is no rustc stderr to classify once nothing is generated (#654). The helpers under test are `#[allow(dead_code)]` with no production caller; the tests pin nothing reachable today. | - | maps a rustc failure of the generated build back to source |
| `classify_unresolved_backing_item_for_rust_extern_context` | retire | `dies` | classifies a rustc `error[E0425]` line in the generated Cargo build's stderr as an unresolved `@rust.extern` backing item; there is no rustc stderr to classify once nothing is generated (#654). The helpers under test are `#[allow(dead_code)]` with no production caller; the tests pin nothing reachable today. | - | maps a rustc failure of the generated build back to source |
| `wraps_rust_extern_failure_back_to_incan_declaration_span` | retire | `dies` | renders a rustc `error[E0425]` of the generated build as a diagnostic at the `@rust.extern` declaration span; the rustc failure it wraps dies with the generated build (#654). The helpers under test are `#[allow(dead_code)]` with no production caller; the tests pin nothing reachable today. | - | maps a rustc failure of the generated build back to source |

Per-test overrides in `loaves/toolchain/incan-cli/src/test_runner/execution.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `merge_test_runner_dependencies_promotes_dev_deps_into_dependencies` | retire | `dies` | merges `[dependencies]` and `[dev-dependencies]` into the generated test-runner crate's Cargo dependency set (`promoted_oven_test_dependencies`); the generated crate dies with #654. Its user-visible half — a test file using a `[rust-dev-dependencies]` crate through `incan test` — is untwinned: no CLI test uses `rust-dev-dependencies`; it goes with #1337's interop spec (slice 7). | - | libtest harness of the generated test crate |
| `merge_test_runner_dependencies_unifies_normal_and_dev_features` | retire | `dies` | unions Cargo feature lists and refuses a version conflict inside the generated test-runner crate's Cargo dependency set; the generated crate dies with #654. Its user-visible half — a normal/dev version conflict surfacing as an `incan test` failure naming the crate — is untwinned and goes with #1337's interop spec (slice 7). | - | libtest harness of the generated test crate |
| `parse_libtest_outcomes_detects_ok_and_failed` | retire | `dies` | parses libtest's `test ... ok\|FAILED` lines from the generated test crate's transcript; the transcript dies with the crate (#654). `incan test` reporting a pass and a failure is proved by the re-point `integration_tests.rs::e2e_generated_harness_success_reports_the_incan_test_identity_issue996` and `e2e_failure_skip_and_assert_reporting_share_one_project`. | - | libtest harness of the generated test crate |
| `parse_libtest_outcomes_normalizes_prefixed_names` | retire | `dies` | strips the generated runner crate's name from libtest's qualified test names in the transcript; the crate and its transcript die with #654. The `.incn::fn` identity in the report is carried by `e2e_generated_harness_success_reports_the_incan_test_identity_issue996` (re-point). | - | libtest harness of the generated test crate |
| `successful_native_batch_preserves_a_passing_harness_result_issue996` | retire | `dies` | maps the libtest summary line (`test result: ok. 1 passed`) of the generated test crate to a passed harness result; the transcript dies with #654. The user-visible half, a passing Incan test reported under its own identity (#996), is the re-point `integration_tests.rs::e2e_generated_harness_success_reports_the_incan_test_identity_issue996`. | - | libtest harness of the generated test crate |
| `extracts_unindented_libtest_panic_payloads_from_a_batch` | retire | `dies` | extracts assertion payloads from the `thread '...' panicked at` lines of the generated test crate's transcript; the transcript dies with #654. The assertion message reaching the `incan test` report (`AssertionError: custom boom`) is the re-point `integration_tests.rs::e2e_failure_skip_and_assert_reporting_share_one_project`. Teardown payload lines are carried by `e2e_fixture_teardown_failure_scenarios_share_one_project` (re-point). | - | libtest harness of the generated test crate |
| `runner_crate_name_is_derived_from_batch_suffix` | retire | `dies` | derives the generated test-runner crate's name (`test_runner_<hash>`) from the batch directory suffix; the crate dies with #654. | - | libtest harness of the generated test crate |
| `native_test_outputs_do_not_alias_distinct_execution_groups` | retire | `dies` | names the native test binary of one generated execution group so two groups of the same file do not overwrite each other's receipt-verified output; the generated crate and its outputs die with #654. Report correctness across normal and xfail groups of one file is carried by `e2e_markers_parametrize_timeout_and_collection_errors_share_projects` (re-point). | - | libtest harness of the generated test crate |
| `batch_suffix_is_path_independent_but_content_sensitive` | retire | `dies` | hashes a batch's sources into the generated test-runner crate directory suffix (`batch_<hash>`); a data-structure invariant of the generated crate directory, which dies with #654. | - | libtest harness of the generated test crate |
| `inject_file_test_harness_emits_tests_module` | retire | `dies` | asserts the injected `mod __incan_file_tests` text with one `fn incan_harness_<i>_<name>` per case and the `set_current_dir` guard; generated Rust text, dies with #654. The cwd guard is carried by `e2e_test_runner_preserves_fixture_cwd_for_file_and_batch_runs` (re-point). | generated_text | libtest harness of the generated test crate |
| `inject_file_test_harness_wraps_async_tests_and_fixtures` | retire | `dies` | asserts the injected harness text for an async test with a yield fixture (`__incan_async_block_on`, `__incan_run_teardown`); generated Rust text, dies with #654. Async tests and fixture teardown through `incan test` are the re-point `integration_tests.rs::e2e_fixture_lifetime_success_scenarios_share_one_project` and `e2e_fixture_teardown_failure_scenarios_share_one_project`. | - | libtest harness of the generated test crate |

Per-test overrides in `loaves/toolchain/incan-cli/tests/canonical_item_imports.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `an_import_does_not_reach_across_modules` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_codegraph_and_inspection_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `inspect_bindings_projects_checked_declaration_facts` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `diagnostic_facts_keep_related_spans_and_type_payloads_across_cli_and_codegraph` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_rust_reports_current_generated_rust_files` | retire | `dies` | `incan inspect rust --format json`: asserts the generated-project report (`rust_files` with a `crate_root`, `generated.project_path` under `target/lib`) and `#[doc = ...]` text in the generated crate root; `inspect rust` output dies with #654. | - | `incan inspect rust` over an executable and a library project; the report and the generated Rust it points at die with #654. |
| `inspect_codegraph_exports_multifile_imports_and_public_symbols` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_codegraph_distinguishes_sibling_binding_stable_identities_issue1629` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_codegraph_exports_checked_registry_facts` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_codegraph_attaches_facade_paths_to_checked_registry_facts` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |
| `inspect_codegraph_strict_directory_rejects_semantic_diagnostics` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_decorator_and_partial_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `test_facade_reexport_preserves_declared_source_import_alias_target_issue57` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli/facade_reexport_alias_target` | - | codegen, build_run | twinned by the InQL #57 program run to completion: the facade's `col`, `count` and `count_expr` resolve through the modules that imported their same-named builders under an alias. The retired test only asserted that `--emit-rust` succeeded. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_interop_target_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `codegraph_projects_checked_c_bindings_and_explicit_unsafe_calls` | keep | - | - | - | `incan inspect codegraph`/`inspect bindings`/`check` only; representation inspection from checked facts, no generated Rust. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `module_qualified_stdlib_type_annotation_emits_and_runs_issue1437` | re-point | - | - | codegen, build_run | bakes and runs the program and also asserts `--emit-rust` text; the run stays, the generated-text assertion drops in slice 7. |
| `build_union_widening_converts_generated_wrappers_issue741` | re-point | - | - | codegen, build_run | builds through `incan build` (the build must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; the build assertion stays, the generated-text assertion drops in slice 7. |
| `build_pub_helper_wraps_union_call_result_as_option_payload_issue745` | re-point | - | - | codegen, build_run | builds through `incan build` (the build must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; the build assertion stays, the generated-text assertion drops in slice 7. |
| `build_pub_method_accepts_dependency_owned_union_alias_payload_issue755` | re-point | - | - | codegen, build_run | builds through `incan build` (the build must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; the build assertion stays, the generated-text assertion drops in slice 7. |
| `build_locked_map_err_string_literal_closure_issue880` | re-point | - | - | codegen, build_run | builds through `incan build` (the build must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; the build assertion stays, the generated-text assertion drops in slice 7. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_rust_interop_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `rust_std_io_trait_interop_borrows_receivers_and_propagates_results_issues878_888` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `rust_trait_object_method_arguments_borrow_by_metadata_issue832` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `rust_concrete_reference_arguments_borrow_by_metadata_issue861` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `rust_std_result_and_contextual_f32_interop_compile_together_issues801_802` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `cold_library_build_preserves_rust_string_compound_assignment_issue896` | retire | - | - | generated_text, build_run | open (twin-h-cli): the library build is the only non-text assertion and the `str_concat` selection under cold Rust metadata is generated text. The surviving behavior, `+=` and `+` with a `rust::`-returned `String` operand, needs a `[rust-dependencies]` path into the checkout (`development_support_crate_dir("incan_std_core")`) that a fixture cannot carry. A `rust::std::string::String.from(...)` stand-in does not type as `str` (`from` is generic), so no in-fixture substitute exists. Question (with Q1): does Rust-crate interop behavior get a twin in this corpus (a fixture would need a Rust crate with a `String`-returning function and the inspection bake of Q1), or does it move with #1337's spec? If the latter, this row is `dies` with the cold-metadata half named. |
| `rust_method_into_bound_keeps_string_argument_inferable_issue804` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `build_metadata_free_into_bound_tokenizer_encode_issue804` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `comprehension_over_rust_iterator_consumes_it_by_value_issue1490` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_surface_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `fixture_handoff_copy_rejects_symlinks` | unaffected | - | - | - | test-support helper (fixture hand-off copy); no compiler invocation. |
| `check_json_reports_parser_diagnostics` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_json_reports_typechecker_diagnostics` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_json_reports_tooling_diagnostics` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_json_reports_import_diagnostics` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `explain_reports_known_and_unknown_diagnostic_codes` | keep | - | - | - | `incan explain` only; diagnostics catalog surface, no generated Rust. |
| `requires_incan_allows_compatible_project_commands` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `init_creates_project_scaffold_with_expected_content` | unaffected | - | - | - | CLI surface (argument parsing, scaffolding, lifecycle, doctor); no compiler semantics. |
| `tools_doctor_reports_text_and_json` | unaffected | - | - | - | CLI surface (argument parsing, scaffolding, lifecycle, doctor); no compiler semantics. |
| `tools_metadata_api_reports_docstring_drift` | keep | - | - | - | `incan tools metadata api` only; API metadata from checked facts, no generated Rust. |
| `tools_metadata_api_reports_public_import_aliases` | keep | - | - | - | `incan tools metadata api` only; API metadata from checked facts, no generated Rust. |
| `check_json_reports_parser_and_typechecker_warnings_without_failing` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_json_reports_warnings_alongside_errors_when_typechecking_fails` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_rejects_statement_tuple_unpack_of_non_tuple_without_leaking_generated_rust` | keep | - | - | - | `incan check` only; checker surface (diagnostics through the CLI), no generated Rust. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/cli_workspace_and_lock_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `workspace_inspect_reports_deterministic_scope_and_stale_member_locks` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `workspace_lock_scopes_command_feature_flags_to_the_invoking_member_issue1414` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `workspace_root_library_without_a_script_publishes_the_canonical_lock_issue997` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `rooted_workspace_cold_lock_and_selected_member_preserve_identity_issues908_909_931` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `workspace_lock_concurrent_publishers_leave_one_parseable_root_lock` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `workspace_fmt_fans_out_in_member_order_without_changing_single_project_semantics` | keep | - | - | - | `incan workspace fmt` only; formatter surface fanned out per member, no generated Rust. |
| `workspace_check_fans_out_with_one_member_scoped_json_report` | keep | - | - | - | `incan workspace check` only; checker surface fanned out per member, no generated Rust. |
| `a_plain_build_of_a_toolchain_loaf_selects_its_rust_binaries_and_names_the_interim_receipt_inputs_issue1698` | unaffected | - | - | build_run | `incan build` dispatch for a toolchain Loaf (`[[rust.bin]]`, #1698): asserts the stored-plan route is taken and its refusals; no Incan compilation, not the Rust backend. |

Per-test overrides in `loaves/toolchain/incan-cli/tests/integration_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `build_explicit_mutable_rust_generic_reaches_codegen_through_normal_cli_path` | retire | - | - | codegen, generated_text, build_run | open (harness-deps): the build succeeds and the assertions are the projected ABI in `main.rs` (`ProviderHandle<(&mut i64, &mut i64)>`), generated text. A behavior twin exists and passes standalone and under a root admitted to the explicit bake: the same program run to completion, printing its marker line (this program's `rust::std::vec::Vec` import launched no Cargo itself, but the bake that records its authority is the same explicit bake). Waits for a Cargo-free bake under the suite: a program with a `rust::` import needs source-current project inspection authority, which only an explicit `incan oven bake --project .` of its project records, and that bake runs Rust inspection through Cargo (`cargo metadata` of the inspection workspace and the toolchain's `std` sources, `rustc --print` probes, `cargo check`; measured 2026-09-20, ten launches per fixture), which under the suite needs the root registered with `explicit_bake_cargo` and is ruled out. The candidate fixture is in the harness-deps slice folder. |
| `decorated_method_explicit_mutable_rust_generic_keeps_static_and_wrapper_abi` | retire | - | - | codegen, generated_text, build_run | open (harness-deps): the build succeeds and the assertions are the shared ABI spelling in `main.rs`, generated text. A behavior twin exists and passes standalone and under a root admitted to the explicit bake: the same program run to completion (`7`, then its marker line). Waits for a Cargo-free bake under the suite: a program with a `rust::` import needs source-current project inspection authority, which only an explicit `incan oven bake --project .` of its project records, and that bake runs Rust inspection through Cargo (`cargo metadata` of the inspection workspace and the toolchain's `std` sources, `rustc --print` probes, `cargo check`; measured 2026-09-20, ten launches per fixture), which under the suite needs the root registered with `explicit_bake_cargo` and is ruled out. The candidate fixture is in the harness-deps slice folder. |
| `test_cli_fmt_preserves_block_decl_docstrings_and_export_doc_surface` | keep | - | - | parser | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_accepts_assert_identity_bool_literals` | keep | - | - | - | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_wraps_long_parenthesized_logical_expression_chain` | keep | - | - | - | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_preserves_fstring_escaped_newline_roundtrip` | keep | - | - | - | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_applies_rfc053_vertical_spacing_contract` | keep | - | - | parser | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_keeps_two_blank_lines_between_static_and_function` | keep | - | - | parser | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_fmt_keeps_trailing_comment_after_multiline_function` | keep | - | - | parser | `incan fmt` only; formatter surface, no generated Rust. |
| `test_cli_check_accepts_trailing_comma_in_multiline_function_params` | keep | - | - | - | `incan --check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `test_compound_assign_float_with_int_rhs` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_valid_fixtures` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_invalid_fixtures` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_help_is_banner_free` | unaffected | - | - | - | CLI argument surface (`--help`, `--version`, an unknown flag); never reaches the compiler. |
| `test_version_is_single_line_and_banner_free` | unaffected | - | - | - | CLI argument surface (`--help`, `--version`, an unknown flag); never reaches the compiler. |
| `test_parse_error_is_banner_free` | unaffected | - | - | - | CLI argument surface (`--help`, `--version`, an unknown flag); never reaches the compiler. |
| `test_imported_static_initializer_does_not_deadlock_issue680` | re-point | - | - | generated_text, build_run | Builds and runs (prints `ok` within 30 s with a static reached directly and through a facade, #680) and also reads the generated Rust; the run assertion stays, the generated-text assertion drops in slice 7. Re-point, not retire: the run half has no other coverage. |
| `lexer_token_surface_cases` | keep | - | - | - | lexer only |
| `test_python_like_numeric_ops_compile` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_hello_world_codegen` | retire | `dies` | IrCodegen text of `examples/hello.incn` (`fn main()`, `println!`, the message). The test returns early because that path no longer exists (the example is `examples/simple/hello.incn`), so it proves nothing today; the example itself is a case of the re-point `examples` fixture root, run by `make examples`. | codegen, generated_text, checker, parser | IrCodegen over examples/hello.incn, a path that no longer exists (the test skips itself); the example is run by the `examples` fixture root. |
| `test_method_alias_codegen_rewrites_to_target_method` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli/method_alias_calls_target.incn` | - | codegen, parser | twinned by a program whose model declares `mean = avg` and prints both calls; the retired assertion (the alias lowering to the target's canonical projection, no wrapper) was generated text. |
| `test_check_web_route_uses_proc_macro_passthrough` | keep | - | - | - | `incan --check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `explicit_legacy_sdk_inventory_blocks_lock_preheat` | unaffected | - | - | - | `incan lock` / `workspace inspect` only; lock orchestration over Oven, not the Rust backend. |
| `test_check_cyclic_explicit_call_site_generics_cross_module_succeeds` | keep | - | - | - | `incan --check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `test_rfc041_rusttype_interop_typechecks_end_to_end` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_rfc041_rusttype_with_methods_typechecks` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_rfc041_rust_coercion_codegen_smoke` | retire | - | - | codegen, checker, parser | open (harness-deps): IrCodegen text only (`Duration::from_secs_f32`). A behavior twin exists and passes standalone and under a root admitted to the explicit bake: a program passing a float literal to `rust::std::time::Duration.from_secs_f32` and printing `as_millis()` (`1500`). Waits for a Cargo-free bake under the suite: a program with a `rust::` import needs source-current project inspection authority, which only an explicit `incan oven bake --project .` of its project records, and that bake runs Rust inspection through Cargo (`cargo metadata` of the inspection workspace and the toolchain's `std` sources, `rustc --print` probes, `cargo check`; measured 2026-09-20, ten launches per fixture), which under the suite needs the root registered with `explicit_bake_cargo` and is ruled out. The candidate fixture is in the harness-deps slice folder. |
| `test_rfc041_structural_coercion_codegen_smoke` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli/annotated_collection_literals.incn` | - | codegen, generated_text, checker, parser | twinned by a program that binds the annotated `Option[int]`, `List[str]` and `Dict[str, float]` literals and reads them back; the retired assertion was the Rust expressions they lowered to. |
| `test_rfc009_numeric_resize_and_decimal_codegen_smoke` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli/numeric_resize_policies_and_decimal.incn` | - | codegen, generated_text, checker, parser | twinned by a program that prints each resize policy's result and the decimal literal; the retired assertion was the casts and stdlib helper calls they lowered to. |
| `build_lib_imported_static_decorator_receiver_materializes_string_arg_issue671` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli/imported_static_decorator_receiver` | - | generated_text, build_run | twinned by the #671 program run to completion: the decorator factory called on the imported static receiver with a str argument leaves the decorated function callable. The retired test built `--lib` and asserted how the str argument materialized in generated text. |
| `test_model_with_decorator` | keep | - | - | parser | lex/parse/typecheck only |
| `test_class_with_traits` | keep | - | - | parser | lex/parse/typecheck only |
| `test_trait_supertraits_compile_source` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_trait_constructor_rejected_in_full_pipeline` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_method_with_mut_self` | keep | - | - | parser | lex/parse/typecheck only |
| `test_generic_instance_method_full_pipeline` | keep | - | - | checker, parser | lex/parse/typecheck only |
| `test_match_with_case` | keep | - | - | parser | lex/parse/typecheck only |
| `test_list_comprehension` | keep | - | - | parser | lex/parse/typecheck only |
| `test_generic_type` | keep | - | - | parser | lex/parse/typecheck only |
| `test_yield_expression` | keep | - | - | parser | lex/parse/typecheck only |
| `test_fixture_decorator` | keep | - | - | parser | lex/parse/typecheck only |
| `test_rust_crate_import` | keep | - | - | parser | lex/parse/typecheck only |
| `test_rust_from_import` | keep | - | - | parser | lex/parse/typecheck only |

Per-test overrides in `loaves/toolchain/incan-cli/tests/rfc031_pub_import_integration_tests.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `compiled_provider_preserves_shared_rust_interop_contracts_issues834_835_961` | retire | - | - | codegen, generated_text, build_run, checker | open (twin-h-cli): the provider bake and the consumer build succeed over a local Rust crate (`receiver_factory`, `[rust-dependencies]`); the assertions are the manifest `rust_abi` type-parameter record and generated provider text (method turbofish, `&mut [f32]` callback, receiver specialization). A twin would be a project fixture carrying a Rust crate plus the provider bake of Q2 and the `rust::` inspection bake of Q1. Question: does Rust-crate interop behavior get a twin in this corpus (fixture carries Rust source), or does it move with #1337's spec? |
| `oven_build_projects_structural_mutable_reference_generics_for_an_unrelated_provider` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `external_pub_consumer_uses_flattened_nested_union_variant_index_issue1198` | re-point | - | - | codegen, build_run | builds or runs through `incan` (the build or run must succeed) and then asserts Rust text read from `target/incan/<project>/src/main.rs`; that assertion drops in slice 7, the build or run assertion stays. |
| `fmt_dependency_collection_does_not_prepare_persistent_library_artifact` | keep | - | - | - | `incan fmt` only; formatter surface, no generated Rust. |
| `check_reports_unknown_pub_library` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_reports_missing_pub_export` | keep | - | - | checker | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_reports_pub_manifest_load_failure` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_passes_for_pub_imported_manifest_type` | keep | - | - | checker | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_reports_missing_pub_library_artifacts` | keep | - | - | checker | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `check_reports_pub_library_artifact_mismatch` | keep | - | - | checker | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_resolves_a_manifest_declared_package_relative_c_header` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_copies_a_sqlite_error_view_with_an_explicit_bound` | retire | `dies` | `--emit-rust` text half (the `__incan_checked_c_copy_utf8` bounded-copy helper); the `incan check` acceptance in the same test is checker-only and survives. The binding's `header` is a host-specific absolute path (`sqlite_header_path()`) and the check needs the host verifier, so a fixture cannot carry the program. | codegen | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter, as its sibling `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` does. |
| `consumer_check_models_a_sqlite_caller_owned_byte_buffer_through_a_declared_shim` | retire | `dies` | `--emit-rust` text half (`__incan_checked_c_finish_span`, `*mut u8`, `-> usize`); the `incan check` acceptance in the same test is checker-only and survives. The declared shim symbol `incan_sqlite_random_bytes` has no implementation and the header is a host-specific path, so the program cannot run and no behavior fixture is possible. | codegen | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter, as its sibling `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` does. |
| `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` | retire | `dies` | `--emit-rust` text half (`*const f32`, the `f32` output carrier, the `#[link(... kind = "framework")]` attribute); the `incan check` acceptance in the same test is checker-only and survives. The declared shim symbol `incan_accelerate_sum_f32` has no implementation, so the program cannot run and no behavior fixture is possible. | codegen, generated_text | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter. |
| `consumer_check_supports_checked_byte_spans_and_caller_owned_buffers` | retire | `dies` | `--emit-rust` text half (`__incan_checked_c_finish_span`, `.as_ptr()`/`.as_mut_ptr()`); the `incan check` acceptance in the same test is checker-only and survives. The declared shim symbol `fixture_copy_prefix` has no implementation, so the program's `main` cannot run and no behavior fixture is possible. | codegen | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter, as its sibling `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` does. |
| `consumer_check_rejects_unpaired_or_immutable_checked_byte_buffer_arguments` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_rejects_checked_byte_span_escape_and_reuse` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_preserves_checked_c_f32_scalar_identity` | retire | `dies` | `--emit-rust` text half (an `f32` carrier in the emitted binding); the `incan check` acceptance in the same test is checker-only and survives. The declared shim symbol `fixture_absolute_f32` has no implementation, so the program cannot run and no behavior fixture is possible. | codegen | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter, as its sibling `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` does. |
| `consumer_check_preserves_exact_checked_c_scalar_carriers` | retire | `dies` | `--emit-rust` text half (one exact Rust carrier per `c.*` scalar); the `incan check` acceptance in the same test is checker-only and survives. The declared shim symbols (`fixture_i8` ... `fixture_size`) have no implementation, so the program cannot run and no behavior fixture is possible. | codegen | `incan check` (checker-only) followed by `--emit-rust`, whose generated Rust text is the assertion; nothing is built or run, so the test retires with the emitter, as its sibling `consumer_check_models_an_accelerate_f32_span_through_a_declared_framework_shim` does. |
| `consumer_check_supports_checked_f32_spans_for_paired_numeric_pointers` | keep | - | - | - | `incan check` and `inspect bindings` only; representation inspection from checked facts, no generated Rust. |
| `consumer_check_rejects_unpaired_checked_f32_span_arguments` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `sqlite_checked_c_tooling_projects_the_shared_descriptor` | keep | - | - | - | `incan check` and `inspect bindings` only; representation inspection from checked facts, no generated Rust. |
| `consumer_check_reports_checked_c_signature_mismatch_at_the_binding` | keep | - | - | - | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugars_external_vocab_block_via_wasm` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_passes_request_payload_into_external_vocab_desugarer` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_accepts_expression_desugar_output_in_statement_position` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_reports_external_vocab_desugarer_failure` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_build_plans_source_backed_vocab_helper_calls_with_defaults_and_unions_issue729` | retire | - | - | generated_text, build_run, checker | open (twin-h-cli): the consumer build succeeds and the assertions are generated text (`querykit::count(` filled from the helper default, `COUNT_SENTINEL` through the provider path, no re-owned `__IncanUnion`). The desugared `where true:` calls come from a vocab companion crate (`write_vocab_companion_crate_with_source`, a wasm desugarer) a fixture cannot carry, on top of the provider bake of Q2. The same helpers' default, union and string behavior through ordinary calls is the pub sibling's candidate twin; the re-point sibling `consumer_build_plans_vocab_helper_calls_like_ordinary_calls_issue729` proves desugared helper calls build. Question: is that enough for `dies`, or does the vocab lane want a fixture with a companion? |
| `consumer_build_plans_source_backed_pub_helper_calls_with_defaults_and_unions_issue729` | retire | `loaves/compiler/incan_test_support/fixtures/behavior/cli_dependencies/dependency_helper_defaults_and_unions` | - | generated_text, build_run, checker | the consumer build succeeded and the assertions were generated text (`querykit::count(` filled from the helper default, `COUNT_SENTINEL` and `DEFAULT_LABEL` through the provider module path, no re-owned `__IncanUnion`, `.to_string()` arguments); the twin is a project fixture with `deps/querykit` whose `main` prints `adjusted=5`, `order_count=__querykit_count_no_argument__` and `orders=7` (union parameter, omitted-argument default, dependency-owned const default), with the provider baked by the runner (harness-deps). Narrowing the dependency's `Union[IntLiteralExpr, StringLiteralExpr]` in the consumer is accepted by the checker but the legacy route does not compile it, so the twin narrows inside the provider. |
| `consumer_check_passes_scoped_query_surface_artifacts_to_desugarer` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_passes_expr_list_item_metadata_to_desugarer_issue724` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugars_colon_vocab_expression_in_assignment_issue727` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugars_colon_vocab_expression_in_return_issue727` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugars_colon_vocab_expression_preserves_inline_clauses_issue727` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugars_braced_vocab_expression_with_compound_clauses_issue727` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugared_public_field_callee_call_typechecks_as_method_issue727` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `consumer_check_desugared_generic_method_call_uses_expected_return_type_issue735` | keep | - | - | checker, parser | `incan check` / `--check` only; checker surface (diagnostics through the CLI), no generated Rust. |
| `fmt_activates_clean_source_dependency_vocab_before_parsing_issue756` | keep | - | - | - | `incan fmt` only; formatter surface, no generated Rust. |

### `loaves/toolchain/incan-lsp` (93 tests in 6 files: keep 92, retire 1)

| File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
|---|---:|---:|---:|---|---:|---:|---|---|---|---|
| `loaves/toolchain/incan-lsp/src/backend.rs` | 56 | 9413 | 2077 | keep | - | - | required | #1561 | checker 21, parser 33, lsp 3 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/call_site_type_args.rs` | 4 | 747 | 48 | keep | - | - | - | #1561 | parser 1 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/diagnostics.rs` | 7 | 395 | 168 | keep | - | - | - | #1561 | checker 3 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/main.rs` | 3 | 98 | 30 | keep | - | - | - | #1561 | - | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/src/semantic_tokens.rs` | 14 | 1394 | 284 | keep | - | - | - | #1561 | parser 13 | language server over the checker; no codegen. Reviewed at crate level. |
| `loaves/toolchain/incan-lsp/tests/rfc081_embedded_conformance.rs` | 9 | 730 | 730 | keep (keep 8, retire 1) | 0/1 | 0 | - | #1561 | codegen 1, checker 2, parser 1, formatter 3, lsp 1 | RFC 081 embedded-fragment conformance through desugar, typecheck and formatter; one test asserts the emitter refuses. |

Per-test overrides in `loaves/toolchain/incan-lsp/tests/rfc081_embedded_conformance.rs`:

| Test | Disposition | Twin | Dies | Lanes | Notes |
|---|---|---|---|---|---|
| `every_submode_survives_desugar_typecheck_and_lowering_then_refuses_emission` | retire | - | - | codegen, checker | asserts an emitter refusal; the refusal moves to the replacement route's source profile |

??? note "Unaffected crates (1141 tests in 89 files)"

    Every test in these crates is `unaffected`: the cutover does not touch them. They are listed so the summary reconciles to the whole tree.

    #### `loaves/compiler/incan_oven_facet` (6 tests in 2 files: unaffected 6)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/compiler/incan_oven_facet/src/lib.rs` | 4 | 465 | 235 | unaffected | - | - | - | #1561 | - | Oven facet of the compiler; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_oven_facet/tests/oven_pr_regressions.rs` | 2 | 210 | 210 | unaffected | - | - | - | #1561 | - | Oven facet of the compiler; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/compiler/incan_provider` (97 tests in 9 files: unaffected 97)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/compiler/incan_provider/src/compiled_sdk.rs` | 1 | 74 | 16 | unaffected | - | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/dependency_resolver.rs` | 21 | 1161 | 533 | unaffected | - | - | - | #1561 | checker 20 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/inventory.rs` | 7 | 1036 | 590 | unaffected | - | - | - | #1561 | checker 3 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/lock_semantics.rs` | 14 | 2063 | 1320 | unaffected | - | - | - | #1561 | checker 12 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/requirements.rs` | 11 | 1002 | 275 | unaffected | - | - | - | #1561 | checker 8 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/sdk_build.rs` | 8 | 869 | 253 | unaffected | - | - | - | #1561 | run 1, checker 1 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/sdk_store.rs` | 11 | 1178 | 511 | unaffected | - | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/src/vocab_extraction.rs` | 14 | 1713 | 296 | unaffected | - | - | - | #1561 | run 3 | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/incan_provider/tests/stdlib_effect_digest.rs` | 10 | 313 | 313 | unaffected | - | - | - | #1561 | - | SDK provider store, lock semantics, vocab extraction; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/compiler/rust_inspect` (103 tests in 7 files: unaffected 103)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/compiler/rust_inspect/src/cache_tests.rs` | 41 | 2610 | 2610 | unaffected | - | - | required | #1561 | replacement 1 | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/digest/tests.rs` | 17 | 634 | 634 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/extractor.rs` | 20 | 3243 | 1279 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/lib.rs` | 2 | 206 | 149 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/loader.rs` | 15 | 2105 | 630 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/mir_digest.rs` | 5 | 455 | 235 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |
    | `loaves/compiler/rust_inspect/src/receiver_contract.rs` | 3 | 101 | 43 | unaffected | - | - | - | #1561 | - | Rust metadata extraction that feeds the checker; no emit/driver dependency. Reviewed at crate level. |

    #### `loaves/kernel/incan_codegraph` (5 tests in 1 file: unaffected 5)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/kernel/incan_codegraph/src/lib.rs` | 5 | 1312 | 174 | unaffected | - | - | - | #1561 | - | codegraph record format. Reviewed at crate level. |

    #### `loaves/kernel/incan_vocab` (6 tests in 1 file: unaffected 6)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/kernel/incan_vocab/src/lib.rs` | 6 | 514 | 205 | unaffected | - | - | - | #1561 | checker 6 | vocab registration contract crate. Reviewed at crate level. |

    #### `loaves/oven/oven_cargo_compat` (143 tests in 10 files: unaffected 143)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/oven/oven_cargo_compat/src/cargo_process.rs` | 3 | 209 | 72 | unaffected | - | - | - | #1561 | run 1 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/compiler_suite_foundation.rs` | 7 | 1542 | 978 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/harvest.rs` | 13 | 1671 | 807 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/lib.rs` | 68 | 9360 | 4749 | unaffected | - | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/loaf_bake.rs` | 10 | 1592 | 750 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/loaf_bake/vocab_support.rs` | 3 | 1023 | 290 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/registry_sources.rs` | 2 | 699 | 129 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/rustc_trace.rs` | 5 | 494 | 103 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/selected_graph_projection.rs` | 21 | 3696 | 1385 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_cargo_compat/src/selected_unit_capture.rs` | 11 | 2361 | 783 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_interop` (20 tests in 1 file: unaffected 20)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/oven/oven_interop/src/lib.rs` | 20 | 4394 | 2720 | unaffected | - | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_model` (162 tests in 9 files: unaffected 162)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/oven/oven_model/src/loaf_registry.rs` | 5 | 685 | 245 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/lock.rs` | 24 | 1893 | 994 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/manifest.rs` | 59 | 3874 | 1234 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/oven_interop.rs` | 8 | 2098 | 603 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/env.rs` | 18 | 860 | 476 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/toolchain.rs` | 4 | 295 | 58 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/project_lifecycle/version.rs` | 10 | 418 | 104 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/toolchain_layout.rs` | 15 | 875 | 353 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_model/src/workspace.rs` | 19 | 2065 | 604 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_rustc` (291 tests in 16 files: unaffected 291)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/oven/oven_rustc/src/loaf.rs` | 28 | 4707 | 1338 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/loaf_mirror.rs` | 9 | 758 | 429 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test.rs` | 34 | 2881 | 1419 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). A source module (the native test runner) with a `#[cfg(test)]` region, measured by that region. Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test/case_slice.rs` | 5 | 174 | 87 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/native_test/evidence.rs` | 4 | 258 | 74 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/plan/composition.rs` | 8 | 1496 | 686 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/plan/selection.rs` | 1 | 518 | 51 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc.rs` | 94 | 11409 | 6517 | unaffected | - | - | required | #1561 | run 3 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/compiled_unit.rs` | 7 | 844 | 476 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/inspection.rs` | 31 | 2617 | 1951 | unaffected | - | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_closure.rs` | 8 | 1087 | 509 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_executor.rs` | 9 | 1341 | 759 | unaffected | - | - | - | #1561 | run 4 | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/runtime_foundation.rs` | 26 | 2152 | 1473 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/selected_unit.rs` | 13 | 1793 | 717 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/substitution.rs` | 10 | 708 | 335 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_rustc/src/rustc/toolchain.rs` | 4 | 604 | 51 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/oven/oven_store` (94 tests in 6 files: unaffected 94)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/oven/oven_store/src/closure_proof.rs` | 1 | 107 | 27 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/lib.rs` | 13 | 2285 | 550 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/process.rs` | 10 | 769 | 769 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/progress.rs` | 6 | 343 | 99 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/store.rs` | 55 | 6391 | 2080 | unaffected | - | - | required | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |
    | `loaves/oven/oven_store/src/store_mirror.rs` | 9 | 635 | 373 | unaffected | - | - | - | #1561 | - | Oven ring; no compiler-crate dependency (scripts/check_oven_ring.py). Reviewed at crate level. |

    #### `loaves/stdlib/async` (27 tests in 6 files: unaffected 27)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/async/rust/src/channel.rs` | 8 | 540 | 115 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/race.rs` | 5 | 157 | 58 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/runtime.rs` | 2 | 104 | 33 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/sync.rs` | 4 | 663 | 125 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/task.rs` | 3 | 178 | 46 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/async/rust/src/time.rs` | 5 | 184 | 85 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/core` (88 tests in 9 files: unaffected 88)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/core/rust/src/collections.rs` | 28 | 605 | 242 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/conversions.rs` | 6 | 73 | 41 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/frozen.rs` | 1 | 445 | 16 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/iter.rs` | 11 | 365 | 96 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/num.rs` | 26 | 1008 | 290 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/strings.rs` | 5 | 405 | 42 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/src/version.rs` | 9 | 249 | 89 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/tests/errors.rs` | 1 | 32 | 32 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/core/rust/tests/string_len.rs` | 1 | 10 | 10 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/data` (8 tests in 2 files: unaffected 8)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/data/rust/src/collections.rs` | 1 | 144 | 17 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/data/rust/src/json.rs` | 7 | 708 | 108 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/derive` (8 tests in 2 files: unaffected 8)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/derive/incan_web_macros/src/lib.rs` | 6 | 451 | 133 | unaffected | - | - | - | #1561 | parser 6 | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |
    | `loaves/stdlib/derive/incan_web_macros/tests/route_runtime.rs` | 2 | 197 | 197 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/interop` (1 test in 1 file: unaffected 1)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/interop/vocab_companion/src/lib.rs` | 1 | 188 | 17 | unaffected | - | - | - | #1561 | checker 1 | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/stdlib/testing` (3 tests in 1 file: unaffected 3)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/stdlib/testing/rust/src/lib.rs` | 3 | 275 | 80 | unaffected | - | - | - | #1561 | - | stdlib runtime crate; no compiler-crate dependency. Reviewed at crate level. |

    #### `loaves/third_party/ra_ap_proc_macro_api` (2 tests in 1 file: unaffected 2)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/third_party/ra_ap_proc_macro_api/src/legacy_protocol/msg.rs` | 2 | 430 | 244 | unaffected | - | - | - | #1561 | - | vendored crate. Reviewed at crate level. |

    #### `loaves/toolchain/oven-cli` (77 tests in 5 files: unaffected 77)

    | File | Tests | Lines | Test lines | Disposition | Twins | Dies | Split | Owner | Signals | Notes |
    |---|---:|---:|---:|---|---:|---:|---|---|---|---|
    | `loaves/toolchain/oven-cli/src/commands/oven.rs` | 58 | 6841 | 3320 | unaffected | - | - | required | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/case_partition.rs` | 6 | 379 | 171 | unaffected | - | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/harvest.rs` | 3 | 437 | 69 | unaffected | - | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/oven/suite_environment.rs` | 2 | 899 | 32 | unaffected | - | - | - | #1561 | - | Oven CLI; bakes and harvests, no compiler semantics. Reviewed at crate level. |
    | `loaves/toolchain/oven-cli/src/commands/tools.rs` | 8 | 1863 | 414 | unaffected | - | - | - | #1561 | run 1 | Oven tools command; the build_run hit is a Cargo config hint, not an Incan build. |
