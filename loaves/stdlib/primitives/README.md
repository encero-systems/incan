# Standard primitives

`std.primitives` is the foundation shared by the standard library, compiler and platform code. Higher-level standard modules depend on it; it does not depend on their convenience APIs. RFC 168 defines the boundary, and RFC 164 supplies the ownership contracts used as resource APIs migrate.

## Current implementation

The package currently implements only `hexlify(data: bytes) -> str`. It returns two lowercase hexadecimal characters per byte, preserves input order and returns an empty string for empty input. The implementation lives in `src/primitives/encoding/hex.incn`; `src/primitives.incn` re-exports it at the root. `std.encoding.hex` continues to delegate to the same encoder. No second algorithm is introduced.

The grouped import and root re-export are the first skeleton change. The package builds, both imports execute, and the existing encoding surface type-checks against the candidate compiler. The full encoding regression also passes with the matching release profile. Combined source and installed qualification is still pending; this README is not a release claim.

```incan
from std.primitives import hexlify
from std.primitives.encoding.hex import hexlify as encode_hex

def main() -> None:
    assert hexlify(b"Az") == "417a"
    assert encode_hex(b"Az") == "417a"
```

The package manifest declares no dependencies and its authored source imports no other standard package. The current compiler nevertheless demands `incan_stdlib_core`, even for an empty library. The dependency-accounting slice must explain or remove each demand; the whole existing core package is not exempt merely because of its name.

## Proposed module skeleton

Only the encoding module above exists today. The following modules describe implementation destinations, not available imports. Add each source module when its implementation and tests land; do not publish empty placeholder types or methods.

| Public module | Owns | Stays above this layer |
| --- | --- | --- |
| `std.primitives.models` | Shared value types, enums and data contracts, including `Endian` | Resource-specific handles and behavior |
| `std.primitives.errors` | Canonical primitive errors, including `IoError` | Consumer-specific error policy |
| `std.primitives.fs` | Primitive Path, file handles, explicit filesystem operations, mappings, file locks and temporary-resource acquisition | Text codecs, globbing, recursive workflows, whole-file conveniences |
| `std.primitives.io` | Borrowed read cursors, owned write buffers and their functional operations; primitive read/write traits | Combined convenience `BytesIO`, encoding policy and source/sink orchestration |
| `std.primitives.hash` | Byte-level hashing algorithms and their incremental state | Whole-file hashing, algorithm selection policy and traversal |
| `std.primitives.encoding` | Byte/text conversion algorithms, grouped by encoding | File/stream orchestration and convenience defaults |
| `std.primitives.environ` | Precise host environment observations, including missing/invalid values | Typed configuration, fallback values and configuration merging |
| `std.primitives.vocab` | Vocabulary descriptors and structured transformation contracts | Compiler execution/admission and convenient authoring helpers |
| `std.primitives.async` | Runtime mechanisms and foundational async vocabulary | Task groups, deadline composition, concurrency limits and error policies |

Temporary directories are filesystem resources; environment-derived temporary-directory selection must state its host dependency. Native backend modules may live below their owning family (for example `fs.unix` or `fs.windows`). Their exact layout is private implementation work unless an operation intentionally exposes a platform-specific contract.

Shared `models` depend only on language foundations; `errors` may use shared models but neither imports its consumers. Byte I/O does not depend on filesystem resources. Filesystem code imports shared errors directly, and may depend on byte I/O only for a concrete implementation need. Resource types remain in their owning modules. Public primitive operations are module-level functions; opaque representation does not require an object-oriented public API.

The root is a curated entry point, not a wildcard aggregation of every module. It must not activate async vocabulary or require an async runtime merely because somebody imports Path or hexlify. Family imports and root aliases refer to the same declaration whenever they expose the same contract.

## Dependency direction

```text
std.fs / std.io / std.hash / std.encoding / std.async / std.vocab
                              |
                              v
                      std.primitives
                              |
                              v
           accounted language foundations + native libraries
```

Compiler and platform consumers can import primitives directly. Primitives must not import the compiler implementation, Oven, a storage engine or a high-level std facade. A Rust import is a signal to inspect a boundary, not a reason to move an entire domain library.

Shared errors and traits move once and are re-exported from their existing homes. Convenience types may remain distinct and delegate; preserving every old public type does not require giving it the primitive type's identity.

## First resource test drive

The following is proposed API, not executable on the current seed. It shows the concrete consumer the filesystem slice must satisfy. Names follow RFC 168's illustrative example and remain implementation-stage choices.

```incan
from std.primitives.fs import Path, open_read, read_chunk
from std.primitives.errors import IoError

def source_byte_count(path: Borrowed[Path]) -> Result[int, IoError]:
    mut file = open_read(path)?
    mut total = 0

    loop:
        # A positive bound limits this read. Short reads are not EOF.
        chunk = read_chunk(file, max_bytes=65536)?
        if chunk == b"":
            return Ok(total)
        total += len(chunk)
```

The proposed `open_read` returns `Result[Owned[File], IoError]`; `read_chunk` accepts `BorrowedMut[File]` and returns independent bytes. The handle is released on success or propagation. Reading does not silently decode text, produce compiler diagnostics, bypass capabilities or promise that external writers cannot change the file. The example retains only one chunk at a time.

This deliberately begins with owned chunks. A retained zero-copy view is a separate resource contract: it must name its source and retain that dependency through outcomes. Mappings, locks and cursors receive those tests before consumers rely on them.

## Path compatibility

Today's `std.fs.Path` is a newtype with rich convenience methods. Its existing representation, constructor and operations are compatibility obligations; setting up primitives is not permission to change those semantics.

The primitive Path is a separate low-level surface. It need not support `/` or be interchangeable with the convenience Path at parameter boundaries. The high-level implementation may wrap or delegate to it. Any later class representation or inheritance change must prove compatibility; it is not a prerequisite for grouping the package.

## Delivery boundaries

1. Repair standard provider re-exports (#2510), including canonical type identity and checked callable projections.
2. Explain the required language-core dependency closure.
3. Establish ownership declarations and executable resource behavior through the RFC 164 slices.
4. Move foundational errors/traits, then filesystem, buffer and algorithm operations with compatibility tests.
5. Move vocabulary and async foundations without activating unrelated behavior.
6. Prove a primitive-only compiler source reader and the platform store; then qualify the installed toolchain.

The current skeleton changes no file-resource, ownership or async semantics. The full per-slice plan tracks the bootstrap checkpoints, warning-release requirement and acceptance gates. RFC 164 and RFC 168 remain separate completion claims.

## Authored resource boundaries

[Boundary declarations and acceptance cases](contracts/README.md) now specify filesystem handles, independent chunks, borrowed read cursors and functional write-buffer operations. They live outside the compiled source root while RFC 164 support is implemented. The contracts include explicit source dependencies, close/transfer behavior, error-state guarantees and rejected programs; they do not provide callable placeholder implementations.
