# Conversion traits (Reference)

This page documents stdlib traits for explicit conversions.

Current status:

- `std.traits.convert` remains a documented trait family, but RFC 023 closeout for its `.incn` source is blocked for now because `from` is still a hard keyword in declaration position.
- Follow-up is tracked in [RFC 043](../../../RFCs/043_rust_trait_impl_from_incan.md).

## From / Into

- **`From[T]`**
    - Intended hook: `@classmethod def from(cls, value: T) -> Self`
- **`Into[T]`**
    - Hook: `def into(self) -> T`

## TryFrom / TryInto

- **`TryFrom[T]`**
    - Intended hook: `@classmethod def try_from(cls, value: T) -> Result[Self, str]`
- **`TryInto[T]`**
    - Hook: `def try_into(self) -> Result[T, str]`
