# `stdlib/observability`

SDK component `stdlib-observability` (today `crates/incan_stdlib/stdlib/components/stdlib-observability`).

Namespace roots: `std.logging`, `std.telemetry`

Runtime crates: no runtime crates

```text
stdlib/observability/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_observability` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.logging` should pull this component's runtime crates and nothing else.
