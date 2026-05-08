# RFC 065: `std.hash` — stable hashing primitives for data and integrity workflows

- **Status:** Planned
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized numeric types)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
    - RFC 064 (`std.encoding` binary-text encoding and decoding)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/343
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC standardizes `std.hash` as Incan's standard library module for stable hashing over bytes, files, and binary readers. The module defines explicit algorithm namespaces, separates cryptographic hashes from non-cryptographic fast hashes, and provides consistent one-shot, incremental, and file/reader helper APIs.

## Motivation

Hashing is foundational in analytics, data pipelines, and systems tooling: checksums, content addressing, deduplication keys, integrity checks, and reproducible fingerprints all depend on it. Without a standard module, projects duplicate hash wrappers and make inconsistent algorithm choices.

## Goals

- Provide a standard hash surface in `std.hash`.
- Separate cryptographic and non-cryptographic hash families explicitly.
- Define the initial algorithm namespaces and baseline functions each namespace must expose.
- Include MD5 for interoperability and file-fingerprint/checksum workflows, with explicit non-security positioning.
- Support one-shot and incremental update/finalize workflows.
- Include first-class file/stream hashing helpers in addition to incremental hashing APIs.
- Keep output representation explicit: cryptographic digests are `bytes`, non-cryptographic hashes additionally expose width-specific integer helpers, and text rendering composes through `std.encoding`.

## Non-Goals

- Replacing full cryptography/key-management modules.
- Standardizing password hashing APIs in this RFC.
- Standardizing CRC/Adler checksum algorithms in this RFC.
- Hiding algorithm choice behind implicit defaults in high-stakes contexts.
- Defining keyed MACs, signatures, or authenticated encryption.
- Exposing backend crate names or backend-specific option objects as part of the public Incan contract.

## Guide-level explanation

```incan
from std.hash import sha256
from std.encoding import hex

digest = sha256.digest(payload)
println(hex.encode(digest))
```

```incan
from std.hash import xxh3_64

h = xxh3_64.new()
h.update(chunk1)
h.update(chunk2)
value = h.finalize_u64()
println(value)
```

```incan
from std.hash import file_digest
from std.encoding import hex
from std.fs import Path

digest = file_digest(Path("events.parquet"), "sha256")?
println(hex.encode(digest))
```

## Reference-level explanation

### Module scope

`std.hash` must expose algorithm-specific namespaces with a shared shape. The initial namespaces are:

- cryptographic namespaces: `sha224`, `sha256`, `sha384`, `sha512`, `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512`, `shake128`, `shake256`, `blake2b`, `blake2s`, `blake3`, and `md5`;
- non-cryptographic namespaces: `xxh3_64`, `xxh3_128`, `xxh64`, and `xxh32`;
- top-level helpers for file and binary-reader hashing.

Family grouping namespaces such as `std.hash.sha2`, `std.hash.sha3`, and `std.hash.xxh` may be added as re-export convenience surfaces, but the algorithm namespaces above are the required import targets.

### Core model

Each algorithm namespace must support:

- one-shot hashing over `bytes`;
- incremental hasher construction with `new`, `update`, and finalization;
- explicit output type: `bytes` for digest output and fixed-width unsigned integers for non-cryptographic integer helpers;
- deterministic, portable output for identical input across supported platforms.

Baseline cryptographic and MD5 namespace shape:

- `digest(data: bytes) -> bytes`
- `new() -> Hasher`
- `Hasher.update(chunk: bytes) -> None`
- `Hasher.finalize_bytes() -> bytes`

Baseline SHAKE namespace shape:

- `digest(data: bytes, length: int) -> Result[bytes, HashError]`
- `new() -> Hasher`
- `Hasher.update(chunk: bytes) -> None`
- `Hasher.finalize_bytes(length: int) -> Result[bytes, HashError]`

Baseline non-cryptographic namespace shape:

- `digest(data: bytes) -> bytes`
- `hash_u32(data: bytes) -> u32` where the algorithm width is 32-bit
- `hash_u64(data: bytes) -> u64` where the algorithm width is 64-bit
- `hash_u128(data: bytes) -> u128` where the algorithm width is 128-bit
- `new() -> Hasher`
- `Hasher.update(chunk: bytes) -> None`
- `Hasher.finalize_bytes() -> bytes`
- `Hasher.finalize_u32() -> u32` where the algorithm width is 32-bit
- `Hasher.finalize_u64() -> u64` where the algorithm width is 64-bit
- `Hasher.finalize_u128() -> u128` where the algorithm width is 128-bit

Width-specific integer helpers must only exist for matching non-cryptographic algorithms. For example, `xxh3_64.finalize_u64()` is valid and `sha256.finalize_u64()` is not part of this RFC.

### File and reader helpers

`std.hash` must provide top-level helpers that stream input through the same algorithm implementations used by one-shot and incremental APIs:

- `file_digest(input: Path | File, algorithm: str, chunk_size: int = 65536) -> Result[bytes, HashError]`
- `reader_digest(input: BinaryReader, algorithm: str, chunk_size: int = 65536) -> Result[bytes, HashError]`
- `file_hash_u32(input: Path | File, algorithm: str, chunk_size: int = 65536) -> Result[u32, HashError]`
- `file_hash_u64(input: Path | File, algorithm: str, chunk_size: int = 65536) -> Result[u64, HashError]`
- `file_hash_u128(input: Path | File, algorithm: str, chunk_size: int = 65536) -> Result[u128, HashError]`
- `reader_hash_u32(input: BinaryReader, algorithm: str, chunk_size: int = 65536) -> Result[u32, HashError]`
- `reader_hash_u64(input: BinaryReader, algorithm: str, chunk_size: int = 65536) -> Result[u64, HashError]`
- `reader_hash_u128(input: BinaryReader, algorithm: str, chunk_size: int = 65536) -> Result[u128, HashError]`

The `algorithm` string must match one of the required algorithm namespace names. Unknown algorithm names, unsupported integer widths, invalid SHAKE output lengths, invalid chunk sizes, and I/O failures must produce `HashError`.

### API shape policy

The module must avoid hidden global defaults. Callers choose algorithms explicitly by importing an algorithm namespace or passing an algorithm name to file/reader helpers. `std.hash` must not provide a generic `hash(data)` helper that chooses an algorithm implicitly.

## Design details

### Family separation

The docs and namespace must make security posture obvious:

- cryptographic hashes for integrity/security-sensitive digests where a hash function is appropriate;
- non-cryptographic hashes for speed-oriented partitioning or hash-key workflows.
- MD5 documented as interoperability/checksum oriented and unsuitable for collision-resistant security usage.

### Initial algorithm set

`std.hash` commits to the following initial algorithm set:

- cryptographic: `sha2` (224/256/384/512), `sha3` (224/256/384/512), `shake` (128/256), `blake2b`, `blake2s`, `blake3`, `md5`;
- non-cryptographic: `xxh3_64`, `xxh3_128`, `xxh64`, `xxh32`.

The required public names are the algorithm namespace names listed in the Module scope section. Family labels such as `sha2`, `sha3`, `shake`, and `xxh` describe documentation grouping and optional re-export modules; they are not substitutes for the required per-algorithm namespaces.

### Configurable algorithms

The initial `blake2b` and `blake2s` surfaces must expose their standard unkeyed digest sizes by default (`64` bytes for `blake2b`, `32` bytes for `blake2s`). Optional digest-size configuration may be added as explicit parameters, but keyed hashing is out of scope for this RFC because it crosses into MAC/key-management concerns.

`shake128` and `shake256` must require an explicit output length for one-shot and finalization calls. The implementation must reject non-positive lengths and lengths that cannot be represented safely by the backend.

### MD5 safety signaling

MD5 remains part of `std.hash` for practical ecosystem interoperability. Documentation and examples must identify MD5 as unsuitable for collision-resistant security usage. Compiler or runtime warning behavior is not part of the public contract in this RFC.

### Checksum boundary

CRC/Adler checksums are intentionally out of scope for this RFC and belong in a future dedicated checksum-focused RFC or module.

### Output policy

Raw digest bytes are the core output. Text rendering (`hex`) composes via `std.encoding` instead of being duplicated in every hash API.

### Finalize result policy

`std.hash` follows a Python-aligned shape for cryptographic hashes and an analytics-friendly shape for non-cryptographic hashes:

- cryptographic and MD5 hashers expose byte-digest finalization (`finalize_bytes`, plus hex via `std.encoding`);
- non-cryptographic hashers expose both byte finalization and typed integer helpers where algorithm width makes this natural (for example `finalize_u32`, `finalize_u64`, `finalize_u128`).

### File and stream helpers

`std.hash` includes first-class helpers for hashing files and readers directly, aligned with Python ergonomics while remaining explicit:

- `file_digest(input, algorithm)` hashes a `std.fs.Path` or `std.fs.File` and returns digest bytes;
- `reader_digest(input, algorithm)` hashes a binary reader and returns digest bytes;
- width-specific `file_hash_u*` and `reader_hash_u*` helpers return typed integers only for matching non-cryptographic algorithms;
- algorithm selection is explicit by constructor or algorithm name string;
- helpers are convenience APIs over the same deterministic incremental hashing model, not a separate semantics path.

These helpers must process input incrementally and must not require full input materialization. `chunk_size` defaults to `65536` bytes and must be positive.

### Error model

`HashError` must represent:

- unknown algorithm names;
- unsupported output width requests;
- invalid SHAKE output lengths;
- invalid chunk sizes;
- I/O errors while opening or reading files/readers;
- backend hashing failures if an implementation backend can fail after validation.

Algorithm-specific backend details may be preserved as metadata, but the language-level error type must remain stable.

### Stability and portability

Algorithm outputs must be deterministic and portable across platforms for identical input. Non-cryptographic integer helpers must define byte order for byte conversion; this RFC requires little-endian interpretation for integer helpers whose backend exposes bytes rather than native integers.

## Alternatives considered

1. **Single `hash(data)` helper**
   - Too ambiguous and unsafe; hides algorithm choice.

2. **Only cryptographic hashes**
   - Too narrow for analytics and high-throughput data engineering workflows.

3. **Only fast hashes**
   - Too weak for integrity-sensitive use cases.

4. **File helpers returning mutable hasher objects**
   - Too error-prone for a completed file operation. The final digest is the normal result of hashing a whole file; callers who need custom interleaving can use `new`, `update`, and `finalize_*` directly.

## Drawbacks

- Surface can sprawl if too many algorithms are included too early.
- Family separation requires careful docs to avoid misuse.
- Dynamic algorithm-name helpers introduce runtime errors for invalid names, so static algorithm namespaces remain the preferred normal API.

## Layers affected

- **Stdlib / runtime**: algorithm implementations, hasher objects, file/reader helpers, and stable output behavior.
- **Language surface**: the required algorithm namespaces, hasher types, helper functions, and `HashError` must be available as specified.
- **Execution handoff**: implementations must preserve deterministic hashing semantics and stream input without whole-file materialization.
- **Docs / examples**: algorithm selection guidance, MD5 misuse avoidance, digest-vs-text encoding guidance, and file/reader examples.

## Design Decisions

- `std.hash` includes both cryptographic and non-cryptographic hash families in one module, with clear API-level separation between them.
- The core cryptographic set is `sha224`, `sha256`, `sha384`, `sha512`, `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512`, `shake128`, `shake256`, `blake2b`, `blake2s`, `blake3`, and `md5`.
- The core non-cryptographic set is `xxh3_64`, `xxh3_128`, `xxh64`, and `xxh32`.
- Required import targets are per-algorithm namespaces; family grouping namespaces are optional re-export conveniences.
- Algorithm namespaces expose one-shot `digest`, incremental `new`/`update`/`finalize_*`, and width-specific integer helpers where applicable.
- MD5 is part of the main `std.hash` surface for interoperability and file-fingerprint workflows; the spec does not relegate it to a separate legacy namespace.
- MD5 is explicitly non-security-positioned in the spec, but any runtime warning behavior is implementation detail rather than part of the public contract.
- CRC and Adler-family checksum algorithms are out of scope for this RFC.
- The public API includes both one-shot hashing helpers and incremental hasher objects.
- The public API includes first-class file and reader hashing helpers rather than forcing all file hashing to be manually composed from `std.fs` reads plus incremental updates.
- Cryptographic hashes are bytes-first and expose digest bytes as the primary finalized representation.
- Non-cryptographic hashes also expose integer-oriented finalize helpers (`u32`, `u64`, `u128` where applicable) for analytics and systems workflows that want numeric hash outputs directly.
- Hex rendering is convenience surface layered through explicit helpers and must not obscure the distinction between raw hash bytes and text encodings.
- File and reader helpers return finalized digest bytes or typed integer values, not mutable hasher objects.
- `HashError` is the stable error boundary for invalid algorithm names, unsupported widths, invalid SHAKE lengths, invalid chunk sizes, I/O failures, and backend failures.
