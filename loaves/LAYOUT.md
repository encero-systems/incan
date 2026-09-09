# Workspace layout skeleton

This branch carries a **skeleton only**: directories and READMEs describing a proposed layout, next to the existing `src/` and `crates/`, which it would replace. No manifests, no code, nothing Cargo or `incan` will read. It exists to look at the shape and argue with the cuts.

## Rings

Five directories. Dependencies point inward only: `toolchain → compiler → kernel`, `toolchain → oven → kernel`, `stdlib → kernel`. The compiler and Oven rings do not depend on each other except that `compiler/incan_driver` consumes Oven's model and store as a client.

```text
incan/
  loaves/
    kernel/      incan_lang · incan_syntax · incan_semantics · incan_vocab · incan_codegraph
    compiler/    incan_frontend · incan_ir · incan_emit · incan_format · incan_provider · incan_inspect · incan_driver
    oven/        oven_model · oven_store · oven_rustc · oven_registry · oven_interop · oven_cargo_compat
    stdlib/      core · interop · system · codecs · compression · data · async · observability · web · testing · derive
    toolchain/   incan-cli · incan-lsp · oven-cli · release · ide
    third_party/ vendored patches, expected to empty
  examples/
  workspaces/    benchmarks · docs-site   (ide and release move under loaves/toolchain)
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
3. Split the remainder into `incan_frontend`, `incan_ir`, `incan_emit`, and the `oven_*` crates.
4. Move directories into rings; give each ring a version.
5. Rename `incan_core` to `incan_lang`.

## Where the root crate goes

Nothing remains under `src/`, `crates/`, or `tests/`. The root `incan` crate ceases to exist; `compiler/incan_driver` is its closest successor.

| In `src/` today | Goes to |
| --- | --- |
| `frontend/` | `compiler/incan_frontend` |
| `backend/ir/lower/`, IR type modules, `numeric_adapters.rs` | `compiler/incan_ir` |
| `backend/ir/emit/`, `conversions.rs`, `codegen.rs`, `backend/replacement/` | `compiler/incan_emit` |
| `backend/project/` | `oven/oven_rustc` (plan, generator) and `oven/oven_cargo_compat` (cargo_toml, runner) |
| `format/` | `compiler/incan_format` |
| `provider/`, `library_manifest/`, `compiled_sdk.rs`, `semantics_registry.rs` | `compiler/incan_provider` |
| `rust_inspect/` | `compiler/incan_inspect` |
| `cli/commands/build.rs` logic, `cli/commands/common.rs` session and discovery, `generated_cache.rs`, `replacement_compatibility*`, `compiler_stack.rs` | `compiler/incan_driver` |
| `cli/` command surface, `main.rs` | `toolchain/incan-cli` |
| `cli/commands/{oven,lock,tools}.rs` | `toolchain/oven-cli` |
| `lsp/`, `bin/lsp.rs` | `toolchain/incan-lsp` |
| `bin/generate_*` | `toolchain/release` (inventory generators) and `toolchain/ide` (grammar keywords) |
| `manifest.rs`, `workspace.rs`, `lockfile.rs`, `dependency_resolver.rs`, `project_lifecycle/`, `toolchain_layout.rs` | `oven/oven_model` |
| `oven/` store and loaf modules, `oven.rs` | `oven/oven_store` |
| `oven/rustc.rs` | `oven/oven_rustc` |
| `oven/legacy_cargo.rs` | `oven/oven_cargo_compat` |
| `oven_interop.rs`, `oven/interop.rs` | `oven/oven_interop` |
| `version.rs` | `kernel/incan_lang`; the toolchain manifest overrides it per bundle |
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

## Timing

Decided 2026-09-09: do this rewrite once `0.6.0-dev.4` has landed, as its own slice, following the migration order above.

## Decide first

- Does `oven_registry` fetch crates itself, or borrow a Cargo binary? This is the largest new dependency in the programme and it shapes the Windows slice later.
- Do stdlib components become real Loaves in 0.6, or stay a directory convention until Oven bakes this repository itself?
