# RFC 056: `std.io` — in-memory byte streams and binary parsing helpers

- **Status:** Planned
- **Created:** 2026-04-13
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 009 (sized integers)
    - RFC 010 (temporary filesystem objects)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/291
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces `std.io` as Incan's in-memory binary I/O module. Its core abstraction is `BytesIO`, a writable and seekable byte stream over an in-memory `bytes` buffer for binary parsing, protocol work, fixtures, and transformation pipelines that should not depend on filesystem paths. The user-facing shape is intentionally recognizable to Python users coming from `io.BytesIO`, while the underlying semantics also take advantage of Rust's `Cursor` and `BufRead` model where that produces a cleaner complete contract for cursor movement, delimiter-based reads, and exact-width numeric helpers.

## Motivation

Not all binary input starts as a file on disk. Tests, network clients, generated fixtures, embedded assets, decompression stages, and parser pipelines often begin with a `bytes` value that is already in memory. Those users should not need to import `std.fs` or drop into `rust::std::io::Cursor` just to move a cursor, read exact byte counts, skip until a delimiter, or decode fixed-width numbers. `std.io` gives Incan a standard home for that in-memory story while staying separate from path and OS-file concerns. That separation matters for real binary formats: `std.fs` should get bytes into memory or stream them from a file, while `std.io.BytesIO` should make the parsing and re-encoding work itself possible in pure Incan.

## Goals

- Provide a `BytesIO`-like type over `bytes` for in-memory binary parsing and rewriting.
- Standardize cursor, exact-read, delimiter-read, and overwrite semantics so users do not hand-roll slicing logic for every parser.
- Commit to a complete exact-width numeric read and write surface aligned with RFC 009, including both endian families for multi-byte values.
- Keep `std.io` independent from filesystem path APIs; users should be able to parse a `bytes` value without importing `std.fs`.
- Make the pure-Incan binary parsing story strong enough for real format readers such as GGUF-style metadata and tensor-descriptor parsers.

## Non-Goals

- Standardizing filesystem file handles; that belongs to RFC 055.
- Defining async networking or async stream protocols here.
- Mirroring Python's entire `io` hierarchy.
- Introducing spill-to-disk behavior in `BytesIO`; spooled temporary storage belongs in `std.tempfile`, not here.
- Defining general `Reader` / `Writer` protocol families in this RFC.
- Reproducing Rust `Read` / `Seek` / `BufRead` trait names one-to-one as the user-facing surface.

## Guide-level explanation

Authors use `std.io.BytesIO` when they already have a `bytes` value and want to parse or rewrite it incrementally.

```incan
from std.io import BytesIO

buf = BytesIO(data)
magic = buf.read_exact(4)?
version = buf.read_u32_le()?
metadata_count = buf.read_u64_le()?

payload = buf.read_until(0u8)?
remaining = buf.remaining()
```

`BytesIO` is also writable. It overwrites from the current cursor position unless the caller explicitly seeks elsewhere first.

```incan
from std.io import BytesIO

out = BytesIO()
out.write(b"GGUF")?
out.write_u32_le(3u32)?
out.write_u64_le(42u64)?

blob = out.into_bytes()
```

The mental model is: `std.fs` gets bytes into or out of files, while `std.io` walks through and rewrites bytes already in memory.

## Reference-level explanation

### Module split and compatibility target

- The standard library must expose `std.io` for in-memory byte-stream reading, writing, and cursor semantics.
- `std.io` is deliberately separate from `std.fs`: open OS-file handles belong to the filesystem module, while `BytesIO` operates on already-materialized `bytes`.
- The surface should be recognizable to Python users coming from `io.BytesIO`, but the committed contract is broader than Python's minimal cursor methods because Incan also standardizes explicit numeric parsing helpers.
- The committed numeric helper surface depends on RFC 009. Width-specific reads and writes must use the sized numeric vocabulary introduced there.
- Implementations may use Rust `std::io::Cursor`, `BufRead`, and primitive byte-conversion helpers internally, but user-visible semantics are defined by this RFC and stdlib docs, not by Rust trait names.

### Required capabilities (committed contract)

The `std.io` contract commits to the following `BytesIO` surface:

- Direct construction: `BytesIO(initial: bytes = b"") -> BytesIO`.
- Byte reads: `read(size: int = -1) -> Result[bytes, E]`, `read_exact(size: int) -> Result[bytes, E]`.
- Delimiter helpers: `read_until(byte: u8) -> Result[bytes, E]`, `skip_until(byte: u8) -> Result[int, E]`.
- Cursor helpers: `tell() -> int`, `seek(offset: int, whence: int = 0) -> Result[int, E]`, `rewind() -> Result[(), E]`, `seek_relative(offset: int) -> Result[(), E]`.
- Byte writes: `write(data: bytes) -> Result[int, E]`, `truncate(size: int | None = None) -> Result[int, E]`.
- Buffer extraction and inspection: `getvalue() -> bytes`, `into_bytes() -> bytes`, `remaining() -> int`.
- Exact-width numeric reads and writes aligned with RFC 009.

### Normative cursor and buffer semantics

Cursor behavior is normative:

- A newly constructed `BytesIO(initial)` starts with its cursor at position `0`.
- `read(size)` must return at most `size` bytes and must advance the cursor by the returned byte count.
- `read(size)` with `size = -1` must return the remaining bytes.
- `read(size)` at EOF must return an empty `bytes` value.
- `read_exact(size)` must fail if fewer than `size` bytes remain.
- `seek(offset, whence)` must follow the Python-style `whence` model: `0` for start, `1` for current position, and `2` for end.
- `rewind()` is the convenience form of seeking to the start of the buffer.
- `seek_relative(offset)` moves relative to the current cursor position and must fail if the resulting position would be invalid.

Write behavior is also normative:

- `BytesIO` is always readable, writable, and seekable; separate `readable()` / `writable()` / `seekable()` predicates are not part of the committed surface.
- `write(data)` writes from the current cursor position. It does not imply append semantics unless the caller has already moved the cursor to the end.
- `write(data)` must either write the full byte slice or fail. Partial-write behavior is not part of the user-visible contract for an in-memory buffer.
- `truncate(size=None)` must shrink or extend the buffer to `size`; when `size` is omitted, it uses the current cursor position.

Delimiter behavior is normative:

- `read_until(byte)` must return bytes up to and including the delimiter when the delimiter is found.
- `read_until(byte)` must return the remaining bytes when EOF is reached before the delimiter.
- `skip_until(byte)` must discard bytes until the delimiter or EOF and return the total number of discarded bytes, including the delimiter when it is found.
- `read_until(byte)` and `skip_until(byte)` must return `0`-length / `0`-count results at EOF.

Buffer extraction behavior is normative:

- `getvalue()` returns a `bytes` snapshot of the buffer contents.
- `into_bytes()` consumes the `BytesIO` value and returns the underlying `bytes` without promising a copy.
- `remaining()` returns the number of unread bytes from the current cursor position to the logical end of the buffer.

### Numeric helper surface

The numeric helper surface is committed, not tentative:

- One-byte reads and writes: `read_u8() -> Result[u8, E]`, `read_i8() -> Result[i8, E]`, `write_u8(value: u8) -> Result[(), E]`, `write_i8(value: i8) -> Result[(), E]`.
- Unsigned integer reads and writes for `u16`, `u32`, `u64`, and `u128` in both endian families: `read_u16_le`, `read_u16_be`, `read_u32_le`, `read_u32_be`, and so on through `u128`; matching `write_u16_le`, `write_u16_be`, `write_u32_le`, `write_u32_be`, and so on through `u128`.
- Signed integer reads and writes for `i16`, `i32`, `i64`, and `i128` in both endian families: `read_i16_le`, `read_i16_be`, `read_i32_le`, `read_i32_be`, and so on; matching write helpers.
- Floating-point reads and writes for `f32` and `f64` in both endian families: `read_f32_le`, `read_f32_be`, `read_f64_le`, `read_f64_be`; matching write helpers.
- Endianness suffixes are not used for `u8` and `i8`, because byte order is meaningless for one-byte values.
- Convenience aliases for Incan defaults are part of the surface: `read_int_le`, `read_int_be`, `write_int_le`, `write_int_be`, `read_float_le`, `read_float_be`, `write_float_le`, and `write_float_be`.
- RFC 009 defines `int` as the ergonomic alias for `i64` and `float` as the ergonomic alias for `f64`, so those convenience helpers are aliases for the corresponding `i64` and `f64` forms rather than independent numeric contracts.

### Expected API shape (skeletal)

#### `BytesIO`

- `BytesIO(initial: bytes = b"") -> BytesIO`.
- `read(size: int = -1) -> Result[bytes, E]`.
- `read_exact(size: int) -> Result[bytes, E]`.
- `read_until(byte: u8) -> Result[bytes, E]`.
- `skip_until(byte: u8) -> Result[int, E]`.
- `tell() -> int`.
- `seek(offset: int, whence: int = 0) -> Result[int, E]`.
- `rewind() -> Result[(), E]`.
- `seek_relative(offset: int) -> Result[(), E]`.
- `write(data: bytes) -> Result[int, E]`.
- `truncate(size: int | None = None) -> Result[int, E]`.
- `getvalue() -> bytes`.
- `into_bytes() -> bytes`.
- `remaining() -> int`.
- `read_u8() -> Result[u8, E]`, `read_i8() -> Result[i8, E]`.
- `read_u16_le() -> Result[u16, E]`, `read_u16_be() -> Result[u16, E]`, and corresponding helpers for `u32`, `u64`, and `u128`.
- `read_i16_le() -> Result[i16, E]`, `read_i16_be() -> Result[i16, E]`, and corresponding helpers for `i32`, `i64`, and `i128`.
- `read_f32_le() -> Result[f32, E]`, `read_f32_be() -> Result[f32, E]`, `read_f64_le() -> Result[f64, E]`, `read_f64_be() -> Result[f64, E]`.
- Matching `write_*` helpers for every committed numeric read helper.
- `read_int_le() -> Result[int, E]`, `read_int_be() -> Result[int, E]`, `read_float_le() -> Result[float, E]`, `read_float_be() -> Result[float, E]`.
- `write_int_le(value: int) -> Result[(), E]`, `write_int_be(value: int) -> Result[(), E]`, `write_float_le(value: float) -> Result[(), E]`, `write_float_be(value: float) -> Result[(), E]`.

### Errors and compatibility

- Operations must surface failure through ordinary `Result` returns unless a helper is explicitly documented otherwise.
- Error payloads should be actionable, including at minimum the failed operation, the requested size or delimiter where relevant, and the cursor position when that improves debugging.
- This RFC is additive. It does not change existing filesystem or builtin contracts.
- `std.io` helpers must not require `rust::` knowledge in ordinary documentation or examples.
- If RFC 009's sized numeric model changes materially before implementation, the width-specific helper signatures in this RFC must be updated to match that final language contract rather than silently drifting.

## Design details

### Why `std.io` is separate from `std.fs`

`BytesIO` solves a different problem than file handles. It helps when the bytes are already in memory. That includes tests, network payloads, decompressed buffers, and parser stages after a file has already been read. Keeping `std.io` separate avoids turning the filesystem module into a generic "everything binary" bucket.

### Python-shaped surface, Rust-backed semantics

The surface should feel familiar to Python users: `BytesIO(data)`, `read`, `write`, `tell`, `seek`, and `getvalue` are all recognizable from `io.BytesIO`. But the substrate is Rust, and Rust gives a few extra semantics that are worth standardizing instead of hiding.

Rust's `Cursor` model is the reason `BytesIO` should be treated as a real writable stream instead of a read-only parser shim. A new cursor starts at the beginning, not the end, and writes overwrite from the current cursor position rather than implying append behavior. Rust also makes `rewind()` and `seek_relative(...)` natural convenience operations, so Incan should expose them instead of forcing callers to encode every cursor move through raw `seek(...)` calls.

Rust's `BufRead` model also gives a strong case for delimiter-based helpers. `read_until` and `skip_until` are not esoteric parser machinery; they are the simple, direct way to handle NUL-terminated strings, line-like records, or bounded marker scans inside binary formats. They belong in a real in-memory binary I/O contract.

### Why the numeric helper surface is broad

Once `std.io` commits to exact-width numeric parsing, arbitrary seams become harder to defend. Supporting only little-endian reads or only a couple of widths would leave the API lopsided for no principled reason. The Rust substrate already supports endian-aware conversion for the full sized-integer and sized-float family, and RFC 009 already defines that vocabulary at the language level. The coherent design is therefore: full width family, both endian families for multi-byte values, and matching write helpers.

The default `int` and `float` aliases do still matter ergonomically, so `read_int_le` / `read_int_be` and `read_float_le` / `read_float_be` are good additions. But they are just shorthand over the exact-width forms, not a second independent numeric model.

### Why `getbuffer()` and generic protocols stay out

Python's `BytesIO.getbuffer()` exposes a mutable view over the underlying buffer. That is powerful, but it also introduces aliasing and resize constraints that are not worth standardizing before Incan has a broader borrowed-buffer story. This RFC therefore keeps the safe extraction surface small: `getvalue()` for a snapshot and `into_bytes()` for ownership transfer.

The same boundary applies to general `Reader` / `Writer` protocols. Those may well make sense later for `BytesIO`, `std.fs.File`, temporary files, network bodies, or InQL adapters. But that is a cross-cutting stream-abstraction RFC, not part of the in-memory byte-stream contract itself. RFC 056 should finish the concrete `BytesIO` design rather than smuggling in a second library proposal.

### Interaction with temporary storage

Spill-to-disk behavior does not belong in `BytesIO`. Python puts that concept in `tempfile.SpooledTemporaryFile`, not in `io.BytesIO`, and Incan should keep the same separation. RFC 056 is about pure in-memory streams. If Incan later standardizes spooled temporary files in RFC 010, that type should align with `BytesIO` where practical without being defined as a magical disk-spilling `BytesIO`.

## Alternatives considered

1. **No `std.io`; use slicing and builtins only** — too low-level and repetitive for real parsers.
2. **Fold `BytesIO` into `std.fs`** — rejected because in-memory byte streams are not path-based filesystem APIs.
3. **Expose only a Rust-shaped `Cursor` API** — exposes substrate vocabulary instead of an Incan-facing contract.
4. **Require separate `struct`-style unpacking for every numeric read and write** — workable, but worse ergonomically for the common fixed-width cases.
5. **Include spill-to-disk behavior directly in `BytesIO`** — rejected because storage policy and tempfile lifecycle are separate concerns better handled in `std.tempfile`.

## Drawbacks

- `std.io` is a modest but real additional stdlib surface to maintain.
- The full numeric helper family creates a larger testing matrix than a tiny parser-only API would.
- Excluding buffer-view APIs means some zero-copy workflows will still need Rust interop or a later dedicated buffer abstraction.

## Implementation architecture

*(Non-normative.)* A practical delivery implements `BytesIO` as a normal Incan stdlib type backed by Rust cursor and buffer primitives, with exact-width conversions delegated to Rust's primitive byte-conversion helpers. The public API should stay Incan-first even when the runtime maps directly onto `Cursor<Vec<u8>>`-like semantics underneath.

## Layers affected

- **Stdlib / runtime (`incan_stdlib`)**: new `std.io` module and the `BytesIO` type.
- **Language surface**: the module, constructor, and methods must be available as specified.
- **Builtin numeric surface**: numeric helper signatures depend on RFC 009's sized numeric types and aliases.
- **LSP / tooling**: completions and hovers for `std.io`.
- **Docs / examples**: binary parsing examples should use `std.io.BytesIO` instead of `rust::` recipes for the common in-memory path.

## Design Decisions

- `BytesIO` uses direct construction: `BytesIO(data)`, not `BytesIO.new(data)`.
- `BytesIO` is a writable, seekable, in-memory binary stream rather than a read-only parser cursor.
- The committed contract includes `read_until`, `skip_until`, `rewind`, `seek_relative`, `truncate`, `getvalue`, `into_bytes`, and `remaining`.
- Numeric helpers cover the full RFC 009 width family, with both endian families for multi-byte values and matching write helpers.
- Convenience aliases for `int` and `float` are included, but the exact-width forms remain canonical.
- `BytesIO` does not include `close()`, `getbuffer()`, spill-to-disk behavior, or generic `Reader` / `Writer` protocols.
