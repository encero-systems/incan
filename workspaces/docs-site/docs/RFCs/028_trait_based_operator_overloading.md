# RFC 028: trait-based operator overloading

- **Status:** In Progress
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 025 (multi-instantiation trait dispatch), RFC 027 (vocab crate — block/desugaring substrate), RFC 024 (extensible derive protocol), RFC 029 (union types), RFC 040 (scoped DSL surface forms), RFC 054 (explicit call-site generics)
- **Issue:** [#162](https://github.com/dannys-code-corner/incan/issues/162)
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

### Boundary with scoped DSL surface forms

This RFC defines ordinary global operator semantics. If `a >> b`, `a << b`, `a |> b`, or `a <| b` is valid under this RFC, it is valid because the operand types expose the corresponding global operator surface (`Shr`, `Shl`, `PipeForward`, `PipeBackward`, or compatible dunders).

Explicit DSL blocks may reuse the same glyphs or expression-form surfaces with block-local meaning, but that scoped reuse is not defined here and does not imply that the operand types globally implement the corresponding operator trait. Imports alone do not change the meaning of operators in ordinary code. See RFC 040 for the scoped-surface mechanism.

### Resolution rules

When the typechecker encounters `a + b` on a non-primitive or explicitly operator-driven path:

1. Look for explicit `Add[typeof(b), _]` support, a compatible `__add__` method, or both
2. If found → operator resolves to `a.__add__(b)`, and the compiler may synthesize the corresponding operator-trait view for generic reasoning
3. If neither is found → type error: "type `Foo` does not support `+` with `Bar`; consider defining `__add__` or implementing `Add[Bar, Output]`"

The same rule applies to the other operator traits in this RFC.

The dunder method is the implementation surface. The operator trait is the nominal capability surface. A type that explicitly adopts an operator trait must expose a compatible dunder method with the trait-required signature; a dunder-only implementation may still let the compiler synthesize the matching operator-trait view for generic reasoning. If explicit trait adoption and the visible dunder method disagree, the type declaration is invalid rather than leaving the call site to guess which surface is authoritative.

When RFC 025 multi-instantiation trait dispatch is available, the same operator trait may be adopted multiple times with different right-hand operand types, and each trait instantiation may provide its own same-name dunder implementation. Without that mechanism, a type has at most one implementation body per operator dunder name. RFC 029 union types do not replace this rule: a union-typed operand must be narrowed before member-specific operator traits or methods are used.

Primitive operators retain their existing language-defined semantics. This RFC extends operator resolution for user-defined types and generic, trait-constrained code; it does not replace the builtin numeric/string rules with a mandatory trait-dispatch path for every operator expression.

### Comparison semantics

`Eq` and `Ord` are specified in Incan terms:

- `Eq` provides `__eq__`
- `Eq` may explicitly provide `__ne__` as a default method, commonly as `not self.__eq__(other)`
- `Ord` requires `Eq` and `__lt__`
- `Ord` may explicitly provide `__le__`, `__gt__`, and `__ge__` as default methods

As with arithmetic operators, a type may advertise this surface through the trait, through compatible dunders, or through both. Comparison fallback behavior must be explicit in a trait definition or a user-defined dunder method; the compiler does not invent hidden comparison hooks merely because another comparison hook exists. This keeps the public language model consistent with the explicit vocabulary-hook surface even if a backend chooses a different internal lowering strategy.

### Indexing semantics

Indexing is part of this RFC and is defined in Incan terms rather than borrowed from Rust's lvalue model:

- `a[key]` resolves through `GetItem[Key, Output]` and `__getitem__(key)`
- `a[key] = value` resolves through `SetItem[Key, Value]` and `__setitem__(key, value)`

Slice protocols, multi-index semantics, and range-based indexing conventions are not specified by this RFC.

### Reflected (right-hand) operators need more design, but should not be ruled out

Python-style reflected operators such as `__radd__` and `__rmul__` are important for mixed-type expressions and pipeline-heavy DSLs.

This RFC should not hard-rule them out, but it does not specify them. The initial dispatch model is left-hand dunder/trait resolution only. A follow-up design can add reflected hooks once the language has a precise rule for dispatch order, ambiguity, and explicit qualification when both operands expose applicable operator capabilities.

### Augmented assignment operators

In this RFC, compound assignment has a clear baseline meaning: desugar through the corresponding binary operator. The initial implementation covers the arithmetic compound forms that already existed plus the compound forms for the new bitwise, shift, and matmul operators:

- `a += b` → `a = a + b`
- `a -= b` → `a = a - b`
- `a *= b` → `a = a * b`
- `a /= b` → `a = a / b`
- `a //= b` → `a = a // b`
- `a %= b` → `a = a % b`
- `a @= b` → `a = a @ b`
- `a &= b` → `a = a & b`
- `a |= b` → `a = a | b`
- `a ^= b` → `a = a ^ b`
- `a <<= b` → `a = a << b`
- `a >>= b` → `a = a >> b`

That desugaring is the fallback semantic contract. Before applying the fallback, compound assignment first looks for an explicit in-place operator hook for the same glyph:

- `a += b` may resolve through `__iadd__` / `AddAssign`
- `a -= b` may resolve through `__isub__` / `SubAssign`
- `a *= b` may resolve through `__imul__` / `MulAssign`
- `a /= b` may resolve through `__idiv__` / `DivAssign`
- `a //= b` may resolve through `__ifloordiv__` / `FloorDivAssign`
- `a %= b` may resolve through `__imod__` / `ModAssign`
- `a @= b` may resolve through `__imatmul__` / `MatMulAssign`
- `a &= b` may resolve through `__iand__` / `BitAndAssign`
- `a |= b` may resolve through `__ior__` / `BitOrAssign`
- `a ^= b` may resolve through `__ixor__` / `BitXorAssign`
- `a <<= b` may resolve through `__ilshift__` / `ShlAssign`
- `a >>= b` may resolve through `__irshift__` / `ShrAssign`

An in-place hook is an explicit operator-protocol hook, not hidden compiler magic. It must return a value assignable to the left-hand target type. The implementation may mutate and return the receiver or construct a replacement value, but the source-level contract remains `a = <hook-result>`. If no in-place hook is available, compound assignment falls back to the corresponding binary operator assignment listed above.

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

**`@derive(Eq, Ord)`:** Models with `@derive(Eq)` get auto-generated `__eq__` (field-wise comparison). Manually implementing `__eq__` overrides the derived version. This RFC relies on that comparison-trait surface but does not redefine derive semantics; those remain governed by RFC 024.

**Trait composition:** A type can implement multiple operator traits: `model Vec3 with Add[Vec3, Vec3], Mul[float, Vec3], Neg[Vec3]`. Each trait impl is independent.

**Pattern matching:** Comparison operators (`==`, `<`) are used in `match`/`case` guards. Custom `Eq`/`Ord` implementations must be respected in pattern matching comparisons.

**Generics:** Operator traits are generic (`Add[Rhs, Output]`). A type can implement `Add[int, MyType]` and `Add[float, MyType]` — different behavior for different right-hand types. Generic constraints still speak in trait language even when a concrete type chooses to declare its operator support through dunders alone; the compiler may infer the trait view from the matching dunder surface.

**Multi-instantiation trait dispatch:** Multiple implementations of the same operator dunder for different right-hand operand types are governed by RFC 025. This RFC relies on that dispatch model rather than defining a separate operator-specific overload system.

**Union types:** RFC 029 union values do not implicitly expose the operator surface of their member types. A union-typed value must be narrowed before member-specific operator traits or dunders are available.

**Rust interop:** Raw `rust::...` imported types are not assumed to satisfy Incan operator protocols automatically. If a Rust-backed type should participate in Incan operators, the normal path is to wrap it in an Incan type/newtype and define the relevant dunders or traits there. RFC 043 (`impl` on `rusttype`, `@rust.derive`) is the normative place for Rust-side trait contracts on those wrappers; RFC 026 is superseded.

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
- **Open reflected dispatch details**: Reflected operators are likely useful, but their exact dispatch rules still need sharper specification. Leaving reflected dispatch under-specified for too long would create confusion.

## Layers affected

- **Language surface**: operator spellings and dunder declarations must remain unambiguous.
- **Type system**: operator usage must resolve against dunder methods and operator traits according to the RFC's dispatch rules.
- **Execution handoff**: implementations must preserve the typechecked operator semantics across backends without leaking backend-specific operator rules into user-facing behavior.
- **Stdlib / runtime**: the nominal operator trait surface used for generic capability expression and documentation must be available.
- **Docs / tooling**: operator capability, trait vocabulary, and dispatch behavior must be explained clearly enough that overloaded operators remain understandable.

## Implementation Plan

### Phase 1: Spec and lifecycle

- Record the settled operator dispatch decisions in the RFC.
- Verify RFC 025 and RFC 029 dependencies against `main`.
- Move RFC 028 into `In Progress` with a checklist that can track implementation slices.

### Phase 2: Stdlib operator protocol surface

- Extend `std.traits.ops` with the missing operator traits standardized by this RFC.
- Add explicit compound-assignment traits for in-place hooks.
- Keep comparison fallback behavior explicit in trait definitions rather than hidden in compiler logic.
- Update stdlib trait docs so users understand dunders as implementation hooks and traits as capability contracts.

### Phase 3: Parser, AST, formatter, and surface syntax

- Add or verify parser support for operator tokens that are not already represented in expression positions, including `@`, `|>`, `<|`, bitwise glyphs, and compound assignment forms.
- Preserve decorator `@` versus matmul `@` as a positional grammar distinction.
- Ensure AST, formatter, and syntax diagnostics round-trip the new operator forms consistently.

### Phase 4: Typechecker operator resolution

- Add a shared operator-protocol mapping from operator syntax to dunder name, trait name, and result contract.
- Resolve user-defined binary, unary, comparison, indexing, and compound-assignment operators through the left-hand dunder/trait surface.
- Use RFC 025 multi-instantiation dispatch when the same operator trait is adopted for multiple right-hand operand types.
- Reject trait/dunder disagreement and ambiguous candidates with diagnostics that name the operator, operand types, and competing candidates.
- Preserve builtin primitive, string, list, membership, identity, logical, and range semantics where this RFC says they are not overloadable.

### Phase 5: Lowering and emission

- Preserve the typechecked operator-resolution result into IR rather than re-resolving against backend behavior.
- Lower resolved operator calls through the same call/method-call machinery used for ordinary dunder calls where possible.
- Emit Rust infix or `std::ops` forms only when they faithfully preserve the resolved Incan semantics; otherwise emit helper or direct method calls.
- Route ownership, borrowing, and clone decisions through the existing IR ownership policy.

### Phase 6: Tests, diagnostics, docs, and release notes

- Add parser, formatter, typechecker, lowering, codegen snapshot, and integration coverage for custom operators.
- Add targeted negative tests for missing hooks, trait/dunder mismatch, ambiguity, non-overloadable operators, and union operands that require narrowing.
- Update authored language docs and stdlib trait reference pages.
- Add a release notes entry for the active development version.
- Run targeted verification for each slice and the repository-level gate before closeout.

## Progress Checklist

### Spec / lifecycle

- [x] Settle dunder-versus-trait authority and record it in `Design Decisions`.
- [x] Verify RFC 029 union-type boundary on `main`.
- [x] Verify RFC 025 multi-instantiation trait dispatch on `main`.
- [x] Move RFC 028 to `In Progress`.

### Stdlib / runtime

- [x] Add missing operator trait stubs to `std.traits.ops`.
- [x] Add explicit compound-assignment operator traits.
- [x] Ensure comparison trait defaults remain explicit trait methods.
- [x] Update stdlib trait docs for operator protocols.

### Parser / AST / formatter

- [x] Verify existing operator tokens and add missing expression-position operators.
- [x] Parse matmul `@` without confusing decorator `@`.
- [x] Parse pipe operators `|>` and `<|`.
- [x] Parse bitwise operators and compound-assignment forms required by the RFC.
- [x] Add formatter round-trip coverage for newly supported operator forms.

### Typechecker

- [x] Add canonical operator-to-dunder/trait metadata.
- [x] Resolve user-defined binary operators through dunder/trait capability.
- [x] Resolve user-defined unary operators through dunder/trait capability.
- [x] Resolve comparison operators through explicit dunder/trait capability.
- [x] Resolve indexing and assignment indexing through explicit operator protocol hooks.
- [x] Resolve compound assignment through explicit in-place hooks before binary fallback.
- [x] Use RFC 025 dispatch for multiple same-trait operator instantiations.
- [x] Emit diagnostics for missing hooks, trait/dunder mismatch, and ambiguous candidates.
- [x] Preserve non-overloadable operator behavior for identity, membership, logical, and range operators.

### Lowering / IR / emission

- [x] Preserve resolved operator calls in IR.
- [x] Lower resolved dunder/operator calls through existing call or method-call machinery where possible.
- [x] Emit Rust infix or `std::ops` only when it matches resolved Incan semantics.
- [x] Add helper/direct method emission for operator semantics without native Rust syntax.
- [x] Keep ownership and argument-shape handling centralized in IR ownership policy.

### Tests

- [x] Add parser and formatter tests for newly supported operators.
- [x] Add typechecker tests for valid operator protocols.
- [x] Add typechecker diagnostics for invalid or ambiguous operator protocols.
- [x] Add codegen snapshot tests that exercise operators in expressions.
- [x] Add integration tests for custom operator behavior.

### Docs / release notes

- [x] Update authored operator-overloading documentation.
- [x] Update stdlib trait reference documentation.
- [x] Add release notes for the active development version.

## Design Decisions

- Operator implementation canonicalizes through the left-hand dunder method for the operator. Operator traits are the nominal capability vocabulary; their required methods are the corresponding dunders.
- A dunder-only implementation may synthesize the corresponding trait view for generic reasoning. Explicit trait adoption and direct dunder definitions must agree; disagreement is a type error.
- Multiple same-name operator dunder implementations for different operand types are governed by RFC 025 multi-instantiation trait dispatch. RFC 028 does not define a separate operator-specific overload system.
- RFC 029 union types are not a substitute for multi-instantiation dispatch. Union values must be narrowed before member-specific operator methods or traits are available.
- Compound assignment first resolves an explicit in-place operator hook when present, then falls back to ordinary binary operator assignment. In-place hooks are explicit dunder/trait hooks and return the replacement value assigned back to the left-hand target.
- Reflected right-hand operators are deferred to a follow-up design.
- Candidate ambiguity is a hard diagnostic. Exact type matches may beat compatible matches, but equally specific candidates must not be guessed. When ambiguity comes from trait inheritance, library composition, or RFC 025 multi-instantiation dispatch, the diagnostic must name the competing candidates and point to the explicit-qualification mechanism once that syntax exists.
- Comparison fallback behavior must be explicit in trait definitions or user dunders. The compiler does not synthesize hidden comparison hooks merely because a related hook exists.
