# Reflection (Reference)

Models and classes provide built-in reflection helpers for runtime introspection. Generic type-argument reflection also exposes stable source names for primitive type arguments.

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

## Generic Value Reflection

Generic helpers may call `value.__class_name__()` and `value.__fields__()` on a type parameter. The compiler treats those calls as reflection capabilities and emits the required runtime bounds for the generated Rust function, so the generic helper has the same field metadata result as a direct concrete call when it is instantiated with a reflectable model or class.

```incan
def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())
```

## Generic Type Reflection

Generic schema helpers may also reflect on an explicit type argument without constructing a dummy value. This is the intended shape for APIs that need a model's schema rather than one model instance.

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

Primitive type arguments support `T.__class_name__()` for type-directed APIs that need a stable source-level primitive identity:

```incan
def primitive_target[T]() -> str:
    return str(T.__class_name__())

println(primitive_target[int]())    # "int"
println(primitive_target[float]())  # "float"
println(primitive_target[str]())    # "str"
println(primitive_target[bool]())   # "bool"
```

Primitive type arguments do not support `T.__fields__()`, because primitives do not have source fields.

## Type Tokens

A type name in value position is accepted only when the expected type is a compiler-emitted `Type[T]` token. `Type[T]` is a zero-sized marker used for type-directed APIs, not a general runtime type object. This lets libraries offer overloads whose return types remain precise:

```incan
def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return int_column_expr(expr)

def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return float_column_expr(expr)

def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return dynamic_cast(expr, target)

amount: IntColumnExpr = cast(col("amount"), int)
total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
```

Aliases preserve the overload set, so compatibility spellings such as `safe_cast = alias cast` can expose the same `Type[T]` token and fallback-string call surface without wrapper overloads.

Use explicit type arguments such as `helper[int](...)` when an API needs compile-time type reflection inside the function body. A single generic function still has one declared return type, so `cast[T](expr)` cannot by itself express "return `IntColumnExpr` for `T=int` and `FloatColumnExpr` for `T=float`" unless that API declares a broader union or a future type-level return mapping feature.

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
