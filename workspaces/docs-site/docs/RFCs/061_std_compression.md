# RFC 061: `std.compression` — codec-based compression and decompression

- **Status:** Draft
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
- **Shipped in:** —

## Summary

This RFC proposes `std.compression` as Incan's standard library module for byte-oriented compression and decompression. The module standardizes a codec-submodule surface for common codecs, supports both one-shot (`bytes -> bytes`) and streaming workflows, and keeps codec autodetection explicit and opt-in. The goal is to make compression a first-class Incan capability without leaking backend crate APIs into the language contract.

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

`std.compression` should provide:

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

### Expected capability areas

The contract should cover:

- compression and decompression over `bytes`;
- streaming compression and decompression without forcing full in-memory materialization;
- codec-accurate error reporting for invalid data and truncated input;
- explicit codec naming in normal workflows;
- explicit opt-in autodetection only through dedicated functions.

### API direction

The surface should be codec-submodule-first and consistent across codecs.

Per-codec baseline APIs:

- `compress(data: bytes, level: int = default) -> Result[bytes, CompressionError]`
- `decompress(data: bytes) -> Result[bytes, CompressionError]`
- `compress_stream(source: File | BytesIO, target: File | BytesIO, level: int = default, chunk_size: int = 65536) -> Result[None, CompressionError]`
- `decompress_stream(source: File | BytesIO, target: File | BytesIO, chunk_size: int = 65536) -> Result[None, CompressionError]`

Top-level autodetection APIs:

- `decompress_auto(data: bytes, allowed: List[Codec] = Codec.all()) -> Result[(Codec, bytes), CompressionError]`
- `decompress_auto_stream(source: File | BytesIO, target: File | BytesIO, allowed: List[Codec] = Codec.all(), chunk_size: int = 65536) -> Result[Codec, CompressionError]`

Autodetection should not be coupled to file extensions. It should use codec signatures and framing checks where applicable and fail explicitly when detection is ambiguous or unsupported.

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

`bytes -> bytes` APIs are required for simple usage and small payloads. Streaming APIs are also part of the core contract because large-file and pipeline workflows are a first-class use case and should not require immediate Rust interop.

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

`std.compression.snappy` should expose:

- framed APIs as the default (`compress`, `decompress`, stream helpers);
- advanced raw APIs under a nested namespace such as `snappy.raw`.

Raw Snappy support is included for systems integrations that need block-level behavior (for example Parquet-family readers and writers), but this is intentionally not the primary path for general application compression workflows.

### Error model

`CompressionError` should represent codec and I/O failure classes cleanly, including:

- invalid or corrupted compressed data;
- truncated input;
- unsupported options or unsupported codec in autodetection;
- I/O errors in stream workflows.

Codec-specific detail can be preserved in error metadata, but the language-level error surface should remain stable and codec-neutral at the type boundary.

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

## Drawbacks

- Supporting seven codecs in core stdlib increases implementation and test surface area.
- Streaming APIs over multiple backends require careful behavior and error-contract consistency.
- Different codecs have different level/option semantics, so docs must be explicit to avoid false uniformity.

## Layers affected

- **Stdlib / runtime**: must provide codec modules and one-shot or stream behavior contracts.
- **Language surface**: the module and codec submodules must be available as specified.
- **Execution handoff**: implementations must preserve codec behavior without backend leakage.
- **Docs / examples**: must standardize bytes, stream, and autodetection usage patterns.

## Design Decisions

- `std.compression` is a dedicated codec module and is not folded into `std.io` or `std.fs`.
- The initial codec set is `gzip`, `zlib`, `deflate`, `zstd`, `bz2`, `lzma`, and `snappy`.
- The public surface is codec-submodule-based (`std.compression.gzip`, etc.).
- Core contract includes both one-shot (`bytes -> bytes`) and streaming APIs.
- Streaming APIs operate directly on `std.fs.File` and `std.io.BytesIO` in this RFC.
- Codec autodetection is in scope only via explicit opt-in APIs.
- Normal codec operations remain explicit and never rely on hidden autodetection.
- Snappy support is framed-format-first in core stdlib.
- Raw Snappy is available as an advanced `snappy.raw` surface and is not part of autodetection.
- Archive container formats remain out of scope for this RFC.
