# RFC 064: `std.encoding` — binary-text encoding and decoding utilities

- **Status:** In Progress
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

This RFC proposes `std.encoding` as Incan's standard library module for binary-text representation transforms. The module standardizes explicit encoding and decoding across the major text-safe binary encodings, provides value and finite source/sink APIs through the same canonical verbs, and keeps Python-familiar surface naming first-class while preserving strict-by-default decoding semantics.

## Motivation

Encoding transforms are core interoperability primitives for APIs, identifiers, signatures, fixture data, transport payloads, and storage boundaries. Without a standard module, teams repeatedly rebuild wrappers for the same formats and diverge on strictness, alphabets, and error behavior.

Incan should provide one coherent language-level encoding surface rather than forcing users into ad hoc helpers or backend-specific interop.

## Goals

- Provide a complete north-star binary-text encoding surface in `std.encoding`.
- Include value and finite source/sink encode/decode APIs in the contract.
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

fingerprint = hex.hexlify(raw)
digest = hex.unhexlify(fingerprint)?
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

source = Path("payload.bin")
target = Path("payload.b64")
base64.encode(source, target)?
```

## Reference-level explanation

### Module scope

`std.encoding` must provide:

- `std.encoding.hex` (base16)
- `std.encoding.base32`
- `std.encoding.base64`
- `std.encoding.base85`
- `std.encoding.base58`
- `std.encoding.bech32`

### Core model

- encode APIs accept in-memory `bytes` or a finite binary source and write encoded ASCII bytes to a finite binary sink;
- decode APIs accept in-memory `str`, encoded ASCII stream bytes, or an encoded path source and write decoded bytes to a finite binary sink;
- decode errors are structured;
- strict decode is default;
- lenient decode is explicit and separately named;
- variant-specific encodings use explicit function families, not hidden flags.

### North-star API direction

Per encoding family, the contract must include:

- Python-shaped one-shot value helpers where the ecosystem has established names
- source/sink encode/decode through the same canonical verb
- explicit variant-specific functions where needed

Baseline shape:

- `encode(source: bytes | Path | BytesIO, target: Path | BytesIO, chunk_size: int = 65536) -> Result[str, EncodingError]`
- `decode(source: str | Path | BytesIO, target: Path | BytesIO, chunk_size: int = 65536) -> Result[bytes, EncodingError]`
- `decode_lenient(text: str) -> Result[bytes, EncodingError]` where leniency is meaningful
- value helpers such as `hexlify(data)`, `unhexlify(text)`, `b64encode(data)`, and `b64decode(text)` where names are already conventional

The source/sink form is the I/O primitive. A finite file path is just a bounded stream endpoint: implementations open it and flow through the same transform path instead of adding a separate batch-file algorithm. Encoders write ASCII bytes so callers can pipe encoded output to files and transports without requiring a text-mode writer. Decoders must reject non-ASCII input bytes before alphabet-specific validation.

## Design details

### Initial core families (north star)

`std.encoding` includes these families:

- **hex/base16** with lowercase output from `hex.encode` and strict decode requiring even-length ASCII hex input;
- **base32** with standard Base32 and explicit extended-hex helpers where implemented;
- **base64** with standard and URL-safe helpers;
- **base85** variants using explicitly named families: `a85*`, `b85*`, and `z85*`;
- **base58** with Bitcoin-alphabet `b58*` as the baseline surface and explicit names for any additional alphabets;
- **bech32** and **bech32m** helpers that keep human-readable part and payload words explicit.

### Python familiarity

Python-shaped naming must be first-class API surface, not compatibility afterthought. Examples:

- `b64encode`, `b64decode`, `urlsafe_b64encode`, `urlsafe_b64decode`
- `b32encode`, `b32decode`
- `a85encode`, `a85decode`, `b85encode`, `b85decode`

Canonical names (`encode`, `decode`) should be the preferred API in new docs. Python-shaped names remain first-class compatibility spellings for variant clarity and migration familiarity.

### Strictness policy

Decode behavior is strict by default:

- malformed alphabet, invalid length, invalid padding, and illegal characters produce errors.

Lenient behavior must be explicit:

- separate `*_decode_lenient` APIs rather than boolean strictness flags.

Lenient APIs may normalize ASCII whitespace and common case differences only when that behavior is documented for the family. They must not silently switch alphabets, ignore checksum failures, or recover from truncated input in a way that changes the decoded bytes.

### Variant explicitness

Where formats have multiple non-interchangeable variants, the variant must be in the API name. This avoids silent ambiguity:

- Base64 standard vs URL-safe
- Base85 families (`a85`, `b85`, `z85`)
- Bech32 vs Bech32m
- Base58 alphabet variants where relevant

Default helper names are allowed only when the default is a widely recognized family baseline. For this RFC, `b64*` is standard Base64, `urlsafe_b64*` is URL-safe Base64, `b58*` is Bitcoin-alphabet Base58, and Bech32m must not be hidden behind plain `bech32_*` names.

### Streaming support

Streaming encode/decode is part of this RFC's north-star contract, not a follow-on.

The canonical encode/decode APIs must compose directly with `std.fs.Path` and `std.io.BytesIO` and define consistent chunking/error behavior. Path values are opened as finite binary sources or sinks and use the same source/sink transform path as in-memory byte streams.

Partial source/sink failures must be reported as errors. Implementations may have already written earlier chunks to the target by the time an error occurs, so the docs must not promise transactional output.

### Error model

`EncodingError` must be the stable error type for this module. It must distinguish at least:

- invalid alphabet character;
- invalid length;
- invalid or missing padding where padding is required;
- checksum or separator failure for checksum-bearing formats such as Bech32;
- non-ASCII representation bytes in source/sink decode;
- I/O failure in source/sink workflows.

Family-specific detail may be carried as metadata, but callers must be able to handle the stable error categories without depending on backend crate messages.

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

- **Stdlib / runtime**: encoding implementations, source/sink adapters, and error surfaces.
- **Language surface**: the module and submodule families must be available as specified.
- **Execution handoff**: implementations must preserve deterministic transformation behavior.
- **Docs / examples**: strict/lenient guidance, variant choice guidance, and source/sink patterns.

## Implementation Plan

### Phase 1: Registry + module scaffolding

- Register `std.encoding` and its submodules in the stdlib namespace registry.
- Add `.incn` source modules for `encoding/prelude`, `hex`, `base32`, `base64`, `base85`, `base58`, and `bech32`.
- Define the source-owned `EncodingError` boundary and shared helpers in Incan.

### Phase 2: Pure Incan one-shot algorithms

- Implement strict one-shot encode/decode for the required encoding families in `.incn` source.
- Keep Python-shaped helper names first-class and map canonical helper names to the same behavior.
- Implement lenient decode helpers only where the normalization rule is documented and deterministic.
- Avoid `rust.extern` type implementations for the encoding API surface; if a compiler/runtime limitation blocks pure Incan implementation, record the limitation and keep the public contract source-owned.

### Phase 3: Stream composition

- Implement source/sink helpers over `std.fs.Path` and `std.io.BytesIO` by composing the encoding algorithms with existing binary read/write APIs.
- Preserve the RFC's non-transactional partial-write behavior for source/sink errors.
- Reject non-ASCII representation bytes before alphabet-specific source/sink decode validation.

### Phase 4: Tests, docs, and release metadata

- Add focused stdlib registry, typechecker, codegen, and end-to-end tests for imports, value behavior, strict errors, lenient behavior, variants, and source/sink calls.
- Add user-facing stdlib reference docs and examples for `std.encoding`.
- Update release notes and development version metadata for the active `0.3` line.

## Progress Checklist

### Spec / lifecycle

- [x] Move RFC 064 to `Planned` after design review.
- [x] Move RFC 064 to `In Progress` when implementation starts.
- [x] Keep the RFC checklist aligned with completed implementation slices.

### Registry / source modules

- [x] Register `std.encoding` with required submodules in `STDLIB_NAMESPACES`.
- [x] Add source-owned `std.encoding` prelude and shared error/helper surface.
- [x] Add `hex`, `base32`, `base64`, `base85`, `base58`, and `bech32` `.incn` modules.

### One-shot algorithms

- [x] Implement strict hex/base16 encode and decode.
- [x] Implement strict Base32 and documented lenient Base32 decode behavior.
- [x] Implement strict standard and URL-safe Base64 helpers.
- [x] Implement Base85 family helpers for `a85`, `b85`, and `z85`.
- [x] Implement Bitcoin-alphabet Base58 helpers.
- [x] Implement Bech32 and Bech32m helpers with checksum validation.

### Streams

- [x] Implement `std.io.BytesIO` source/sink encode helpers for byte-oriented supported families: hex, Base32, Base64, Base85, and Base58.
- [x] Implement `std.io.BytesIO` source/sink decode helpers for byte-oriented supported families: hex, Base32, Base64, Base85, and Base58.
- [x] Implement `std.fs.Path` finite source/sink helpers by opening binary files and using the same transform path.
- [x] Cover `std.io.BytesIO` source/sink composition in RFC behavior tests.
- [x] Cover `std.fs.Path` source/sink composition in RFC behavior tests.

### Tests

- [x] Add registry/typechecker coverage for `std.encoding` imports and unknown submodules.
- [x] Add codegen/stdlib source snapshot coverage for the new modules.
- [x] Add end-to-end one-shot tests for representative valid values.
- [ ] Add strict error tests for invalid alphabet, length, padding, checksum, and non-ASCII source input. Current coverage includes invalid length, invalid character, and Bech32 checksum paths.
- [x] Add lenient decode tests where leniency is exposed.
- [x] Add source/sink round-trip tests for the implemented byte-oriented surfaces.

### Docs / release

- [x] Add authored `std.encoding` reference docs.
- [x] Link `std.encoding` from stdlib reference navigation/index pages.
- [x] Add `0.3` release notes entry.
- [x] Bump the active development version from `0.3.0-dev.44`.

## Design Decisions

- `std.encoding` includes `hex`, `base32`, `base64`, `base85`, `base58`, and `bech32` families in the north-star contract.
- Python-shaped naming is first-class API surface.
- Strict decode is default.
- Lenient decode is explicit and separately named.
- Value convenience and finite source/sink APIs are both in scope in this RFC.
- Variant ambiguity is avoided by explicit function-family naming.
- Source/sink APIs operate on binary streams and use ASCII representation bytes for encoded text.
- `EncodingError` is the stable module error type and must expose family-neutral error categories.
- Media codecs and compression codecs are explicitly out of scope for this module.
