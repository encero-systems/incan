# `incan_frontend`

Ring: **compiler**

Typechecker, semantic analysis, vocab desugar pass, body IR, API metadata.

## Current sources

- `loaves/compiler/incan_frontend/src/`

## May depend on

`kernel`, `incan_format`, `rust_inspect`, `incan_semantics_stdlib`, `oven_model`

Owns the wasmtime dependency through the vocab desugar runtime. The manifest model and provider contract live in this crate, below the provider loaders, so typechecking does not depend on the loader crate.

The frontend lives here, with the provider contract (`provider/{plan,sdk,features,error}`) and the semantics registry; the provider loaders stay with `incan_provider`, and the crate depends on `oven_model` for the project manifest and the toolchain layout.
