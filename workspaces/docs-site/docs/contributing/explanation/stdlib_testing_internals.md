# `std.testing` — compiler internals

This page documents the internal integration model, runtime boundary design, and open design questions for the
`std.testing` stdlib module. It is aimed at compiler contributors, not Incan users.

> For the user-facing guide, see [Language → How-to → `std.testing` guide].
> For the API reference, see [Language → Reference → Testing].

[Language → How-to → `std.testing` guide]:../../language/how-to/testing_stdlib.md
[Language → Reference → Testing]:../../language/reference/testing.md

## Integration model (RFC 023 Phase 5)

`std.testing` is compiled from Incan source instead of relying on hardcoded Rust assertion helpers.

- **Source of truth**: `crates/incan_stdlib/stdlib/testing.incn`.
- **Rust module mapping**: the file declares `rust.module("incan_stdlib::testing")`, routing host-boundary calls to the
  `incan_stdlib::testing` Rust module.
- **Incan-implemented assertions**: `assert`, `assert_eq`, `assert_ne`, `assert_true`, `assert_false`, `assert_is_some`,
  `assert_is_none`, `assert_is_ok`, `assert_is_err` are all written in Incan source. They delegate to `fail()` /
  `fail_t()` for the actual panic.
- **Host-boundary primitives** (`@rust.extern`):
    - `fail(msg)` / `fail_t[T](msg)` — panic primitives implemented in `incan_stdlib::testing`.
    - `assert_raises` — not yet implemented (FIXME in stdlib source).
    - Marker entrypoints (`skip`, `xfail`, `slow`, `fixture`, `parametrize`) — their Rust implementations intentionally
      panic with a "runtime misuse" message; they exist only to satisfy the extern boundary.
- **Marker metadata**: each marker extern carries `metadata={...}` on its `@rust.extern` annotation. `incan test` reads
  this metadata from the parsed stdlib source (via `src/frontend/testing_markers.rs`) as the single source of truth for
  marker semantics.

This design keeps user-facing assertion behavior in one stdlib Incan file while limiting Rust host code to unavoidable
panic/failure primitives.

## Runtime boundary shape

The runtime boundary is intentionally narrow:

- **Incan-first assertions**: behavior lives in `testing.incn`, not duplicated Rust wrappers. Adding a new assertion
  helper means editing Incan source, not touching `incan_stdlib::testing`.
- **Host orchestration for markers**: discovery and execution semantics (`skip`, `xfail`, `slow`, fixtures, parametrize)
  are resolved by `incan test` from stdlib metadata at discovery time — they are never invoked at runtime in normal test
  execution.
- **Fail-fast on runtime misuse**: the Rust marker stubs panic immediately with a clear message if a marker function is
  ever called outside the test runner (e.g., used as a regular function call instead of a decorator).

## Marker metadata flow

```text
crates/incan_stdlib/stdlib/testing.incn    src/frontend/testing_markers.rs
┌──────────────────────────┐          ┌──────────────────────────────┐
│ @rust.extern(metadata={  │  parse   │  TestingMarkerSemantics      │
│   "marker_kind": "skip", │ ───────► │  ├ markers: { skip, xfail,   │
│   "runner_only": true    │          │  │            slow, fixture, │
│ })                       │          │  │            parametrize }  │
│ pub def skip(...)        │          │  └ metadata per marker       │
└──────────────────────────┘          └──────────────┬───────────────┘
                                                     │
                                      src/cli/test_runner/discovery.rs
                                                     │
                                      ┌──────────────▼───────────────┐
                                      │  resolve decorator → marker  │
                                      │  apply skip/xfail/slow/etc.  │
                                      └──────────────────────────────┘
```

Key files:

|                   File                    |                           Role                            |
| ----------------------------------------- | --------------------------------------------------------- |
| `crates/incan_stdlib/stdlib/testing.incn` | Canonical stdlib source (assertions + marker decls)       |
| `crates/incan_stdlib/src/testing.rs`      | Rust host-boundary implementations (panic stubs)          |
| `src/frontend/testing_markers.rs`         | Parses marker metadata from stdlib; cached via `OnceLock` |
| `src/cli/test_runner/discovery.rs`        | Consumes marker semantics for test discovery              |
