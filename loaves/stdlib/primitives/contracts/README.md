# Primitive boundary contracts

These Incan source files are authoring contracts for the RFC 168 implementation. They are deliberately outside `src/` and absent from the package entrypoint. Ellipsis bodies specify operations still to implement; they are not runtime fallbacks. They must never be shipped as working implementations or added to a passing census by skipping their bodies.

`fs.incn` and `io.incn` describe future `std.primitives.fs` and `std.primitives.io`. Acceptance examples import those public homes, not these draft files. Ownership forms come from the RFC 164 prelude. `IoError` and `Endian` are the existing contracts from `loaves/stdlib/system/src/io.incn`. Move `IoError` once into `std.primitives.errors` and `Endian` into `std.primitives.models` in slice 05; re-export those same declarations from the old home. Do not create a parallel error or byte-order enum just to compile this draft. Resource classes below reserve names and describe ownership; their private native storage and construction checks are implementation work, not empty runtime objects.

## Functional operations and shared contracts

Primitives expose resource types and functions rather than instance methods or associated factories. Resource representation stays private; functions enforce state, validation, cleanup, and ownership rules. The older Incan 0.2 structure is not a constraint on this design. The class declarations below are placeholders for opaque resource representation, not a decision to expose a class-based API; the concrete representation syntax remains implementation work.

| Layer | Responsibility | Allowed internal dependencies |
| --- | --- | --- |
| `std.primitives.models` | Shared value types, enums, and data contracts, including `Endian` | Core language types only |
| `std.primitives.errors` | Canonical primitive error types, including `IoError` | Core language types; `std.primitives.models` when required by error payloads |
| `std.primitives.io` | Byte cursors, buffers, and their functions | `std.primitives.models` and `std.primitives.errors` |
| `std.primitives.fs` | Paths, native handles, and filesystem functions | `std.primitives.models` and `std.primitives.errors`; byte I/O only where an implementation actually needs it |
| Higher-level `std.io` and `std.fs` | Convenience APIs and composition | Relevant primitives and shared substrate |

`std.primitives.models` and `std.primitives.errors` form the shared substrate and must not import their consumers. `models` must not depend on `errors`; error payloads may use shared models when necessary. Neither module depends on `io` or `fs`. Primitives must not import higher-level standard-library modules, and byte I/O must not depend on filesystem resources. Importing a common error must not force `fs` to depend on the buffer-and-cursor module. Any actual `fs` to `io` dependency remains one-way and must serve an implementation need.

`IoError` has one canonical declaration in `std.primitives.errors`. Shared value types and enums belong in `std.primitives.models`; `Endian` moves there from `std.io`. These module names replace the provisional `io_contracts` proposal. Existing public import locations re-export the same declarations, preserving type identity. Resource-specific types such as `File`, `ReadCursor`, and `WriteBuffer` remain in their respective resource modules; `models` is for shared data contracts, not every type. Extract actual shared contracts rather than creating a general-purpose dumping ground.

Public operations follow ordinary use. `append` mutates a borrowed buffer and returns `Result[None, IoError]`; it does not return an exclusive reference just to support chaining. Returned exclusive references remain a separate compiler acceptance requirement. Functional APIs retain the same borrowing, consumption, source-dependency, and separate-compilation guarantees.

## Files and independent chunks

`open_read` owns one read-only handle. Its Path borrow lasts for the call; retaining a file must not retain a borrow of its path argument. `read_chunk` requires exclusive access to advance the file position. A positive maximum bounds allocation and returned length; a short nonempty read is not EOF. An empty success is EOF. Zero and negative limits are rejected before reading. Bytes returned by this operation are independently owned, so another read or release of the file cannot invalidate them.

`close` consumes the handle on both success and failure. Its error reports cleanup failure without making the consumed handle usable again. Destruction remains the fallback for paths that do not explicitly close. Read-only close is not a durability operation; future writable handles must expose flush/sync failures explicitly. No operation here silently decodes text, converts errors to strings or bypasses capabilities.

## Cursors and buffers

`read_cursor` owns its cursor position but borrows the supplied immutable bytes. The source clause belongs to the returned aggregate, despite the outer `Owned`: the input must remain valid until the cursor and every dependent view finish use. This needs RFC 164's aggregate-dependency stage; direct borrowed-return support alone is insufficient.

`read_exact` first validates its entire requested range. On a negative length or insufficient bytes, cursor position is unchanged and the error contains no borrowed payload. On success it advances and returns a shared byte view. Zero length is an empty view at the current position. The first contract conservatively names the whole cursor as source, so another cursor mutation is rejected while that view remains in use. A future, more precise backing-field projection requires an explicit checked contract; do not silently infer it across a provider boundary.

`WriteBuffer` owns bytes. `append` mutates a borrowed buffer and returns `Result[None, IoError]`, retaining no result view. Its initial contract validates the size and obtains capacity before appending, so a recoverable error leaves the buffer unchanged. This is a promise of this operation, not automatic rollback supplied by `Result`. Resource exhaustion that the native allocator cannot report recoverably must not be advertised as an `IoError`.

`buffer_view` borrows existing bytes without copying. `finish_buffer` consumes the buffer and returns independent bytes; no surviving view may depend on that buffer. Native representation conversion must be proved compatible; `Borrowed[bytes]` is not permission to cast an incompatible native slice or allocate a hidden repair copy.

## Mapping and lock compatibility

The existing `MappedView` explicitly retains its mapping; it is not a lexical borrowed byte view. Moving it must preserve that independent retention. Keep `MappedFile.view`, `MappedView.slice` and explicit `to_bytes()` behavior; do not relabel the existing view as `Borrowed[bytes]`. A new lexical mapping-view operation can be added separately once native borrowed representation is proven.

Existing file locks retain an owned descriptor and coordinate cooperating processes only. Preserve contention as `Ok(None)` for try-lock calls, and preserve scoped release. Moving a lock does not make uncooperative external writes safe. Existing mapping invariants against external truncation/writes remain necessary.

## Acceptance matrix

All rows below are pending. The test sources are staged here, not registered as passing tests. Port them into the existing contract families as their prerequisite stage becomes executable.

| Case | Required observation | Earliest stage |
| --- | --- | --- |
| source_reader.incn | Missing file is typed Err; positive chunk bound; no lost short reads; independent returned bytes; explicit close consumes once | S2 + S5 file boundary |
| cursor.incn | Cursor owns position but retains data dependency; exact read advances; retained view remains valid | S4 aggregate + S5 native |
| fluent_buffer.incn | Sequential functional appends mutate the same buffer; finish returns independent bytes | S2/S3 + native buffer |
| reject_cursor_escape.incn | Return of cursor backed by a local is rejected at source, not repaired by copying | S4 |
| reject_buffer_reuse.incn | Buffer mutation overlapping a still-used shared view is rejected with producer/conflict/use spans | S2/S3 |
| reject_close_with_borrow.incn | Consuming a handle while a retained shared reference is later used is rejected | S2/S3 |
| cursor error branch in cursor.incn | Failed exact read leaves position unchanged; a consumed Err with no borrowed payload does not pin the cursor | S4 |
| dedicated exclusive-return fixture | RFC 164 returned exclusive references work independently of primitive buffer API choices | S3/S4 |
| dependency graph | Shared models do not import errors or consumers; errors do not import consumers; io does not import fs; primitives do not import convenience facades | Slice 05 onward |
| provider-only consumer | All contracts work without reading implementation bodies; aliases/re-exports retain modes and source identities | S1–S5 |
| old std facades | Same IoError/Endian identity and old convenience behavior | Slice 05 onward |

Use a temporary file supplied by the test harness for the file example, with a known payload crossing a chunk boundary and a separate nonexistent path. Do not hardcode a developer machine's path. Test injected short reads in the primitive implementation; ordinary filesystem reads alone do not prove short-read handling.

S1 must continue to reject executable use that later stages cannot enforce. Activation means supplying implementations, removing ellipsis bodies, checking resource/native behavior, and registering accepted and rejected tests. It does not mean teaching the checker to ignore these obligations.

## Remaining primitive families

[Remaining families and complete test drives](remaining-families.md) covers hashing, encoding, environment observation, filesystem resources, async and vocabulary. Each section distinguishes existing behavior from proposed functional contracts and names the evidence needed before activation. The accompanying Incan files remain uncompiled contract sketches.
