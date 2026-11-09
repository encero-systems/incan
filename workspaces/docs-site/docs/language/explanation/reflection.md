# Reflection contracts and use

Reflection separates a declaration's field descriptions from an instance's field values. A schema helper can inspect a model type without constructing a dummy value. A generic helper that already has an instance can ask for the same metadata through that instance. The [reflection reference](../reference/reflection.md) lists the exact operations, result types and visibility rules.

## Inspecting an instance

```incan
model User:
    name: str
    email: str


def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())


def main() -> None:
    user = User(name="Alice", email="alice@example.com")
    println(user.__class_name__())
    println(reflected_field_count(user))
    field_names = [field.name for field in user.__fields__()]
    has_email = any(field.name == "email" for field in user.__fields__())
    println(field_names)
    println(has_email)
```

The class-name call prints `User`; the field count is `2`. The two comprehensions collect the canonical field names and check whether the declaration exposes `email`. No explicit `FieldInfo` import is needed to inspect returned records; import the type when an annotation needs to name it.

The compiler retains a generic value's selected reflection operation until the receiver's concrete declaration is known. It then uses that declaration's metadata and visibility rules. This preserves immutable field descriptions instead of changing metadata into the string-valued result returned by generic field-value reflection. Named field overlays provide a stable view for collection and reflection protocols without dynamic attribute access; their returned lists do not mutate the original object.

Runtime reflection omits private fields. Checked API metadata and library manifests still retain those declarations and their visibility for compiler use. Inherited `type_name` values preserve the declaring package's Incan spelling even when native compilation uses a qualified provider-owned type. `FieldInfo.extra` is reserved for additional metadata and remains empty.

## Selecting a type without an instance

An explicit type argument such as `schema_field_count[User]()` selects the schema without constructing a `User`. Use this form for serializers or schema descriptions that do not otherwise need a value. Primitive type arguments expose their source names through `T.__class_name__()`: `int`, `float`, `str` and `bool` each produce the corresponding name. They have no source fields, so `T.__fields__()` is unavailable for those arguments.

A `Type[T]` token serves a different purpose: it lets an ordinary argument select a typed overload. For example, a column library could define the following API. Here `ColumnExpr`, `IntColumnExpr`, `FloatColumnExpr`, `col`, the conversion functions and `mul` belong to that hypothetical library, not to the standard library.

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

Each overload declares its result type. A symbol alias such as `safe_cast = alias cast` preserves that overload set without wrapper functions. By contrast, `cast[T](expr)` has one declared result type; explicit type arguments alone do not give it a different result declaration for each `T`. A broader union is possible where that is the intended API. A future type-level return mapping would be a separate capability, not a consequence of these tokens.

## Handwritten traversal and compiler-generated metadata

The typed `Walk` and `Reconstruct` protocols let a library use the adopter's own code to visit or restore its stored representation. They are distinct from public-field inspection. Importing the protocols does not generate those members or make scalars and containers adopt them. Automatic adoption and complete stored-field generation are separate compiler work. The public [protocol reference](../reference/stdlib/reflection.md#handwritten-typed-value-protocols) lists the handwritten API.

The primitive protocol leaves the existing native-backed `FieldInfo` carrier intact. Its planned `stored_type`, `default_is_empty`, `position` and `renamed_from_lifecycle` extensions belong to the compiler's metadata producer; an Incan-only shadow would describe facts the producer does not supply. A handwritten shape must describe the representation it actually visits, not claim that generated metadata already exists. DecimalValue preserves exact coefficient and declared scale; automatic decimal adaptation likewise belongs to that later compiler work.
