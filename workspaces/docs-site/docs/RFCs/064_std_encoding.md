# RFC 064: `std.encoding` — binary-text encoding and decoding utilities

- **Status:** Draft
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
    - RFC 061 (`std.compression` codec-based compression and decompression)
    - RFC 065 (`std.hash` stable hashing primitives)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/342
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.encoding` as Incan's standard library module for binary-text representation transforms. The module standardizes explicit encoding and decoding across the major text-safe binary encodings, provides one-shot and streaming APIs, and keeps Python-familiar surface naming first-class while preserving strict-by-default decoding semantics.

## Motivation

Encoding transforms are core interoperability primitives for APIs, identifiers, signatures, fixture data, transport payloads, and storage boundaries. Without a standard module, teams repeatedly rebuild wrappers for the same formats and diverge on strictness, alphabets, and error behavior.

Incan should provide one coherent language-level encoding surface rather than forcing users into ad hoc helpers or backend-specific interop.

## Goals

- Provide a complete north-star binary-text encoding surface in `std.encoding`.
- Include one-shot and streaming encode/decode APIs in the contract.
- Keep strict-vs-lenient decoding behavior explicit and deterministic.
- Keep format and alphabet choices explicit where multiple variants exist.
- Make Python-familiar naming first-class to reduce adoption friction.

## Non-Goals

- Replacing cryptographic primitives (`std.hash` / `std.crypto` scope).
- Guessing encodings implicitly from payload shape.
- Defining media codecs (video/audio/image codecs).
- Defining compression codecs (handled by `std.compression`).
- Standardizing arbitrary proprietary alphabet variants with no interoperability value.

## Guide-level explanation

```incan
from std.encoding import base64, hex

token = base64.urlsafe_b64encode(payload)
raw = base64.urlsafe_b64decode(token)?

fingerprint = hex.encode(raw)
digest = hex.decode(fingerprint)?
```

```incan
from std.encoding import base58, bech32

pk = base58.b58encode(pubkey_bytes)
decoded = base58.b58decode(pk)?

human_readable, words = bech32.bech32_decode(address)?
```

```incan
from std.encoding import base64
from std.fs import Path

source = Path("payload.bin").open("rb")?
target = Path("payload.b64").open("wb")?
base64.b64encode_stream(source, target)?
```

## Reference-level explanation

### Module scope

`std.encoding` should provide:

- `std.encoding.hex` (base16)
- `std.encoding.base32`
- `std.encoding.base64`
- `std.encoding.base85`
- `std.encoding.base58`
- `std.encoding.bech32`

### Core model

- input/output types are explicit (`bytes`, `str`, stream-like values);
- decode errors are structured;
- strict decode is default;
- lenient decode is explicit and separately named;
- variant-specific encodings use explicit function families, not hidden flags.

### North-star API direction

Per encoding family, the contract should include:

- one-shot encode/decode
- streaming encode/decode
- explicit variant-specific functions where needed

Baseline shape:

- `encode(data: bytes) -> str`
- `decode(text: str) -> Result[bytes, EncodingError]`
- `decode_lenient(text: str) -> Result[bytes, EncodingError]` (where leniency is meaningful)
- `encode_stream(source: File | BytesIO, target: File | BytesIO, chunk_size: int = 65536) -> Result[None, EncodingError]`
- `decode_stream(source: File | BytesIO, target: File | BytesIO, chunk_size: int = 65536) -> Result[None, EncodingError]`

## Design details

### Initial core families (north star)

`std.encoding` includes these families:

- **hex/base16**
- **base32** (standard and explicit variant alphabets where applicable)
- **base64** (standard + URL-safe)
- **base85** variants (explicitly named families: `a85*`, `b85*`, `z85*`)
- **base58** (explicit alphabet variant naming where needed)
- **bech32** / **bech32m**

### Python familiarity

Python-shaped naming should be first-class API surface, not compatibility afterthought. Examples:

- `b64encode`, `b64decode`, `urlsafe_b64encode`, `urlsafe_b64decode`
- `b32encode`, `b32decode`
- `a85encode`, `a85decode`, `b85encode`, `b85decode`

Incan may also expose canonical parallel names (`encode`, `decode`) as long as behavior is identical and documentation keeps naming clear.

### Strictness policy

Decode behavior is strict by default:

- malformed alphabet, invalid length, invalid padding, and illegal characters produce errors.

Lenient behavior must be explicit:

- separate `*_decode_lenient` APIs rather than boolean strictness flags.

### Variant explicitness

Where formats have multiple non-interchangeable variants, the variant must be in the API name. This avoids silent ambiguity:

- Base64 standard vs URL-safe
- Base85 families (`a85`, `b85`, `z85`)
- Bech32 vs Bech32m
- Base58 alphabet variants where relevant

### Streaming support

Streaming encode/decode is part of this RFC's north-star contract, not a follow-on.

Stream APIs should compose directly with `std.fs.File` and `std.io.BytesIO` and define consistent chunking/error behavior.

### Line wrapping policy

No implicit line wrapping by default.

Legacy wrapped output (for MIME-like contexts) is explicit opt-in through dedicated APIs or options that are clearly named.

### Out-of-scope boundary

`std.encoding` is representation-focused.

Out of scope for this module:

- video/audio/image codecs
- compression codecs (`gzip`, `zstd`, etc.) and container semantics

Those belong to dedicated modules (`std.compression`, `std.archive`, and future media-focused libraries).

## Alternatives considered

1. **Minimal set only (`hex + base64`)**
   - Too small for the north star and pushes common encodings back to ecosystem fragmentation.

2. **Fold encoding into `std.io`**
   - Wrong boundary; encoding transforms are representation concerns, not cursor mechanics.

3. **Single generic encode/decode with hidden variant flags**
   - Too ambiguous and error-prone where formats have non-interchangeable variants.

## Drawbacks

- Broader family coverage increases API surface area and documentation burden.
- Variant-explicit naming is longer at call-sites.
- Streaming support requires careful contract wording around chunking and partial failure.

## Layers affected

- **Stdlib / runtime**: encoding implementations, stream adapters, and error surfaces.
- **Language surface**: the module and submodule families must be available as specified.
- **Execution handoff**: implementations must preserve deterministic transformation behavior.
- **Docs / examples**: strict/lenient guidance, variant choice guidance, and streaming patterns.

## Design Decisions

- `std.encoding` includes `hex`, `base32`, `base64`, `base85`, `base58`, and `bech32` families in the north-star contract.
- Python-shaped naming is first-class API surface.
- Strict decode is default.
- Lenient decode is explicit and separately named.
- One-shot and streaming APIs are both in scope in this RFC.
- Variant ambiguity is avoided by explicit function-family naming.
- Media codecs and compression codecs are explicitly out of scope for this module.
