# `stdlib/compression`

SDK component `stdlib-compression`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.compression`

Runtime crates: flate2, zstd, bzip2, xz2, snap

```text
stdlib/compression/
  loaf.toml        the component is one Loaf (RFC 117)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        reserved for an `incan_std_compression` facet; this component has no Rust of its own today
```

Component-level tests belong under `loaves/stdlib/compression/tests/`.

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.compression` should pull this component and its declared dependency closure, without unrelated components.
