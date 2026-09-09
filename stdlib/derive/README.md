# `stdlib/derive`

Host-side proc-macro crates used by generated code and by stdlib components.

```text
stdlib/derive/
  incan_derive/        today crates/incan_derive (drops its unused proc-macro2 dependency)
  incan_web_macros/    today crates/incan_web_macros; its tokio/axum/tower dev-dependencies stay dev-only
```

These are host units in RFC 119 terms: compiled for the build host, expanded by the selected rustc through its normal proc-macro ABI.
