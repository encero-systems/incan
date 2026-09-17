# `oven_store`

Ring: **oven**

Bounded Loaf store, receipts, identities, publication.

## Sources and remaining moves

- `loaves/oven/oven_store/src/lib.rs` (the receipt model, whole: the store keys on it and it names nothing of the bakers)
- `loaves/oven/oven_store/src/` — store, closure proof, progress and process modules

`loaves/oven/oven_rustc/src/loaf.rs` went to `oven_rustc` instead: a Loaf is baked, and the module reaches into `rustc` and `legacy_cargo` to do it.

## May depend on

`oven_model`

Unit identity per RFC 124. Cross-project rlib sharing lives here.

The receipt model is the crate root; `test_support` (feature `test_support`) holds the frozen fixture project the store tests publish, for the crates above.
