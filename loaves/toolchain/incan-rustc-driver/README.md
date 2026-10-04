# Pinned native driver boundary

This Loaf contains the Rust adapter required by `rustc_private`. Its input types are authored in `incan_mir_plan` and consumed through `incan_mir_plan::caller::incan`; no Rust mirror of the plan is maintained. Declarations are injected AST items with diverging placeholders, and bodies are supplied through `mir_built`.

The executable has a scalar conformance invocation (`SOURCE CRATE OUTPUT SYSROOT RUNTIME_RLIB [normal|overflow|dangling] [DEPENDENCY_DIR ...]`) and a Rust-unit invocation (`--rust-unit SYSROOT [rustc_private] -- RUSTC_ARGUMENTS`). The scalar invocation obtains its hand-filled plan from an Incan function. The output fixture is an Incan Loaf, so callable output needs no new authored Rust runtime.

Build-time identity inputs are `INCAN_DRIVER_SYSROOT`, `INCAN_DRIVER_RUSTC_IDENTITY` (complete `rustc -vV` output), `INCAN_DRIVER_LIBRARY` (a relative sysroot library path), and `INCAN_DRIVER_LIBRARY_DIGEST` (SHA-256 of those exact library bytes). Missing inputs are refusals. These inputs and declared unit permissions must enter Oven's build identity; setting them as untracked ambient variables is insufficient.

## Delivery boundary

The native sources are not yet build-verified. Oven recognizes the manifest's unit capabilities but currently refuses them before creating receipts or outputs, because this project planner does not select an identity-bound seed compiler. Pinned stable rustc rejects the first `rustc_private` unit with E0554. The spike seeds its driver with `RUSTC_BOOTSTRAP=1`; that route is forbidden here. An approved seed artifact and its Oven provider/identity contract are needed to complete the build path. Oven must also compose the Rust unit's own registry dependencies (`thiserror`, `sha2`) with its Incan caller closure; the stage 3b-0 planner currently consumes only the sibling library's closure.

A native binary, native output assertion, overflow-span assertion, or one-bake end-to-end result is not claimed until that build path is wired and run.
