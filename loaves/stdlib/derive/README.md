# `stdlib/derive`

Host-side proc-macro crates used by generated code and by stdlib components.

```text
stdlib/derive/
  incan_derive/        the derive macros generated code and the stdlib components use
  incan_web_macros/    the web attribute macros; their tokio/axum/tower dev-dependencies stay dev-only
```

These are host units in RFC 119 terms: compiled for the build host, expanded by the selected rustc through its normal proc-macro ABI.
