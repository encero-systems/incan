# RFC 025: multi-instantiation trait dispatch

- **Status:** Draft
- **Created:** 2026-02-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 050 (enum methods & trait adoption)
    - RFC 051 (`JsonValue`)
    - RFC 023 (compilable stdlib and rust.module binding)
    - RFC 024 (extensible derive protocol)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/150
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC proposes allowing a type to adopt multiple instantiations of the same generic trait with different type parameters. When this results in multiple methods with the same name but different parameter types, the compiler resolves which implementation to call based on the argument type at the call site. This is compile-time dispatch, not runtime overloading.

## Motivation

### One trait, multiple key types

Incan's `Index[K, V]` trait (from `std.traits.indexing`) defines `__getitem__(self, key: K) -> V` for subscript access. Some types naturally need indexing by more than one key type:

- `JsonValue` needs `value["key"]` (str) and `value[0]` (int)
- A `DataFrame` might need `df["column"]` (str) and `df[0]` (int for row access)
- A `Matrix` might need `m[(0, 1)]` (tuple) and `m[0]` (int for row slice)

Today, a type can only adopt `Index` once — `with Index[str, V]` or `with Index[int, V]`, but not both — because same-name trait methods still collide in the current language model.

### Rust handles this naturally

Rust already demonstrates that this pattern is coherent: one type may implement the same generic trait multiple times with different type parameters, and the compiler selects the matching implementation from type context. Incan should support the same underlying capability instead of forcing users into wrapper traits or artificial API splits.

## Non-Goals

- **General method overloading.** This RFC does not add the ability to define two freestanding `def foo(x: int)` and `def foo(x: str)` at module level. Same-name methods are permitted only when they arise from different trait instantiations.
- **Runtime dispatch.** Resolution happens at compile time based on argument types. There is no dynamic dispatch or `isinstance`-style checking.
- **Union types.** `str | int` as a first-class type is a separate concern. This RFC solves the multi-key problem through the trait system, not through type unions.

## Guide-level explanation (how users think about it)

### Adopting a trait multiple times

A type can adopt the same generic trait with different type parameters:

```incan
from std.traits.indexing import Index

enum JsonValue with Index[str, JsonValue], Index[int, JsonValue]:
    Null
    Bool(bool)
    Int(int)
    Float(float)
    String(str)
    Array(List[JsonValue])
    Object(Dict[str, JsonValue])

    def __getitem__(self, key: str) -> JsonValue: ...
    def __getitem__(self, key: int) -> JsonValue: ...
```

The two `__getitem__` methods are not overloads. They are implementations of two different trait instantiations, and the compiler matches each definition to its trait by comparing parameter types.

### Call-site resolution

The compiler resolves which implementation to use based on the argument type at the call site:

```incan
value["name"]    # str argument → Index[str, JsonValue].__getitem__
value[0]         # int argument → Index[int, JsonValue].__getitem__
```

This works in chains too:

```incan
value["users"][0]["name"].as_str()
```

Each `[]` resolves independently based on its argument type.

> Note: RFC 051 covers the draft `JsonValue` surface, while RFC 050 covers the enum-language features that make an enum-backed design possible.

### Works for any generic trait

This is not `Index`-specific. Any generic trait can be adopted multiple times:

```incan
trait Into[T]:
    def into(self) -> T: ...

model Measurement with Into[float], Into[int]:
    raw: float

    def into(self) -> float:
        return self.raw

    def into(self) -> int:
        return round(self.raw)

# Context always disambiguates:
reading = Measurement(raw=1.23)
let precise: float = reading.into()   # Into[float]
let rounded: int = reading.into()     # Into[int]
```

The compiler resolves `reading.into()` based on the expected return type from context (the binding's type annotation). If the context doesn't disambiguate, the compiler reports an ambiguity error.

### Multi-format serialization (the foundational use case)

Beyond `Index` and `Into`, the primary real-world motivation for multi-instantiation is **multi-format serialization**. Many applications require a single model to serve multiple wire formats — JSON, YAML, Protobuf, Avro — each via a derivable trait. A generic `Serializable[F]` trait makes this composable:

```incan
trait Serializable[F]:
    def serialize(self) -> bytes: ...

@derive(json, yaml)
model CustomerEvent with Serializable[Json], Serializable[Yaml]:
    customer_id: str
    email: str
```

A generic function can then operate over any format and any model without knowing which one:

```incan
def publish[F, T with Serializable[F]](event: T) -> bytes:
    return event.serialize()

# Call site: F and T are resolved at monomorphization
let json_bytes = publish[Json](my_event)   # T inferred as CustomerEvent; Serializable[Json]::serialize
let yaml_bytes = publish[Yaml](my_event)  # Serializable[Yaml]::serialize
```

The type parameter `F` determines which `Serializable` instantiation the compiler selects. The bound `T with Serializable[F]` links the model type to the format using ordinary Incan `with` syntax in the type-parameter list. This pattern is the foundation of the `@derive(format)` protocol described in RFC 024.

## Reference-level explanation (precise rules)

### Trait adoption

A type may list the same trait name multiple times in its `with` clause, provided each instantiation has different type arguments:

```incan
model Foo with Trait[A], Trait[B]:  # OK — different type args
model Bar with Trait[A], Trait[A]:  # ERROR — duplicate instantiation
```

### Method disambiguation

When multiple trait instantiations produce methods with the same name, the compiler resolves which to call using:

1. **Argument types** — the most common case. `value["key"]` vs `value[0]` is unambiguous because `str` and `int` are distinct types.
2. **Expected return type** (provisional) — when argument types are identical but return types differ (for example `Into[float]` vs `Into[int]`), the compiler may use the expected type from surrounding context such as a binding annotation or function argument. See the unresolved questions section for the remaining design work here.
3. **Explicit qualification** — if neither argument nor return type disambiguates, the call is an error. The user must qualify which trait they mean. The exact qualification syntax is still open, but this should be the fallback rather than the common path.

### Symbol table representation

The language and compiler model must support more than one same-name method entry when those methods arise from distinct trait instantiations. The exact internal representation is implementation detail, but the public rule is that trait-origin information must be preserved well enough for type-directed dispatch and diagnostics to stay coherent.

### Rust emission

Each trait instantiation lowers to a separate Rust trait implementation. That backend mapping is straightforward and is one reason this RFC is a good semantic fit for the language rather than a forced abstraction.

## Design details

### Syntax

No new syntax is proposed. The existing `with Trait[A], Trait[B]` clause already parses a comma-separated list of trait adoptions. If the parser currently rejects duplicate trait names in the `with` clause, that restriction must be lifted. Same-name `def` declarations are permitted inside the body when they correspond to different trait instantiations.

### Semantics

The rule is simple: **same-name methods are permitted if and only if they satisfy different `with` trait adoptions.** This is not general overloading — it's the trait system resolving dispatch.

### Interaction with existing features

#### Enum, model, and class types

Multi-instantiation works on all three declaration types that support `with`.

#### Built-in types (`List`, `Dict`)

Built-in collection types currently have compiler-level indexing support. This RFC does not change that, but it provides the mechanism for user-defined types to achieve the same capability through traits.

#### `@rust.extern` methods

`@rust.extern` methods in multi-instantiation traits work normally. Each `__getitem__` can independently be `@rust.extern` or pure Incan.

#### Generic function bounds

A generic function can require multiple instantiations of the same trait in its bounds:

```incan
def lookup[T with Index[str, V], Index[int, V]](data: T, key: str, idx: int) -> V:
    ...
```

This falls out naturally from the trait system. Each `with` bound is an independent constraint, and the function body can call `data[key]` and `data[idx]` with each use resolving to the matching `Index` instantiation.

#### Cross-trait method name collisions

Multi-instantiation of the *same* generic trait is the primary use case, but the same-name rule applies to *any* combination of adopted traits. Consider:

```incan
trait Readable:
    def read(self, n: int) -> str: ...

trait Parseable:
    def read(self, s: str) -> SomeResult: ...

model Source with Readable, Parseable:
    def read(self, n: int) -> str: ...        # satisfies Readable
    def read(self, s: str) -> SomeResult: ... # satisfies Parseable
```

This is permitted — the two `read` methods come from different trait adoptions and have different parameter types, so the compiler can disambiguate at the call site. However, if two different traits produce methods with **identical signatures**, no disambiguation is possible and the type declaration is an error (see [Diagnostics](#diagnostics) below).

> Note: permitted does not mean that this is encouraged. Avoid ambiguous method signatures whenever possible.

### Diagnostics

The following error scenarios must have clear, actionable diagnostics.

#### 1. Duplicate method with no trait backing

Same-name methods without corresponding trait adoptions are never permitted:

```incan
model Foo:
    def process(self, x: int) -> str: ...
    def process(self, x: str) -> str: ...
    #   ^^^^^^^ error: duplicate method `process`
    #   note: same-name methods are only permitted when they implement
    #         different trait adoptions in the `with` clause
```

#### 2. Duplicate trait instantiation

Adopting the same trait with identical type arguments is redundant and likely a mistake:

```incan
model Bar with Index[str, int], Index[str, int]:
    #                           ^^^^^^^^^^^^^^^^ error: duplicate trait
    #           instantiation `Index[str, int]` — each instantiation
    #           must have different type arguments
```

#### 3. Ambiguous call (argument types match multiple candidates)

When the argument type doesn't uniquely select a trait instantiation:

```incan
value.some_method(x)
#     ^^^^^^^^^^^ error: ambiguous call to `some_method` — multiple
#     trait instantiations match:
#       - TraitA.some_method(self, x: int) -> str
#       - TraitB.some_method(self, x: int) -> bool
#     help: annotate the expected return type, or use explicit trait
#     qualification (see deferred question #1)
```

#### 4. Irreconcilable cross-trait collision

Two different traits produce methods with identical signatures on the same type:

```incan
trait Logger:
    def write(self, msg: str) -> None: ...

trait Serializer:
    def write(self, msg: str) -> None: ...

model Sink with Logger, Serializer:
    #              ^^^^ error: method `write` from `Serializer` conflicts
    #   with `write` from `Logger` — both have signature
    #   `(self, msg: str) -> None` and cannot be disambiguated
    #   help: rename one of the trait methods, or adopt only one of
    #   these traits
```

### Compatibility / migration

Fully additive. Existing code that adopts a trait once is unaffected. The only new capability is adopting the same trait with different type arguments.

## Alternatives considered

### 1. Union types (`str | int`)

A single `Index[str | int, JsonValue]` adoption with one `__getitem__`. Rejected as a dependency — union types are a larger language feature. Multi-instantiation dispatch solves the immediate problem through the existing trait system.

### 2. `@overload` decorator (Python-style)

Declare multiple signatures, implement once with runtime dispatch. Rejected because it's a runtime mechanism in a compile-time language. Multi-instantiation dispatch is resolved entirely at compile time.

### 3. Separate method names

`get_by_key(str)` and `get_by_index(int)` instead of two `__getitem__`. Works but breaks the `[]` subscript syntax
and feels un-Pythonic.

### 4. Compiler special-casing per type

Give `JsonValue` special compiler support for multi-key indexing without a general mechanism. Rejected because it doesn't scale — every type with the same need would require its own compiler special-case.

## Drawbacks

- **Ambiguity errors**: when the compiler can't determine which instantiation to use from context, it must report an
error. The error messages need to be clear about *why* the call is ambiguous and *how* to resolve it.
- **Symbol table complexity**: the method table representation needs to support multiple entries per method name. This
is an internal complexity increase, though the user-facing model is simple.
- **Compile-time cost**: resolving multi-instantiation dispatch requires checking argument types against all candidates.
For typical usage (2-3 instantiations), this is negligible.
- **Teachability**: "two methods with the same name" is a new concept for Python-background users, who are accustomed to
one-name-one-definition. The key teaching point is that these are *trait implementations*, not overloads — the trait system makes the distinction principled rather than ad-hoc. This puts a high bar on tooling: the LSP must surface which trait instantiation a call resolves to (e.g., hover info showing `Index[str, JsonValue].__getitem__`), and diagnostics for ambiguous calls must clearly explain the competing candidates and how to disambiguate.

## Implementation architecture

- [ ] Verify parser allows duplicate trait names in `with` clauses; lift restriction if needed
- [ ] Update the symbol table to support multiple method entries per name (grouped by trait origin)
- [ ] Update the typechecker to allow same-name methods when they correspond to different trait instantiations
- [ ] Update call resolution to disambiguate based on argument types (and optionally return type context, see
[deferred question #2](#deferred-questions))
- [ ] Update lowering to emit separate `impl Trait<T> for Type` blocks per instantiation
- [ ] Add diagnostics:
    - Duplicate trait instantiation (`with Trait[A], Trait[A]`)
    - Ambiguous call (argument types match multiple candidates)
    - Return-type ambiguity (if return-type dispatch is supported: explain how to annotate the expected type)
- [ ] Add codegen snapshot tests for multi-instantiation dispatch
- [ ] Add integration tests for `Index` with multiple key types

## Layers affected

- **Typechecker / symbol resolution**: must allow multiple instantiations of the same generic trait on one type and resolve call sites against the correct instantiation.
- **Method resolution**: same-name methods arising from different trait instantiations must remain distinguishable without turning into general-purpose overloading.
- **Lowering / emission**: must preserve the resolved instantiation choice into generated backend code without introducing runtime dispatch.
- **Docs / tooling**: must explain ambiguity diagnostics and qualification escape hatches clearly when the compiler cannot pick one instantiation unambiguously.

## Design Decisions

1. **Trait-driven, not general overloading**: same-name methods are only allowed when they arise from different trait
instantiations. This keeps the language simple and the dispatch rule principled.

2. **Compile-time resolution**: no runtime dispatch. The compiler knows which implementation to call from the argument
types at the call site.

## Unresolved questions

1. **Explicit qualification syntax**: when disambiguation fails, how does the user specify which trait instantiation
they mean? Options include `value.Index[str].__getitem__("key")`, turbofish-style `value.__getitem__::<str>("key")`, or something else. This is expected to be rare in practice.

2. **Return-type-only disambiguation**: is it sufficient to resolve based on expected return type alone (e.g.,
   `let precise: float = reading.into()`)? Or should this always be an error requiring explicit qualification?

3. **Type-parameter-level dispatch**: in the multi-format serialization pattern, disambiguation comes from a generic
type parameter resolved at monomorphization, not from argument or return types at the call site:

   ```incan
   def publish[F, T with Serializable[F]](event: T) -> bytes:
       return event.serialize()
   ```

When the caller writes `publish[Json](my_event)`, `F = Json` and `T` is inferred from the argument. The compiler picks `Serializable[Json]::serialize`. This uses standard Incan `with` syntax in the type parameter list — no
   `where` clause. Incan will need this mechanism once generic functions with trait bounds are implemented.

## References

- RFC 050 — Enum Methods and Enum Trait Adoption
- RFC 051 — `JsonValue` for `std.json`
- RFC 023 — Compilable Stdlib & Rust Module Binding
- RFC 024 — Extensible Derive Protocol
- Rust trait system — multiple `impl Trait<T> for Type` blocks

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
