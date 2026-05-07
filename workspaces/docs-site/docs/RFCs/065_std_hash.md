# RFC 065: `std.hash` — stable hashing primitives for data and integrity workflows

- **Status:** Draft
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

This RFC proposes `std.hash` as Incan's standard library module for stable hashing operations over bytes and stream-like inputs. The module should clearly separate cryptographic hashes from non-cryptographic fast hashes while keeping a consistent API shape.

## Motivation

Hashing is foundational in analytics, data pipelines, and systems tooling: checksums, content addressing, deduplication keys, integrity checks, and reproducible fingerprints all depend on it. Without a standard module, projects duplicate hash wrappers and make inconsistent algorithm choices.

## Goals

- Provide a standard hash surface in `std.hash`.
- Separate cryptographic and non-cryptographic hash families explicitly.
- Include MD5 for interoperability and file-fingerprint/checksum workflows, with explicit non-security positioning.
- Support one-shot and incremental update/finalize workflows.
- Include first-class file/stream hashing helpers in addition to incremental hashing APIs.
- Keep output representation explicit (`bytes`, optional hex helper via `std.encoding`).

## Non-Goals

- Replacing full cryptography/key-management modules.
- Standardizing password hashing APIs in this RFC.
- Standardizing CRC/Adler checksum algorithms in this RFC.
- Hiding algorithm choice behind implicit defaults in high-stakes contexts.

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

digest = file_digest(Path("events.parquet"), "sha256")
println(hex.encode(digest.finalize_bytes()))
```

## Reference-level explanation

### Module scope

`std.hash` should expose algorithm-specific submodules or constructors with a shared shape:

- cryptographic family (for example SHA-2/BLAKE3);
- non-cryptographic family (for example xxhash);
- MD5 available in the main module surface for compatibility/file hashing where collision resistance is not a requirement;

### Core model

Each algorithm surface should support:

- one-shot digest over `bytes`;
- incremental hasher object (`new`, `update`, `finalize`);
- explicit output type (`bytes` or fixed-width integer).

### API shape direction

The module should avoid hidden global defaults. Callers should choose algorithms explicitly.

## Design details

### Family separation

The docs and namespace should make security posture obvious:

- cryptographic hashes for integrity/security-sensitive digests;
- non-cryptographic hashes for speed-oriented partitioning or hash-key workflows.
- MD5 documented as interoperability/checksum oriented and unsuitable for collision-resistant security usage.

### Initial algorithm set

`std.hash` commits to the following initial algorithm set:

- cryptographic: `sha2` (224/256/384/512), `sha3` (224/256/384/512), `shake` (128/256), `blake2b`, `blake2s`, `blake3`, `md5`;
- non-cryptographic: `xxh3_64`, `xxh3_128`, `xxh64`, `xxh32`.

### MD5 safety signaling

MD5 remains part of `std.hash` for practical ecosystem interoperability. The RFC mandates clear non-security guidance in documentation and examples; whether additional compiler/runtime warning behavior exists is an implementation detail.

### Checksum boundary

CRC/Adler checksums are intentionally out of scope for this RFC and should be handled by a future dedicated checksum-focused RFC/module.

### Output policy

Raw digest bytes should be the core output. Text rendering (`hex`) should compose via `std.encoding` instead of being duplicated in every hash API.

### Finalize result policy

`std.hash` follows a Python-aligned shape for cryptographic hashes and an analytics-friendly shape for non-cryptographic hashes:

- cryptographic and MD5 hashers expose byte-digest finalization (`finalize_bytes`, plus hex via `std.encoding`);
- non-cryptographic hashers expose both byte finalization and typed integer helpers where algorithm width makes this natural (for example `finalize_u32`, `finalize_u64`, `finalize_u128`).

### File and stream helpers

`std.hash` includes first-class helpers for hashing files and readers directly, aligned with Python ergonomics while remaining explicit:

- `file_digest(input, algorithm)` hashes a `std.fs.Path`, `std.fs.File`, or binary reader and returns the algorithm hasher/digest object;
- algorithm selection is explicit by constructor or algorithm name string;
- helpers are convenience APIs over the same deterministic incremental hashing model, not a separate semantics path.

### Stability and portability

Algorithm outputs must be deterministic and portable across platforms for identical input.

## Alternatives considered

1. **Single `hash(data)` helper**
   - Too ambiguous and unsafe; hides algorithm choice.

2. **Only cryptographic hashes**
   - Too narrow for analytics and high-throughput data engineering workflows.

3. **Only fast hashes**
   - Too weak for integrity-sensitive use cases.

## Drawbacks

- Surface can sprawl if too many algorithms are included too early.
- Family separation requires careful docs to avoid misuse.

## Layers affected

- **Stdlib / runtime**: algorithm implementations and stable output behavior.
- **Language surface**: the module families, hasher types, and helper functions must be available as specified.
- **Execution handoff**: implementations must preserve deterministic hashing semantics.
- **Docs / examples**: algorithm selection guidance and misuse avoidance.

## Design Decisions

- `std.hash` includes both cryptographic and non-cryptographic hash families in one module, with clear API-level separation between them.
- The core cryptographic set is `sha2` (`sha224`, `sha256`, `sha384`, `sha512`), `sha3` (`sha3_224`, `sha3_256`, `sha3_384`, `sha3_512`), `shake` (`shake128`, `shake256`), `blake2b`, `blake2s`, `blake3`, and `md5`.
- The core non-cryptographic set is `xxh3_64`, `xxh3_128`, `xxh64`, and `xxh32`.
- MD5 is part of the main `std.hash` surface for interoperability and file-fingerprint workflows; the spec does not relegate it to a separate legacy namespace.
- MD5 is explicitly non-security-positioned in the spec, but any runtime warning behavior is implementation detail rather than part of the public contract.
- CRC and Adler-family checksum algorithms are out of scope for this RFC.
- The public API includes both one-shot hashing helpers and incremental hasher objects.
- The public API includes first-class file or stream hashing helpers rather than forcing all file hashing to be manually composed from `std.fs` reads plus incremental updates.
- Cryptographic hashes are bytes-first and expose digest bytes as the primary finalized representation.
- Non-cryptographic hashes also expose integer-oriented finalize helpers (`u32`, `u64`, `u128` where applicable) for analytics and systems workflows that want numeric hash outputs directly.
- Hex rendering is convenience surface layered through explicit helpers and must not obscure the distinction between raw hash bytes and text encodings.
