# std.checksum reference

`std.checksum` provides non-security checksum primitives for compatibility with file formats, containers, and protocols. Checksums are useful for accidental-corruption detection and wire-format interoperability; they are not collision-resistant security primitives.

## Imports

```incan
from std.checksum import crc32
```

## CRC32

The initial `std.checksum` surface exposes the IEEE CRC-32 algorithm through the `crc32` namespace.

| API | Returns | Description |
| --- | --- | --- |
| `crc32.value(data: bytes)` | `u32` | Return the native CRC32 integer value. |
| `crc32.digest(data: bytes)` | `bytes` | Return four CRC32 bytes in big-endian/network byte order. |
| `crc32.new()` | `Crc32` | Create an incremental hasher. |

Known vector:

```incan
crc32.value(b"abc") == 891568578
crc32.digest(b"abc") == b"\x35\x24\x41\xc2"
```

## Incremental Use

Use `new()`, `update(...)`, and a finalizer when data arrives in chunks:

```incan
from std.checksum import crc32

mut h = crc32.new()
h.update(b"a")
h.update(b"bc")
value = h.finalize_u32()
digest = h.finalize_bytes()
```

`finalize_u32()` snapshots the current value. `finalize_bytes()` returns the same value as big-endian bytes.

## Relationship To std.hash

Use `std.hash` for deterministic hashes, digests, and non-cryptographic partition keys. Use `std.checksum` when an external format explicitly requires checksum semantics such as CRC32. Do not use CRC32 for password hashing, keyed MACs, signatures, authenticated encryption, or collision-resistant security decisions.

## See Also

- [`std.hash` reference](hash.md)
- [Hashing data](../../how-to/hashing_data.md)
