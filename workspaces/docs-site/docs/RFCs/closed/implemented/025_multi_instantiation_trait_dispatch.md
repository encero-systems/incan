# RFC 025: multi-instantiation trait dispatch

- **Status:** Implemented
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
- **Shipped in:** v0.3

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

reading = Measurement(raw=1.23)
precise: float = reading.into()   # Into[float]
rounded: int = reading.into()     # Into[int]
```

The compiler resolves `reading.into()` from the expected return type when the candidates are instantiations of the same generic trait family. The expected type may come from a typed binding, a function argument, an annotated return position, or equivalent explicit context. `let` is optional in Incan; what matters is the presence of a concrete expected type.

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

A generic function can operate over any format and any model without knowing which one:

```incan
def publish[F, T with Serializable[F]](event: T) -> bytes:
    return event.serialize()

# Call site: F and T are resolved at monomorphization.
json_bytes = publish[Json](my_event)   # T inferred as CustomerEvent; Serializable[Json]::serialize
yaml_bytes = publish[Yaml](my_event)   # Serializable[Yaml]::serialize
```

The type parameter `F` determines which `Serializable` instantiation the compiler selects. The bound `T with Serializable[F]` links the model type to the format using ordinary Incan `with` syntax in the type-parameter list. This pattern is the foundation of the `@derive(format)` protocol described in RFC 024 and is in scope for this RFC.

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
2. **Expected return type within one generic trait family** — when argument types are identical but return types differ across instantiations of the same generic trait (for example `Into[float]` vs `Into[int]`), the compiler may use an explicit expected type from surrounding context such as a typed binding, function argument, or annotated return position.
3. **Resolved generic trait parameters** — when a generic bound links a receiver type parameter to a trait instantiation parameter, as in `T with Serializable[F]`, a resolved `F` selects the corresponding trait method during generic checking and monomorphization.
4. **Ambiguity error** — if the preceding rules do not choose exactly one candidate, the call is an error. This RFC does not introduce explicit qualification or aliasing syntax for cross-trait same-name collisions.

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

The implementation covers models, classes, and enums. Enum support depends on RFC 050's enum method and trait-adoption surface, so RFC 025 implementations must be based on a compiler version that includes RFC 050.

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

Generic bounds may also bind a trait instantiation to another type parameter:

```incan
def publish[F, T with Serializable[F]](event: T) -> bytes:
    return event.serialize()
```

At a call such as `publish[Json](event)`, the explicit call-site type argument resolves `F` to `Json`, the event argument resolves `T`, and the body call `event.serialize()` selects the `Serializable[Json]` method. If `F` cannot be resolved from explicit type arguments, value arguments, or other ordinary inference, the compiler reports an unresolved generic dispatch error rather than guessing.

#### Cross-trait method name collisions

Multi-instantiation of the *same* generic trait is the primary use case. Same-name methods from different trait families remain errors in RFC 025, even when their value-parameter types differ:

```incan
trait Readable:
    def read(self, n: int) -> str: ...

trait Parseable:
    def read(self, s: str) -> SomeResult: ...

model Source with Readable, Parseable:
    def read(self, n: int) -> str: ...        # satisfies Readable
    def read(self, s: str) -> SomeResult: ... # satisfies Parseable
    # error: cross-trait same-name method collision
```

The restriction is intentional for this RFC: different trait families need explicit qualification or aliasing to lower reliably, and that syntax is not defined here. Return-type disambiguation is reserved for different instantiations of the same generic trait family. Future explicit qualification or trait-aliasing syntax may relax this, but RFC 025 reports these cases as conflicts instead of guessing.

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

#### 3. Ambiguous call (no expected return type)

When a same-family generic trait method can only be resolved by return type, but no expected type is available:

```incan
value = measurement.into()
#                   ^^^^ error: ambiguous call to `into` — multiple
#                        `Into[T]` instantiations match
#     help: add an expected type, for example:
#           value: float = measurement.into()
```

#### 4. Irreconcilable cross-trait collision

Two different trait families produce methods with the same name and the same value-parameter types on the same type:

```incan
trait Logger:
    def write(self, msg: str) -> None: ...

trait Serializer:
    def write(self, msg: str) -> bytes: ...

model Sink with Logger, Serializer:
    #              ^^^^ error: method `write` from `Serializer` conflicts
    #   with `write` from `Logger` — both accept `(self, msg: str)`;
    #   return-type disambiguation only applies within one generic
    #   trait family
    #   help: rename or alias one of the trait methods, or adopt only
    #   one of these traits
```

### Compatibility / migration

Fully additive. Existing code that adopts a trait once is unaffected. The only new capability is adopting the same trait with different type arguments.

## Alternatives considered

### 1. Union types (`str | int`)

A single `Index[str | int, JsonValue]` adoption with one `__getitem__`. Rejected as a dependency — union types are a larger language feature. Multi-instantiation dispatch solves the immediate problem through the existing trait system.

### 2. `@overload` decorator (Python-style)

Declare multiple signatures, implement once with runtime dispatch. Rejected because it's a runtime mechanism in a compile-time language. Multi-instantiation dispatch is resolved entirely at compile time.

### 3. Separate method names

`get_by_key(str)` and `get_by_index(int)` instead of two `__getitem__`. Works but breaks the `[]` subscript syntax and feels un-Pythonic.

### 4. Compiler special-casing per type

Give `JsonValue` special compiler support for multi-key indexing without a general mechanism. Rejected because it doesn't scale — every type with the same need would require its own compiler special-case.

## Drawbacks

- **Ambiguity errors**: when the compiler can't determine which instantiation to use from context, it must report an error. The error messages need to be clear about *why* the call is ambiguous and *how* to resolve it.
- **Symbol table complexity**: the method table representation needs to support multiple entries per method name. This is an internal complexity increase, though the user-facing model is simple.
- **Compile-time cost**: resolving multi-instantiation dispatch requires checking argument types against all candidates. For typical usage (2-3 instantiations), this is negligible.
- **Teachability**: "two methods with the same name" is a new concept for Python-background users, who are accustomed to one-name-one-definition. The key teaching point is that these are *trait implementations*, not overloads — the trait system makes the distinction principled rather than ad-hoc. This puts a high bar on tooling: the LSP must surface which trait instantiation a call resolves to (e.g., hover info showing `Index[str, JsonValue].__getitem__`), and diagnostics for ambiguous calls must clearly explain the competing candidates and how to disambiguate.

## Implementation architecture

Implementation must preserve trait-instantiation identity from adoption collection through method lookup, call resolution, lowering, and Rust emission. The parser and declaration collector may accept multiple adoptions with the same trait name only when their type arguments differ; duplicate identical instantiations must be rejected with a targeted diagnostic. The typechecker must allow same-name methods only when each candidate can be matched to a distinct adopted trait method signature, and call resolution must choose a single candidate from argument types, same-family expected return type, or resolved generic trait parameters. Lowering and emission must carry the resolved trait instantiation to generated Rust so each adopted instantiation emits as its own trait implementation rather than a runtime branch. Test coverage must include duplicate-instantiation diagnostics, duplicate unbacked methods, unresolved return-type dispatch, cross-trait collisions, generic-bound dispatch, codegen snapshots, and an `Index`-style integration case.

## Layers affected

- **Typechecker / symbol resolution**: must allow multiple instantiations of the same generic trait on one type and resolve call sites against the correct instantiation.
- **Method resolution**: same-name methods arising from different trait instantiations must remain distinguishable without turning into general-purpose overloading.
- **Lowering / emission**: must preserve the resolved instantiation choice into generated backend code without introducing runtime dispatch.
- **Docs / tooling**: must explain ambiguity diagnostics and qualification escape hatches clearly when the compiler cannot pick one instantiation unambiguously.

## Implementation Plan

### Phase 1: Trait Adoption Identity

- Preserve each adopted trait instantiation as a distinct semantic entry instead of collapsing adoptions by trait name.
- Reject duplicate identical trait instantiations such as `Trait[A], Trait[A]` with a targeted diagnostic.
- Keep compatibility with existing single-adoption code and existing trait-supertrait rules from RFC 042.

### Phase 2: Same-Name Method Conformance

- Allow same-name methods on a type only when each method satisfies a distinct adopted trait method signature.
- Reject duplicate unbacked same-name methods.
- Reject cross-trait same-name collisions because this RFC has no aliasing or qualification syntax to disambiguate them.

### Phase 3: Call Resolution

- Resolve same-name trait methods by argument types first.
- Resolve same-family generic trait methods by explicit expected return type when argument types alone are insufficient.
- Resolve calls through generic bounds such as `T with Serializable[F]` when the trait-instantiation parameter is known.
- Emit clear diagnostics for ambiguous calls, unresolved generic dispatch, and missing expected return types.

### Phase 4: Lowering and Emission

- Preserve the selected trait instantiation in lowering so later stages do not rediscover or guess the dispatch target.
- Emit separate Rust trait implementations for each adopted instantiation.
- Ensure generated Rust uses the selected trait implementation without runtime branching.

### Phase 5: Tests and Documentation

- Add parser/typechecker tests for duplicate instantiations, valid multi-instantiation adoption, duplicate unbacked methods, cross-trait conflicts, argument-based dispatch, expected-return dispatch, and generic-bound dispatch.
- Add codegen snapshot coverage for multi-instantiation trait implementations and selected calls.
- Add an integration test for an `Index[str, V]` plus `Index[int, V]` style type.
- Update user-facing trait docs and release notes.

## Progress Checklist

### Spec / design

- [x] Resolve explicit qualification / aliasing scope: cross-trait same-name collisions are errors for RFC 025; aliasing remains future work.
- [x] Resolve return-type disambiguation scope: expected return type may disambiguate within one generic trait family.
- [x] Resolve generic type-parameter dispatch scope: `T with Serializable[F]` style dispatch is in scope.

### Trait adoption identity

- [x] Preserve duplicate trait-name adoptions as distinct instantiations when type arguments differ for model/class declarations.
- [x] Reject duplicate identical trait instantiations for model/class declarations.
- [x] Extend trait-adoption identity to enum declarations after RFC 050 lands on `main`.
- [x] Add remaining focused tests for enum adoption identity and duplicate-instantiation diagnostics.

### Same-name method conformance

- [x] Allow same-name methods backed by distinct trait instantiations for model/class declarations.
- [x] Reject duplicate unbacked same-name methods for model/class declarations.
- [x] Reject cross-trait same-name collisions for model/class declarations.
- [x] Extend same-name method conformance to enum methods after RFC 050 lands on `main`.
- [x] Add remaining focused tests for enum-backed valid and invalid same-name method declarations.

### Call resolution

- [x] Resolve method calls by argument type across model/class multi-instantiated traits.
- [x] Resolve same-family methods by explicit expected return type.
- [x] Resolve generic-bound dispatch for `T with Trait[F]` when `F` is known.
- [x] Emit diagnostics for ambiguous same-family calls and cross-trait collisions.
- [x] Extend call resolution to enum methods after RFC 050 lands on `main`.
- [x] Add remaining typechecker tests for enum call-resolution paths.

### Lowering / emission

- [x] Preserve selected trait instantiation through model/class lowering.
- [x] Emit separate Rust impls for each model/class trait instantiation.
- [x] Ensure generated model/class call sites target the selected implementation without runtime dispatch.
- [x] Extend lowering/emission to enum methods after RFC 050 lands on `main`.
- [x] Add codegen coverage for same-family model/class and enum trait implementations.

### Docs / release notes

- [x] Update authored trait/stdlib docs for model/class/enum multi-instantiation dispatch.
- [x] Add a v0.3 release notes entry for the implemented slice.
- [x] Keep RFC 025 progress checklist current as implementation lands.

## Design Decisions

1. **Trait-driven, not general overloading**: same-name methods are only allowed when they arise from different trait instantiations. This keeps the language simple and the dispatch rule principled.

2. **Compile-time resolution**: no runtime dispatch. The compiler knows which implementation to call from the argument types at the call site.

3. **Expected return type may disambiguate within one generic trait family**: when multiple instantiations of the same generic trait differ only by return type, an explicit expected type may select the matching instantiation. If no expected type is available, the call is ambiguous.

4. **Generic trait-parameter dispatch is in scope**: a bound such as `T with Serializable[F]` links the receiver type to a trait instantiation selected by `F`. Once `F` is resolved, calls through that bound select the corresponding trait method.

5. **Cross-trait same-name collisions remain errors for now**: this RFC does not define explicit qualification or aliasing syntax for two different trait families that expose the same method name. Future work may add trait-level aliasing or qualification syntax, but this RFC reports those cases as conflicts instead of guessing.

6. **Union returns are not the dispatch mechanism**: if argument types choose a single candidate, the call keeps that candidate's precise return type. Union return types remain useful for APIs that intentionally return a union, but they are not a substitute for trait-instantiation dispatch.

## References

- RFC 050 — Enum Methods and Enum Trait Adoption
- RFC 051 — `JsonValue` for `std.json`
- RFC 023 — Compilable Stdlib & Rust Module Binding
- RFC 024 — Extensible Derive Protocol
- Rust trait system — multiple `impl Trait<T> for Type` blocks
