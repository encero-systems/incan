# `stdlib/web`

SDK component `stdlib-web` (today `crates/incan_stdlib/stdlib/components/stdlib-web`).

Namespace roots: `std.web`

Runtime crates: axum, inventory, incan_web_macros

```text
stdlib/web/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_web` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.web` should pull this component's runtime crates and nothing else.
