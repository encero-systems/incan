//! Oven's registry client: the contract crate for RFC 125, `incan.pub` Loaf registry and baked asset distribution.
//!
//! This crate holds the ring's place for registry index access, source-Loaf and baked-asset acquisition, checksum
//! and signature verification, and trust policy. It is deliberately empty: RFC 125 owns the contract and is still
//! a draft, and the layout rewrite (#1478) reserves the crate so the oven ring's dependency line — this crate over
//! `oven_model`, later `oven_store` for the fetched artifacts' publication — is fixed before the first line of
//! client code exists. The decision recorded there: fetch natively, per RFC 125, with crates.io as a read-only
//! secondary source; no Cargo binary is borrowed for downloads.
//!
//! What lands here when RFC 125 is planned, in the RFC's own terms: the sparse index reader (`index/<scope>/<name>`
//! and `index/-/<name>`, one JSON object per published version), the per-version asset manifest and its RFC 124 unit
//! identities, the signed registry event stream (yank, unyank, ownership, advisory, supersession), registry identity
//! by root public key with transport as a detail, and the resolver rules — filter by version requirement and by
//! `requires` against the installed toolchain, skip yanked versions unless the lock already pins them, select the
//! maximal satisfying version, record version, source, digest and satisfaction kind in `oven.lock`. Resolution
//! executes nothing.
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
