# `stdlib/data`

SDK component `stdlib-data`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.collections`, `std.graph`, `std.hash`, `std.json`, `std.toml`, `std.math`, `std.uuid`, `std.datetime`, `std.regex`, `std.serde`

Runtime crates: serde, serde_json, toml, toml_edit, serde_path_to_error, libm, rand, regex, the eight hash crates

```text
stdlib/data/
  loaf.toml        the component is one Loaf; a `[rust.source]` table names its Rust facet's root (RFC 119)
  src/             Incan source: the `.incn` modules for the roots above
  rust/            the `incan_std_data` facet: the Rust half of this component, declared by `[rust.source]` in `loaf.toml`
  tests/           reserved for the component's own tests; empty today
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.collections` should pull this component's runtime crates and nothing else.
