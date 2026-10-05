# Pinned native driver boundary

This Loaf contains the Rust adapter required by `rustc_private`. Its input types are authored in `incan_mir_plan` and consumed through `incan_mir_plan::caller::incan`; no Rust mirror of the plan is maintained. Declarations are injected AST items with diverging placeholders, and bodies are supplied through `mir_built`.

The executable has a scalar conformance invocation (`SOURCE CRATE OUTPUT SYSROOT RUNTIME_RLIB [normal|overflow|dangling] [DEPENDENCY_DIR ...]`) and a Rust-unit invocation (`--rust-unit SYSROOT [rustc_private] -- RUSTC_ARGUMENTS`). The scalar invocation obtains its hand-filled plan from an Incan function. The output fixture is an Incan Loaf, so callable output needs no new authored Rust runtime.

Build-time identity inputs are `INCAN_DRIVER_SYSROOT`, `INCAN_DRIVER_RUSTC_IDENTITY` (complete `rustc -vV` output), `INCAN_DRIVER_LIBRARY` (a relative sysroot library path), and `INCAN_DRIVER_LIBRARY_DIGEST` (SHA-256 of those exact library bytes). Missing inputs are refusals. These inputs and declared unit permissions must enter Oven's build identity; setting them as untracked ambient variables is insufficient.

## Oven build and conformance

The manifest declares `rustc_private`, `rustc-dev`, and `rustc_driver` on the executable unit. Oven requires pinned rustc 1.98.0 with rustc-dev installed. It strips ambient bootstrap permission, then grants only the declared normalized crate name with `-Zallow-features=rustc_private`. The grant is receipt-bound and records rustc's bootstrap-derived `Cheat` permission. Driver-built Rust units use the session callback instead.

Oven composes the executable's `thiserror` and `sha2` closure with its sibling Incan plan caller closure. The first driver build does not require a preexisting driver. At startup, the executable checks the complete compiler identity, canonical sysroot, and the path and bytes of the loaded rustc-dev driver library. Ambient bootstrap permission is a startup refusal.

The scalar conformance root builds the plan and driver in one Oven invocation, prepares the Incan-authored output fixture through its caller facet, and checks debug and release native output, overflow source location, malformed-plan refusal and startup refusal:

```sh
make test-one TEST_ROOT=loaves/compiler/incan_driver/tests/native_driver_project_tests.rs
```

The root selects installed rustc 1.98.0 with rustc-dev, because ordinary compiler-suite compiler closures omit rustc-dev metadata. Its publisher capability applies only to explicit Oven bakes.

This hand-filled scalar plan does not supply Body IR lowering or a resident driver. Those remain stages 3c and 3d; the normal compiler route has not changed.
