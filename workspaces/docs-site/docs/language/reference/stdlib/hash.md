# std.hash reference

`std.hash` provides deterministic hashing primitives for bytes, files, and binary readers.

```incan
from std.hash import sha256, xxh3_64, file_digest, reader_digest, HashError
from std.encoding import hex
```

Use cryptographic hash namespaces such as `sha256`, `sha3_256`, and `blake3` when the output is a byte digest. Use non-cryptographic namespaces such as `xxh3_64` when the goal is fast partitioning, bucketing, or reproducible non-security fingerprints. `sha1` and `md5` are present for interoperability and checksum workflows; do not use them for collision-resistant security decisions.

## Algorithm namespaces

`std.hash` exposes these import targets:

| Family                 | Namespaces                                     |
| ---------------------- | ---------------------------------------------- |
| SHA-2                  | `sha224`, `sha256`, `sha384`, `sha512`         |
| SHA-3                  | `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512` |
| SHAKE                  | `shake128`, `shake256`                         |
| BLAKE                  | `blake2b`, `blake2s`, `blake3`                 |
| Compatibility          | `sha1`, `md5`                                  |
| Fast non-cryptographic | `xxh3_64`, `xxh3_128`, `xxh64`, `xxh32`        |

Family grouping modules may be added later, but per-algorithm namespaces are the stable import targets.

## One-shot digests

Cryptographic and compatibility namespaces expose `digest(data: bytes) -> bytes`.

```incan
from std.hash import sha256
from std.encoding import hex

digest = sha256.digest(b"payload")
println(hex.encode(digest))
```

SHAKE namespaces require an explicit output length:

```incan
from std.hash import shake256
from std.encoding import hex

digest = shake256.digest(b"payload", 32)?
println(hex.encode(digest))
```

## Incremental hashers

Every namespace exposes `new()`. The returned hasher accepts byte chunks with `update`.

```incan
from std.hash import sha256
from std.encoding import hex

h = sha256.new()
h.update(b"pay")
h.update(b"load")
println(hex.encode(h.finalize_bytes()))
```

Non-cryptographic hashers also expose typed integer finalizers when the algorithm width matches:

```incan
from std.hash import xxh3_64

h = xxh3_64.new()
h.update(b"partition-key")
bucket_key = h.finalize_u64()
```

Integer helpers are intentionally absent from cryptographic namespaces. Use digest bytes plus `std.encoding.hex` when a textual digest is needed.

## File and Reader Helpers

`file_digest` hashes a `std.fs.Path` or `std.fs.File` incrementally and returns digest bytes.

```incan
from std.hash import file_digest
from std.encoding import hex
from std.fs import Path

digest = file_digest(Path("events.parquet"), "sha256")?
println(hex.encode(digest))
```

For SHAKE algorithms, pass a positive output `length` after `chunk_size`:

```incan
digest = file_digest(Path("events.parquet"), "shake128", 65536, 32)?
```

Use width-specific helpers for non-cryptographic integer output:

```incan
from std.hash import file_hash_u64
from std.fs import Path

fingerprint = file_hash_u64(Path("events.parquet"), "xxh3_64")?
```

`reader_digest` hashes any `std.io.BinaryReader`, such as `BytesIO`, incrementally and returns digest bytes.

```incan
from std.hash import reader_digest
from std.encoding import hex
from std.io import BytesIO

digest = reader_digest(BytesIO(b"payload"), "sha256")?
println(hex.encode(digest))
```

SHAKE reader digests use the same explicit length slot:

```incan
digest = reader_digest(BytesIO(b"payload"), "shake256", 1024, 32)?
```

Use `reader_hash_u32`, `reader_hash_u64`, and `reader_hash_u128` for matching non-cryptographic reader hashes.

`chunk_size` defaults to `65536` bytes and must be positive.

## Errors

Fallible helpers return `Result[..., HashError]`.

| Field       | Meaning                                                                                                                         |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `kind`      | Stable category such as `unknown_algorithm`, `unsupported_width`, `invalid_length`, `invalid_chunk_size`, or an I/O error kind. |
| `algorithm` | The algorithm name involved in the failure, when available.                                                                     |
| `detail`    | Human-readable explanation.                                                                                                     |

```incan
from std.hash import file_digest
from std.fs import Path

match file_digest(Path("events.parquet"), "unknown"):
    Ok(_) => println("hashed")
    Err(err) => println(err.kind)
```

## Boundaries

`std.hash` does not provide password hashing, keyed MACs, signatures, authenticated encryption, CRC, or Adler checksums. Those require separate APIs because their security and compatibility contracts are different from ordinary byte hashing.
