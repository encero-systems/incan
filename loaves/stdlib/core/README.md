# `stdlib/core`

SDK component `stdlib-core`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.features`, `std.prelude`, `std.registry`, `std.result`, `std.reflection`, `std.this`, `std.derives`, `std.traits`, `std.runtime`

Runtime crates: mandatory; no runtime crates

```text
stdlib/core/
  loaf.toml        the component is one Loaf; a `[rust.source]` table names its Rust facet's root (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/            the `incan_std_core` facet: the Rust half of this component, declared by `[rust.source]` in `loaf.toml`
```

Component-level tests are reserved under `loaves/stdlib/core/tests/`; the directory is empty today. The facet's Rust tests live in `loaves/stdlib/core/rust/tests/`.

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.features` should pull this component's runtime crates and nothing else.
