# `stdlib/compression`

SDK component `stdlib-compression` (today `crates/incan_stdlib/stdlib/components/stdlib-compression`).

Namespace roots: `std.compression`

Runtime crates: flate2, zstd, bzip2, xz2, snap

```text
stdlib/compression/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_compression` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.compression` should pull this component's runtime crates and nothing else.
