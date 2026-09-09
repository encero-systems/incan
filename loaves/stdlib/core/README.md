# `stdlib/core`

SDK component `stdlib-core` (today `crates/incan_stdlib/stdlib/components/stdlib-core`).

Namespace roots: `std.features`, `std.prelude`, `std.registry`, `std.result`, `std.reflection`, `std.this`, `std.derives`, `std.traits`, `std.runtime`

Runtime crates: mandatory; no runtime crates

```text
stdlib/core/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_core` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.features` should pull this component's runtime crates and nothing else.
