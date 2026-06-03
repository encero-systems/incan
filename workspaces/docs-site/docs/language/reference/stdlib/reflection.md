# std.reflection (reference)

This page documents the `std.reflection` surface exposed by the standard library. Use it when you want to inspect field metadata produced by models and classes.

!!! info "Related pages"
    - If you want the language-facing explanation of automatic reflection on models and classes, see: [Language → Reference → Reflection].

<!-- References -->
[Language → Reference → Reflection]:../reflection.md

## Importing the reflection API

Import with:

```incan
from std.reflection import FieldInfo
```

You only need to import `FieldInfo` when you want to spell the type explicitly in an annotation. Calling `obj.__fields__()` or generic type-level reflection such as `T.__fields__()` and inspecting the returned records does not require an explicit import.

Primitive type-argument names such as `T.__class_name__()` for `T=int`, `T=float`, `T=str`, or `T=bool` are compiler-backed and also do not require a `std.reflection` import. Type-token parameters such as `Type[int]` are part of the language reflection surface rather than the `std.reflection` import surface; use the language reference when designing type-directed overload APIs.

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

## Compiler-generated field value views

Model and class values expose `__field_value__(name: str) -> Option[T]` and `__field_items__() -> list[tuple[str, T]]` directly; no `std.reflection` import is required. `T` is the common field type when all exposed fields share one type, otherwise a union of the exposed field types. These views are read-only and use the same field ordering as `__fields__()`.
