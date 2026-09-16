# `stdlib/async`

SDK component `stdlib-async`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.async`

Runtime crates: tokio (rt-multi-thread, macros, time, sync, net)

```text
stdlib/async/
  loaf.toml        the component is one Loaf; a `[rust.source]` table names its Rust facet's root (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/            the `incan_std_async` facet: the Rust half of this component, declared by `[rust.source]` in `loaf.toml`
  tests/           `.incn` and `.rs` tests side by side
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.async` should pull this component's runtime crates and nothing else.
