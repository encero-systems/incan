# `stdlib/interop`

SDK component `stdlib-interop`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.interop`

Runtime crates: no runtime crates

```text
stdlib/interop/
  loaf.toml        the component is one Loaf (RFC 117)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        reserved for an `incan_std_interop` facet; this component has no Rust of its own today
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.interop` should pull this component's runtime crates and nothing else.
