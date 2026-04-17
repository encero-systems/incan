# std.derives.* (reference)

This page documents the derive-related trait families available under `std.derives.*`.
Import these traits when you want to adopt them explicitly or refer to them in annotations and bounds.

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

Traits under `std.derives.*` describe capabilities such as equality, ordering, copying, and display formatting.

- Many of these traits are also the ones used by `@derive(...)`.
- You can import the trait names directly when you want to refer to them in type-level positions.
- The collection traits in `std.derives.collection` are ordinary trait families for collection-like behavior, not derive markers.

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

Use these when you want a custom type to participate in collection-style APIs through explicit trait adoption.
