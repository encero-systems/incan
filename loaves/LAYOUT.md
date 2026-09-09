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
    toolchain/   incan · incan-lsp · oven · release · ide
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
- Binaries keep their product names: `incan`, `incan-lsp`, `oven`.
- `incan_core` becomes `incan_lang`, so that `core` means the mandatory stdlib component and nothing else.

## Why these cuts

Measured on 0.6.0-dev.4 (see the dependency audit): the root crate is 380k lines in one compilation unit and is the serial tail of every build. Four import knots decide the layout:

| Knot | Edges today | Cut |
| --- | --- | --- |
| `frontend` ↔ `library_manifest` | 106 / 51 | provider *contract* crate the frontend imports; loaders stay in `incan_provider` |
| `backend`, `oven`, `lsp` → `cli` | 9 / 3 / 6 | `cli/commands/common.rs` session and discovery move to `incan_driver` and `incan_provider` |
| `backend` → `oven` | 33 | `backend/project/` is Oven code; it moves to `oven_rustc` and `oven_cargo_compat` |
| `cli/commands/build.rs` | 20.8k lines | the driver, not a command; `toolchain/incan` keeps the clap surface only |

## Rust beside Incan

Only the stdlib has both languages. Each component directory holds `src/*.incn` and `rust/src/` as one Loaf with a conventional Rust facet (RFC 119). The compiler and Oven rings are Rust-only and do not pretend otherwise.

## Migration order (no flag day)

1. Cut the four knots inside the root crate as module moves. No new crates; tests untouched.
2. Extract `incan_driver` and `incan_provider`. `incan-lsp` compiles alone.
3. Split the remainder into `incan_frontend`, `incan_ir`, `incan_emit`, and the `oven_*` crates.
4. Move directories into rings; give each ring a version.
5. Rename `incan_core` to `incan_lang`.

## Decide first

- Does `oven_registry` fetch crates itself, or borrow a Cargo binary? This is the largest new dependency in the programme and it shapes the Windows slice later.
- Do stdlib components become real Loaves in 0.6, or stay a directory convention until Oven bakes this repository itself?
