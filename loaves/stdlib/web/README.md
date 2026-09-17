# `stdlib/web`

SDK component `stdlib-web`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.web`

Runtime crates: axum, inventory, incan_web_macros

```text
stdlib/web/
  loaf.toml        the component is one Loaf; a `[rust.source]` table names its Rust facet's root (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/            the `incan_std_web` facet: the Rust half of this component, declared by `[rust.source]` in `loaf.toml`
  tests/           reserved for the component's own tests; empty today
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.web` should pull this component's runtime crates and nothing else.
