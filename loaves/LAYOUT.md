# Workspace layout skeleton

This branch carries a **skeleton only**: directories and READMEs describing a proposed layout, next to the existing `src/` and `crates/`, which it would replace. No manifests, no code, nothing Cargo or `incan` will read. It exists to look at the shape and argue with the cuts.

## Rings

Five directories. Dependencies point inward only: `toolchain → compiler → kernel`, `stdlib → kernel`, and **`oven` depends on no Incan ring at all**. Oven knows nothing about Incan; the compiler ring supplies `incan_oven_facet`, which implements Oven's provider interface, and the two binaries wire the two together. `compiler/incan_driver` consumes Oven's model and store as a client. Measured on dev.4, the proposed Oven sources already import nothing from the compiler except the handful of edges listed under *The Oven boundary* below, so the strict rule is cheap to reach and is the single most valuable property this layout can guarantee: Oven could be lifted into its own repository by moving one directory.

```text
incan/
  loaves/
    kernel/      incan_lang · incan_syntax · incan_semantics · incan_vocab · incan_codegraph
    compiler/    incan_frontend · incan_ir · incan_emit · incan_format · incan_provider · incan_inspect · incan_driver · incan_oven_facet
    oven/        oven_model · oven_store · oven_rustc · oven_registry · oven_interop · oven_cargo_compat
    stdlib/      core · interop · system · codecs · compression · data · async · observability · web · testing · derive
    toolchain/   incan-cli · incan-lsp · oven-cli
    third_party/ vendored patches, expected to empty
  examples/
  workspaces/    benchmarks · docs-site · ide · release   (not Loaves: a TypeScript extension and shell packaging stay workspaces)
  scripts/
  assets/
```

`loaves/` is the one container the way `crates/` is today. Rings live inside it so the repository root stays stable when a ring is added, split, or retired, and so the root reads as a project, not as a dependency graph.

Each ring has its own version line. Cross-ring edges are semver requirements, never equalities. The user-facing `Incan 0.6` is a toolchain manifest pinning one version of every ring.

## Naming

- `incan_<thing>` for kernel and compiler crates.
- `oven_<thing>` for the build system.
- `incan_std_<component>` for stdlib runtime crates, matching the `stdlib-<component>` ids in `sdk-components.toml`.
- Binaries keep their product names: `incan`, `incan-lsp`, `oven`. Their directories say what they are: `incan-cli`, `incan-lsp`, `oven-cli`.
- `incan_core` becomes `incan_lang`, so that `core` means the mandatory stdlib component and nothing else.
- `incan_semantics_stdlib` is compiler implementation per `layering.md`, not a contract; it goes to `compiler/incan_provider`, never into the kernel.

## Why these cuts

Measured on 0.6.0-dev.4 (see the dependency audit): the root crate is 380k lines in one compilation unit and is the serial tail of every build. Four import knots decide the layout:

| Knot | Edges today | Cut |
| --- | --- | --- |
| `frontend` ↔ `library_manifest` | 106 / 51 | provider *contract* crate the frontend imports; loaders stay in `incan_provider` |
| `backend`, `oven`, `lsp` → `cli` | 9 / 3 / 6 | `cli/commands/common.rs` session and discovery move to `incan_driver` and `incan_provider` |
| `backend` → `oven` | 33 | `backend/project/` is Oven code; it moves to `oven_rustc` and `oven_cargo_compat` |
| `cli/commands/build.rs` | 20.8k lines | the driver, not a command; `toolchain/incan-cli` keeps the clap surface only |

## Rust beside Incan

Only the stdlib has both languages. Each component directory holds `src/*.incn` and `rust/src/` as one Loaf with a conventional Rust facet (RFC 119). The compiler and Oven rings are Rust-only and do not pretend otherwise.

## Migration order (no flag day)

1. Cut the four knots inside the root crate as module moves. No new crates; tests untouched.
2. Extract `incan_driver` and `incan_provider`. `incan-lsp` compiles alone.
3. Split the remainder into `incan_frontend`, `incan_ir`, `incan_emit`, `incan_oven_facet`, and the `oven_*` crates. From here on the Oven-only `cargo check` above runs in CI.
4. Move directories into rings; give each ring a version.
5. Rename `incan_core` to `incan_lang`.

## Where the root crate goes

Nothing remains under `src/`, `crates/`, or `tests/`. The root `incan` crate ceases to exist; `compiler/incan_driver` is its closest successor.

| In `src/` today | Goes to |
| --- | --- |
| `frontend/` | `compiler/incan_frontend` |
| `backend/ir/lower/`, IR type modules, `numeric_adapters.rs` | `compiler/incan_ir` |
| `backend/ir/emit/`, `conversions.rs`, `codegen.rs`, `backend/replacement/` | `compiler/incan_emit` |
| `backend/project/generator.rs` | `compiler/incan_driver`: it renders the generated Rust project from the checked program and provider facts (7 frontend, 2 provider, 2 library_manifest imports). Putting it in Oven would hand the Oven ring the whole frontend |
| `backend/project/plan.rs`, `lock_projection.rs`, `mod.rs` | `oven/oven_rustc` (no outward imports) |
| `backend/project/cargo_toml.rs` | `oven/oven_cargo_compat`, once its two `cli` edges are cut |
| `backend/project/runner.rs` | `oven/oven_cargo_compat`, with its two cfg-gated `rust_inspect` calls inverted into a facet hook so the compat crate does not drag rust-analyzer into Oven |
| `format/` | `compiler/incan_format` |
| `provider/`, `library_manifest/`, `compiled_sdk.rs`, `semantics_registry.rs` | `compiler/incan_provider` |
| `rust_inspect/` | `compiler/incan_inspect` |
| `cli/commands/build.rs` logic, `cli/commands/common.rs` session and discovery, `generated_cache.rs`, `replacement_compatibility*`, `compiler_stack.rs` | `compiler/incan_driver` |
| `cli/` command surface, `main.rs` | `toolchain/incan-cli` |
| `cli/commands/{oven,lock,tools}.rs` | `toolchain/oven-cli` |
| `lsp/`, `bin/lsp.rs` | `toolchain/incan-lsp` |
| `bin/generate_*` | `workspaces/release` (inventory generators) and `workspaces/ide` (grammar keywords); they are tools of those workspaces, not crates of a ring |
| `manifest.rs`, `workspace.rs`, `project_lifecycle/`, `toolchain_layout.rs` | `oven/oven_model` (no outward imports today) |
| `lockfile.rs` | `oven/oven_model` for the generic lock; its SDK-provider and library-manifest sections (`SdkProviderDescriptor`, `LibraryManifest`, `LibraryRustAbi`, `LibraryManifestIndex`) move to `compiler/incan_oven_facet` behind a lock extension point |
| `dependency_resolver.rs` | `oven/oven_model` for resolution; the `Span`/`CompileError` diagnostics adapter and the `StdlibExtraCrateSource` registry lookup move to `compiler/incan_oven_facet` |
| `oven/` store and loaf modules, `oven.rs` | `oven/oven_store` |
| `oven/rustc.rs` | `oven/oven_rustc` |
| `oven/legacy_cargo.rs` | `oven/oven_cargo_compat` |
| `oven_interop.rs`, `oven/interop.rs` | `oven/oven_interop` |
| `version.rs` | `kernel/incan_lang` for the language version. The coupling that actually has to change is elsewhere: `emit/program.rs` writes `incan_stdlib::__incan_stdlib_version_check!(<exact compiler version>)` into every generated crate. Under per-ring versions that becomes a compatibility-range or `incan-v1` ABI check, or every stdlib publication forces a compiler release |
| `numeric.rs` (a re-export) | deleted; callers import `incan_lang` |
| `lib.rs`, `README.md` | deleted; replaced by ring READMEs |

| In `tests/` today | Goes to |
| --- | --- |
| codegen snapshots, lowering, ownership, construction diagnostics | `compiler/incan_emit/tests` |
| parity corpus, replacement, generated-Rust artifact and audit tests, protected bindings | `compiler/incan_driver/tests` |
| CLI integration, layering guardrails, example capability coverage | `toolchain/incan-cli/tests` |
| Oven PR regressions, generated cache integration | `oven/oven_rustc/tests` |
| property tests | split by subject: formatting to `incan_format`, conversions to `incan_emit` |
| `fixtures/` | beside the tests that use them |

`cargo test` at the root then runs the workspace rather than one crate; `make test` should call it that way.

## The Oven boundary

Every import from the proposed Oven-ring sources into an Incan ring, measured on dev.4, and where it goes so the strict arrow holds:

| Edge today | Disposition |
| --- | --- |
| `oven/rustc.rs` → `cli::commands::build` (2: plan selection) | internal to `oven_rustc` once #1266 moves the plan API |
| `oven/legacy_cargo.rs` → `cli::commands::common::discover_active_sdk_inventory`, `provider::SdkInventory`, `library_manifest` digests, `backend::project::runner` | `oven_cargo_compat` takes SDK inventory and provider digests through the facet's provider hook; the runner reference becomes internal |
| `oven/loaf.rs`, `dependency_resolver.rs` → `incan_core::lang::stdlib::StdlibExtraCrateSource` | `incan_oven_facet` supplies extra crate sources as provider facts |
| `lockfile.rs` → `provider::*`, `library_manifest::*`, `frontend::LibraryManifestIndex` | Incan-specific lock sections live in `incan_oven_facet` behind an extension point in `oven_model` |
| `dependency_resolver.rs` → `frontend::ast::Span`, `frontend::diagnostics::CompileError` | resolution errors are Oven's own type; the facet maps them to compiler diagnostics |
| `backend/project/runner.rs` → `rust_inspect` (cfg-gated out-dir records) | facet hook |

`compiler/incan_oven_facet` is the one named place Oven learns about Incan. Without a named crate the dependency gets drawn wherever is convenient and the property dies quietly.

**CI rule, from step 3 onward:** `cargo check -p oven_model -p oven_store -p oven_rustc -p oven_registry -p oven_interop -p oven_cargo_compat` must pass in a tree with no compiler crates present. That is the property test for the ring rule and it is cheap.


## Timing

Decided 2026-09-09: do this rewrite once `0.6.0-dev.4` has landed, as its own slice, following the migration order above.

## Decide first

- Does `oven_registry` fetch crates itself, or borrow a Cargo binary? This is the largest new dependency in the programme and it shapes the Windows slice later.
- Do stdlib components become real Loaves in 0.6, or stay a directory convention until Oven bakes this repository itself?
