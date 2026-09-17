# `stdlib/codecs`

SDK component `stdlib-codecs`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.checksum`, `std.encoding`

Runtime crates: crc32fast

```text
stdlib/codecs/
  loaf.toml        the component is one Loaf (RFC 117)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        reserved for an `incan_std_codecs` facet; this component has no Rust of its own today
  tests/           reserved for the component's own tests; empty today
```

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.checksum` should pull this component's runtime crates and nothing else.
