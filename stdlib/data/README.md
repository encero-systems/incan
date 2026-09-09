# `stdlib/data`

SDK component `stdlib-data` (today `crates/incan_stdlib/stdlib/components/stdlib-data`).

Namespace roots: `std.collections`, `std.graph`, `std.hash`, `std.json`, `std.math`, `std.uuid`, `std.datetime`, `std.regex`, `std.serde`

Runtime crates: serde, serde_json, libm, rand, regex, the eight hash crates

```text
stdlib/data/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_data` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.collections` should pull this component's runtime crates and nothing else.
