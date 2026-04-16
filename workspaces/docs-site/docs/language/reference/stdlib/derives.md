# std.derives.* (reference)

This page documents the curated `std.derives.*` standard-library surface.
The implementation source of truth lives in:

- `crates/incan_stdlib/stdlib/derives/comparison.incn`
- `crates/incan_stdlib/stdlib/derives/copying.incn`
- `crates/incan_stdlib/stdlib/derives/string.incn`
- `crates/incan_stdlib/stdlib/derives/collection.incn`

!!! info "Related pages"
    - If you want the language-facing explanation of derives, trait authoring, and conflict rules, see:
      [Language → Reference → Derives & traits].
    - If you want the per-family reference pages, see:
      [Comparison], [Copying and Default], and [String representation].

<!-- References -->
[Language → Reference → Derives & traits]:../derives_and_traits.md
[Comparison]:../derives/comparison.md
[Copying and Default]:../derives/copying_default.md
[String representation]:../derives/string_representation.md

## Importing derive traits

Import from the specific derive submodule:

```incan
from std.derives.comparison import Eq, Ord, Hash
from std.derives.copying import Clone, Copy, Default
from std.derives.string import Debug, Display
```

## Surface model

Traits under `std.derives.*` are source-defined capability contracts.

- The trait signatures are declared in `.incn` source.
- The compiler typechecks against those source signatures.
- When a type uses `@derive(...)`, codegen realizes the implementation through ordinary Rust `#[derive(...)]` expansion on the adopting type.
- These traits are not modeled as runtime helper calls through `incan_stdlib::derives::*`.

## Submodules

### `std.derives.comparison`

Provides:

- `Eq`
- `Ord`
- `Hash`

See [Comparison].

### `std.derives.copying`

Provides:

- `Clone`
- `Copy`
- `Default`

See [Copying and Default].

### `std.derives.string`

Provides:

- `Debug`
- `Display`

See [String representation].

### `std.derives.collection`

Provides collection-protocol traits for custom types, including:

- `Contains[T]`
- `Bool`
- `Len`
- `Iterable[T]`
- `Iterator[T]`

These are source-defined stdlib traits, but unlike the derive families above they are not realized via Rust `#[derive(...)]`.
