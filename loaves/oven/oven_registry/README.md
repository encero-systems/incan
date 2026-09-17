# `oven_registry`

Ring: **oven**

Registry index access, crate acquisition, checksum and signature verification, trust policy. New crate.

## Moves here from

- new — no existing code; the compiler graph has no HTTP, TLS, tar, or gzip today

## May depend on

`oven_model`

The largest new dependency in the 0.6 programme. The fetch decision is recorded on #1478: `oven_registry` fetches natively per RFC 125 (crates.io read-only, as a secondary source); no Cargo binary is borrowed for downloads. This slice reserves the crate's place; RFC 125 owns its contract.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
