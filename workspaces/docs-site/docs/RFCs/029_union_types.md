# RFC 029: union types and type narrowing


- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 028 (trait-based operator overloading)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/315
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce first-class union types to Incan. `Union[A, B, ...]` is the canonical form, and `A | B` is equivalent syntactic sugar.

Union types let a value be one of several alternative types without requiring the user to declare a named enum at each use site. Narrowing is checked at compile time through `isinstance(...)` and exhaustive `match`, using Python-style type patterns such as `case int(n)` and `case str(s)`.

## Motivation

Programs often need a value that may be one of several unrelated types. Today, expressing that shape directly in Incan is awkward. Users must introduce a named enum purely for transport, push the type distinction out to API boundaries, or contort APIs around overloads that do not really model the underlying data.

Real programs need richer unions:

```incan
# Not expressible today

# A parser that returns either an int or a string
def parse_value(raw: str) -> int | str:
    if raw.isdigit():
        return int(raw)
    return raw

# An API handler that returns different response types
def handle(request: Request) -> Response | Redirect | Error:
    ...

# A configuration value that can be several types
config_value: str | int | bool | List[str] = load_config("key")
```

Without union types, users must resort to explicit wrapper enums (boilerplate), boundary-level type erasure, or overloaded functions (limited).

### What a union is

A union is a type whose value may be one of several alternatives:

```incan
Union[int, str]          # canonical form
int | str                # sugar form

Option[Union[int, str]]  # canonical form when `None` is present
int | str | None         # sugar form
```

`Union[...]` is the canonical spelling for unions that do not include `None`. When `None` is present, the canonical form uses `Option[...]`. In both cases, `|` is syntactic sugar.

When `None` appears in a union, the type canonicalizes through `Option[...]`. `T | None` is equivalent to `Option[T]`, and `A | B | None` is equivalent to `Option[Union[A, B]]`.

A union is an anonymous, structural, closed sum type:

- **anonymous**: users do not declare a named enum for each use site
- **structural**: `int | str` and `str | int` are the same type
- **closed**: the set of possible member types is fixed by the annotation itself

This RFC adds that anonymous union facility. It does not change existing named enums such as `Option[T]`, which continue to use their own declared variants like `Some(...)` and `None`.

## Guide-level explanation (how users think about it)

### Declaring union types

The canonical form is `Union[A, B, ...]` for ordinary unions and `Option[...]` for unions that include `None`. The `|` operator is syntactic sugar:

```incan
# Canonical form
x: Union[int, str] = "hello"
y: Option[Union[int, str]] = 42
z: Union[Response, Redirect, Error] = handle(req)

# Sugar form (equivalent)
x: int | str = "hello"
y: int | str | None = 42
z: Response | Redirect | Error = handle(req)
```

Both forms are interchangeable in any annotation position (function params, return types, variable bindings).

Unions are **unordered** — `Union[int, str]` and `Union[str, int]` are the same type. Duplicates are eliminated — `Union[int, int, str]` is `Union[int, str]`. Nested unions are flattened — `Union[Union[int, str], bool]` is `Union[int, str, bool]`.

### Using union values with pattern matching

A value of union type must be **narrowed** before type-specific operations can be used:

```incan
def describe(value: int | str | None) -> str:
    match value:
        case int(n):
            return f"number: {n}"
        case str(s):
            return f"text: {s}"
        case None:
            return "nothing"
```

The compiler enforces **exhaustiveness** — every member of the union must be covered, or a wildcard `case _` must be present.

`case int(n)` and `case str(s)` are **type patterns**. They mean:

- check whether the current union value holds a value of type `int` / `str`
- if so, bind the inner value to `n` / `s`

This is the pattern-matching counterpart of `isinstance(value, int)`.

### Using union values with `isinstance` (compile-time type narrowing)

`isinstance` is a **compile-time narrowing construct**, not runtime reflection. The compiler uses it to determine which union variant is active and lowers it to Rust pattern matching (`if let`). There is no `TypeId`, boxed type erasure, or runtime type introspection involved.

```incan
def process(value: int | str) -> int:
    if isinstance(value, int):
        # value is narrowed to `int` here (compiler knows this statically)
        return value * 2
    else:
        # value is narrowed to `str` here
        return len(value)
```

After an `isinstance` check, the type of the variable is narrowed within the guarded block. This RFC extends that same idea to `is None` / `is not None` checks on unions that canonicalize through `Option[...]`:

```incan
def greet(name: str | None) -> str:
    if name is not None:
        # name is narrowed to `str` here
        return f"Hello, {name}"
    return "Hello, stranger"
```

### Returning union types

Functions can declare union return types. The typechecker validates that all return paths produce a member of the union:

```incan
def parse_id(raw: str) -> int | str:
    if raw.isdigit():
        return int(raw)    # int is in int | str
    return raw             # str is in int | str
```

### Unions in collections

Union types compose with builtin generic types:

```incan
items: List[int | str] = [1, "two", 3, "four"]
mapping: Dict[str, int | bool] = {"count": 42, "active": True}
```

## Reference-level explanation (precise rules)

### Syntax

```text
type_expr  ::= type_atom ("|" type_atom)*
type_atom  ::= IDENT type_args? | "None"
type_args  ::= "[" type_expr ("," type_expr)* ","? "]"
```

`Union[A, B]` is the canonical form for unions that do not include `None`. `A | B` is syntactic sugar that the parser desugars to `Union[A, B]` during parsing. When `None` appears in the union, normalization rewrites the type through `Option[...]`.

The `|` operator binds looser than `[]`, so `List[int | str]` parses as `List[Union[int, str]]`.

Trait constraints are orthogonal to unions. `T with Serializable` constrains a type parameter; `A | B` describes a value-level set of alternatives. A union member may itself be a trait-adopting type, but a union does not implicitly mean "some type satisfying trait X". Operator or method use that depends on a particular member's traits must happen after narrowing.

### Match patterns for unions

When matching on a union value, including the `Option[...]`-canonicalized form of a union containing `None`, the following additional pattern forms are valid:

```text
union_type_pattern ::= TYPE_IDENT "(" IDENT ")"
none_pattern       ::= "None"
```

Examples:

```incan
match value:
    case int(n):
        ...
    case str(s):
        ...
    case None:
        ...
```

A type pattern `T(name)` is valid only when the scrutinee has a union type containing `T`.

The binding captures the **whole narrowed value** of type `T`. It is not structural destructuring of the internals of `T`. For example:

```incan
def handle(result: Response | Error) -> str:
    match result:
        case Response(r):
            return r.status_text()
        case Error(e):
            return e.message()
```

Here `r` has type `Response` and `e` has type `Error`.

### Type checking rules

- **Subtyping**: `T` is assignable to any union containing `T`. E.g., `int` is assignable to `int | str`.
- **Union-to-union assignability**: `A | B` is assignable to `A | B | C`. More generally, a source union is assignable to a target union when every source member is assignable to some target member.
- **Equivalence**: Unions are unordered sets. `int | str == str | int`. Duplicates and nested unions are normalized.
- **`None` canonicalization**: A union containing `None` canonicalizes through `Option[...]`. `T | None == Option[T]`, and `A | B | None == Option[Union[A, B]]`.
- **Narrowing**: After `isinstance(x, T)`, `x` is `T` in the true branch and `Union minus T` in the else branch. After   `x is None`, `x` is `None` in the true branch and `Union minus None` in the else branch. After `x is not None`, `x` is `Union minus None` in the true branch and `None` in the else branch. Narrowing does **not** persist after the conditional block.
- **Exhaustiveness**: `match` on a union type must cover all constituent types or include a wildcard arm.
- **No implicit generic-union synthesis**: The compiler does not invent a union type solely to satisfy an otherwise unconstrained generic parameter. When generic inference would require synthesizing a new union, the user must write the annotation explicitly.
- **`Option[T]` integration**: `Option[T]` remains the named enum type from the standard library, and this RFC treats `T | None` as equivalent to that existing maybe-value abstraction.

### Narrowing scope rules

```incan
def example(x: int | str | None):
    if isinstance(x, int):
        print(x + 1)       # x is int here
    elif x is not None:
        print(x.upper())   # x is str here (int and None eliminated)
    else:
        print("none")      # x is None here

    # x is int | str | None again here
```

| Check                | True branch narrows to | Else branch narrows to |
| -------------------- | ---------------------- | ---------------------- |
| `isinstance(x, T)`   | `T`                    | Union minus `T`        |
| `x is None`          | `None`                 | Union minus `None`     |
| `x is not None`      | Union minus `None`     | `None`                 |

### Lowering model

Backends need a closed internal representation for union values. The important language-level constraint is that this representation must preserve:

- the fixed set of member alternatives;
- compile-time narrowing semantics;
- exhaustive matching behavior;
- `T | None` canonicalization through `Option[T]`.

One reasonable backend strategy is a compiler-generated closed sum representation for each distinct union shape, while unions containing `None` reuse the ordinary `Option[...]` path. The specific emitted type names and backend data structures are implementation detail rather than part of the language contract.

#### Pattern matching behavior

```incan
def normalize(value: int | str) -> int:
    match value:
        case int(n):
            return n + 1
        case str(s):
            return len(s)
```

The language-level meaning is that each arm sees the narrowed member type and exhaustive coverage is checked against the full closed set of member types.

#### `isinstance` behavior (compile-time only)

`isinstance` is resolved entirely through the compiler's type and control-flow reasoning. There is no runtime reflection requirement in this RFC.

```incan
if isinstance(value, int):
    use_int(value)
```

#### Construction behavior

Assigning a concrete value to a union-typed binding wraps it in the appropriate variant:

```incan
x: int | str = 42
```

The backend may materialize an internal tagged representation, but the source-level contract is simply that `42` is accepted as a member of `int | str`.

### Interaction with existing features

- **`Option[T]`**: `T | None` and `A | B | None` canonicalize through `Option[...]`. Existing `Option[T]` code and `Some(...)` / `None` matching continue to work exactly as they do today.
- **`isinstance`**: Remains available as the `if`-style narrowing construct. `match value: case T(x): ...` is the pattern-matching form of the same narrowing idea.
- **Operator/method dispatch**: Unions do not implicitly satisfy member-specific operator traits or method surfaces. If only some members support `+`, `.upper()`, or a trait-constrained API, the value must be narrowed first.
- **Serde**: How backends serialize unions is not a language-level concern of this RFC.
- **Display**: Backend/runtime display behavior for generated union representations is not specified by this RFC.
- **Builtin generics**: Unions compose with builtin generic types: `List[int | str]`, `Result[int | str, Error]`.

### Design decision: unions are not `Variant`

This RFC defines unions as **closed, compile-time-known alternatives**. A union such as `int | str` means that the set of possible member types is fixed by the annotation itself and can therefore participate in exhaustiveness checking and compile-time narrowing.

This RFC does **not** define an open-ended dynamic value type such as `Variant` (like Apache Spark's `VariantType`). If Incan adds `Variant` in a future RFC, it should be treated as a separate boundary-oriented abstraction for semi-structured or runtime-shaped data, not as another spelling of `Union[...]`.

### Compatibility / migration

Non-breaking. This is purely additive — existing code is unaffected.

## Alternatives considered

### `Either[A, B]` / `OneOf[A, B, C]` (named generic wrapper)

Use a library type like `Either<A, B>` or `OneOf<A, B, C>` instead of auto-generated enums. Rejected because nesting `Either[A, Either[B, C]]` for 3+ types is unwieldy, and fixed-arity `OneOf` types are a common source of frustration in TypeScript/Python.

### Trait objects (`Box<dyn Any>`)

Lower unions to `Box<dyn Any>` with runtime downcasting. Rejected because it loses all compile-time type safety, requires allocation, and defeats the purpose of a typed language.

### Require explicit `enum` declarations

Require users to define their own Rust-style enums for sum types. Rejected because the goal is Python-like ergonomics — Python's `int | str` type hints don't require boilerplate wrapper types.

## Drawbacks

- **Hidden representation**: Backends need a concrete closed representation for unions, and that representation may occasionally leak into low-level debugging contexts.
- **Inference complexity**: Union types interact with generic inference in subtle ways. This RFC takes the conservative rule that the compiler does not synthesize fresh union types during otherwise unconstrained generic inference.

## Layers affected

- **Language surface**: both `Union[...]` and `A | B` must be accepted as the same type-level construct.
- **Type system**: union members must be canonicalized, narrowing rules enforced, and exhaustiveness checking performed where this RFC requires it.
- **Execution handoff**: implementations must use a stable closed representation for unions without changing the language-level narrowing semantics.
- **Docs / tooling**: the relationship between unions, `Option`, `match`, and narrowing must be explained clearly enough that users can predict when narrowing is required.

## Design Decisions

- `Union[A, B, ...]` is the canonical form, and `A | B` is equivalent syntactic sugar.
- Unions are closed, compile-time-known alternatives rather than an open-ended dynamic `Variant` type.
- `T | None` canonicalizes through `Option[T]` rather than defining a separate nullable union representation.
- Narrowing is compile-time-driven and occurs through `isinstance(...)` and exhaustive `match`.
- Union values do not implicitly expose the full method or operator surface of all members until they have been narrowed.
