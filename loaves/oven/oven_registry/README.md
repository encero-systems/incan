# `oven_registry`

Ring: **oven**

Registry index access, source-Loaf and baked-asset acquisition, checksum and signature verification, trust policy. The contract crate for RFC 125: it exists so the ring's dependency line is fixed before the first line of client code, and it is deliberately empty while RFC 125 is a draft.

## Sources

- `loaves/oven/oven_registry/src/lib.rs` — the crate root, docs only: what lands here in the RFC's own terms

## May depend on

`oven_model`; `oven_store` once fetched artifacts are published into the store

The largest new dependency in the 0.6 program. The fetch decision is recorded on #1478: `oven_registry` fetches natively per RFC 125 (crates.io read-only, as a secondary source); no Cargo binary is borrowed for downloads.
