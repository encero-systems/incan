# std.hash reference

`std.hash` provides deterministic hashing primitives for bytes, files, and binary readers. For task-oriented examples, see [Hashing data](../../how-to/hashing_data.md).

## Imports

```incan
from std.hash import HashError, file_digest, reader_digest, sha256, xxh3_64
```

## Algorithm namespaces

`std.hash` exposes these import targets:

| Family | Namespaces |
| --- | --- |
| SHA-2 | `sha224`, `sha256`, `sha384`, `sha512` |
| SHA-3 | `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512` |
| SHAKE | `shake128`, `shake256` |
| BLAKE | `blake2b`, `blake2s`, `blake3` |
| Compatibility | `sha1`, `md5` |
| Fast non-cryptographic | `xxh3_64`, `xxh3_128`, `xxh64`, `xxh32` |

Family grouping modules may be added later, but per-algorithm namespaces are the stable import targets.

## One-shot digest APIs

| Namespace family | API | Returns | Notes |
| --- | --- | --- | --- |
| SHA-2, SHA-3, BLAKE, compatibility | `algorithm.digest(data: bytes)` | `bytes` | Fixed-length digest bytes. |
| SHAKE | `algorithm.digest(data: bytes, length: int)` | `Result[bytes, HashError]` | `length` must be positive. |
| Fast non-cryptographic | `algorithm.digest(data: bytes)` | `bytes` | Little-endian byte representation of the algorithm's native integer output. |

`sha1` and `md5` are present for interoperability and checksum workflows; do not use them for collision-resistant security decisions.

## Incremental hashers

Every algorithm namespace exposes `new()`. The returned hasher accepts byte chunks with `update`.

| Hasher family | Methods |
| --- | --- |
| Fixed byte digest hashers | `update(chunk: bytes) -> None`, `finalize_bytes() -> bytes` |
| SHAKE digest hashers | `update(chunk: bytes) -> None`, `finalize_bytes(length: int) -> Result[bytes, HashError]` |
| 32-bit non-cryptographic hashers | `update(chunk: bytes) -> None`, `finalize_bytes() -> bytes`, `finalize_u32() -> u32` |
| 64-bit non-cryptographic hashers | `update(chunk: bytes) -> None`, `finalize_bytes() -> bytes`, `finalize_u64() -> u64` |
| 128-bit non-cryptographic hashers | `update(chunk: bytes) -> None`, `finalize_bytes() -> bytes`, `finalize_u128() -> u128` |

Integer finalizers are intentionally absent from cryptographic namespaces. Use digest bytes plus `std.encoding.hex` when a textual digest is needed.

## File and reader helpers

| API | Returns | Description |
| --- | --- | --- |
| `file_digest(input: Path \| File, algorithm: str, chunk_size: int = 65536, length: int = 0)` | `Result[bytes, HashError]` | Stream a path or open file through a hash algorithm and return digest bytes. SHAKE algorithms require a positive `length`; fixed-output algorithms ignore `length`. |
| `file_hash_u32(input: Path \| File, algorithm: str, chunk_size: int = 65536)` | `Result[u32, HashError]` | Stream a path or open file through a 32-bit non-cryptographic hash. Currently supported by `xxh32`. |
| `file_hash_u64(input: Path \| File, algorithm: str, chunk_size: int = 65536)` | `Result[u64, HashError]` | Stream a path or open file through a 64-bit non-cryptographic hash. Currently supported by `xxh64` and `xxh3_64`. |
| `file_hash_u128(input: Path \| File, algorithm: str, chunk_size: int = 65536)` | `Result[u128, HashError]` | Stream a path or open file through a 128-bit non-cryptographic hash. Currently supported by `xxh3_128`. |
| `reader_digest(input: BinaryReader, algorithm: str, chunk_size: int = 65536, length: int = 0)` | `Result[bytes, HashError]` | Stream any `std.io.BinaryReader` through a hash algorithm and return digest bytes. |
| `reader_hash_u32(input: BinaryReader, algorithm: str, chunk_size: int = 65536)` | `Result[u32, HashError]` | Stream any `std.io.BinaryReader` through a 32-bit non-cryptographic hash. |
| `reader_hash_u64(input: BinaryReader, algorithm: str, chunk_size: int = 65536)` | `Result[u64, HashError]` | Stream any `std.io.BinaryReader` through a 64-bit non-cryptographic hash. |
| `reader_hash_u128(input: BinaryReader, algorithm: str, chunk_size: int = 65536)` | `Result[u128, HashError]` | Stream any `std.io.BinaryReader` through a 128-bit non-cryptographic hash. |

`chunk_size` must be positive. Reader helpers consume `BinaryReader.chunks(chunk_size)`, whose successful zero-length read marks EOF rather than hashing an empty chunk.

## Errors

Fallible helpers return `Result[..., HashError]`.

| Field | Meaning |
| --- | --- |
| `kind` | Stable category such as `unknown_algorithm`, `unsupported_width`, `invalid_length`, `invalid_chunk_size`, or an I/O error kind. |
| `algorithm` | The algorithm name involved in the failure, when available. |
| `detail` | Human-readable explanation. |

One-shot namespace helpers that are infallible raise `ValueError` for the same validation detail where applicable.

## Boundaries

`std.hash` does not provide password hashing, keyed MACs, signatures, authenticated encryption, CRC, or Adler checksums. Those require separate APIs because their security and compatibility contracts are different from ordinary byte hashing. Use [`std.checksum`](checksum.md) for CRC32 compatibility checksums.

## See also

- [Hashing data](../../how-to/hashing_data.md)
- [`std.checksum` reference](checksum.md)
- [`std.encoding` reference](encoding.md)
- [`std.io` reference](io.md)
- [`std.fs` reference](fs.md)
