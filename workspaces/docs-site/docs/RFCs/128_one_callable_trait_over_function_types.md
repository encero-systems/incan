# RFC 128: One `Callable[(A, B) -> R]` trait over function types

- **Status:** Draft
- **Created:** 2026-09-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (compilable stdlib; generic `with` bounds)
    - RFC 035 (first-class named function references; the `Callable[Params, R]` type shorthand)
    - RFC 036 (user-defined decorators; callable shapes as function types)
    - RFC 041 (first-class Rust interop authoring; the `Fn`, `FnMut`, and `FnOnce` markers)
    - RFC 042 (traits are always abstract)
    - RFC 068 (protocol hooks for core language syntax; `__call__` and the fixed-arity callable traits)
    - RFC 115 (fallible iteration and combinators; stored generic callbacks)
    - #1790 (a `mut` parameter marker in function types)
- **Issue:** [#1752](https://github.com/encero-systems/incan/issues/1752)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

This RFC replaces the fixed-arity callable traits `Callable0[R]`, `Callable1[A, R]`, and `Callable2[A, B, R]` with one trait, `Callable[(A, B) -> R]`, whose single type argument is an ordinary Incan function type. The function type carries both the parameter list and the return type, so the trait has no arity limit, a model can hold a callable in a bounded field (`model Holder[F with Callable[(int) -> str]]`), and a model that adopts `Callable[(int, int) -> int]` is checked against the `__call__` signature the argument spells. The RFC 041 markers `Fn[...]`, `FnMut[...]`, and `FnOnce[...]` keep their spelling, lose their two-parameter limit, and mean `Callable[(params) -> R]` with the return type taken from the call that passes the value. `Callable` is compiler-known: `std.traits.callable` declares and documents it, and the checker derives the `__call__` requirement from the argument, because a source trait cannot derive a method signature from a type argument. The RFC records why the generated code cannot keep one nominal trait per arity without imposing a limit, and recommends lowering `Callable` bounds to the host language's own function traits.

## Core model

1. **One trait.** `Callable[F]` takes exactly one type argument, and `F` must be a function type: `() -> R`, `(A) -> R`, `(A, B) -> R`, and so on. Function types are written with `->`; `=>` belongs to closure expressions only.
2. **The argument is the contract.** The trait's one requirement is `__call__(self, ...)` with the argument's parameter count, parameter types, and return type. Whatever a function type can express, `Callable` expresses, with no rule of its own.
3. **No arity limit.** The language imposes none, and the generated code must not impose one that a program can observe.
4. **Three ways to satisfy a bound.** A named function or closure whose function type matches, a declaration that adopts the matching `Callable`, or a type parameter already bounded by it.
5. **The markers are shorthand.** On a function or method type parameter, `Fn[A, B]`, `FnMut[A, B]`, and `FnOnce[A, B]` mean `Callable[(A, B) -> R]` with `R` taken from each call. A nominal declaration has no call to take `R` from, so a marker there stays refused and the diagnostic points at `Callable`.
6. **One identity everywhere.** A value that satisfies a `Callable` bound in one package satisfies the same bound declared in any other package.

## Motivation

The callable vocabulary stops at two parameters. `Callable2[A, B, R]` is the largest trait, so a generic callback with three arguments has no bound to write, and the `Fn[...]` markers, which lower through the same vocabulary, refuse a third parameter with `INCAN-T0106`. The standard library admits the gap in `std.traits.callable` itself, where a comment suggests declaring `Callable3`, `Callable4`, and so on by hand, or gathering the arguments into a model. Neither is a language answer. A three-argument callback is ordinary, and the limit is an artifact of spelling each parameter as a separate type argument.

The marker spelling has a second gap. `Fn[int]` names the parameters and leaves the return type to the call that passes the value. A function has such a call; a model does not. `model Holder[F with Fn[int]]` therefore has no return type anywhere, and the checker refuses it. The workaround is `Callable1[int, R]` with an extra type parameter for `R`, which stops at two parameters and makes the author count arguments to pick a trait name.

Both gaps close when the bound spells a function type. `(A, B, C) -> R` already exists as the type of a function value: it has no arity limit, it names the return type, and it gains every future feature of function types without further work, for example the `mut` parameter marker proposed in #1790. `Callable[(A, B, C) -> R]` puts that function type inside a capability, so it works as a bound, as an adoption, and on a nominal owner.

## Goals

- Define `Callable[F]`, with a function-type argument, as the one callable capability, with no arity limit.
- Let functions, methods, models, classes, enums, traits, newtypes, and type aliases bound a type parameter with `Callable[...]`, and let nominal declarations hold such a value in a field.
- Check an adopter's `__call__` against the signature its `Callable` argument spells, with a diagnostic that shows the expected signature.
- Keep the RFC 041 markers, remove their parameter limit, and narrow `INCAN-T0106` to a marker on a nominal declaration.
- Remove `Callable0`, `Callable1`, and `Callable2`, with diagnostics that name the replacement spelling and a migration of every use in the standard library, examples, and documentation.
- Keep one identity for the callable capability across package boundaries, so a function, a closure, or an adopting model satisfies a bound declared in another package.
- State which promises of RFC 068, RFC 041, RFC 115, and, if the shorthand is retired, RFC 035 this RFC replaces, without editing those RFCs.

## Non-Goals

- Changing closure syntax, closure parameter inference, or what a closure may capture.
- Adding keyword, default, or rest parameters to function types. `Callable` inherits function types as they are.
- Designing the `mut` parameter marker. #1790 owns it; `Callable` inherits it when it lands.
- Allowing the `Fn[...]` markers on nominal declarations. `Callable[...]` is the spelling there.
- Satisfying a `Callable` bound structurally. As RFC 068 already rules, a type that defines `__call__` without adopting `Callable` can be called, but only adoption satisfies a bound.
- Overloading `__call__`. A declaration adopts at most one `Callable` instantiation, because it has one `__call__`.
- Variadic generics in general.

## Guide-level explanation

### Bounding a callback

`Callable` names what can be called, with which arguments, and what comes back. The type argument is the function type you would write for a function value:

```incan
from std.traits.callable import Callable


def render_row[Formatter with Callable[(str, int, float) -> str]](formatter: Formatter) -> str:
    return formatter("widget", 3, 9.5)


def csv_row(name: str, count: int, price: float) -> str:
    return f"{name},{count},{price}"


def main() -> None:
    println(render_row(csv_row))
    println(render_row((name, count, price) => f"{count} x {name} at {price}"))
```

A named function and a closure both satisfy the bound. The closure's parameter types come from the bound, as they do today. Any number of parameters works, including none:

```incan
def twice[Make with Callable[() -> int]](make: Make) -> int:
    return make() + make()
```

The return type is part of the argument, so it can be generic:

```incan
def map_all[U, Mapper with Callable[(int) -> U]](items: list[int], mapper: Mapper) -> list[U]:
    mut out: list[U] = []
    for item in items:
        out.append(mapper(item))
    return out
```

### Adopting `Callable`

A model or class that owns callable behavior adopts `Callable` and defines `__call__`. The checker derives the required signature from the argument:

```incan
model TableRow with Callable[(str, int, float) -> str]:
    separator: str

    def __call__(self, name: str, count: int, price: float) -> str:
        return f"{name}{self.separator}{count}{self.separator}{price}"


def main() -> None:
    println(render_row(TableRow(separator=" | ")))
```

A `__call__` that does not match is refused where it is declared, and the diagnostic shows the signature the argument asks for (wording illustrative):

```incan
model Broken with Callable[(str, int, float) -> str]:
    def __call__(self, name: str, count: str, price: float) -> str:
        return name
```

```text
'Broken.__call__' does not match 'Callable[(str, int, float) -> str]': parameter 2 is 'str', expected 'int'
  expected: def __call__(self, _: str, _: int, _: float) -> str
```

### Holding a callable in a model

Because the return type is spelled, a nominal declaration can bound its own type parameter and store the value:

```incan
from std.traits.callable import Callable


model Validator[Check with Callable[(str) -> bool]]:
    name: str
    check: Check

    def run(self, value: str) -> str:
        check = self.check
        if check(value):
            return f"{self.name}: ok"
        return f"{self.name}: rejected"


def main() -> None:
    not_empty = Validator(name="not-empty", check=(value) => len(value) > 0)
    println(not_empty.run("incan"))
    println(not_empty.run(""))
```

The same works for classes, enums, traits, newtypes, and type aliases.

### Rust interop markers

The `std.rust` markers keep their spelling. They name parameters only and take the return type from the value passed, which a function or method call supplies:

```incan
from std.rust import Fn


def run_triple[F with Fn[int, int, int]](f: F) -> None:
    f(1, 2, 3)
```

There is no parameter limit. On a model's type parameter there is no call to take the return type from, so the marker is refused, and the message points at the spelling that works there (wording illustrative):

```incan
model Holder[F with Fn[int]]:
    callback: F
```

```text
INCAN-T0106: Callable marker 'Fn[int]' cannot bound type parameter 'F' of model 'Holder'; write 'Callable[(int) -> R]' with 'R' the return type
```

### From the old names

| Before                     | After                          |
| -------------------------- | ------------------------------ |
| `Callable0[R]`             | `Callable[() -> R]`            |
| `Callable1[A, R]`          | `Callable[(A) -> R]`           |
| `Callable2[A, B, R]`       | `Callable[(A, B) -> R]`        |
| no three-argument trait    | `Callable[(A, B, C) -> R]`     |

The old names no longer resolve. Code that still uses one is refused with a diagnostic that shows its replacement with the actual type arguments filled in.

## Reference-level explanation

### The `Callable` trait

- `std.traits.callable` must export exactly one callable trait, `Callable`, and the standard prelude must re-export it.
- `Callable` must take exactly one type argument, and that argument must be a function type. The zero-parameter form is `() -> R`.
- For `Callable[(P1, ..., Pn) -> R]`, the one required method is `__call__` with the signature `(self, p1: P1, ..., pn: Pn) -> R`: `n` parameters after the receiver, in order, with the argument's parameter types, and the argument's return type. Parameter names are not part of the contract.
- A parameter marker a function type carries, such as the `mut` marker proposed in #1790, must carry over to the corresponding `__call__` parameter unchanged. `Callable[(mut Box, int) -> int]` requires `__call__(self, mut box: Box, value: int) -> int`.
- The argument may mention any type in scope, including the type parameters of the declaration that owns the bound, and may itself contain function types.

### Where `Callable` may appear

- As a `with` bound on a type parameter of a function, method, model, class, enum, trait, newtype, or type alias.
- In the adoption list of a model, class, enum, or newtype.
- As a supertrait in a trait declaration's adoption list (RFC 042).
- In annotation position, where RFC 042 gives every trait instantiation its abstract meaning: some value that satisfies the bound, whether a matching function, a matching closure, or an adopter.

### Satisfying a bound

A type `T` satisfies `Callable[(P1, ..., Pn) -> R]` when one of these holds:

1. `T` is a function type, as for a named function or a closure, with exactly `n` parameters, and a value of `T` is assignable to a variable annotated `(P1, ..., Pn) -> R`.
2. `T` is a declaration that adopts `Callable[G]` with `G` compatible with the bound's argument under the same rule.
3. `T` is a type parameter whose own bounds include a compatible `Callable`.

Any other type must be refused at the call, construction, or assignment that supplies it, and the diagnostic must name the bound and the supplied type. A closure passed where a `Callable` bound is expected must take its parameter types from the bound.

### Calling a bounded value

A value whose type is a `Callable`-bounded type parameter must be callable with call syntax. The call must be checked as a call to a value of the argument's function type, and its result has the argument's return type.

### Adoption

- A declaration that adopts `Callable[G]` must define `__call__` with the signature derived from `G`. A missing `__call__`, a different parameter count, a differing parameter type or marker, or a differing return type must be refused at the declaration, and the diagnostic must show the derived signature.
- A declaration must not adopt `Callable` more than once, directly or through supertraits.
- Adoption does not change call syntax: RFC 068 already resolves `value(args)` through a compatible `__call__`.

### The `Fn`, `FnMut`, and `FnOnce` markers

- On a type parameter of a function or method, `F with Fn[P1, ..., Pn]` must mean `F with Callable[(P1, ..., Pn) -> R]`, where `R` is a type the checker determines from the argument at each call. `FnMut` and `FnOnce` follow the same rule. A bare marker names zero parameters. There must be no limit on `n`.
- On a type parameter of any other declaration, a marker must be refused with `INCAN-T0106`. The message must name the replacement `Callable[(P1, ..., Pn) -> R]` with the written parameter types filled in.

### Removed names

`Callable0`, `Callable1`, and `Callable2` must not resolve. An import, bound, adoption, or annotation that names one of them must be refused with a diagnostic that shows the replacement built from the written type arguments: `Callable1[int, str]` becomes `Callable[(int) -> str]`.

### Diagnostics

- A `Callable` with no type argument, with more than one, or with one that is not a function type must be refused under one new stable `INCAN-T` code. The message must show the corrected spelling built from the written arguments: `Callable[int, str]` becomes `Callable[(int) -> str]`. For a single argument that is not a function type, such as `Callable[int]`, either `Callable[(int) -> R]` or `Callable[() -> int]` may be meant, so the message must offer both.
- The removed-name diagnostic may share that code or take its own; either way it must have a catalog entry and an `incan explain` text.
- `INCAN-T0106` keeps its code and covers only a marker on a nominal declaration. Its parameter-count case must be removed from the catalog entry, the `incan explain` text, and the CLI reference.

### Library metadata

A library's checked metadata must record a `Callable` bound in its source spelling, `Callable[(A, B) -> R]`, so a consumer checks its arguments against the same contract the library was checked against.

## Design details

### Syntax

No new syntax is needed for bounds and adoption. `Callable[(A, B) -> R]` is a trait name applied to one type argument, and `(A, B) -> R` already parses as a type in that position.

### Why a trait around a function type

A bare function type as the bound, `F with (int) -> str`, was considered and rejected. A function type is the type of a value; a `with` clause names capabilities, and today the checker refuses a type in bound position because no argument can satisfy it. Adoption makes the difference visible: `model TableRow with (str, int, float) -> str` reads as a claim that the model is a function type, which it is not; it has a call. `model TableRow with Callable[(str, int, float) -> str]` reads as what it is. Diagnostics follow the same line: "does not adopt `Callable[(int) -> str]`" names a capability the author can add.

### Why `Callable` is compiler-known

A source trait declares its methods with fixed parameter lists. `Callable`'s one method has as many parameters as its argument has, and Incan has no variadic generics to spell that in source. The checker therefore supplies the requirement: it reads the argument and derives `__call__`. `std.traits.callable` still declares `Callable`, so the name has a home, an import path, and documentation, and the reference pages are generated from it like any other trait.

### Interaction with existing features

- **Call syntax (RFC 068).** Unchanged. `__call__` stays the hook; call syntax resolves through a compatible hook with or without adoption; bounds require adoption.
- **Traits as annotations (RFC 042).** `Callable[(A) -> R]` in annotation position has RFC 042's abstract meaning. A function-typed annotation `(A) -> R` keeps its meaning as the type of function values.
- **Decorators (RFC 036).** Decorator shapes are function types and are unaffected.
- **Fallible iteration (RFC 115).** The combinators keep their behavior; their bounds change spelling, for example `Callable[(T) -> U]` for `map` and `Callable[(U, T) -> U]` for `fold`.
- **Rust interop (RFC 041).** The markers keep their spelling and meaning, without the parameter limit.
- **Function-type features.** Anything a function type gains later, `Callable` gains with it, because the argument is a function type and the checker derives `__call__` from it rather than from a list of its own.

### Supersession of closed RFC decisions

Closed RFCs remain historical records and are not edited. This RFC replaces the following promises for code written against it:

- **RFC 068:** the "Callable object" row of its protocol table, which names "fixed-arity callable traits such as `Callable0[R]`, `Callable1[A, R]`, and `Callable2[A, B, R]`" as the nominal capability, and the Design Decisions bullet that lists "fixed-arity callable traits" among the capabilities generic code can name. The capability is `Callable[(A, B) -> R]`, one trait for every arity. RFC 068's `__call__` hook, its structural resolution of call syntax, its rule that bounds require adoption, and its rule that a hook must type-check against the trait it claims all remain, the last one now against the derived signature.
- **RFC 041:** its recorded lowering of the markers, under which each marker becomes the fixed-arity callable trait of its arity, a marker names at most two parameters, and all three markers lower to the same bound. The markers now mean `Callable[(params) -> R]` without a limit, and how `FnMut` and `FnOnce` differ in the generated code follows the backend decision below. The Incan-facing marker names, their use in `with` clauses, and the refusal of a marker on a nominal declaration's type parameter remain.
- **RFC 115:** the statement that callback parameters "use the canonical fixed-arity callable traits" with "explicit `Callable1` / `Callable2` adopters", the `Callable1[...]` and `Callable2[...]` spellings in its combinator signatures, the paragraph that relies on RFC 068's fixed-arity traits and on "the backend's callable-value bridge", and the non-normative note that a backend may bridge function and closure values to the canonical fixed-arity traits. The combinator set, their semantics, their `Clone` requirements, and the promise that callbacks accept functions, capturing closures, compatible enum variant constructors, and adopting models, without narrowing them to function pointers, remain.
- **RFC 035**, if the shorthand is retired (Unresolved questions): the `Callable[Params, R]` type-position shorthand and its desugaring table. First-class function values and the arrow function type, which RFC 035 already calls canonical, remain.

### Compatibility and migration

This is a breaking change on the 0.6 line. The removed names get no aliases and no deprecation period; the diagnostics carry the exact replacement, so each use is a mechanical rewrite:

- `Callable0[R]` to `Callable[() -> R]`, `Callable1[A, R]` to `Callable[(A) -> R]`, `Callable2[A, B, R]` to `Callable[(A, B) -> R]`, and `from std.traits.callable import Callable1, Callable2` to `from std.traits.callable import Callable`.
- In the standard library: `std.traits.callable` itself, the standard and trait preludes, the feature inventory in `std.features`, and the `FallibleIterator` combinators and their state models in `std.derives.collection`.
- Examples, test programs, and documentation that name the old traits, including the callable, traits, and collection-protocol reference pages, the Rust interop how-to, the CLI reference entry for `INCAN-T0106`, and the release notes.

Published library artifacts whose metadata names the old traits must be rebuilt against a toolchain that implements this RFC.

## Alternatives considered

- **Add `Callable3`, `Callable4`, and so on.** Moves the limit rather than removing it, keeps authors counting arguments to pick a name, and does nothing for the return type of a marker on a nominal owner.
- **Variadic type arguments, `Callable[A, B, R]`.** The last argument silently becomes the return type, `Callable[R]` reads as a one-parameter callable, and the spelling collides with the RFC 035 shorthand, where `Callable[A, R]` already means `(A) -> R`.
- **Python's `Callable[[A, B], R]`.** Incan has no list-of-types syntax, and `(A, B) -> R` is already the language's function type. The language reference maps Python's spelling to the arrow form for exactly this reason.
- **A bare function type as the bound.** Rejected under "Why a trait around a function type".
- **A return type on the marker, `Fn[int] -> str`.** New syntax for one construct, and it makes the Rust interop vocabulary the way to write an ordinary Incan bound. The markers stay shorthand for functions and methods.
- **A hidden return-type parameter on nominal owners.** It would let `model Holder[F with Fn[int]]` compile by adding a parameter the author never wrote, which changes the type's arity everywhere it is named.
- **Keep the fixed-arity traits for the standard library and add `Callable` beside them.** Two spellings for one capability, and the fixed-arity ones would still carry the limit.

## Drawbacks

- Every use of `Callable0`, `Callable1`, and `Callable2` must be rewritten, and, if the shorthand is retired, every use of `Callable[A, R]` in a type position as well.
- `Callable` is a compiler-known trait. `std.traits.callable` documents it but cannot state its requirement in source, which makes it a special case beside the ordinary source traits.
- Until the shorthand question is settled, `Callable[...]` means a function type in a type position and a trait in a bound, which is exactly the kind of position-dependent meaning the language otherwise avoids.
- Under the recommended backend, a model adopter that flows into a `Callable`-bounded parameter is passed as its call, so code that needs the model's own type through that parameter is refused (Unresolved question 2).

## Implementation architecture

This section is non-normative. It records what the generated code must preserve and the options for preserving it.

### Constraints

- The standard library is compiled once, before and independently of any program, into a prebuilt SDK crate. Every executable, test batch, and library package links that crate and reaches the standard library through a re-export of it; none may compile a second copy. The current `Callable0`, `Callable1`, and `Callable2` have one identity only because they live in that crate.
- Library packages are compiled as crates of their own, so a trait generated inside a package is a different trait from the same-named trait generated inside another.
- On a stable toolchain, the host language has one family of callable bounds that covers every arity: its own `Fn`, `FnMut`, and `FnOnce` traits, written in the parenthesized form. A crate's own trait covers closures only through one blanket implementation per arity, and that implementation must be written in the trait's own crate, because the orphan rule refuses a blanket implementation of a foreign trait over an uncovered type parameter. A model, by contrast, can implement a foreign trait anywhere, since the model is local to its own package.

### Option B: one generated trait per arity, per package (not viable)

Generating `Callable3` inside each package that uses three parameters has no limit and lets closures cross packages, because each package's bridge covers every closure. A model does not cross: a model adopting `Callable[(A, B, C) -> R]` implements its own package's trait, a bound declared in another package names that package's trait, and no crate can add the missing implementation because both the trait and the model are foreign to it. This breaks the one-identity requirement.

### Option C: a fixed family in the SDK (a limit)

The SDK crate declares `Callable0` through some `CallableK` with a bridge for each. One identity, but `K` is a limit fixed when the SDK is built, which the no-limit requirement rules out.

### Option A: one tuple-argument trait plus closure adapters

The SDK crate declares one trait over an argument tuple, `Callable<(A, B, C), R>`, which adopting models implement directly. Functions and closures do not implement it; lowering wraps each one in a small package-local adapter wherever it flows into a `Callable`-bounded parameter, and calls go through the trait method with the arguments packed into a tuple. One identity and no limit, and a model adopter keeps its own type. The cost falls on the common case: closures are what nearly every higher-order call passes, including every standard-library combinator call, so wrapping touches almost every such call site, including values nested inside collections and options; adopter implementations change shape to take a tuple; and bounds over parameters the compiler passes without copying need higher-ranked forms spelled out. Several changes to the frozen emitter follow.

### Option D: the host's function traits, with model adopters converted (recommended)

`F with Callable[(A, B) -> R]` lowers to the host's own `F: Fn(A, B) -> R`. Functions and closures satisfy it natively at every arity, and it has one identity in every crate. A model adopter cannot implement the host's function traits on a stable toolchain, so lowering converts it where it flows into a `Callable`-bounded parameter: the adopter is passed as a closure that calls its `__call__`. Adoption itself generates no trait implementation; `__call__` remains an ordinary method that call syntax already reaches. The markers map to their namesakes, `Fn` to `Fn`, `FnMut` to `FnMut`, and `FnOnce` to `FnOnce`, which closes the gap recorded in RFC 041 where all three lowered to the same bound and a closure that is only `FnMut` or `FnOnce` was refused.

Its costs, stated plainly:

- The emitter prints trait bounds only in the angle-bracket form, which the host refuses for its function traits. D needs one recorded change to the frozen emitter: a bound printer for the parenthesized form. Removing the fixed-arity bridge the emitter prints today is a deletion in the same area.
- A model adopter passed to a `Callable`-bounded parameter is passed as its call, so the type parameter is instantiated with the function type, not the model. Code that relies on the model's own type through that parameter, such as an explicit `Validator[TableRow]` or reading `validator.check.separator`, cannot be supported and must be refused (Unresolved question 2).
- The conversion happens where lowering can see the flow: call arguments, constructor fields, assignments, and returns. A model adopter nested inside a collection or option that flows into `list[F]` or `Option[F]` needs an element-wise conversion or a refusal.
- A value bounded by `FnOnce[...]` may be called once. The checker must refuse a second call on any path, or the host compiler reports it in generated code. A value bounded by `FnMut[...]` is called through a binding lowering must declare as changeable.
- A trait that adopts `Callable` as a supertrait cannot pass it on as a host function-trait supertrait, because its model adopters cannot implement that. Its lowered form carries the derived `__call__` as an ordinary trait method instead, and generic code bounded by that trait calls through it.

D is recommended because the cost lands on the rare case, a model adopter, instead of the common one, a function or closure, and because the host's parenthesized bounds already handle parameters the compiler passes without copying at every arity. Every option requires rebuilding published artifacts whose metadata names the old traits.

## Layers affected

- **Parser / AST**: none for bounds and adoption. If the shorthand is retired, the type-position rewrite of `Callable[Params, R]` is removed, and a type-position `Callable[...]` with two arguments must be refused with the arrow spelling.
- **Typechecker / Symbol resolution**: `Callable` is a compiler-known trait whose `__call__` requirement is derived from its function-type argument; bound satisfaction, adoption validation, the single-adoption rule, and calls through bounded values follow that requirement; the markers mean `Callable[(params) -> R]` without a count limit; `INCAN-T0106` narrows to nominal owners; the new stable code and the removed-name diagnostic are added; under Option D, a model adopter supplied to a `Callable`-bounded parameter instantiates it with the function type, and a second call of an `FnOnce[...]` value is refused.
- **IR Lowering**: `Callable` bounds and markers lower to the chosen backend representation with no arity limit; under Option D, model adopters are converted at the flow sites listed above, and `FnMut`-bounded values get a changeable binding.
- **Emission**: under Option D, one recorded change to the frozen emitter for parenthesized function-trait bounds, and removal of the fixed-arity bridge.
- **Stdlib**: `std.traits.callable` declares `Callable` and carries its documentation, with a three-argument example and a model holding a callable; the preludes re-export it; the `FallibleIterator` combinators and the feature inventory migrate. If the shorthand is retired, `std.result` and `std.regex` signatures migrate to arrow types.
- **Library metadata**: `Callable` bounds are recorded in their source spelling.
- **Formatter**: prints `Callable[(A, B) -> R]` bounds and adoption lists without rewriting the argument.
- **LSP / Tooling**: hover shows `Callable[(A, B) -> R]`; completion offers `Callable` from `std.traits.callable` and the prelude; `incan explain` covers the new code and the narrowed `INCAN-T0106`.

## Inspectability and tooling surface

- **Artifact or metadata:** a library's checked metadata records each `Callable` bound in source spelling; generated reference documentation for `std.traits.callable` comes from its declaration.
- **Inspection command:** `incan check` reports every refusal named above; `incan explain` renders the new code and `INCAN-T0106`; LSP hover shows the bound and the derived `__call__` signature.
- **Diagnostics:** the new stable code for a malformed `Callable` argument, the removed-name diagnostic with its replacement, the adoption mismatch with the derived signature, and `INCAN-T0106` for a marker on a nominal owner.
- **Provenance:** each diagnostic is anchored at the bound, adoption entry, or `__call__` declaration that caused it; a bound violation names the call or construction site that supplied the type.
- **Not implicit:** no arity limit exists anywhere, in the language or the generated code; a marker's hidden return type is visible in hover as the `R` of its `Callable` meaning; under Option D, the conversion of a model adopter is a checked step whose result, the function type, is what hover shows for the instantiated parameter.

## Acceptance criteria

This RFC is done when:

- the checker accepts `Callable` bounds with zero, one, three, and five parameters and with a generic return type, and refuses a non-function argument, a missing argument, and extra arguments under the new stable code with the corrected spelling;
- adoption with a matching `__call__` is accepted and a mismatch in count, parameter type, or return type is refused with the derived signature, and a second `Callable` adoption on one declaration is refused;
- each nominal declaration kind (model, class, enum, trait, newtype, type alias) accepts a `Callable`-bounded type parameter;
- `Fn[...]`, `FnMut[...]`, and `FnOnce[...]` with three or more parameters are accepted on functions and methods, and a marker on a nominal owner is refused with `INCAN-T0106` pointing at `Callable[(...) -> R]`;
- every use of `Callable0`, `Callable1`, and `Callable2` is refused with its replacement, and none remains in the standard library, examples, tests, or documentation;
- behavior fixtures run: a three-argument callback passed as a named function, a closure, and an adopting model; a model holding a `Callable[(int) -> str]` field constructed with a closure; a zero-argument callable; a callable with thirteen parameters, past twelve, where the host's standard library stops implementing its traits for tuples; an adopting model from one package passed to a `Callable`-bounded function declared in another; and, under Option D, each marker lowered to its namesake, with a fixture for a closure that only `FnMut[...]` admits if Incan source can write one, or a checker test that pins why it cannot;
- the reference pages for callable objects, traits, and collection protocols, the Rust interop how-to, the CLI reference, and the release notes describe `Callable[(A, B) -> R]` and the removal;
- if the shorthand is retired, a type-position `Callable[A, R]` is refused with its arrow spelling and no use remains.

## Design Decisions

- **One trait, one argument.** `Callable` takes exactly one type argument, an ordinary function type written with `->`. `=>` stays closure-only.
- **No arity limit.** Neither the language nor the generated code imposes one.
- **The return type is spelled.** Nominal owners can bound a type parameter with `Callable[...]` and hold the value in a field.
- **Adoption is checked.** `__call__` is derived from the argument and checked at the declaration.
- **A trait around a function type, not a bare function type.** A function type is a value type; `with` names a capability, and adoption reads right only with a capability.
- **The old names are removed.** `Callable0`, `Callable1`, and `Callable2` get no aliases; diagnostics carry the replacement, and every use in the standard library and documentation is migrated.
- **The markers keep their spelling.** `Fn`, `FnMut`, and `FnOnce` lower to `Callable[(params) -> R]` with the return from the call, have no parameter limit, and stay refused on nominal owners with `INCAN-T0106` pointing at `Callable[...]`.
- **Inherited features.** `Callable` expresses whatever function types express, including the `mut` parameter marker of #1790 once it lands, with no rule of its own.
- **Compiler-known.** `std.traits.callable` declares and documents `Callable`; the checker supplies its requirement.
- **One identity.** A callable value satisfies the same bound in every package; a backend that breaks this is not acceptable.

## Unresolved questions

- **Which backend representation carries `Callable`?** The options are under "Implementation architecture": B breaks cross-package adoption, C is a limit, A wraps every closure, and D wraps model adopters. Recommendation: Option D, lowering to the host's own function traits, with the marker mutability gap closed as a side effect, one recorded emitter change, and the costs listed there accepted.
- **Under Option D, what type argument does a model adopter produce?** When `TableRow` flows into `Check with Callable[(str) -> bool]`, the generated code passes it as its call. Recommendation: the checker instantiates `Check` with the function type `(str) -> bool`, not `TableRow`; an explicit type argument naming an adopter, such as `Validator[TableRow]`, is refused with a diagnostic that names the function type; and a model adopter nested in a collection or option supplied to `list[Check]` or `Option[Check]` is refused with a hint to convert the elements with a closure.
- **Should the RFC 035 `Callable[Params, R]` type-position shorthand be retired?** With `Callable` a trait, RFC 042 already gives `Callable[(A) -> R]` a meaning in annotation position, and the shorthand gives `Callable[A, R]` another, so the same name would mean two things depending on position and argument count. Recommendation: retire it in the same release, with no deprecation period, so `(A) -> R` is the only spelling of a function type; refuse a type-position `Callable[A, R]` with a diagnostic that shows the arrow form; migrate its uses in `std.result` (six signatures), `std.regex` (three), the feature inventory, two examples, test programs, about ten documentation pages, and the open draft RFCs that use it.
- **May an adopter's `__call__` take `mut self`?** A stateful callable object changes itself on each call, which a plain function value does not. Recommendation: no, in this RFC. `Callable`'s derived receiver is `self`, a `mut self` `__call__` is refused as a mismatch, and stateful callables are left to a follow-up that decides how they relate to the `FnMut` marker.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
