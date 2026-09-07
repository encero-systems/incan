# Oven and toolchain ownership map

This is the checked-in ownership map required by [#871](https://github.com/encero-systems/incan/issues/871): a component-by-component inventory of the Oven/toolchain control plane, classifying every supported responsibility as **Incan semantic control**, **Rust host kernel**, or **explicitly excluded** (compiler-internal, not part of the Incan-vs-Oven axis at all).

Audited against `release/v0.5` (post v0.5 RC3, PR #1076 merged).

## The classification rule

This map does not invent its own criteria. It applies the rule this repo already decided, quoted verbatim:

- **RFC 118**: "Keep the Incan CLI native Incan while preserving Oven's Rust operational core/API as the explicit cache, store, lease, lifetime, concurrency, and crash-safety exception." Layers affected: "Oven API and operational core — must own planning, resolver/provider choice, policy, target selection, scheduling, artifacts, receipts, caches, stores, leases, and crash-safe publication through language-neutral owned snapshots and opaque handles." Also: "Compiler service API — must expose bounded compilation and semantic services to Oven without making Oven invoke or import the Incan CLI."
- **RFC 119**: "The Rust core remains narrow but real: ...Oven's operational core/API owns planning, resolution, policy, stores, leases, provider execution, and crash-safe publication in Rust. Its public API is language-neutral and does not leak Rust borrows." And: "Incan-first authoring remains policy: a Rust facet makes Rust projects and explicitly justified mixed work supportable. It does not authorize new Rust in Incan products without a demonstrated limitation and tracked removal path."
- **RFC 117**: generators/providers are "ordinary typed Incan functions" — provider *logic* is Incan-owned even where the engine that runs providers is legitimately Rust.

A component is **Rust host kernel** only if it does planning, resolver/provider choice, policy evaluation, target selection, scheduling, artifact/receipt handling, caching, store/lease management, or crash-safe publication. Everything else currently in Rust is either an **Incan semantic control** migration candidate, or a **compiler-service exception** (the frontend/LSP/codegraph — compiler-internal territory this repo already treats as Rust, a different axis than Oven's operational core).

## Summary

- **Confirmed Rust host kernel**: the large majority of the audited surface — receipts, crash-safe publication, leases, resolver/provider-choice, scheduling, caches, and direct-rustc/Cargo-boundary invocation all cleanly match the quoted rule.
- **Migration candidates (Incan semantic control)**: a materially-sized, concrete set — see table below. Most are pure data/schema/validation logic with zero filesystem, process, or crash-safety dependency.
- **Genuinely uncertain, needs an explicit decision**: a smaller set where the code doesn't cleanly match either bucket, or where classification depends on a decision this map surfaces but doesn't make unilaterally.
- **Real architectural findings** (not just classification, but things worth a decision): three are named below — the Cargo.lock/manifest reimplementation tension, the LSP→CLI boundary crossing, and the CLI's shim-vs-core blur in `build.rs`/`test_runner/execution.rs`.
- **No existing implementation-tracking issue covers this migration work in fine grain.** #1034 ("migrate the full Oven semantic control plane to Incan authoring") is the umbrella; per its own stated scope it explicitly depends on this map ("Migration of every non-kernel Oven responsibility identified by the #870 ownership map"). This document is that map. Fine-grained delivery issues per component should be split out from #1034 once this lands, not invented ad hoc.
- **Test coverage was mechanically verified per file**, not guessed — see the dedicated section below. The one finding worth reading before anything else: the largest migration-candidate block (`library_manifest/{model,wire,type_refs,validation}.rs`, 3,500 lines) has zero in-file unit tests.

## Confirmed Rust host kernel

Test counts are in-file `#[cfg(test)] mod tests` line/function counts, mechanically grepped — not a coverage-quality judgment, just a presence signal. See "Test coverage" below for how to read these.

| Component | File(s) | Responsibility matched | Tests (lines/fns) |
|---|---|---|---|
| Oven receipts & build-unit identity | `src/oven.rs` | Artifacts, receipts, crash-safe publication (staged-file + atomic rename) | 431 / 11 |
| Oven process-tree containment | `src/oven/process.rs` | Scheduling/crash-safety around provider execution (process-group isolation/reaping) | 0 / 0 ⚠️ |
| Compiler-suite process capability | `src/oven/compiler_suite_env.rs` | Policy (capability admission table), artifacts (resolved closure) | 0 / 0 ⚠️ |
| Interop target requirements & deployment planning | `src/oven_interop.rs` | Policy (manifest validation), target selection/locking | 531 / 6 |
| Provider catalog, feature graph, SDK inventory | `src/provider/{features,plan,sdk}.rs` | Resolver/provider-choice, policy — the actual engine that runs providers, not provider logic itself | 1,346 / 36 |
| Generated Cargo artifact cache | `src/generated_cache.rs` | Caches, stores, leases (advisory-locked, size-bounded eviction) | 446 / 18 |
| Provider artifact content-addressing | `src/library_manifest/artifact.rs` | Artifacts (deterministic digest identity) | 822 / 13 |
| Dependency resolver | `src/dependency_resolver.rs` | Resolver/policy over the Cargo dependency graph | 549 / 23 |
| Lockfile crash-safe publication & semantic lock state | `src/lockfile.rs` | Crash-safe publication (stage → sync → atomic rename), lock identity/artifacts | 1,443 / 26 |
| Workspace dependency inheritance, scope resolution, build ordering | `src/workspace.rs` (back half) | Resolver/policy (`workspace = true` inheritance), target selection, scheduling (topological build order) | 566 / 18 (whole file, front/back half not separable) |
| Loaf envelope, generation commit, resolver/policy, toolchain resolution, build-unit identity, baker pipeline | `src/oven/loaf.rs` | Artifacts/receipts, crash-safe publication, resolver/provider-choice (tie-break policy), target selection | 1,444 / 27 |
| Native test process supervision & scheduling | `src/oven/native_test.rs` (bulk) | Scheduler, target selection, capability policy, receipt model (RFC 119 test-execution language, matched near-verbatim) | 699 / 19 (whole file, includes the small libtest-parsing migration candidate below) |
| Store data model, publication, leases, inspection/pruning | `src/oven/store.rs` | Policy (capacity limits), crash-safe publication, leases (advisory-locked), caches/stores (eviction) | 856 / 25 |
| Interop receipt/plan model, native bake, toolchain/SDK selection, shim compilation | `src/oven/interop.rs` (bulk) | Artifacts/receipts, resolver/provider-choice, provider execution | 2,707 / 20 |
| Rustc artifact manifest/plan, registry-leaf resolution, project inspection authority, direct rustc/rustdoc invocation | `src/oven/rustc.rs` (bulk) | Planning, artifacts, resolver/provider-choice, stores/leases, direct rustc invocation (named explicitly in RFC 119 point 10) | 2,990 / 47 (whole file, includes the flagged diagnostic-formatting subset below) |
| Direct-Rustc & compiler-suite plan orchestration, Cargo unit-graph translation, subprocess wrapper, registry/SDK staging, publisher lease/lock, capacity accounting | `src/oven/legacy_cargo.rs` (bulk) | Planning, artifacts, receipts, leases, crash-safe publication, scheduling — Cargo treated as authoritative per RFC 119's "explicit compatibility operation" | 3,966 / 58 (whole file, includes the flagged Cargo.lock-reimplementation subset below) |
| Explicit Oven Alpha CLI surface | `src/cli/commands/oven.rs` | Textbook match for "Oven CLI... implementation language non-normative," covered by the cache/store/lease/crash-safety exception | 2,465 / 45 |
| Oven-shaped commands: `lock`, `init`/`new`, `env`/`version`, `workspace inspect`, vocab companion crate compilation | `src/cli/commands/{lock,init,lifecycle,workspace,vocab_extraction}.rs` | Dependency/lock/registry lifecycle, manifest/workspace selection, environments/project mutation — all named explicitly under "oven" in RFC 118's own command-ownership table | 488/14, 315/16, 103/4, 44/1, 294/14 |

## Migration candidates (Incan semantic control)

| Component | File(s) | Why it doesn't match the Rust-kernel rule | Lines | Tests (lines/fns) |
|---|---|---|---|---|
| Env overlay resolution | `src/project_lifecycle/env.rs` | Pure in-memory config merge; own doc comment already excludes CLI parsing, manifest I/O, filesystem updates | 859 | 475 / 18 |
| Project version bump policy | `src/project_lifecycle/version.rs` | Pure SemVer string logic, no I/O | 417 | 103 / 10 |
| Toolchain constraint compatibility | `src/project_lifecycle/toolchain.rs` | Pure semver comparison, no I/O | 299 | 57 / 4 |
| Library manifest model, wire format, type refs, validation | `src/library_manifest/{model,wire,type_refs,validation}.rs` | Schema/serde/structural validation only; carries artifact digests as opaque strings produced elsewhere (in `artifact.rs`, which stays Rust) — the identity computation and the schema are already cleanly separated in the code | 3,500 | **0 / 0 ⚠️ — the single largest coverage gap found in this audit** |
| Manifest (`incan.toml`) parsing & shape validation | `src/manifest.rs` | TOML deserialization + `deny_unknown_fields` + structural checks; sits upstream of the resolver rather than doing resolution | 2,505 | 538 / 28 |
| Workspace member discovery (glob expansion, ancestor-directory walking) | `src/workspace.rs` (front half) | Structural data-model construction, same character as manifest parsing — flagged with lower confidence than the rest of this table, see Uncertain section for the counter-argument | ~450 of 2,003 | 566 / 18 (whole file, not separable) |
| Libtest transcript parsing (case counts, timings) | `src/oven/native_test.rs` (`parse_libtest_*`) | Pure string/JSON extraction from an already-captured transcript, zero I/O | ~130 | 699 / 19 (whole file, not separable) |
| Rustc diagnostic bounding/rendering | `src/oven/rustc.rs` (`OvenRustcDiagnosticReport` `Display`, `parse_rustc_diagnostics`) | Presentation/truncation policy over rustc's own diagnostic JSON, no Rust-specific dependency | ~580 | 2,990 / 47 (whole file, not separable) |
| Compiled SDK module inventory | `src/compiled_sdk.rs` | Pure derived-set query over a decision (`ProviderPlan`) made elsewhere; module doc says as much directly | 73 | 15 / 1 |
| `inspect bindings`, `check`/`explain`, `fmt`, debug flags, stdlib loader | `src/cli/commands/{binding_inspect,diagnostics,format,debug,stdlib_loader}.rs` | Direct-source/semantic work per RFC 118's own command-ownership table row: "Language services and semantic products → incan" | ~1,744 combined | `binding_inspect` 0/0 ⚠️, `diagnostics` 0/0 ⚠️, `format` 0/0 ⚠️, `debug` 21/1, `stdlib_loader` 0/0 ⚠️ |
| `inspect codegraph`, `tools metadata` (metadata/model portion) | `src/cli/commands/{codegraph,tools}.rs` | Same rule as above | ~2,991 + partial of 2,338 | `codegraph` 115/3 (thin for its size), `tools` 508/9 (shared with the doctor portion below, not separable) |
| Test discovery, module graph, reporting, types | `src/cli/test_runner/{discovery,module_graph,reporter,types}.rs` | Semantic source-level test discovery and result presentation, no Oven/project state | ~2,740 combined | `discovery` 828/33, `module_graph` 127/3, `reporter` 0/0 ⚠️, `types` 0/0 ⚠️ |

## Genuinely uncertain — needs an explicit decision, not a default

| Component | File(s) | The tension | Tests (lines/fns) |
|---|---|---|---|
| Toolchain self-location (stdlib path resolution) | `src/toolchain_layout.rs` | Doesn't match any item on the RFC 118/119 list at all. Plausibly justified by bootstrapping (must run before any Incan runtime is located, needs `current_exe`/filesystem primitives) — but that justification is not stated anywhere in the file, so per RFC 119's "demonstrated limitation and tracked removal path" clause it should be flagged rather than assumed acceptable. | 328 / 14 |
| Workspace member discovery | `src/workspace.rs` (front half) | Structurally similar to manifest parsing (migration candidate), but gates workspace-wide command correctness before any command observes a malformed topology — a plausible reason to keep it Rust-adjacent. Judgment call, not asserted either way. | 566 / 18 (whole file, not separable) |
| `incan build` / `incan run` / `incan inspect rust` | `src/cli/commands/build.rs` | The *command surface* is exactly the "project-scale convenience" RFC 118 requires to delegate shallowly to Oven — but ~90% of its 17,328 lines *is* Oven's plan-selection/build-unit/artifact/receipt logic itself, not a thin delegation call. This is a real misplacement, not an ownership-classification ambiguity: the logic belongs in Oven's operational core/API, and the CLI file should shrink to a thin caller. **Decided** in [#1094](https://github.com/encero-systems/incan/issues/1094): the logic moves into `src/oven/plan.rs`, alongside the low-level plan/receipt types it already composes. | 4,599 / 82 (26.5% of the file is tests — a real safety net exists for whatever refactor eventually splits this file) |
| Test harness build/execution | `src/cli/test_runner/execution.rs` | Same shim-vs-core blur as `build.rs` — it does call into Oven's plan/store, but also independently constructs build-unit inputs rather than making one clean delegation call. **Decided** alongside `build.rs` in [#1094](https://github.com/encero-systems/incan/issues/1094); tracked for implementation by #1095. | 513 / 23 |
| `incan cache`, `inspect interop-plan`, `inspect providers`/`inspect features` | `src/cli/commands/{cache,interop_plan,provider_inspect}.rs` | RFC 118's own command-ownership table places caches/providers/interop-plan facts under `oven inspect`, not `incan inspect` — these are Oven-operational facts currently surfaced under the wrong CLI surface. Not a Rust-vs-Incan question; a command-placement question. | `cache` 16/2, `interop_plan` 0/0 ⚠️, `provider_inspect` 0/0 ⚠️ |
| `commands/common.rs` | `src/cli/commands/common.rs` | A genuine grab-bag straddling both sides: module collection/typecheck orchestration (semantic, Incan CLI) sits alongside SDK-provider-store prep, dependency resolution, Cargo policy flags, and rust-inspect workspace prep (Oven). Needs decomposition along the boundary rather than one classification. | 2,835 / 77 |
| `incan tools doctor` | `src/cli/commands/tools.rs` (doctor portion) | Neither semantic nor Oven project/artifact work — installation/toolchain-layout diagnostics. Closest to RFC 118's "Distribution and documentation" layer, which sits outside the Incan-vs-Oven split entirely. | shared 508/9 with the metadata/model portion above, not separable |
| Cargo.lock/manifest reimplementation inside the legacy-Cargo boundary | `src/oven/legacy_cargo.rs` (`locked_local_package`, `prune_lock_to_package`, `locked_generated_project*`, workspace-inheritance merging — ~950 lines) | This block hand-parses and hand-writes `Cargo.lock`/`Cargo.toml` content and reimplements Cargo's own workspace-dependency-inheritance merging in Rust, before re-invoking Cargo to "normalize" the result. The validation half (confirming Cargo's output matches a pinned release graph) is legitimate policy per RFC 118. The construction half sits in tension with RFC 119's explicit statement that Oven must not "reinterpret Cargo results as an Oven-native lock." Needs a narrowing decision, not a default. | 3,966 / 58 (whole file, not separable) |
| LSP → CLI internals boundary crossing | `src/lsp/backend.rs` (`prepare_lsp_rust_inspect_workspace*`, `resolved_rust_inspect_dependencies`, `spawn_rust_inspect_prewarm`, ~400 lines) | The compiler-service layer directly imports and calls CLI-owned `dependency_resolver::resolve_dependencies`, `cli::commands::common::{CargoPolicy, cargo_command_flags, resolve_lock_context}`, and `resolve_generated_cargo_target` (which yields a `GeneratedCacheLease`) in-process. RFC 118 requires the inverse relationship: "Compiler service API — must expose bounded compilation and semantic services to Oven without making Oven invoke or import the Incan CLI." Here the compiler-service layer is reaching into CLI-owned resolver/policy/lease-acquisition code directly, rather than the Incan CLI going through Oven's API. Needs reconciling once the `commands/common.rs` split above happens — the boundary these calls should cross may change shape entirely. | 7,027 / 48 (whole file — large overall coverage, but the specific glue is not separable) |

## Compiler-service exception (out of scope for the Incan-vs-Oven axis)

Confirmed legitimately Rust, but not because of the Oven-core exception — this is the separate, already-accepted "the compiler itself is written in Rust" exception (AGENTS.md), covering semantic analysis rather than project/build orchestration.

| Component | File(s) | Tests (lines/fns) |
|---|---|---|
| LSP protocol implementation, diagnostics conversion, call-site generic-arg helpers | `src/lsp/{mod,diagnostics,call_site_type_args,backend}.rs` (excluding the flagged rust-inspect glue above) | `diagnostics` 78/4, `call_site_type_args` 47/4, `backend` 7,027/48 (whole file) |
| Codegraph record schema | `crates/incan_codegraph/src/lib.rs` — explicitly disclaims orchestration in its own README | 78 / 2 |
| Rust-inspect (rust-analyzer embedding for interop metadata) | `crates/rust_inspect/src/*.rs`, `src/rust_inspect/mod.rs` (a zero-logic re-export facade over the crate) — inherently Rust (wraps `ra_ap_hir`), and its `loader.rs` shows the *correct* trust direction: it defers to Oven's already-verified "sealed" source selection rather than re-deciding it | `lib.rs` 144/2, `loader.rs` 364/9, `extractor.rs` 362/9, `cache.rs` see note below, `cache_resolve.rs`/`generic_params.rs`/`cache_timing.rs`/`error.rs` 0/0 (small, mechanical, low individual risk) |
| Surface semantics registry | `src/semantics_registry.rs` — typechecker/IR-lowering plumbing, not on the Oven/CLI axis at all | 0 / 0 (24-line trivial glue, low individual risk) |

## Test coverage

Recorded per #871's own required field ("public APIs, required capabilities, receipts, invariants, migration status, **tests**, and responsible delivery issue for every component"). Numbers above are in-file `#[cfg(test)] mod tests` line counts and `#[test]`-annotated function counts, mechanically grepped against every file in this audit — a presence/scale signal, not a test-quality judgment. A ⚠️ marks zero in-file unit tests found.

**Cross-cutting integration coverage** exists alongside the in-file numbers above and may cover some of the flagged files even without in-file unit tests: `tests/oven_pr_regressions.rs`, `tests/toolchain_installer_tests.rs`, and five `tests/fixtures/oven_*` fixture directories. This audit did not trace which specific files each integration test actually exercises — that would be a separate, deeper pass. Treat the ⚠️ rows below as "no unit-level safety net found in the file itself, verify integration coverage before touching," not as "definitely untested."

One correction against a false positive: `crates/rust_inspect/src/cache.rs` (4,792 lines) greps as having no in-file test module, but its real test coverage lives in a separate `cache_tests.rs` file (1,756 lines) that this audit was explicitly told to skip when scoping the LSP/codegraph track. It is not a coverage gap — just a different test-file convention than the `mod tests` pattern used everywhere else in this codebase.

**Real gaps worth flagging before any migration work starts, ranked by risk:**

1. **`library_manifest/{model,wire,type_refs,validation}.rs` (3,500 lines, migration candidate, 0 tests).** This is the one finding that should change how #1034's work gets sequenced — migrating a 3,500-line, currently-untested schema/validation layer with no existing safety net is materially riskier than migrating a well-tested one. Writing characterization tests against current behavior before migrating this block is worth calling out explicitly as prerequisite work, not assumed away.
2. **`src/oven/process.rs` (0 tests) and `src/oven/compiler_suite_env.rs` (0 tests) — both confirmed Rust host kernel, not migration candidates, but untested low-level OS process/policy code** is its own risk category worth someone's attention independent of this migration effort.
3. **A cluster of already-thin CLI commands with zero in-file tests**: `binding_inspect.rs`, `diagnostics.rs`, `format.rs`, `stdlib_loader.rs`, `interop_plan.rs`, `provider_inspect.rs`, `reporter.rs`, `types.rs`. Several of these (`check`/`explain`, `fmt`) plausibly have snapshot/integration coverage elsewhere per CONTRIBUTING.md's property-testing section — worth confirming rather than assuming, given they showed up as zero here.

## Structural note: the CLI split itself

`src/cli/mod.rs` currently implements a single, universal `incan` CLI — every Oven-shaped command (`init`, `new`, `lock`, `env`, `workspace`, `cache`, `oven ...`) is nested under the `incan` binary today, which is the exact shape RFC 118's "Alternatives considered" section rejects as the wrong end-state ("collapses language semantics, project mutation, cache/store ownership, registry trust... into one ambiguous command family"). This matches RFC 118's own stated release boundary — v0.5 does not backport the two-surface `incan`/`oven` split, which targets v0.7 — so it is not a defect this map is flagging as new. It's registered here because it's the container every "Oven CLI (currently under `incan`)" row above will eventually be carved out of.

## Done-when checklist (per #871)

- [x] Every supported responsibility has one classification and one rationale, recorded above.
- [x] Tests recorded per component (in-file unit-test presence, mechanically verified) — see "Test coverage."
- [x] One linked acceptance path per responsibility — Done. The confirmed-kernel and compiler-service-exception rows don't need one (no migration implied). Every migration-candidate and uncertain row now has its own delivery issue, filed as a native GitHub sub-issue of #1034: #1081-#1091 cover the straightforward migration candidates, and #1092-#1100 cover the rows that need an explicit maintainer decision before implementation can be scoped (e.g. #1099 for the `legacy_cargo.rs` Cargo.lock reimplementation, #1100 for the LSP-to-CLI boundary crossing in `lsp/backend.rs`, #1094/#1095 for the `build.rs` shim-vs-core blur).
- [x] The map demonstrates Rust is a bounded host kernel and Incan owns the semantic control plane — confirmed for the large majority of the audited surface; the exceptions and tensions are named explicitly above rather than smoothed over.
