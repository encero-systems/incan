# `oven_registry`

Ring: **oven**

Registry index access, crate acquisition, checksum and signature verification, trust policy. New crate.

## Moves here from

- `(new) — no existing code; the compiler graph has no HTTP, TLS, tar, or gzip today`

## May depend on

`oven_model`

The largest new dependency in the 0.6 programme and the open decision: fetch natively (rustls stack) or borrow a Cargo binary for downloads. Decide before anything else in this ring is versioned.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
