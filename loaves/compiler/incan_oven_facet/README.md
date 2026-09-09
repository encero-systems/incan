# `incan_oven_facet`

Ring: **compiler**

Implements Oven's provider interface for Incan. This is the one named crate through which Oven learns anything about Incan; Oven itself depends on no Incan ring.

## Moves here from

- the SDK-provider and library-manifest sections of `src/lockfile.rs` (`SdkProviderDescriptor`, `SdkComponent`, `ProviderIdentity`, `LibraryManifest`, `LibraryRustAbi`, `LibraryManifestIndex` use)
- the compiler-diagnostics adapter in `src/dependency_resolver.rs` (`Span`, `CompileError`) and its `StdlibExtraCrateSource` registry lookup
- the `StdlibExtraCrateSource` use in `src/oven/loaf.rs`
- the SDK inventory discovery `src/oven/legacy_cargo.rs` takes from `cli::commands::common`
- the cfg-gated `rust_inspect` out-dir hook in `src/backend/project/runner.rs`
- the generated-project stdlib baseline, supplied to Oven as plan facts

## May depend on

`kernel`, `incan_provider`, `incan_inspect`, and the Oven ring's public interface

The binaries in `toolchain/` wire this crate into Oven. Nothing in `oven/` may import it.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
