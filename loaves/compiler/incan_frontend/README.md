# `incan_frontend`

Ring: **compiler**

Typechecker, semantic analysis, vocab desugar pass, body IR, API metadata.

## Current sources

- `loaves/compiler/incan_frontend/src/`

## May depend on

The dependency contract is declared in `loaves/compiler/incan_frontend/loaf.toml`. Kernel contracts, `incan_format` and the optional `incan_semantics_stdlib` packs support checking; Rust facts arrive from rustc metadata through the installed ABI caches and the native driver.

Owns the admitted native vocabulary-worker protocol and its public syntax bridge. The manifest model and provider contract live in this crate, below the provider loaders, so typechecking does not depend on the loader crate.

The frontend owns the provider contract (`provider/{plan,sdk,features,error}`) and the semantics registry. Provider loading lives in `incan_driver`; Oven reads project manifests and passes unit descriptions to the driver.
