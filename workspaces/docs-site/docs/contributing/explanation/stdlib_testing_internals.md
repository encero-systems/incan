# `std.testing` — compiler internals

This page documents the internal integration model, runtime boundary design, and open design questions for the `std.testing` stdlib module. It is aimed at compiler contributors, not Incan users.

> For the user-facing guide, see [Language → How-to → `std.testing` guide].
> For the API reference, see [Standard library reference → `std.testing`].

[Language → How-to → `std.testing` guide]:../../language/how-to/testing_stdlib.md
[Standard library reference → `std.testing`]:../../language/reference/stdlib/testing.md

## Integration model (RFC 171)

`std.testing` is compiled from Incan source instead of relying on hardcoded Rust assertion helpers.

- **Source of truth**: `loaves/stdlib/testing/src/testing.incn`.
- **Direct Rust imports**: narrow host functions are imported from `rust::incan_std_testing`; the source names every boundary item explicitly.
- **Incan-implemented assertions**: `assert`, `assert_eq`, `assert_ne`, `assert_true`, `assert_false`, `assert_is_some`, `assert_is_none`, `assert_is_ok`, `assert_is_err`, and `fail` are all written in Incan source. They call the directly imported generic `fail_t()` primitive for the actual panic.
- **Marker declarations**: runner-only functions such as `skip`, `xfail`, `slow`, `fixture`, and `parametrize` carry checked `@marker(...)` metadata. Their ordinary Incan bodies call private Rust imports that intentionally report runtime misuse.
- **Known blocker**:
    - `assert_raises` remains unimplemented as an Incan-side placeholder (fails via `fail_t`) until parser/lowering support for `assert ... raises ...` lands.
- **Marker metadata**: each marker declaration carries named literal fields in `@marker(...)`. `incan test` reads the checked API metadata (via `loaves/compiler/incan_frontend/src/testing_markers.rs`) as the single source of truth for marker semantics.
- **Surface semantics routing**:
    - Pack contracts live in `loaves/kernel/incan_semantics_core` (`SurfaceFeatureKey`, pack trait, registry).
    - Stdlib handlers live in `loaves/compiler/incan_semantics_stdlib` and are included in every compiler consumer.
    - The frontend calls into the registry through `loaves/compiler/incan_frontend/src/surface_semantics.rs`; the former IR adapter was removed.
    - `assert` statement syntax is lowered through registry-provided canonical call target metadata.
    - Lowered assert calls carry canonical callee paths (`std.testing.assert_*`) in IR.
    - Emission uses canonical paths for stdlib dispatch, so `import std.testing` and `from std.testing import assert_*` behave consistently.

This design keeps user-facing assertion behavior in one stdlib Incan file while limiting Rust host code to unavoidable panic/failure primitives.

## Runtime boundary shape

The runtime boundary is intentionally narrow:

- **Incan-first assertions**: behavior lives in `testing.incn`, not duplicated Rust wrappers. Adding a new assertion helper means editing Incan source, not touching `incan_std_testing`.
- **Host orchestration for markers**: discovery and execution semantics (`skip`, `xfail`, `slow`, fixtures, parametrize) are resolved by `incan test` from stdlib metadata at discovery time — they are never invoked at runtime in normal test execution.
- **Fail-fast on runtime misuse**: the Rust marker stubs panic immediately with a clear message if a marker function is ever called outside the test runner (e.g., used as a regular function call instead of a decorator).

## Marker metadata flow

```text
stdlib testing declarations             frontend marker metadata
┌──────────────────────────┐          ┌──────────────────────────────┐
│ @marker(                 │  check   │  TestingMarkerSemantics      │
│   kind="skip",           │ ───────► │  ├ markers: { skip, xfail,   │
│   runner_only=true,      │          │  │            slow, fixture, │
│ )                        │          │  │            parametrize }  │
│ pub def skip(...)        │          │  └ metadata per marker       │
└──────────────────────────┘          └──────────────┬───────────────┘
                                                     │
                                      driver test discovery
                                                     │
                                      ┌──────────────▼───────────────┐
                                      │  resolve decorator → marker  │
                                      │  apply skip/xfail/slow/etc.  │
                                      └──────────────────────────────┘
```

Key files:

|                   File                    |                           Role                            |
| ----------------------------------------- | --------------------------------------------------------- |
| `loaves/stdlib/testing/src/testing.incn` | Canonical stdlib source (assertions + marker decls)       |
| `loaves/stdlib/testing/rust/src/lib.rs`   | Rust host-boundary implementations (panic stubs), the `incan_std_testing` facet |
| `loaves/compiler/incan_frontend/src/testing_markers.rs`         | Parses marker metadata from stdlib; cached via `OnceLock` |
| `loaves/compiler/incan_driver/src/testing/discovery.rs`        | Consumes marker semantics for test discovery              |
