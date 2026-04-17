# std.reflection (reference)

This page documents the `std.reflection` surface exposed by the standard library.
Use it when you want to inspect field metadata produced by models and classes.

!!! info "Related pages"
    - If you want the language-facing explanation of automatic reflection on models and classes, see: [Language → Reference → Reflection].

<!-- References -->
[Language → Reference → Reflection]:../reflection.md

## Importing the reflection API

Import with:

```incan
from std.reflection import FieldInfo
```

You only need to import `FieldInfo` when you want to spell the type explicitly in an annotation.
Calling `obj.__fields__()` and inspecting the returned records does not require an explicit import.

## Types

### `FieldInfo`

Field metadata returned by `__fields__()`.

| Field         | Type                               | Description                                                   |
| ------------- | ---------------------------------- | ------------------------------------------------------------- |
| `name`        | `FrozenStr`                        | Canonical Incan field identifier                              |
| `alias`       | `Option[FrozenStr]`                | Wire name, if set via `[alias="..."]`                         |
| `description` | `Option[FrozenStr]`                | Documentation string, if set via `[description="..."]`        |
| `wire_name`   | `FrozenStr`                        | Effective wire name (alias if present, else canonical name)   |
| `type_name`   | `FrozenStr`                        | Incan type display (e.g. `"str"`, `"int"`, `"Option[str]"`)   |
| `has_default` | `bool`                             | Whether the field has a default value                         |
| `extra`       | `FrozenDict[FrozenStr, FrozenStr]` | Reserved for future metadata; always empty in current version |

Notes:

- Field metadata like `[alias="..."]` and `[description="..."]` is model-only.
- For a `class`, `FieldInfo.alias` and `FieldInfo.description` are always `None` and `FieldInfo.wire_name == FieldInfo.name`.
