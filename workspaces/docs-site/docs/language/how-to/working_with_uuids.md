# Working with UUIDs

Use `std.uuid.UUID` when application code needs stable identifiers, request IDs, namespace-derived IDs, or wire-format UUID[^uuid] values. UUID operations that parse text, decode bytes, generate values, or inspect layout return `Result`, so malformed input and host failures stay visible at the boundary.

!!! note "What a UUID is"

    A UUID is a 128-bit identifier: 16 bytes commonly rendered as text such as `550e8400-e29b-41d4-a716-446655440000`. It is designed to be generated without a central coordinator. Different UUID versions define how those 16 bytes are produced or interpreted.

```incan
from std.uuid import NAMESPACE_DNS, UUID, UuidError, UuidVariant, UuidVersion
```

## Choose a UUID Version

| Need                                                                    | Use                        |
| ----------------------------------------------------------------------- | -------------------------- |
| A random identifier for records, messages, or resources                 | `UUID.v4()`                |
| A sortable timestamp-shaped request or event ID                         | `UUID.v7()`                |
| A deterministic ID from a namespace and name                            | `UUID.v5(namespace, name)` |
| A legacy MD5 namespace ID for protocol compatibility                    | `UUID.v3(namespace, name)` |
| A Gregorian time-based ID for systems that require v1 layout            | `UUID.v1()`                |
| A sortable Gregorian time-based ID for systems that require v6 layout   | `UUID.v6()`                |
| A vendor-defined payload with RFC 9562 version and variant bits applied | `UUID.v8(raw)`             |

In short:

- Prefer `v7()` for new event-style identifiers that benefit from time ordering.
- Prefer `v4()` when ordering does not matter and the caller only needs an opaque random identifier.
- Use `v5()` for deterministic identifiers that must be regenerated from the same namespace and name.

## Why There Is No UUIDv2 Generator

`std.uuid` deliberately omits a UUIDv2 generator. It can parse and inspect UUIDv2 values for interoperability, but it does not generate new ones. UUIDv2 is the DCE[^dce] Security UUID layout: it embeds local domain identifiers such as POSIX user IDs or group IDs into a UUIDv1-shaped value. RFC 9562 documents version 2 as a known layout, but leaves DCE Security generation outside the core UUID generation algorithms.

For new code, use `UUID.v7()` for sortable generated IDs, `UUID.v4()` for opaque random IDs, or `UUID.v5(...)` for deterministic namespace IDs. Only preserve UUIDv2 values when interoperating with an existing system that already emits them.

## Generate New IDs

```incan
from std.uuid import UUID, UuidError

def create_request_id() -> Result[UUID, UuidError]:
    return UUID.v7()

def create_record_id() -> Result[UUID, UuidError]:
    return UUID.v4()
```

Generation returns `Result` because clock access, randomness, and byte assembly are fallible operations. Use `?` when the caller already returns `Result`.

## Parse User or Protocol Input

`UUID.parse(...)` accepts canonical text, simple 32-digit hex, braced UUIDs, and `urn:uuid:` values. Normalize once at the boundary, then pass `UUID` values through the rest of the program.

```incan
from std.uuid import UUID, UuidError

def parse_header(value: str) -> Result[UUID, UuidError]:
    return UUID.parse(value)

def print_uuid(value: str) -> Result[None, UuidError]:
    uuid = UUID.parse(value)?
    println(uuid.canonical()?)
    return Ok(None)
```

Use `match` when the caller can recover from malformed input:

```incan
from std.uuid import UUID

def print_optional_uuid(value: str) -> None:
    match UUID.parse(value):
        Ok(uuid) => println(uuid.to_string())
        Err(err) => println(err.message())
```

## Format for Logs, URLs, and Protocols

Use `canonical()` or `to_string()` for the familiar lower-case `8-4-4-4-12` form. Use `to_hex()` for compact storage without hyphens and `to_urn()` for protocols that require the UUID URN spelling.

```incan
from std.uuid import UUID, UuidError

def render(uuid: UUID) -> Result[None, UuidError]:
    println(uuid.canonical()?)
    println(uuid.to_hex())
    println(uuid.to_urn())
    return Ok(None)
```

`canonical()` returns `Result` because it formats through the UUID byte layout. `to_string()`, `to_hex()`, and `to_urn()` are convenience methods for common text output.

## Build Deterministic Namespace IDs

Use namespace UUIDs when the same logical name should always produce the same UUID. DNS, URL, OID, and X.500 namespaces are provided as constants.

```incan
from std.uuid import NAMESPACE_DNS, NAMESPACE_URL, UUID, UuidError

def service_id(hostname: str) -> Result[UUID, UuidError]:
    return UUID.v5(NAMESPACE_DNS, hostname)

def route_id(url: str) -> Result[UUID, UuidError]:
    return UUID.v5(NAMESPACE_URL, url)
```

`v5()` uses SHA-1 namespace UUIDs and is the default deterministic choice. Keep `v3()` for compatibility with systems that specifically require MD5 namespace UUIDs.

## Round-Trip Bytes and Integers

Use `to_bytes()` and `from_bytes(...)` for binary protocols. The byte order is the RFC/network-order layout used by UUID text formatting.

```incan
from std.uuid import UUID, UuidError

def roundtrip_bytes(raw: bytes) -> Result[bytes, UuidError]:
    uuid = UUID.from_bytes(raw)?
    return uuid.to_bytes()
```

Use `to_int()` and `from_int(...)` when a database or index stores UUIDs as unsigned 128-bit values:

```incan
from std.uuid import UUID

def restore(value: u128) -> UUID:
    return UUID.from_int(value)
```

The integer form preserves the exact UUID bits. It does not validate version or variant by itself.

## Inspect Version and Variant

Use `version()` and `variant()` when protocol code needs to reject unsupported UUID layouts.

```incan
from std.uuid import UUID, UuidError, UuidVariant, UuidVersion

def require_v7(text: str) -> Result[UUID, UuidError]:
    uuid = UUID.parse(text)?
    if uuid.version()? != UuidVersion.V7:
        return Err(UuidError(kind="unsupported_version", detail="expected a UUIDv7 value"))
    if uuid.variant()? != UuidVariant.Rfc9562:
        return Err(UuidError(kind="unsupported_variant", detail="expected an RFC 9562 UUID"))
    return Ok(uuid)
```

Nil and max UUIDs have special version results because they are sentinel values rather than ordinary generated UUIDs.

## See Also

- [`std.uuid` reference](../reference/stdlib/uuid.md)
- [Error handling](../explanation/error_handling.md)
- [Choosing numeric types](choosing_numeric_types.md)

<!-- footnotes -->
[^uuid]: `UUID` means `Universally Unique Identifier`.
[^dce]: Distributed Computing Environment, an older OSF/Open Group distributed systems stack.
