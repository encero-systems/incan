# Reflection (Reference)

Models and classes provide built-in reflection helpers for runtime introspection. Generic type-argument reflection also exposes stable source names for primitive type arguments.

For the curated `std.reflection` module surface, see [Standard library reference: `std.reflection`](stdlib/reflection.md).

For examples and the distinction between value metadata, type metadata and field values, see [Reflection contracts and use](../explanation/reflection.md).

## `__class_name__() -> str`

Returns the type's name as a string.

```incan
model User:
    name: str

def main() -> None:
    u = User(name="Alice")
    println(u.__class_name__())
```

## `__fields__() -> FrozenList[FieldInfo]`

Returns metadata for the type's runtime-visible fields. Fields without `pub` on a public model, and private class fields, are intentionally absent.

```incan
model User:
    name: str
    email: str

def main() -> None:
    u = User(name="Alice", email="alice@example.com")
    for info in u.__fields__():
        println(f"{info.name}: {info.type_name}")
```

## Generic Value Reflection

Generic helpers may call `value.__class_name__()` and `value.__fields__()` on a type parameter. A reflectable model or class has the same immutable field metadata, declaration order and visibility rules through a generic value as through a direct concrete call. The metadata result is `FrozenList[FieldInfo]`. An authored method named `__fields__` remains an ordinary method with its declared result. A non-reflectable receiver without an authored method is refused.

```incan
model User:
    name: str
    email: str


def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())


def main() -> None:
    user = User(name="Alice", email="alice@example.com")
    println(reflected_field_count(user))
```

## Generic Type Reflection

Generic helpers may reflect on an explicit model or class type argument without a value receiver.

```incan
def schema_field_count[T]() -> int:
    return len(T.__fields__())

def schema_name[T]() -> str:
    return T.__class_name__()
```

Callers instantiate those helpers with a reflectable model or class type:

```incan
model User:
    name: str
    email: str

println(schema_name[User]())
println(schema_field_count[User]())
```

Primitive type arguments support `T.__class_name__()`, returning their source names:

```incan
def primitive_target[T]() -> str:
    return str(T.__class_name__())

println(primitive_target[int]())
println(primitive_target[float]())
println(primitive_target[str]())
println(primitive_target[bool]())
```

Primitive type arguments do not support `T.__fields__()`.

## Type Tokens

A type name in value position is accepted only when the expected type is a `Type[T]` token. `Type[T]` is a zero-sized marker, not a general runtime type object. An overload may accept a specific token such as `Type[int]` and declare its own result type. Aliases preserve the overload set. A single generic function has one declared return type; its explicit type arguments do not select different result declarations.

## `FieldInfo` structure

Each `FieldInfo` record contains:

| Field         | Type                              | Description                                                   |
| ------------- | --------------------------------- | ------------------------------------------------------------- |
| `name`        | `FrozenStr`                       | Canonical Incan field identifier                              |
| `alias`       | `Option[FrozenStr]`               | Wire name, if set via `[alias="..."]`                         |
| `description` | `Option[FrozenStr]`               | Documentation string, if set via `[description="..."]`        |
| `wire_name`   | `FrozenStr`                       | Effective wire name (alias if present, else canonical name)   |
| `type_name`   | `FrozenStr`                       | Incan type display (e.g. `"str"`, `"int"`, `"Option[str]"`)   |
| `has_default` | `bool`                            | Whether the field has a default value                         |
| `extra`       | `FrozenDict[FrozenStr, FrozenStr]`| Empty metadata dictionary |

Notes:

- Field metadata like `[alias="..."]` and `[description="..."]` is **model-only**.
- For a `class`, `FieldInfo.alias` and `FieldInfo.description` are always `None` and `FieldInfo.wire_name == FieldInfo.name`.
- Fields without `pub` on a public model, and private class fields, are excluded from runtime `__fields__()`, `__field_value__()`, and `__field_items__()` results.
- Inherited fields retain the declaring package's Incan `type_name` spelling.
- Calling `obj.__fields__()` and inspecting its records requires no `FieldInfo` import. An explicit `FieldInfo` type annotation requires the corresponding import.

## Runtime Field Overlay Views

Models and classes expose read-only field value views.

| API | Returns | Description |
| --- | --- | --- |
| `obj.__field_value__(name: str)` | `Option[T]` | Returns `Some(value)` for a known public field and `None` for a private or unknown field name. `T` is the common field type when all exposed fields share one type, otherwise a union of the exposed field types. |
| `obj.__field_items__()` | `list[tuple[str, T]]` | Returns public `(field_name, value)` pairs in the same field order used by `__fields__()`. `T` follows the same common-type-or-union rule as `__field_value__`. |

For `class` values, inherited fields are included before fields declared on the child class, matching `__fields__()` ordering. The returned views are read-only snapshots of field names and values; mutating a returned list does not add, remove, rename, or update object fields.

Through an inferred generic value parameter, `__field_value__()` returns `Option[str]` and `__field_items__()` returns `list[tuple[str, str]]`. These field-value strings are distinct from the `FrozenList[FieldInfo]` metadata returned by `__fields__()`.
