# `std.uuid`

`std.uuid` provides RFC 9562 UUID values as ordinary Incan source-defined data. The module stores each `UUID` as a nominal `u128` wrapper and owns parsing, formatting, byte layout, version bits, and variant bits in Incan source. Rust interop is limited to primitive inputs for randomness, clock access, and UTF-8 string bytes; name-based UUID hashing dogfoods `std.hash`.

## Imports

```incan
from std.uuid import NAMESPACE_DNS, UUID, UuidError, UuidVariant, UuidVersion
```

## Types

### `UUID`

```incan
@derive(Clone, Copy, Eq, Ord, Hash)
pub type UUID = newtype u128
```

`UUID` is not a Rust-backed stdlib type. It is a source-defined newtype over `u128`.

Methods:

| Method                                         | Returns                          | Description                                              |
| ---------------------------------------------- | -------------------------------- | -------------------------------------------------------- |
| `UUID.nil()`                                   | `UUID`                           | The nil UUID.                                            |
| `UUID.max()`                                   | `UUID`                           | The maximum UUID value.                                  |
| `UUID.from_int(value: u128)`                   | `UUID`                           | Construct from the unsigned 128-bit representation.      |
| `UUID.from_bytes(raw: bytes)`                  | `Result[UUID, UuidError]`        | Construct from 16 RFC/network-order bytes.               |
| `UUID.parse(text: str)`                        | `Result[UUID, UuidError]`        | Parse canonical, simple, braced, or URN UUID text.       |
| `UUID.v1()`                                    | `Result[UUID, UuidError]`        | Generate a Gregorian time-based UUID.                    |
| `UUID.v3(namespace: UUID, name: str \| bytes)` | `Result[UUID, UuidError]`        | Generate an MD5 namespace UUID.                          |
| `UUID.v4()`                                    | `Result[UUID, UuidError]`        | Generate a random UUID.                                  |
| `UUID.v5(namespace: UUID, name: str \| bytes)` | `Result[UUID, UuidError]`        | Generate a SHA-1 namespace UUID.                         |
| `UUID.v6()`                                    | `Result[UUID, UuidError]`        | Generate a reordered Gregorian time-based UUID.          |
| `UUID.v7()`                                    | `Result[UUID, UuidError]`        | Generate a Unix-epoch-millisecond UUID.                  |
| `UUID.v8(raw: bytes)`                          | `Result[UUID, UuidError]`        | Construct a vendor-specific UUID from 16 bytes.          |
| `uuid.to_int()`                                | `u128`                           | Return the unsigned 128-bit representation.              |
| `uuid.to_bytes()`                              | `Result[bytes, UuidError]`       | Return 16 RFC/network-order bytes.                       |
| `uuid.canonical()`                             | `Result[str, UuidError]`         | Return lower-case `8-4-4-4-12` canonical text.           |
| `uuid.to_string()`                             | `str`                            | Return lower-case canonical text.                        |
| `uuid.to_hex()`                                | `str`                            | Return 32 lower-case hexadecimal digits without hyphens. |
| `uuid.to_urn()`                                | `str`                            | Return `urn:uuid:` plus canonical text.                  |
| `uuid.version()`                               | `Result[UuidVersion, UuidError]` | Inspect the UUID version nibble.                         |
| `uuid.variant()`                               | `Result[UuidVariant, UuidError]` | Inspect the UUID variant bits.                           |

Module constants:

| Constant         | Type   | Description                    |
| ---------------- | ------ | ------------------------------ |
| `NIL`            | `UUID` | The nil UUID.                  |
| `MAX`            | `UUID` | The maximum UUID value.        |
| `NAMESPACE_DNS`  | `UUID` | Standard DNS namespace UUID.   |
| `NAMESPACE_URL`  | `UUID` | Standard URL namespace UUID.   |
| `NAMESPACE_OID`  | `UUID` | Standard OID namespace UUID.   |
| `NAMESPACE_X500` | `UUID` | Standard X.500 namespace UUID. |

### `UuidVersion`

```incan
pub enum UuidVersion:
    Nil
    V1
    V2
    V3
    V4
    V5
    V6
    V7
    V8
    Max
    Unknown(int)
```

### `UuidVariant`

```incan
pub enum UuidVariant:
    Ncs
    Rfc9562
    Microsoft
    Future
    Unknown
```

### `UuidError`

```incan
pub model UuidError with Error:
    pub kind: str
    pub detail: str
```

Known `kind` values include `invalid_length`, `invalid_format`, `invalid_character`, `invalid_bytes`, and `io_error`.

## Generation Helpers

`std.uuid` generates RFC 9562 versions 1, 3, 4, 5, 6, 7, and 8. Version 2 remains parseable and inspectable but has no generator because RFC 9562 leaves DCE Security UUID generation outside the core specification.

## See Also

- [Working with UUIDs](../../how-to/working_with_uuids.md)
- [RFC 060: std.uuid](../../../RFCs/closed/implemented/060_std_uuid.md)
