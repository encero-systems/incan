# std.checksum reference

`std.checksum` provides non-security checksum primitives for compatibility with file formats, containers, and protocols. Checksums are useful for accidental-corruption detection and wire-format interoperability; they are not collision-resistant security primitives.

## Imports

```incan
from std.checksum import Crc32, crc32
```

The same declarations are also available from their foundational home:

```incan
from std.primitives.checksum import Crc32, crc32
```

## CRC32

`std.primitives.checksum` owns the IEEE CRC-32 algorithm through the `crc32` namespace. `std.checksum` reexports the same declarations; values cross either import surface without conversion.

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

| Method | Returns | Contract |
| --- | --- | --- |
| `Crc32.new()` | `Crc32` | Class-owned factory for a fresh incremental hasher; `crc32.new()` delegates to it. |
| `Crc32.update(mut self, chunk: bytes)` | `None` | Add a chunk; empty chunks preserve state. |
| `Crc32.finalize_u32(self)` | `u32` | Snapshot the current checksum without resetting or consuming the hasher. |
| `Crc32.finalize_bytes(self)` | `bytes` | Snapshot as four big-endian bytes. |

Repeated finalization returns the same result until another update. Updating after finalization continues the existing stream. Empty input has integer checksum zero and four zero bytes. These methods do not report typed failures; native allocation or process failures are not converted into checksum results.

## Relationship To std.hash

Use `std.hash` for deterministic hashes, digests, and non-cryptographic partition keys. Use `std.checksum` when an external format explicitly requires checksum semantics such as CRC32. Do not use CRC32 for password hashing, keyed MACs, signatures, authenticated encryption, or collision-resistant security decisions.

## See Also

- [`std.hash` reference](hash.md)
- [Hashing data](../../how-to/hashing_data.md)
