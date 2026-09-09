# `stdlib/async`

SDK component `stdlib-async` (today `crates/incan_stdlib/stdlib/components/stdlib-async`).

Namespace roots: `std.async`

Runtime crates: tokio (rt-multi-thread, macros, time, sync, net)

```text
stdlib/async/
  loaf.toml        the component is one Loaf; its Rust facet is conventional (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        Rust runtime bridge crate `incan_std_async` (today gated code in `crates/incan_stdlib`)
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.async` should pull this component's runtime crates and nothing else.
