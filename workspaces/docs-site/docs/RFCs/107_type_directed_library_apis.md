# RFC 107: Type-directed library APIs and compile-time type tokens

- **Status:** Draft
- **Created:** 2026-06-03
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (overload-based dispatch)
    - RFC 036 (user-defined decorators)
    - RFC 048 (contract-backed models emit and tooling)
    - RFC 054 (explicit call-site generic arguments)
    - RFC 083 (symbol and method aliases)
    - RFC 098 (native associated types)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/752
- **RFC PR:** https://github.com/dannys-code-corner/incan/pull/751
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines the north-star model for type-directed library APIs in Incan: a library may accept compiler-backed `Type[T]` tokens such as `int`, `float`, `str`, `bool`, and model names under explicit expected-type context, use those tokens in overload dispatch to preserve precise return types, and expose the resulting callable surface through aliases, decorators, package manifests, generated documentation, and facade modules without changing behavior across import boundaries. Type names remain compile-time names by default; this RFC does not make types generally first-class runtime values.

## Core model

1. **Types are not generally values:** a bare type name in value position is valid only when expected typing requires `Type[T]`.
2. **`Type[T]` is compiler-backed evidence:** a type token carries a checked source type for dispatch and metadata, not a general runtime type object.
3. **Type-token overloads are ordinary overloads:** overload resolution may use `Type[int]`, `Type[float]`, `Type[str]`, `Type[bool]`, model tokens, and other supported token types to select precise signatures.
4. **Aliases preserve overload identity:** `safe_cast = alias cast` exposes the same overload set under another name rather than creating wrapper functions.
5. **Decorators see source identity:** decorators applied to overloads observe the source callable identity and checked callable surface, not generated implementation names.
6. **Package boundaries must not change behavior:** importing through `from module import name`, `pub from module import name`, package manifests, and facade modules must preserve the same callable surface as same-module use.
7. **Reflection remains bounded:** primitive and model type metadata may support library authoring, but arbitrary type-level computation and runtime reflection are separate features.

## Motivation

Incan libraries increasingly need APIs where the caller chooses a type as part of the operation. Common examples include typed casts, schema selection, serializers, column builders, readers, adapters, and registries. A stringly API such as `cast(expr, "float64")` is easy to expose but weakens typechecking, documentation, completion, and refactoring. Helper families such as `cast_float(expr)` and `cast_int(expr)` are precise but duplicate the public API and make aliases, docs, and decorators drift. Broad union returns keep a single function name, but the caller then loses the useful fact that `cast(expr, float)` returns a float-shaped value.

The v0.3 `Type[T]` work provides a narrow stepping stone: a type name may be used as a value only when the expected type is a compiler-backed `Type[T]` token, and overload resolution can use that token to choose a precise return type. That shape is intentionally narrower than first-class runtime types. It solves the immediate library-authoring problem without claiming that Incan has a complete type-level programming model.

The larger design still needs an RFC because this surface touches several language guarantees at once. A callable should not work locally and then change meaning when imported through a facade. An alias should mean the same thing as the symbol it aliases, with another name. A decorator should observe the callable the author wrote, not generated implementation names. Package metadata and generated docs should describe the same checked surface users can call. Without a deliberate contract, this area risks continuing to evolve as a sequence of narrow fixes around individual library cases.

## Goals

- Define `Type[T]` as the language-level type-token parameter shape for type-directed APIs.
- Allow supported type names to appear in value position only when expected typing requires `Type[T]`.
- Allow overload resolution to select precise callable signatures using `Type[T]` arguments.
- Support primitive type tokens for at least `int`, `float`, `str`, and `bool`.
- Support model type tokens where the model type is visible and checked.
- Preserve overload sets through top-level aliases and public reexports.
- Preserve decorated overload behavior, side effects, callable identity, defaults, checked signatures, and metadata.
- Preserve the same semantics through same-module use, cross-module imports, public package imports, facade reexports, test harnesses, and generated documentation.
- Define diagnostics for bare type names used without `Type[T]` expected context.
- Document where this feature stops so users do not infer broader type-level programming support.

## Non-Goals

- Making all types first-class runtime values.
- Allowing arbitrary type expressions as ordinary runtime data.
- Defining a general `Type` object API comparable to Python's runtime classes.
- Defining `cast[T](expr)` return specialization as part of this RFC.
- Defining general type-level functions, type switches, dependent return types, or arbitrary compile-time evaluation.
- Replacing explicit call-site generics from RFC 054.
- Replacing associated types from RFC 098.
- Allowing overload aliases to change signatures, defaults, decorators, or runtime behavior.
- Defining new syntax for overload declaration beyond the existing overload model.

## Guide-level explanation

A type-directed library API uses `Type[T]` when the type is an argument to the API contract rather than a value carried by the user's data.

```incan
model ColumnExpr:
    name: str

model IntColumnExpr:
    source: str

model FloatColumnExpr:
    source: str

def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)

def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)

amount = cast(col("amount"), int)
price = cast(col("price"), float)
```

The `int` and `float` names in the call are accepted because the overload candidates create an expected `Type[int]` or `Type[float]` context. Outside such a context, the same spelling remains an error:

```incan
def accepts_any[T](value: T) -> None:
    return

accepts_any(int)  # error: `int` is a type name, not an ordinary runtime value
```

This keeps the feature useful without pretending that types are normal objects. The caller can choose the target type, and the return type stays precise:

```incan
def mul(left: FloatColumnExpr, right: FloatColumnExpr) -> FloatColumnExpr:
    return FloatColumnExpr(source="mul")

total = mul(cast(col("unit_price"), float), cast(col("qty"), float))
```

Aliases preserve the overload set. A compatibility spelling does not need wrapper overloads:

```incan
safe_cast = alias cast

value = safe_cast(col("amount"), float)
fallback = safe_cast(col("amount"), "decimal(10,2)")
```

Decorators must see the source callable identity. If a registry decorator records callable metadata, an overloaded `cast` implementation should still register as `cast`, not as a generated backend implementation name:

```incan
@register()
def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)
```

The same API must work through public package boundaries:

```incan
# casts.incn
pub safe_cast = alias cast

# lib.incn
pub from casts import cast, safe_cast

# consumer
from pub::typed_columns import cast, safe_cast

amount = cast(col("amount"), int)
safe = safe_cast(col("safe"), float)
```

## Reference-level explanation

### Type-token type form

`Type[T]` names a compiler-backed token whose payload is the checked type `T`. `Type[T]` is a type in the source type system. Values of this type are not user-constructed objects; they are introduced by expected-type checking when a visible type name appears in a value position that expects `Type[T]`.

An implementation must support `Type[int]`, `Type[float]`, `Type[str]`, and `Type[bool]`. An implementation should support `Type[Model]` for visible model types. Future RFCs may extend token support to enums, traits, type aliases, constrained types, or associated type projections.

### Type names in value position

A type name in value position must be rejected unless the checker has an expected type that is compatible with `Type[T]`. The expected type may come from an explicit parameter type, an overload candidate, a variable annotation, a return position, a collection element context, or another ordinary expected-type source.

When the expected type is `Type[T]`, a type name must resolve using ordinary type resolution. If the resolved type is compatible with `T`, the expression has type `Type[T]`. If the resolved type is incompatible, typechecking must report a type mismatch instead of treating the name as an ordinary value.

When no `Type[T]` expected context exists, the diagnostic should say that the name is a type and can be used as a value only through an API that expects `Type[...]`.

### Overload resolution

Overload resolution must consider `Type[T]` parameters like ordinary typed parameters. A call such as `cast(expr, float)` may select an overload whose second parameter is `Type[float]`. The selected overload's return type must be the resulting call type.

Overload resolution must not broaden a selected `Type[T]` return to the union of all overload returns. The point of the feature is that the target token helps select the precise callable surface.

When multiple overloads accept the same type token argument equally well, the ordinary overload ambiguity rules must apply. `Type[T]` must not introduce a separate priority system.

### Aliases and reexports

An alias of an overload set must preserve the overload set. The alias must not create wrapper functions, duplicate overload declarations, erase decorator metadata, change default metadata, or collapse the selected return type.

Public reexports must preserve the same callable surface. A consumer importing through a facade must see the same overloads, type-token parameters, aliases, decorators, and return types as a consumer importing from the declaring module.

### Decorators and callable metadata

A decorator applied to a type-token overload must receive callable metadata for the source callable surface. `func.__name__` must report the source callable name or alias identity according to the ordinary callable-name rules, not generated backend implementation names.

Decorator side effects that are part of module static initialization must run for decorated overload implementations that are reachable through the public API. The behavior must not depend on whether the callable is invoked directly, through an alias, through a facade, or through a package import.

### Reflection

`T.__class_name__()` may be used in generic code where `T` is a type parameter with the required reflection support. Primitive types should provide stable class-name metadata for `int`, `float`, `str`, and `bool`. Model types should provide class-name metadata according to the existing model reflection rules.

`Type[T]` tokens do not by themselves grant arbitrary reflection. A function that needs `T.__fields__()`, field metadata, schema metadata, or richer type information must still rely on the corresponding reflection capabilities and bounds.

### Package metadata and documentation

Library manifests and checked API metadata must preserve `Type[T]` parameter shapes, overload emitted identities, source callable identities, alias relationships, decorator metadata, defaults, return types, and public reexport paths.

Generated documentation and LSP surfaces should display type-token overloads as ordinary overloads. Documentation for aliases should show the alias as a public name while preserving the target relationship.

## Design details

### Why `cast(expr, int)` instead of only `cast[int](expr)`?

`cast(expr, int)` models the type as part of the value-level API contract. This is useful when a library wants a fallback string overload, additional runtime arguments, or a public alias that treats the type target like any other parameter:

```incan
def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return float_column_expr(expr)

def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return custom_cast(expr, target)
```

`cast[T](expr)` is still useful for generic APIs, and RFC 054 already defines explicit call-site generics. However, precise return specialization for `cast[T](expr)` requires a separate type-level return mapping design. If `T` is `float`, users want `FloatColumnExpr`, not merely a broad `ColumnExpr | FloatColumnExpr | IntColumnExpr` union. That relationship is not "return T"; it is "map source type token T to a library-specific result type." This RFC therefore treats `cast[T](expr)` precision as a future design area, not as the primary mechanism.

### Primitive tokens and model tokens

Primitive tokens are necessary because library APIs often branch on built-in scalar domains. Model tokens are necessary for schema-shaped APIs where the caller chooses a checked model type.

The language should avoid treating primitive tokens and model tokens as two unrelated mechanisms. Both are source type tokens. They may have different metadata capabilities, but they should share expected-type checking, overload resolution, import behavior, aliases, manifests, and documentation.

### Boundary invariants

The same call must typecheck the same way through each public boundary:

```incan
from casts import cast
from public_facade import cast
from pub::package import cast
```

If these import paths expose the same public symbol, a call such as `cast(col("amount"), float)` must select the same overload and return the same type. Divergence across boundaries is a compiler bug, not a library-author responsibility.

### Diagnostic shape

Diagnostics for invalid type-name values should be explicit:

```text
Cannot use type `int` as a value
Types are compile-time names. Use an API that expects `Type[int]`, such as `cast(expr, int)`, or pass an ordinary runtime value.
```

Diagnostics for unsupported tokens should name the unsupported type and the expected token shape.

## Alternatives considered

### Keep stringly type targets

String targets are flexible and familiar, but they are weakly checked. They do not give the compiler enough information to select precise return types, and they make generated docs and registry metadata depend on user-written strings. They should remain useful as escape hatches for dynamic or backend-specific targets, not as the preferred typed API shape.

### Helper families

Helper families such as `cast_int`, `cast_float`, and `cast_string` are easy to implement and type precisely, but they fragment the API. They also force aliases, decorators, docs, registries, examples, and search results to duplicate one semantic operation across many names.

### Broad union returns

A single function returning a broad union can keep one public name, but it pushes type recovery to the caller. Users then need wrapper calls or match narrowing before passing the result to typed helper APIs. That defeats the purpose of choosing a type target at the call site.

### Fully first-class runtime types

Making `int`, `float`, model names, and type aliases ordinary values everywhere would make the surface feel familiar to Python users, but it is much broader than the problem this RFC solves. It would require a runtime type object model, identity rules, equality rules, serialization rules, metadata availability rules, and likely new runtime reflection capabilities.

### Generic return specialization

`cast[T](expr)` with precise library-specific return types is attractive, but it needs a model for type-level return mapping. Associated types, constrained overloads, or future type functions may be the right answer. This RFC intentionally leaves that design open.

## Drawbacks

The main drawback is that `int` sometimes appears in value position even though types are not generally values. The feature is safe only if diagnostics and docs repeatedly explain the expected-type rule.

The second drawback is implementation pressure. Type-token APIs cross parser, typechecker, overload resolution, aliasing, decorator metadata, manifests, docs, tests, LSP, and backend emission. A partial implementation can easily reintroduce boundary-specific behavior.

The third drawback is that this does not solve every type-directed API. Libraries that need generic return specialization, type functions, or richer schema-level reflection will still need future language work.

## Implementation architecture (non-normative)

The implementation should keep one semantic representation for `Type[T]` through typechecking, metadata, lowering, and backend emission. It should avoid separate code paths for same-module, imported, package, facade, alias, and decorated callables. The same checked callable surface should feed overload dispatch, library manifests, generated docs, LSP, and backend code generation.

Backends should treat type tokens as zero-sized compile-time evidence unless a runtime library API explicitly requires a value representation. Generated helper names and backend implementation names must not leak into source callable identity or public metadata.

## Layers affected

- **Parser / AST**: may need explicit representation for `Type[T]` type references and type-name expressions under expected context.
- **Typechecker / Symbol resolution**: must resolve type names in value position only under `Type[T]` expected context, dispatch overloads using type-token parameters, and preserve alias/decorator callable surfaces.
- **IR Lowering**: must lower type-token expressions and overloaded aliases without splitting same-module and import-boundary behavior.
- **Emission**: must emit backend representations for type tokens and preserve source callable names in generated metadata.
- **Stdlib / Runtime (`incan_stdlib`)**: must provide the minimal token carrier and primitive reflection hooks needed by compiler-emitted code.
- **Library manifests / checked API metadata**: must serialize `Type[T]`, overload sets, aliases, decorators, defaults, and package reexports without erasing identity.
- **Formatter**: should preserve ordinary call syntax and type annotations; no special formatting beyond existing type syntax is expected.
- **LSP / Tooling**: should show type-token overloads in completion, hover, signature help, generated API docs, and diagnostics.

## Unresolved questions

- Which non-primitive type categories should `Type[T]` support beyond model types in the first planned version?
- Should enum tokens, trait tokens, type alias tokens, constrained scalar tokens, and associated type projection tokens be admitted together or in staged increments?
- What is the right future mechanism for precise `cast[T](expr)`-style return specialization: associated types, type functions, overload constraints, or something else?
- Should `Type[T]` tokens ever have user-visible equality, hashing, serialization, or pattern-matching behavior, or should they remain dispatch-only evidence?
- How should docs distinguish source callable identity from alias identity when decorators record metadata through an alias?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
