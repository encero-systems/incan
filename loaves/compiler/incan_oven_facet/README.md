# `incan_oven_facet`

Ring: **compiler**

Implements Oven's provider interface for Incan. This is the one named crate through which Oven learns anything about Incan; Oven itself depends on no Incan ring.

## Current sources

- `loaves/compiler/incan_oven_facet/src/lib.rs`: `compiler_identity()` and `provider_hooks()` — Oven's `CompilerIdentity` and `OvenProviderHooks` filled with Incan's facts — and the SDK-staging functions that read the inventory, which `legacy_cargo` used to take from the provider directly

What LAYOUT expected here and where it went instead, measured: the lock's semantic sections needed no extension point because the lock model was already plain data, so they are `incan_provider::lock_semantics`; the dependency resolver is compiler-ring code through and through (Incan inline imports, compiler diagnostics) and lives in `incan_provider`; the `StdlibExtraCrateSource` reads left `loaf.rs` as tests of this crate. The `runner.rs` inspection hook and the generated-project stdlib baseline arrive when `backend/project` moves.

## May depend on

`kernel`, `incan_provider`, `incan_inspect`, and the Oven ring's public interface

The binaries in `toolchain/` wire this crate into Oven. Nothing in `oven/` may import it.
