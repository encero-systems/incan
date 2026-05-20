# Hashing data

Use `std.hash` when a program needs deterministic byte digests, file fingerprints, reader fingerprints, or non-cryptographic partition keys.

## Choose an algorithm family

| Need                                        | Use                                                    |
| ------------------------------------------- | ------------------------------------------------------ |
| General cryptographic byte digest           | `sha256`, `sha3_256`, or `blake3`                      |
| Compatibility with existing protocols       | `sha1` or `md5`, only where the protocol requires them |
| Variable-length extendable output           | `shake128` or `shake256`                               |
| Fast non-security partitioning or bucketing | `xxh3_64`, `xxh3_128`, `xxh64`, or `xxh32`             |

Do not use `sha1` or `md5` for collision-resistant security decisions. Do not use `std.hash` for password hashing, keyed MACs, signatures, authenticated encryption, CRC, or Adler checksums.

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

- [`std.hash` reference](../reference/stdlib/hash.md)
- [`std.encoding` reference](../reference/stdlib/encoding.md)
- [`std.io` reference](../reference/stdlib/io.md)
- [`std.fs` reference](../reference/stdlib/fs.md)
