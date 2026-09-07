# Hashing data

Use `std.hash` when a program needs deterministic byte digests, file fingerprints, reader fingerprints, or non-cryptographic partition keys.

## Choose an algorithm family

| Need                                        | Use                                                    |
| ------------------------------------------- | ------------------------------------------------------ |
| General cryptographic byte digest           | `sha256`, `sha3_256`, or `blake3`                      |
| Compatibility with existing protocols       | `sha1` or `md5`, only where the protocol requires them |
| Variable-length extendable output           | `shake128` or `shake256`                               |
| Fast non-security partitioning or bucketing | `xxh3_64`, `xxh3_128`, `xxh64`, or `xxh32`             |

Do not use `sha1` or `md5` for collision-resistant security decisions. Do not use `std.hash` for password hashing, signatures, authenticated encryption, CRC, or Adler checksums. For keyed authentication of untrusted input, see [Authenticate a value that crossed a boundary](#authenticate-a-value-that-crossed-a-boundary). Use [`std.checksum`](../reference/stdlib/checksum.md) when a protocol or file format requires CRC32.

## Hash bytes in one call

Use one-shot helpers when the payload is already in memory:

```incan
from std.encoding import hex
from std.hash import sha256

digest = sha256.digest(b"payload")
println(hex.encode(digest))
```

SHAKE algorithms require an explicit output length:

```incan
from std.encoding import hex
from std.hash import shake256

digest = shake256.digest(b"payload", 32)?
println(hex.encode(digest))
```

## Hash incrementally

Use `new()`, `update(...)`, and a finalizer when the payload arrives in chunks:

```incan
from std.encoding import hex
from std.hash import sha256

h = sha256.new()
h.update(b"pay")
h.update(b"load")
println(hex.encode(h.finalize_bytes()))
```

Non-cryptographic hashers expose native integer finalizers when the algorithm width matches:

```incan
from std.hash import xxh3_64

h = xxh3_64.new()
h.update(b"partition-key")
bucket_key = h.finalize_u64()
```

## Keep SHA-256 state across methods

Use `Sha256Hasher` when one model or class accumulates a byte stream across methods without retaining every chunk for a later replay:

--8<-- "_snippets/language/examples/sha256_structural_sink.md"

The hasher preserves state, not meaning. Define the ordered canonical bytes for your own identity or serialization contract before calling `append`; `std.hash` only computes the deterministic SHA-256 digest of those bytes. Finalization resets the handle for a new stream.

## Hash files without loading them

Use file helpers for paths or open files:

```incan
from std.encoding import hex
from std.fs import Path
from std.hash import file_digest

digest = file_digest(Path("events.parquet"), "sha256")?
println(hex.encode(digest))
```

For SHAKE algorithms, pass a positive output `length` after `chunk_size`:

```incan
from std.fs import Path
from std.hash import file_digest

digest = file_digest(Path("events.parquet"), "shake128", 65536, 32)?
```

Use width-specific helpers for non-cryptographic integer output:

```incan
from std.fs import Path
from std.hash import file_hash_u64

fingerprint = file_hash_u64(Path("events.parquet"), "xxh3_64")?
```

## Hash binary readers

Use reader helpers when the source implements `std.io.BinaryReader`, such as `BytesIO`:

```incan
from std.encoding import hex
from std.hash import reader_digest
from std.io import BytesIO

digest = reader_digest(BytesIO(b"payload"), "sha256")?
println(hex.encode(digest))
```

SHAKE reader digests use the same explicit length slot:

```incan
from std.hash import reader_digest
from std.io import BytesIO

digest = reader_digest(BytesIO(b"payload"), "shake256", 1024, 32)?
```

Use `reader_hash_u32`, `reader_hash_u64`, and `reader_hash_u128` for matching non-cryptographic reader hashes.

## Authenticate a value that crossed a boundary

Use `hmac_sha256` when a value arrives from somewhere you do not control and you need to know it is one you issued. An unkeyed digest cannot answer that — see [What hashing proves](../explanation/hashing_guarantees.md) for why.

```incan
from std.hash import hmac_sha256

def issue(key: bytes, payload: bytes) -> bytes:
    return hmac_sha256.digest(key, payload)


def accept(key: bytes, payload: bytes, submitted_tag: bytes) -> bool:
    return hmac_sha256.verify(key, payload, submitted_tag)
```

Verify with `verify`, not by recomputing a tag and comparing it with `==`. The comparison must be constant time, and `verify` is; an equality check on tag bytes is not.

Keep the key out of whatever the value crossed to reach you. A key on the far side of that boundary makes the check decorative.

For a value built up in pieces, keep a signer instead of concatenating first:

```incan
mut signer = hmac_sha256.new(key)
for chunk in chunks:
    signer.update(chunk)
tag = signer.finalize_bytes()
```

## Handle invalid requests

Branch on `HashError.kind` when callers can recover:

```incan
from std.fs import Path
from std.hash import file_digest

match file_digest(Path("events.parquet"), "unknown"):
    Ok(_) => println("hashed")
    Err(err) => println(err.kind)
```

Common error categories include `unknown_algorithm`, `unsupported_width`, `invalid_length`, `invalid_chunk_size`, and I/O error kinds.

## See also

- [What hashing proves](../explanation/hashing_guarantees.md)
- [`std.hash` reference](../reference/stdlib/hash.md)
- [`std.checksum` reference](../reference/stdlib/checksum.md)
- [`std.encoding` reference](../reference/stdlib/encoding.md)
- [`std.io` reference](../reference/stdlib/io.md)
- [`std.fs` reference](../reference/stdlib/fs.md)
