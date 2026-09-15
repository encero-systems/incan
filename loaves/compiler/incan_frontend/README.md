# `incan_frontend`

Ring: **compiler**

Typechecker, semantic analysis, vocab desugar pass, body IR, API metadata.

## Moves here from

- `src/frontend/ (117k lines)`

## May depend on

`kernel`, `incan_provider (contract half only)`

Owns the wasmtime dependency through the vocab desugar runtime. The `library_manifest` import cycle (106/51 edges) is cut by depending on the provider *contract* crate, not the loader.

The frontend lives here, with the provider contract (`provider/{plan,sdk,features,error}`) and the semantics registry; the provider loaders stay with `incan_provider`, and the crate depends on `oven_model` for the project manifest and the toolchain layout.
