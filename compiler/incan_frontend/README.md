# `incan_frontend`

Ring: **compiler**

Typechecker, semantic analysis, vocab desugar pass, body IR, API metadata.

## Moves here from

- `src/frontend/ (117k lines)`

## May depend on

`kernel`, `incan_provider (contract half only)`

Owns the wasmtime dependency through the vocab desugar runtime. The `library_manifest` import cycle (106/51 edges) is cut by depending on the provider *contract* crate, not the loader.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
