# `stdlib/db`

The Loaf of `std.db`, RFC 165's typed embedded storage; its Incan sources live in `src/` beside this file.

`std.db` is a thin typed surface over the platform's store substrate, `incan_platform.store`. It owns no storage of its own: every record, view, snapshot and publication goes through the substrate.

```text
stdlib/db/
  loaf.toml        the Loaf and its one dependency, the platform (RFC 117)
  src/             Incan source: the modules of `std.db`, which its prelude module re-exports
```

The prelude is `loaves/stdlib/db/src/db/prelude.incn`. Contract cases for each operation and each refusal belong under `loaves/stdlib/db/tests/`.

The `std.db` namespace is not registered yet, and the toolchain does not ship this Loaf or the platform. Until it does, tests import the modules by their path under `src/`.
