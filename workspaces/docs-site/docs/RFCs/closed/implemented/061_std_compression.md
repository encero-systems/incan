# RFC 061: `std.compression` — codec-based compression and decompression

- **Status:** Implemented
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/339
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** 0.3.0

## Summary

This RFC proposes `std.compression` as Incan's standard library module for byte-oriented compression and decompression. The module standardizes a codec-submodule surface for common codecs, supports both one-shot (`bytes -> bytes`) and streaming workflows, and keeps codec autodetection explicit and opt-in. The goal is to make compression a first-class Incan capability without leaking backend crate APIs into the language contract.

## Core model

`std.compression` is a codec layer over bytes and binary streams. Codec submodules own normal encode/decode workflows, while the top-level module owns explicit autodetection helpers and shared vocabulary such as `Codec` and `CompressionError`.

The stable public contract has three parts:

- per-codec one-shot APIs for callers that already have a `bytes` payload;
- per-codec stream APIs for callers that need to move data between `std.fs.File` and `std.io.BytesIO` without materializing the whole payload;
- explicit top-level autodetection APIs for decompression only.

The module must not expose backend crate names, backend option structs, or backend-specific error types as the user-facing contract. Implementations may use any backend that preserves the observable codec behavior, error categories, streaming semantics, and supported option rules defined here.

## Motivation

Compression is a recurring systems and data task: users archive logs, exchange compressed API payloads, read compressed datasets, and build pipeline stages that transform compressed files. Without a standard module, users either fall into Rust interop immediately or reinvent compression wrappers per project.

This matters because compression sits directly on top of stdlib capabilities already being defined:

- `std.fs` handles path and file I/O;
- `std.io` handles in-memory bytes and cursor-like workflows;
- `std.compression` should provide the codec layer these modules feed into.

Compression should therefore be explicit in the language standard library instead of remaining ecosystem-only glue code.

## Goals

- Provide a standard byte-oriented compression and decompression surface.
- Standardize a concrete initial codec set: `gzip`, `zlib`, `deflate`, `zstd`, `bz2`, `lzma`, and `snappy`.
- Support both one-shot and streaming usage patterns in the core contract.
- Keep the public contract codec-first and Incan-native rather than backend-crate-shaped.
- Keep autodetection explicit and opt-in rather than implicit in normal codec calls.

## Non-Goals

- Standardizing archive container formats such as ZIP or TAR in this RFC.
- Replacing specialized domain-specific compression libraries.
- Standardizing every compression codec and feature flag in the first iteration.
- Hiding codec choice behind implicit autodetection in normal API calls.
- Defining dictionary training APIs or advanced codec-tuning systems in this RFC.

## Guide-level explanation

Authors should be able to compress and decompress bytes directly:

```incan
from std.compression import gzip

compressed = gzip.compress(payload)?
plain = gzip.decompress(compressed)?
```

Streaming workflows should be equally direct:

```incan
from std.compression import zstd
from std.fs import Path

source = Path("events.jsonl.zst").open("rb")?
target = Path("events.jsonl").open("wb")?
zstd.decompress_stream(source, target)?
```

Autodetection should be explicit and opt-in:

```incan
from std import compression

codec, plain = compression.decompress_auto(blob)?
println(codec)
```

## Reference-level explanation

### Module scope

`std.compression` must provide:

- codec submodules:
  - `std.compression.gzip`
  - `std.compression.zlib`
  - `std.compression.deflate`
  - `std.compression.zstd`
  - `std.compression.bz2`
  - `std.compression.lzma`
  - `std.compression.snappy`
- one-shot `bytes -> bytes` operations per codec;
- streaming operations per codec over `std.fs.File` and `std.io.BytesIO`;
- explicit top-level autodetection helpers for decompression.

The top-level module must also expose:

- `Codec`, a stable enum-like codec value used by autodetection results and `allowed` filters;
- `CompressionError`, the stable error boundary for codec, option, stream, and I/O failures.

`Codec` must represent at least `Gzip`, `Zlib`, `Deflate`, `Zstd`, `Bz2`, `Lzma`, and `Snappy`. `Codec.all()` must return those codecs in a stable order and must not include raw Snappy.

### Capability areas

The contract must cover:

- compression and decompression over `bytes`;
- streaming compression and decompression without forcing full in-memory materialization;
- codec-accurate error reporting for invalid data and truncated input;
- explicit codec naming in normal workflows;
- explicit opt-in autodetection only through dedicated functions.

### API direction

The surface must be codec-submodule-first and consistent across codecs.

Per-codec baseline APIs:

- `compress(data: bytes, level: int | None = None) -> Result[bytes, CompressionError]`
- `decompress(data: bytes) -> Result[bytes, CompressionError]`
- `compress_stream(source: File | BytesIO, target: File | BytesIO, level: int | None = None, chunk_size: int = 65536) -> Result[None, CompressionError]`
- `decompress_stream(source: File | BytesIO, target: File | BytesIO, chunk_size: int = 65536) -> Result[None, CompressionError]`

Top-level autodetection APIs:

- `decompress_auto(data: bytes, allowed: List[Codec] = Codec.all()) -> Result[(Codec, bytes), CompressionError]`
- `decompress_auto_stream(source: File | BytesIO, target: File | BytesIO, allowed: List[Codec] = Codec.all(), chunk_size: int = 65536) -> Result[Codec, CompressionError]`

Autodetection should not be coupled to file extensions. It should use codec signatures and framing checks where applicable and fail explicitly when detection is ambiguous or unsupported.

### Public API surface

Each required codec submodule must expose the baseline API above. The meaning of the baseline API is:

- `compress(...)` returns a complete compressed byte payload for the codec represented by the submodule.
- `decompress(...)` returns the complete decompressed bytes and must reject invalid or truncated input with `CompressionError`.
- `compress_stream(...)` reads binary input from `source`, writes compressed output to `target`, and must not require full input materialization.
- `decompress_stream(...)` reads compressed binary input from `source`, writes decompressed output to `target`, and must not require full input materialization.

Each stream API must process data incrementally using `chunk_size`. `chunk_size` must be positive. A non-positive chunk size must return `CompressionError.invalid_chunk_size` or the equivalent stable category.

The `level` argument is a portable compression-level request, not a backend option bag. `None` means the codec's documented default level. Codecs with level support must reject unsupported numeric levels with `CompressionError.invalid_level`. Codecs without configurable levels, including framed Snappy, must reject non-`None` levels with `CompressionError.unsupported_option`.

### Codec behavior

The required codec submodules must have these default meanings:

- `gzip` must read and write gzip-wrapped deflate streams.
- `zlib` must read and write zlib-wrapped deflate streams.
- `deflate` must read and write raw deflate streams.
- `zstd` must read and write zstd frames.
- `bz2` must read and write bzip2 streams.
- `lzma` must read and write XZ/LZMA-family streams through the documented stdlib surface.
- `snappy` must read and write framed Snappy streams by default.

Compressed bytes produced by a codec submodule must be accepted by the same submodule's `decompress(...)` and `decompress_stream(...)` APIs. Cross-codec behavior is not implicit: `gzip.decompress(...)` must not silently fall back to zlib, deflate, or autodetection.

### Autodetection contract

Autodetection is decompression-only in this RFC. `decompress_auto(...)` and `decompress_auto_stream(...)` must examine codec signatures and framing data where the codec has a reliable signature. They must return the detected `Codec` together with the decompressed output, or fail with a stable `CompressionError` category when no allowed codec matches.

The `allowed` argument must restrict the candidate set. An empty `allowed` list must return `CompressionError.unsupported_codec` or an equivalent stable category. Implementations must not try codecs outside `allowed`.

Autodetection must not use file extensions, path names, or MIME-type guesses as part of the contract. Stream autodetection may buffer enough prefix data to identify the codec, but it must preserve that data for decompression and must not require reading the whole stream before it starts producing output.

When signatures are ambiguous or a codec has no reliable signature, autodetection must fail explicitly instead of guessing. Raw Snappy is excluded from autodetection.

### Error model

`CompressionError` must distinguish at least:

- invalid or corrupted compressed data;
- truncated input;
- unsupported codec;
- unsupported option;
- invalid compression level;
- invalid chunk size;
- ambiguous autodetection;
- I/O failure while reading from or writing to streams;
- backend failure after stdlib-level validation.

Codec-specific details may be preserved as metadata, but the top-level error type must remain stable and codec-neutral.

## Design details

### Why compression deserves its own module

Compression is not a generic `bytes` helper. It has codec-specific semantics, error behavior, streaming tradeoffs, and compatibility constraints. A dedicated `std.compression` module keeps these concerns explicit and avoids burying codec behavior inside `std.io` or `std.fs`.

### Why codec submodules

Submodules keep callsites explicit and readable:

- `gzip.compress(...)`
- `zstd.decompress_stream(...)`

This preserves predictability and makes cross-codec behavior easy to compare without turning the API into one overloaded function family with hidden mode switches.

### Codec set and scope

The initial codec set is:

- `gzip`
- `zlib`
- `deflate`
- `zstd`
- `bz2`
- `lzma`
- `snappy`

This set covers the dominant interchange and data-processing codecs while avoiding archive-container scope creep.

For Snappy, the standard-library default should be the framed format surface. Raw Snappy format should exist as an advanced interop surface (for example Parquet-style page compression paths), but it should not be the default path because it weakens streaming and autodetection behavior.

### One-shot and streaming support

`bytes -> bytes` APIs are required for simple usage and small payloads. Streaming APIs are also part of the core contract because large-file and pipeline workflows are a first-class use case and must not require immediate Rust interop.

Streaming support in this RFC is intentionally concrete and practical:

- `source` and `target` accept `std.fs.File` and `std.io.BytesIO`;
- chunked processing is explicit via `chunk_size`;
- stream APIs are per-codec and do not depend on future generic Reader/Writer protocol RFCs.

### Autodetection policy

Autodetection is useful but dangerous when implicit. The module should therefore expose autodetection only through dedicated APIs (`decompress_auto` / `decompress_auto_stream`) and keep normal codec APIs explicit.

This creates a clear policy boundary:

- explicit codec call for predictable behavior;
- explicit autodetection call when convenience is needed.

Snappy autodetection applies to the framed Snappy format. Raw Snappy streams are out of scope for autodetection in this RFC.

### Snappy raw interop surface

`std.compression.snappy` must expose:

- framed APIs as the default (`compress`, `decompress`, stream helpers);
- advanced raw APIs under a nested namespace such as `snappy.raw`.

Raw Snappy support is included for systems integrations that need block-level behavior (for example Parquet-family readers and writers), but this is intentionally not the primary path for general application compression workflows.

### Interaction with existing stdlib work

- `std.fs` remains responsible for path and file operations.
- `std.io` remains responsible for in-memory byte and cursor behavior.
- `std.compression` provides codec behavior on top of those modules.

This keeps module boundaries clean and avoids blending filesystem, cursor, and codec concerns into one API surface.

### Compatibility / migration

This feature is additive. Existing Rust interop or third-party compression code can continue to work, but common codec operations should have a standard Incan path once this module exists.

## Alternatives considered

1. **Push compression entirely to Rust interop**
   - Too low-level and too inconsistent for a batteries-included language standard library.

2. **Fold compression into `std.io`**
   - Wrong boundary. Compression is codec semantics, not generic byte cursor behavior.

3. **Only `bytes -> bytes` in core, stream support later**
   - Too small for the north star and forces immediate escape hatches for common large-file workflows.

4. **Hidden autodetection by default**
   - Too implicit for reliable systems and data pipelines.

5. **Expose backend option structs directly**
   - Too backend-shaped for a stable language contract and likely to make future backend changes breaking.

## Drawbacks

- Supporting seven codecs in core stdlib increases implementation and test surface area.
- Streaming APIs over multiple backends require careful behavior and error-contract consistency.
- Different codecs have different level/option semantics, so docs must be explicit to avoid false uniformity.

## Layers affected

- **Stdlib / runtime**: must provide codec modules, `Codec`, `CompressionError`, one-shot helpers, streaming helpers, and explicit autodetection helpers.
- **Language surface**: the module and codec submodules must be importable as specified, with stable function signatures and error categories.
- **Execution handoff**: implementations must preserve codec behavior, stream incrementally, and avoid backend API leakage.
- **Docs / examples**: must standardize bytes, stream, compression-level, error, Snappy raw, and autodetection usage patterns.

## Implementation Plan

### Phase 1: RFC lifecycle, registry, and source-defined surface

- Move the RFC to `In Progress` and use this plan/checklist as the implementation source of truth.
- Register `std.compression` and its codec submodules in the stdlib namespace registry.
- Add source-defined Incan stdlib modules for `Codec`, `CompressionError`, per-codec APIs, one-shot autodetection helpers, and `snappy.raw`.
- Preserve the dogfooding constraint: implement the stdlib surface in `.incn` over normal Rust crate imports where needed, without new `@rust.extern` function or type implementation surfaces.

### Phase 2: Codec behavior and dependency handoff

- Add stdlib-managed crate dependencies for the codec backends needed by generated projects.
- Implement one-shot compression/decompression for `gzip`, `zlib`, `deflate`, `zstd`, `bz2`, `lzma`, framed `snappy`, and raw `snappy`.
- Implement stream helpers by chunking through `std.fs.File` and `std.io.BytesIO` read/write methods.
- Normalize codec, option, chunk-size, and I/O failures into `CompressionError`.

### Phase 3: Autodetection, validation, and docs

- Implement top-level decompression autodetection with explicit `allowed` filtering and signature/framing checks.
- Add typechecker/registry tests and generated-code snapshots for the new stdlib modules.
- Add integration tests for round trips, invalid options, chunk-size errors, and autodetection behavior.
- Update authored stdlib docs and release notes for the new user-visible module.
- Bump the active dev version by one dev increment once the implementation lands.

## Progress Checklist

Implementation note: the dogfooded `.incn` implementation avoids new `@rust.extern` surfaces and builds as a generated Rust project for one-shot compression, stream compression/decompression, explicit byte autodetection, and explicit stream autodetection. Issue [#548](https://github.com/dannys-code-corner/incan/issues/548) was resolved in this implementation loop by preserving public stdlib dependency APIs in generated projects and fixing the Rust boundary cases surfaced by the RFC 061 fixture.

### Spec / lifecycle

- [x] RFC moved to `Planned` with settled design decisions and issue #339 relabeled as `feature`.
- [x] RFC moved to `In Progress` with implementation plan and progress checklist.
- [x] GitHub issue #339 has the In Progress traceability comment.

### Stdlib registry / dependency handoff

- [x] Register `std.compression` and codec submodules in `STDLIB_NAMESPACES`.
- [x] Add registry tests for known module paths and stub path resolution.
- [x] Add stdlib-managed crate dependencies for codec backends.
- [x] Verify generated projects receive the codec dependencies through the stdlib registry.

### Source-defined stdlib surface

- [x] Add `crates/incan_stdlib/stdlib/compression/prelude.incn`.
- [x] Add internal `_core` and `_auto` source modules for shared vocabulary and autodetection implementation.
- [x] Add `Codec` and `CompressionError` as Incan-authored public types.
- [x] Add per-codec submodules for `gzip`, `zlib`, `deflate`, `zstd`, `bz2`, `lzma`, and `snappy`.
- [x] Add `snappy.raw` advanced interop surface.
- [x] Avoid new `@rust.extern` function or type implementation surfaces.

### Codec behavior

- [x] Implement one-shot `compress` and `decompress` for every required codec at the source/typecheck layer.
- [x] Implement one-shot `compress` and `decompress` for every required codec in a Rust-buildable generated project.
- [x] Implement stream `compress_stream` and `decompress_stream` for every required codec.
- [x] Reject non-positive `chunk_size` values at the source/typecheck layer.
- [x] Reject unsupported compression levels through stable error categories at the source/typecheck layer.
- [x] Normalize invalid data, truncated input, I/O, and backend failures into `CompressionError` at the source/typecheck layer.

### Autodetection

- [x] Implement `decompress_auto` at the source/typecheck layer.
- [x] Implement `decompress_auto_stream` at the source/typecheck layer.
- [x] Enforce the `allowed` codec filter exactly at the source/typecheck layer.
- [x] Reject empty `allowed` lists at the source/typecheck layer.
- [x] Exclude raw Snappy from autodetection.
- [x] Avoid extension/path/MIME guessing.

### Tests

- [x] Add typechecker tests for imports and public symbols.
- [x] Add codegen snapshot coverage for root and codec stdlib modules.
- [x] Add integration tests for bytes round trips.
- [x] Add integration tests for stream round trips.
- [x] Add integration tests for option/chunk-size errors.
- [x] Add integration tests for autodetection.

### Docs / release / version

- [x] Add authored stdlib reference docs for `std.compression`.
- [x] Add release notes entry for RFC 061 / issue #339.
- [x] Bump the workspace dev version when the implementation lands.
- [x] Regenerate RFC snippets/index if lifecycle metadata changes require it.
- [x] Run docs build.
- [x] Run repository verification gate.

## Design Decisions

- `std.compression` is a dedicated codec module and is not folded into `std.io` or `std.fs`.
- The initial codec set is `gzip`, `zlib`, `deflate`, `zstd`, `bz2`, `lzma`, and `snappy`.
- The public surface is codec-submodule-based (`std.compression.gzip`, etc.).
- Core contract includes both one-shot (`bytes -> bytes`) and streaming APIs.
- Streaming APIs operate directly on `std.fs.File` and `std.io.BytesIO` in this RFC.
- Stream APIs must process input incrementally and reject non-positive chunk sizes.
- `Codec` and `CompressionError` are top-level shared vocabulary for autodetection and error handling.
- Compression level is a portable integer request, with `None` selecting the codec default.
- Unsupported levels and unsupported level options fail through stable `CompressionError` categories.
- Codec autodetection is in scope only via explicit opt-in APIs.
- Normal codec operations remain explicit and never rely on hidden autodetection.
- Autodetection uses signatures and framing checks, not file extensions.
- The `allowed` autodetection list is binding and must not be bypassed.
- Snappy support is framed-format-first in core stdlib.
- Raw Snappy is available as an advanced `snappy.raw` surface and is not part of autodetection.
- Archive container formats remain out of scope for this RFC.
