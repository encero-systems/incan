# std.reflection (reference)

`std.reflection` exposes declaration metadata and typed stored-value protocols.

!!! info "Related pages"
    - [Language → Reference → Reflection] specifies type and instance reflection.
    - [Reflection contracts and use](../../explanation/reflection.md) explains inspection and traversal.

<!-- References -->
[Language → Reference → Reflection]:../reflection.md

## Importing the reflection API

The metadata carrier is imported as:

```incan
from std.reflection import FieldInfo
```

`obj.__fields__()` and `T.__fields__()` require no `FieldInfo` import. An annotation naming `FieldInfo` requires that type in scope.

Primitive type-argument names such as `T.__class_name__()` for `T=int`, `T=float`, `T=str`, or `T=bool` are compiler-backed and also do not require a `std.reflection` import. Type-token parameters such as `Type[int]`, model, enum and trait tokens, and constrained generic overloads are part of the language type system rather than the `std.reflection` import surface. They do not grant the field metadata APIs on the token value. See the [language reflection reference](../reflection.md#type-tokens) for their expected-context, overload, storage, and operation contracts.

## Types

### `FieldInfo`

An Incan model returned in the frozen list from `__fields__()`. `std.primitives.reflection.FieldInfo` and `std.reflection.FieldInfo` name the same type. Explicit construction supplies every field below; `==` and `!=` compare all stored fields.

| Field | Type | Description |
| --- | --- | --- |
| `name` | `FrozenStr` | Current declared Incan field identifier |
| `identity` | `FrozenBytes` | Six-byte compiler-derived identity from the earliest declared name |
| `canonical` | `bool` | Whether the field participates in the canonical stored hash; `true` when absent |
| `renamed_from` | `FrozenList[FrozenStr]` | Earlier declared names in chronological order, earliest first |
| `alias` | `Option[FrozenStr]` | Wire name, if set via `[alias="..."]` |
| `description` | `Option[FrozenStr]` | Documentation string, if set via `[description="..."]` |
| `wire_name` | `FrozenStr` | Effective wire name (alias if present, else declared name) |
| `type_name` | `FrozenStr` | Incan type display (e.g. `"str"`, `"int"`, `"Option[str]"`) |
| `has_default` | `bool` | Whether the field has a default value |
| `default_is_empty` | `bool` | Whether the source default is a literal empty list, dictionary, tuple or set; strings, bytes, missing defaults and unevaluated expressions are false |
| `position` | `int` | Zero-based slot before the public view removes private fields; class slots follow the checked parent-first layout |
| `renamed_from_lifecycle` | `Option[LifecycleInfo]` | The rename block's descriptive labels; absent when no lifecycle labels are declared |
| `extra` | `FrozenDict[FrozenStr, FrozenStr]` | Additional metadata; empty |

Notes:

- Field metadata like `[alias="..."]` and `[description="..."]` is model-only.
- For a `class`, `FieldInfo.alias` and `FieldInfo.description` are always `None`, `FieldInfo.wire_name == FieldInfo.name`, `FieldInfo.canonical` is `true`, and `FieldInfo.renamed_from` is empty.

## Compiler-generated field value views

Model and class values expose `__field_value__(name: str) -> Option[T]` and `__field_items__() -> list[tuple[str, T]]` directly; no `std.reflection` import is required. `T` is the common field type when all exposed fields share one type, otherwise a union of the exposed field types. These views are read-only and use the same field ordering as `__fields__()`.

## Handwritten typed-value protocols

`std.primitives.reflection` and `std.reflection` expose the same declarations. These protocols require explicit adoption and handwritten traversal, reconstruction and shape members.

### `Walk` and `Reconstruct`

`Walk.__walk__[E, W with Walker[E]](self, mut walker: W) -> Result[None, E]` visits the adopter's stored representation. Its static `__shape__() -> ShapeInfo` supplies the corresponding description without constructing a value. `Reconstruct` extends `Walk` and requires static `__reconstruct__[E, S with Source[E]](mut source: S) -> Result[Self, E]`.

A handwritten reconstruction must restore complete valid state and retain the source's error context. It must not replay constructor input normalization. Handwritten reconstruction owns its validation calls.

### `Walker[E]`

All members return `Result[None, E]` and take `mut self`. Scalar and begin/end defaults return `Ok(None)`; the structural defaults below recurse through this same mutable walker, preserving state and accounting.

| Method | Parameters | Default behavior |
| --- | --- | --- |
| `bool`, `str`, `bytes` | `value: bool`, `value: str`, `value: bytes`, respectively | No observation |
| `integer`, `unsigned`, `float` | `value: i128`, `value: u128`, `value: f64`, respectively | No observation |
| `decimal` | `value: DecimalValue` | No observation |
| `none` | None | No observation |
| `some[V with Walk]`, `element[V with Walk]` | `value: V` | Call `value.__walk__(self)` |
| `begin_sequence` | `kind: SequenceKind, length: int` | No observation |
| `end_sequence`, `end_dict`, `end_model`, `end_variant` | None | No observation |
| `begin_dict` | `length: int` | No observation |
| `entry[K with Walk, V with Walk]` | `key: K, value: V` | Walk key, then value; propagate the first error |
| `begin_model` | `shape: ShapeInfo` | No observation |
| `field[V with Walk]` | `info: FieldInfo, value: V` | Call `value.__walk__(self)` |
| `begin_variant` | `info: VariantInfo` | No observation |
| `member` | `spelling: FrozenStr` | No observation |
| `reference` | `kind: FrozenStr, identity: FrozenBytes` | No observation; never fetch the referenced value |

`SequenceKind` has `List`, `Set` and `Tuple` and derives `Eq`. Frozen collections use their corresponding collection kind. A reference identity is format/type-owned data; it is not constrained to the six-byte field identity width.

### `Source[E]`

Reads take `mut self` and return the stated `Result`. Sources implement all members; an unsupported read must fail through the source's own error type. No default silently converts a decoding failure to an absent field.

| Method | Parameters | Result payload |
| --- | --- | --- |
| `bool`, `str`, `bytes` | None | `bool`, `str`, `bytes` |
| `integer`, `unsigned`, `float`, `decimal`, `none` | None | `i128`, `u128`, `f64`, `DecimalValue`, `None` |
| `some[V with Reconstruct]` | None | `Option[V]` |
| `begin_sequence` | `kind: SequenceKind` | `None` |
| `element[V with Reconstruct]` | None | `Option[V]`; outer None is end of sequence |
| `begin_dict` | None | `None` |
| `entry[K with Reconstruct, V with Reconstruct]` | None | `Option[tuple[K, V]]`; outer None is end of dictionary |
| `begin_model` | `shape: ShapeInfo` | `None` |
| `field[V with Reconstruct]` | `info: FieldInfo` | `Option[V]`; outer None means absent field |
| `begin_variant` | `shape: ShapeInfo` | `VariantInfo` |
| `end_sequence`, `end_dict`, `end_model`, `end_variant` | None | `None`; validate complete consumption of the selected container |
| `member` | None | `FrozenStr`; selected union member's stored-type spelling |
| `reference` | `kind: FrozenStr` | `FrozenBytes`; no implicit fetch |

The policy/failure members return values directly rather than Result:

| Method | Receiver and parameters | Return |
| --- | --- | --- |
| `missing` | `mut self, info: FieldInfo` | `E` |
| `takes_default` | `self, info: FieldInfo` | `bool` |
| `invalid` | `mut self, error: ValidationError` | `E` |
| `collects` | `self` | `bool` |
| `collected` | `self` | `Option[E]` |
| `malformed` | `mut self, reason: str` | `E` |
| `numeric_range` | `mut self, requested_type: FrozenStr` | `E` |
| `shape_mismatch` | `mut self, expected: FrozenStr, actual: FrozenStr` | `E` |

A present optional field is `Some(None)` when `V=Option[T]`; a missing field is outer None; a read failure is Err. Implementations retain each located failure once. `takes_default` is an explicit format policy, not permission to execute a constructor factory during shape inspection. End operations reject trailing container data according to the format. Collection cannot return a partially reconstructed model. Depth, size and allocation budgets belong to the source and persist across nested calls.

### Shape carriers

`ShapeInfo` fields are `name: FrozenStr`, `identity: FrozenBytes`, `renamed_from: FrozenList[FrozenStr]`, `renamed_from_lifecycle: Option[LifecycleInfo]`, `spelling: FrozenStr`, `fields: FrozenList[FieldInfo]` and `variants: FrozenList[VariantInfo]`. All are supplied explicitly. Static `ShapeInfo.of[T with Walk]() -> ShapeInfo` calls the adopter's `__shape__()`.

`VariantInfo` fields are `name: FrozenStr`, `identity: FrozenBytes`, `description: Option[FrozenStr]`, `fields: FrozenList[FieldInfo]`, `extra: FrozenDict[FrozenStr, FrozenStr]`, `renamed_from: FrozenList[FrozenStr]` and `renamed_from_lifecycle: Option[LifecycleInfo]`. All are supplied explicitly.

`LifecycleInfo` has `since`, `retire_in`, `message`, `by` and `issue`, each `Option[FrozenStr]`, defaulting to None. Its derived equality compares all five fields. Field, shape and variant records use this same carrier; an explicitly empty label stays `Some("")`. These are descriptive facts, not authorization. The complete shape's closed nominal identity and its structural spelling have different meanings; neither is recovered from its display name.

### `DecimalValue`

`DecimalValue.new(coefficient: i128, precision: u8, scale: u8) -> Result[DecimalValue, DecimalValueError]` is checked construction using `@derive(Validate)`. Direct `DecimalValue(...)` construction is refused. The public fields retain these same three types. Validation requires precision in 1..38, scale in 0..precision, and `-10**precision < coefficient < 10**precision`. It uses integer arithmetic, including for i128 minimum, and never rounds through floating point.

The coefficient is value times `10**scale`; `1990, 10, 2` represents `19.90` declared at precision 10 and scale 2. A failed construction returns a `DecimalValueError` model with `kind: DecimalValueErrorKind`, `coefficient: i128`, `precision: u8` and `scale: u8`. The numeric fields retain all supplied inputs. `DecimalValueErrorKind(str)` has `Precision = "precision"`, `Scale = "scale"` and `Coefficient = "coefficient"`; callers can inspect the category and read the inputs directly without unpacking variant payloads. The kind and error model derive `Eq`; error equality compares the category and all three input fields. `validate(self) -> Result[DecimalValue, DecimalValueError]` applies the same checks. This constructor validates at construction; ordinary mutation follows the language's existing rules.

The carrier preserves exact value and declared scale. It provides no conversion to or from `decimal[P, S]`. Codec conversion must preserve the exact value or return the source's numeric-range failure; compatibility between declared decimal types is format policy.
