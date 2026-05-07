# Reflection (Reference)

Models and classes provide built-in reflection helpers for runtime introspection.

For the curated `std.reflection` module surface, see [Standard library reference: `std.reflection`](stdlib/reflection.md).

## `__class_name__() -> str`

Returns the type's name as a string.

```incan
model User:
    name: str

def main() -> None:
    u = User(name="Alice")
    println(u.__class_name__())  # "User"
```

## `__fields__() -> FrozenList[FieldInfo]`

Returns field metadata for the type.

```incan
model User:
    name: str
    email: str

def main() -> None:
    u = User(name="Alice", email="alice@example.com")
    for info in u.__fields__():
        println(f"{info.name}: {info.type_name}")
```

### `FieldInfo` structure

Each `FieldInfo` record contains:

| Field         | Type                              | Description                                                   |
| ------------- | --------------------------------- | ------------------------------------------------------------- |
| `name`        | `FrozenStr`                       | Canonical Incan field identifier                              |
| `alias`       | `Option[FrozenStr]`               | Wire name, if set via `[alias="..."]`                         |
| `description` | `Option[FrozenStr]`               | Documentation string, if set via `[description="..."]`        |
| `wire_name`   | `FrozenStr`                       | Effective wire name (alias if present, else canonical name)   |
| `type_name`   | `FrozenStr`                       | Incan type display (e.g. `"str"`, `"int"`, `"Option[str]"`)   |
| `has_default` | `bool`                            | Whether the field has a default value                         |
| `extra`       | `FrozenDict[FrozenStr, FrozenStr]`| Reserved for future metadata; always empty in current version |

Notes:

- Field metadata like `[alias="..."]` and `[description="..."]` is **model-only**.
- For a `class`, `FieldInfo.alias` and `FieldInfo.description` are always `None` and `FieldInfo.wire_name == FieldInfo.name`.
- You do not need to import `FieldInfo` just to call `obj.__fields__()` and inspect the returned records. Import `FieldInfo` only when you want to spell the type explicitly in an annotation.

### Common patterns

If you only need canonical field names:

```incan
field_names = [f.name for f in model.__fields__()]
```

Check if a field exists:

```incan
has_email = any(f.name == "email" for f in user.__fields__())
```

## Runtime Field Overlay Views

Models and classes also expose compiler-generated read-only field value views. These hooks are intended for collection and reflection protocols that need to treat a model or class value as a stable named-field overlay without using stringly dynamic attribute access.

| API | Returns | Description |
| --- | --- | --- |
| `obj.__field_value__(name: str)` | `Option[T]` | Returns `Some(value)` for a known field and `None` for an unknown field name. `T` is the common field type when all exposed fields share one type, otherwise a union of the exposed field types. |
| `obj.__field_items__()` | `list[tuple[str, T]]` | Returns `(field_name, value)` pairs in the same field order used by `__fields__()`. `T` follows the same common-type-or-union rule as `__field_value__`. |

For `class` values, inherited fields are included before fields declared on the child class, matching `__fields__()` ordering. The returned views are read-only snapshots of field names and values; mutating a returned list does not add, remove, rename, or update object fields.
