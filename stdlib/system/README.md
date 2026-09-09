# `stdlib/system`

SDK component `stdlib-system` (today `crates/incan_stdlib/stdlib/components/stdlib-system`).

Namespace roots: `std.environ`, `std.io`, `std.tempfile`, `std.fs`

Runtime crates: byteorder, encoding_rs, rustix, tempfile

```text
stdlib/system/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_system` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.environ` should pull this component's runtime crates and nothing else.
