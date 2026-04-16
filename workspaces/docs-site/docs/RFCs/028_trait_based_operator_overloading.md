# RFC 028: trait-based operator overloading

- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 027 (vocab crate — block/desugaring substrate), RFC 024 (extensible derive protocol), RFC 040 (scoped DSL glyph surfaces), RFC 054 (explicit call-site generics)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/314
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces operator overloading for Incan, allowing user-defined types to participate in operator expressions (`+`, `-`, `*`, `/`, `%`, `>>`, `<<`, `|>`, `<|`, `@`, `==`, `<`, etc.) through dunder methods and operator traits. The user model is Python-inspired (`__add__`, `__mul__`, `__rshift__`, etc.), while the typechecker and backends enforce those semantics in an Incan-first way.

Part of the surface already exists today as stdlib trait stubs:

- arithmetic traits such as `Add`, `Sub`, `Mul`, `Div`, `Neg`, and `Mod`
- comparison traits such as `Eq` and `Ord`

This RFC turns that partial, documentation-oriented surface into a coherent language feature: it wires operator resolution into the typechecker, defines lowering rules in IR, and specifies how backends preserve those already-resolved Incan semantics. It also expands the trait surface for operators that do not yet have stdlib definitions.

## Motivation

### Operator-heavy domains need custom semantics

Many domains rely on operators having type-specific meaning:

| Domain                   | Operator      | Meaning                     |
| ------------------------ | ------------- | --------------------------- |
| Data pipelines           | `>>` / `<<`   | Forward/backward data flow  |
| Functional/dataflow APIs | `\|>` / `<\|` | Forward/reverse application |
| Linear algebra / ML      | `@`           | Matrix multiplication       |
| ML / tensor libs         | `*`           | Element-wise multiplication |
| Data frames              | `[]`          | Column selection            |
| Financial modeling       | `+`, `-`      | Currency-safe arithmetic    |
| Set operations           | `&`, `\|`     | Intersection, union         |
| String-like types        | `+`           | Concatenation               |

Without operator overloading, all of these must use verbose method calls (`tensor.matmul(other)` instead of `tensor @ other`), making Incan code less expressive than Python for these domains.

### The vocab API defers global operator semantics

RFC 027 defines the vocabulary registration system for keywords and block-level DSL syntax, but leaves **ordinary global operator semantics** to the type system and to this RFC. A separate RFC covers explicit DSL blocks that reuse glyphs such as `>>` or `|>` with block-local meaning. Outside those explicit block contexts, what `>>`, `<<`, `|>`, `<|`, or `@` *means* must still be resolved by the operator protocol defined here.

### The current stdlib already shows the shape, but not the full feature

The stdlib already contains part of the intended protocol surface:

- arithmetic traits for the common numeric operators
- comparison traits for equality and ordering

But today those definitions are not the normative source of operator semantics. Builtin operators are still mostly hard-wired around primitive behavior, and user-defined types do not yet get full trait-dispatched operator resolution. Nothing in the compiler currently resolves `a + b` to `a.__add__(b)` for user-defined types or lowers that resolved protocol call as a first-class part of the operator pipeline.

### Incan semantics come first

This RFC deliberately defines operator behavior in **Incan terms first** and backend terms second.

That means:

- the language spec says which dunder methods and traits make an operator valid
- the typechecker resolves operators against those Incan traits
- backends then implement those semantics as faithfully as they can

This is important because some tempting Rust mappings are misleading if treated as the language model. For example, `len(x)` is not the same concept as Rust's `Sized`, and `a[key] = value` is not the same concept as `std::ops::IndexMut`. The language should not inherit those distortions just because Rust is one backend.

## Guide-level explanation

### Defining operator behavior on a type

You can declare operator behavior by adopting the corresponding operator trait, by defining the matching dunder method, or by doing both. Explicit trait adoption is still the clearest surface for generic constraints, so this RFC continues to use that style in most examples:

```incan
from std.traits.ops import Add, Mul

model Vector with Add[Vector, Vector], Mul[float, Vector]:
    x: float
    y: float

    def __add__(self, other: Vector) -> Vector:
        return Vector(x=self.x + other.x, y=self.y + other.y)

    def __mul__(self, scalar: float) -> Vector:
        return Vector(x=self.x * scalar, y=self.y * scalar)

# Usage — operators dispatch to dunders
a = Vector(x=1.0, y=2.0)
b = Vector(x=3.0, y=4.0)
c = a + b        # calls a.__add__(b) → Vector(4.0, 6.0)
d = a * 2.0      # calls a.__mul__(2.0) → Vector(2.0, 4.0)
```

The same operator should also work when a type defines a compatible `__add__` / `__mul__` without an explicit `with Add[...]` clause. Explicit trait adoption simply makes the capability easier to talk about in generic APIs and docs.

### Pipeline operators for data libraries

A data library could use `>>` for pipeline chaining when the left-hand operand is itself already a pipeline object:

```incan
from std.traits.ops import Shr

class Pipeline with Shr[Step, Pipeline]:
    steps: List[Step]

    def __rshift__(mut self, step: Step) -> Self:
        self.steps.append(step)
        return self

# Usage (assumes all these are Pipeline instances)
result = pipeline >> transform >> validate >> store
```

This is an ordinary global operator-overload example on a `Pipeline` value. A separate RFC covers explicit DSL blocks that may reuse `>>` or `<<` with block-local meaning.

### Pipe operators for value-threading APIs

Libraries can also give `|>` and `<|` ordinary global meanings when they want a first-class pipe/apply surface outside any DSL block:

```incan
from std.traits.ops import PipeForward, PipeBackward

class Query with PipeForward[Transform, Query]:
    def __pipe_forward__(self, transform: Transform) -> Query:
        ...

class Renderer with PipeBackward[Query, Report]:
    def __pipe_backward__(self, query: Query) -> Report:
        ...

report = Renderer.default() <| (users |> filter_active |> group_by_country)
```

These are ordinary global operators in this RFC: their meaning comes from the operand types, not from an enclosing DSL block. A separate RFC covers explicit block-local glyph reuse for DSLs that want the same glyphs with context-sensitive meaning.

### Matrix multiplication

ML libraries can use `@` for matrix multiply:

```incan
from std.traits.ops import MatMul

class Tensor with MatMul[Tensor, Tensor]:
    data: List[List[float]]

    def __matmul__(self, other: Tensor) -> Self:
        # ... matrix multiplication logic
        ...

result = weights @ inputs + bias
```

### Comparison operators

```incan
from std.derives.comparison import Eq, Ord

model Version with Eq, Ord:
    major: int
    minor: int
    patch: int

    def __eq__(self, other: Version) -> bool:
        return (self.major, self.minor, self.patch) == (other.major, other.minor, other.patch)

    def __lt__(self, other: Version) -> bool:
        if self.major != other.major:
            return self.major < other.major
        if self.minor != other.minor:
            return self.minor < other.minor
        return self.patch < other.patch

v1 = Version(major=1, minor=2, patch=0)
v2 = Version(major=1, minor=3, patch=0)
assert v1 < v2
assert v1 != v2
```

The same comparison surface should also be valid when a type defines the compatible dunder methods without explicitly writing `with Eq, Ord`. The traits remain the nominal vocabulary for generic bounds and documentation.

## Reference-level explanation

### Operator-to-trait mapping

The normative mapping is from Incan syntax to Incan traits and dunder methods. Rust notes below are implementation guidance for the Rust backend, not the language definition.

Some traits in this table already exist today (`Add`, `Sub`, `Mul`, `Div`, `Neg`, `Mod`, `Eq`, `Ord`). Others are proposed additions that this RFC standardizes as part of the same protocol family (`FloorDiv`, `Pow`, `Shr`, `Shl`, `PipeForward`, `PipeBackward`, `BitAnd`, `BitOr`, `BitXor`, `MatMul`, `GetItem`, `SetItem`).

| Incan operator | Dunder method       | Incan trait                | Category          | Rust backend note                                |
| -------------- | ------------------- | -------------------------- | ----------------- | ------------------------------------------------ |
| `a + b`        | `__add__`           | `Add[Rhs, Output]`         | Arithmetic        | Lower to `std::ops::Add` when possible           |
| `a - b`        | `__sub__`           | `Sub[Rhs, Output]`         | Arithmetic        | Lower to `std::ops::Sub` when possible           |
| `a * b`        | `__mul__`           | `Mul[Rhs, Output]`         | Arithmetic        | Lower to `std::ops::Mul` when possible           |
| `a / b`        | `__div__`           | `Div[Rhs, Output]`         | Arithmetic        | Lower to `std::ops::Div` when possible           |
| `a // b`       | `__floordiv__`      | `FloorDiv[Rhs, Output]`    | Arithmetic        | Lower via helper semantics or native support     |
| `a % b`        | `__mod__`           | `Mod[Rhs, Output]`         | Arithmetic        | Lower to `std::ops::Rem` when possible           |
| `a ** b`       | `__pow__`           | `Pow[Rhs, Output]`         | Arithmetic        | Lower via helper semantics or method call        |
| `-a`           | `__neg__`           | `Neg[Output]`              | Unary             | Lower to `std::ops::Neg` when possible           |
| `a >> b`       | `__rshift__`        | `Shr[Rhs, Output]`         | Bitwise/Pipeline  | Lower to `std::ops::Shr` when possible           |
| `a << b`       | `__lshift__`        | `Shl[Rhs, Output]`         | Bitwise/Pipeline  | Lower to `std::ops::Shl` when possible           |
| `a \|> b`      | `__pipe_forward__`  | `PipeForward[Rhs, Output]` | Pipe/Application  | Lower via helper semantics or method call        |
| `a <\| b`      | `__pipe_backward__` | `PipeBackward[Rhs, Output]`| Pipe/Application  | Lower via helper semantics or method call        |
| `a & b`        | `__and__`           | `BitAnd[Rhs, Output]`      | Bitwise/Set       | Lower to `std::ops::BitAnd` when possible        |
| `a \| b`       | `__or__`            | `BitOr[Rhs, Output]`       | Bitwise/Set       | Lower to `std::ops::BitOr` when possible         |
| `a ^ b`        | `__xor__`           | `BitXor[Rhs, Output]`      | Bitwise           | Lower to `std::ops::BitXor` when possible        |
| `~a`           | `__invert__`        | `Not[Output]`              | Unary             | Lower to `std::ops::Not` when possible           |
| `a @ b`        | `__matmul__`        | `MatMul[Rhs, Output]`      | Matrix            | Lower via helper trait or method call            |
| `a == b`       | `__eq__`            | `Eq`                       | Comparison        | Rust backend may use `PartialEq`-style lowering  |
| `a != b`       | `__ne__`            | `Eq`                       | Comparison        | Rust backend may lower via equality negation     |
| `a < b`        | `__lt__`            | `Ord`                      | Comparison        | Rust backend may use `PartialOrd`-style lowering |
| `a <= b`       | `__le__`            | `Ord`                      | Comparison        | Rust backend may use `PartialOrd`-style lowering |
| `a > b`        | `__gt__`            | `Ord`                      | Comparison        | Rust backend may use `PartialOrd`-style lowering |
| `a >= b`       | `__ge__`            | `Ord`                      | Comparison        | Rust backend may use `PartialOrd`-style lowering |
| `a[key]`       | `__getitem__`       | `GetItem[Key, Output]`     | Indexing          | Lower via helper trait or method call            |
| `a[key] = v`   | `__setitem__`       | `SetItem[Key, Value]`      | Indexing          | Lower via helper trait or method call            |

This RFC treats symbolic operators and keyword operators as distinct. `a | b` is not an alias for `a or b`; `a & b` is not an alias for `a and b`; and `~a` is not an alias for `not a`.

### Non-goals

This RFC covers operator syntax and operator-like indexing forms. It does **not** define the broader object protocol surface such as `len(x)`, `str(x)`, `repr(x)`, `iter(x)`, `hash(x)`, or `bool(x)`.

Those protocols may eventually exist in Incan, but they should be specified on their own terms rather than being forced into Rust-shaped traits like `Sized`.

This RFC also does **not** define general callable overloading for function/method API surfaces. That route is tracked as a separate follow-on tied to RFC 054's optional/dynamic API boundary.

The following language operators are also explicitly **not overloadable** in this RFC:

- `is`
- `in`
- `not in`
- `and`
- `or`
- `not`
- range operators such as `..` and `..=`

`is` retains identity semantics. `in` / `not in` remain language-defined membership operators. `and` / `or` / `not` keep their built-in logical short-circuit semantics and are not aliases for `&` / `|` / `~`. Ranges remain dedicated syntax rather than trait-dispatched operators.

### Boundary with scoped DSL glyph surfaces

This RFC defines ordinary global operator semantics. If `a >> b`, `a << b`, `a |> b`, or `a <| b` is valid under this RFC, it is valid because the operand types expose the corresponding global operator surface (`Shr`, `Shl`, `PipeForward`, `PipeBackward`, or compatible dunders).

Explicit DSL blocks may reuse the same glyphs with block-local meaning, but that scoped reuse is not defined here and does not imply that the operand types globally implement the corresponding operator trait. Imports alone do not change the meaning of operators in ordinary code. See RFC 040 for the scoped-glyph mechanism.

### Resolution rules

When the typechecker encounters `a + b` on a non-primitive or explicitly operator-driven path:

1. Look for explicit `Add[typeof(b), _]` support, a compatible `__add__` method, or both
2. If found → operator resolves to `a.__add__(b)`, and the compiler may synthesize the corresponding operator-trait view for generic reasoning
3. If neither is found → type error: "type `Foo` does not support `+` with `Bar`; consider defining `__add__` or implementing `Add[Bar, Output]`"

The same rule applies to the other operator traits in this RFC.

Primitive operators retain their existing language-defined semantics. This RFC extends operator resolution for user-defined types and generic, trait-constrained code; it does not replace the builtin numeric/string rules with a mandatory trait-dispatch path for every operator expression.

### Comparison semantics

`Eq` and `Ord` are specified in Incan terms:

- `Eq` provides `__eq__`
- `__ne__` is optional; if absent, `a != b` is defined as `not (a == b)`
- `Ord` requires `Eq` and `__lt__`
- `__le__`, `__gt__`, and `__ge__` are optional convenience hooks
- if those hooks are absent, the compiler derives their semantics from `__lt__` and `__eq__`

As with arithmetic operators, a type may advertise this surface through the trait, through compatible dunders, or through both. This keeps the public language model consistent even if a backend chooses a different internal lowering strategy.

### Indexing semantics

Indexing is part of this RFC and is defined in Incan terms rather than borrowed from Rust's lvalue model:

- `a[key]` resolves through `GetItem[Key, Output]` and `__getitem__(key)`
- `a[key] = value` resolves through `SetItem[Key, Value]` and `__setitem__(key, value)`

Slice protocols, multi-index semantics, and range-based indexing conventions are not specified by this RFC.

### Reflected (right-hand) operators need more design, but should not be ruled out

Python-style reflected operators such as `__radd__` and `__rmul__` are important for mixed-type expressions and pipeline-heavy DSLs.

This RFC should not hard-rule them out. What remains to be nailed down is the exact dispatch order and ambiguity behavior when both the left-hand and right-hand types offer applicable operator hooks. That part needs a deeper design pass, especially for future pipeline-oriented work.

### Augmented assignment operators

In this RFC, compound assignment has a clear baseline meaning: desugar through the corresponding binary operator. For the compound-assignment forms that exist in the language grammar today:

- `a += b` → `a = a + b`
- `a -= b` → `a = a - b`
- `a *= b` → `a = a * b`
- `a /= b` → `a = a / b`
- `a //= b` → `a = a // b`
- `a %= b` → `a = a % b`

That desugaring is the minimum semantic contract. This RFC does not rule out specialized in-place hooks such as `__iadd__` / `AddAssign`; it simply does not fully specify their dispatch rules yet.

### The `@` (matmul) operator

Python added `@` as a dedicated matrix multiplication operator (PEP 465). Incan should support this:

- Parse `a @ b` as `BinaryOp(a, MatMul, b)`
- Resolve to `a.__matmul__(b)` via the `MatMul` trait
- The Rust backend does not need a native `@` operator to support this — it can lower via a helper trait or direct method call

This is a good example of the language-first rule: `@` is part of Incan if the Incan typechecker and standard traits define it, regardless of whether the target backend has a matching built-in operator.

`@` has the same precedence and left-associativity as the multiplicative operators, matching Python.

The disambiguation from decorator `@` is positional, matching Python's rule: a `@` token at the start of a statement that is immediately followed by a name and a function or class definition is a decorator. Any `@` that appears between two expression operands — that is, not at statement position preceding a `def` or `class` — is the MatMul binary operator. The parser never needs to look further than the syntactic position of the `@` token to decide which meaning applies.

### The `|>` and `<|` pipe operators

This RFC also brings `|>` and `<|` into scope as ordinary global operators for libraries that want value-threading, reverse application, or other pipeline-like APIs outside any DSL block:

- Parse `a |> b` as `BinaryOp(a, PipeForward, b)`
- Resolve to `a.__pipe_forward__(b)` via the `PipeForward` trait
- Parse `a <| b` as `BinaryOp(a, PipeBackward, b)`
- Resolve to `a.__pipe_backward__(b)` via the `PipeBackward` trait
- Backends may lower these through helper traits or direct method calls if the target language has no native equivalent

Like `@`, these are part of Incan if the Incan typechecker and standard traits define them, regardless of whether the target backend has matching built-in syntax.

### Operator resolution model

When Incan sees an operator expression such as `lhs + rhs`, the language-level rule is:

1. Determine the dunder surface for the operator (for example `+` maps to `__add__`).
2. Check whether the left-hand type exposes a compatible dunder method, a compatible operator trait view, or both.
3. If a compatible surface exists, resolve the expression through that operator contract and preserve the resolved view for generic reasoning and diagnostics.
4. If no compatible surface exists, produce a type error naming the missing operator capability.

The important point is that user-defined operator expressions resolve through Incan’s operator protocol, not through ambient backend operator behavior. Backends are responsible only for realizing that already-resolved meaning.

### Interaction with existing features

**`@derive(Eq, Ord)`:** Models with `@derive(Eq)` get auto-generated `__eq__` (field-wise comparison). Manually
implementing `__eq__` overrides the derived version. This RFC relies on that comparison-trait surface but does not redefine derive semantics; those remain governed by RFC 024.

**Trait composition:** A type can implement multiple operator traits: `model Vec3 with Add[Vec3, Vec3], Mul[float, Vec3], Neg[Vec3]`. Each trait impl is independent.

**Pattern matching:** Comparison operators (`==`, `<`) are used in `match`/`case` guards. Custom `Eq`/`Ord` implementations must be respected in pattern matching comparisons.

**Generics:** Operator traits are generic (`Add[Rhs, Output]`). A type can implement `Add[int, MyType]` and `Add[float, MyType]` — different behavior for different right-hand types. Generic constraints still speak in trait language even when a concrete type chooses to declare its operator support through dunders alone; the compiler may infer the trait view from the matching dunder surface.

**Rust interop:** Raw `rust::...` imported types are not assumed to satisfy Incan operator protocols automatically. If a Rust-backed type should participate in Incan operators, the normal path is to wrap it in an Incan type/newtype and define the relevant dunders or traits there. **[RFC 043](043_rust_trait_impl_from_incan.md)** (`impl` on `rusttype`, `@rust.derive`) is the normative place for Rust-side trait contracts on those wrappers; [RFC 026](closed/superseded/026_user_defined_trait_bridges.md) is superseded.

## Alternatives considered

### A. Rust-style `impl Add for MyType` syntax

```incan
impl Add[Vector, Vector] for Vector:
    def add(self, other: Vector) -> Vector: ...
```

**Rejected** in favor of Python-style dunder methods because: Incan's target audience is Python developers. `__add__` is immediately familiar. The `with Trait` pattern on models/classes is already established. Adding a separate `impl Trait for Type` block is a significant syntax addition that doesn't align with Incan's Python-first philosophy. The compiler can still emit Rust `impl Add` behind the scenes.

### B. Pure method-based dispatch (dunder-only declaration)

Just define `__add__` as a plain method — the compiler detects the dunder name and wires it to the operator:

```incan
model Vector:
    def __add__(self, other: Vector) -> Vector: ...
```

**Accepted as part of the proposal**: a matching dunder should be enough to make the operator valid. Explicit trait adoption still matters because it gives generic APIs, docs, and diagnostics a nominal vocabulary for capability. In other words, Incan should accept either surface, and the compiler may infer the trait view from the dunder view when needed.

### C. Declarative operator macros

```incan
@operator("+")
def add_vectors(a: Vector, b: Vector) -> Vector: ...
```

**Rejected** because: it's less discoverable than dunder methods, doesn't compose through traits, and introduces a new syntax pattern that neither Python nor Rust developers would expect.

## Drawbacks

- **Compile-time cost**: Each operator trait impl generates a Rust `impl` block. Types with many operator overloads generate many impl blocks. This is the same trade-off Rust makes — acceptable for types that genuinely need operator semantics.
- **Potential for abuse**: Redefining `+` to mean something unexpected (e.g., `+` as string concatenation on non-string types) hurts readability. This is a cultural concern, not a technical one — Python has the same issue.
- **Backend complexity**: Some Incan operator semantics map neatly to host-language primitives, and some do not. Backends may need helper traits, shims, or direct method lowering to preserve the language semantics.
- **Open dispatch details**: Reflected operators and in-place operator hooks are likely useful, but their exact dispatch rules still need sharper specification. Leaving those details under-specified for too long would create confusion.

## Layers affected

- **Language surface**: operator spellings and dunder declarations must remain unambiguous.
- **Type system**: operator usage must resolve against dunder methods and operator traits according to the RFC's dispatch rules.
- **Execution handoff**: implementations must preserve the typechecked operator semantics across backends without leaking backend-specific operator rules into user-facing behavior.
- **Stdlib / runtime**: the nominal operator trait surface used for generic capability expression and documentation must be available.
- **Docs / tooling**: operator capability, trait vocabulary, and dispatch behavior must be explained clearly enough that overloaded operators remain understandable.

## Unresolved questions

- What is the exact fallback order between direct dunder methods, reflected operators, and trait-inferred operator capability?
- Which in-place operator hooks are part of the initial contract, and when should they fall back to ordinary binary operator lowering?
- How should diagnostics present operator ambiguity when multiple plausible trait-driven resolutions exist?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
