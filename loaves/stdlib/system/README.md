# `stdlib/system`

SDK component `stdlib-system`; its Incan sources live in `src/` beside this file.

Namespace roots: `std.environ`, `std.io`, `std.tempfile`, `std.fs`

Runtime crates: byteorder, encoding_rs, rustix, tempfile

```text
stdlib/system/
  loaf.toml        the component is one Loaf (RFC 117)
  src/             Incan source: the `.incn` modules for the roots above
  rust/src/        reserved for an `incan_std_system` facet; this component has no Rust of its own today
```

Component-level tests belong under `loaves/stdlib/system/tests/`.

Rust and Incan live in the same directory because the component is the unit of versioning, ownership, testing, and runtime-dependency attribution. A project importing `std.environ` should pull this component and its declared dependency closure, without unrelated components.
