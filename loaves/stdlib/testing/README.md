# `stdlib/testing`

SDK component `stdlib-testing` (today `crates/incan_stdlib/stdlib/components/stdlib-testing`).

Namespace roots: `std.testing`

Runtime crates: no runtime crates

```text
stdlib/testing/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_testing` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.testing` should pull this component's runtime crates and nothing else.
