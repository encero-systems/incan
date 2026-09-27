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

This RFC replaces `Callable0[R]`, `Callable1[A, R]`, and `Callable2[A, B, R]` with one trait, `Callable[(A, B) -> R]`, with no arity limit. Its function-type argument describes how to call a value; it does not convert that value into a closure or replace its concrete type. Function types gain named, keyword-only, optional, and variadic parameters through the same signature rules used by function and method declarations. Bound receivers are represented separately from caller-supplied parameters, preserving mutable access and ownership requirements. Compiler-derived, read-only `__signature__` metadata exposes this shared model. Stateful `__call__(mut self)` adopters are supported; the `Fn`, `FnMut`, and `FnOnce` markers retain distinct invocation guarantees.

## Core model

1. **One trait.** `Callable[F]` takes one function-type argument. `->` describes a signature; `=>` constructs a closure.
2. **One signature model.** Functions, methods, closures, and callable objects share parameter and return-type rules. `Callable` does not define a parallel parameter system.
3. **Caller-facing parameters.** A bound `self` or `cls` is absent from the argument list. Receiver binding, access, and consumption remain part of the callable contract.
4. **Objects retain their identity.** Satisfying a bound preserves the concrete type, fields, other traits, and state. Calling an object invokes its `__call__`.
5. **Explicit mutation.** `__call__(mut self)` is valid. Invoking it requires mutable access in source; lowering cannot invent that permission.
6. **No arity limit or package-local capability.** The same bound has the same meaning across packages, for functions, closures, and nominal adopters.
7. **Signature metadata.** `__signature__` is a read-only view of the shared signature contract, not a mechanism for changing it.

## Motivation

The callable vocabulary stops at two parameters. `Callable2[A, B, R]` is the largest trait, so a generic callback with three arguments has no bound to write, and the `Fn[...]` markers, which lower through the same vocabulary, refuse a third parameter with `INCAN-T0106`. The standard library admits the gap in `std.traits.callable` itself, where a comment suggests declaring `Callable3`, `Callable4`, and so on by hand, or gathering the arguments into a model. Neither is a language answer. A three-argument callback is ordinary, and the limit is an artifact of spelling each parameter as a separate type argument.

The marker spelling has a second gap. `Fn[int]` names the parameters and leaves the return type to the call that passes the value. A function has such a call; a model does not. `model Holder[F with Fn[int]]` therefore has no return type anywhere, and the checker refuses it. The workaround is `Callable1[int, R]` with an extra type parameter for `R`, which stops at two parameters and makes the author count arguments to pick a trait name.

Both gaps close when the bound spells a function type. `(A, B, C) -> R` already exists as the type of a function value: it has no arity limit, it names the return type, and it gains every future feature of function types without further work, for example the `mut` parameter marker proposed in #1790. `Callable[(A, B, C) -> R]` puts that function type inside a capability, so it works as a bound, as an adoption, and on a nominal owner.

## Goals

- Define `Callable[F]`, with a function-type argument, as the one callable capability, with no arity limit.
- Let functions, methods, models, classes, enums, traits, newtypes, and type aliases bound a type parameter with `Callable[...]`, and let nominal declarations hold such a value in a field.
- Check an adopter's `__call__` against the complete caller-facing signature, including keyword names and omission rules.
- Share signature semantics across functions, methods, closures, and callable objects, and expose compiler-derived `__signature__` metadata.
- Preserve concrete types and receiver permissions through generic calls, stored fields, and package boundaries.
- Keep the RFC 041 markers, remove their parameter limit, and narrow `INCAN-T0106` to a marker on a nominal declaration.
- Remove `Callable0`, `Callable1`, and `Callable2`, with diagnostics that name the replacement spelling and a migration of every use in the standard library, examples, and documentation.
- Keep one identity for the callable capability across package boundaries, so a function, a closure, or an adopting model satisfies a bound declared in another package.
- State which promises of RFC 068, RFC 041, RFC 115, and RFC 035 this RFC replaces, without editing those RFCs.

## Non-Goals

- Changing closure syntax, closure parameter inference, or what a closure may capture.
- Defining a general runtime reflection system or permitting writable signature metadata.
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


def render_row[Formatter with Callable[(str, int, float) -> str]](mut formatter: Formatter) -> str:
    return formatter("widget", 3, 9.5)


def csv_row(name: str, count: int, price: float) -> str:
    return f"{name},{count},{price}"


def main() -> None:
    println(render_row(csv_row))
    println(render_row((name, count, price) => f"{count} x {name} at {price}"))
```

A named function and a closure both satisfy the bound. The closure's parameter types come from the bound, as they do today. Any number of parameters works, including none:

```incan
def twice[Make with Callable[() -> int]](mut make: Make) -> int:
    return make() + make()
```

The return type is part of the argument, so it can be generic:

```incan
def map_all[U, Mapper with Callable[(int) -> U]](items: list[int], mut mapper: Mapper) -> list[U]:
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

### Stateful objects stay objects

```incan
model Counter with Callable[() -> int]:
    value: int

    def __call__(mut self) -> int:
        self.value += 1
        return self.value

    def current(self) -> int:
        return self.value


def advance[C with Callable[() -> int]](mut counter: C) -> int:
    return counter()


def main() -> None:
    mut counter = Counter(value=0)
    println(advance(counter))  # 1
    println(counter.current())  # 1: the same Counter
    println(counter())  # 2
```

Calling through the bound preserves the original object and its state. `C` is `Counter`, including any other traits it adopts; it does not become a function type. A combined bound such as `C with (Named, Callable[() -> int])` retains both capabilities. An explicit owner such as `Holder[Counter]` stores a `Counter`, with its fields and methods intact.

The generic helpers here use `mut` because they permit stateful callbacks. A read-only receiver can also be called through mutable access. A read-only binding cannot invoke a callable that requires `mut self`, and a backend adapter must not make that operation legal.

### Keyword arguments and optional parameters

An unnamed signature describes positional calls:

```incan
Callable[(str, int) -> str]
```

It does not promise that either argument has a particular keyword name. Named parameters make that promise explicit:

```incan
Callable[(text: str, limit: int) -> str]
```

Both positional calls and `formatter(text="hello", limit=20)` are supported by that contract. To require a keyword and allow its omission:

```incan
Callable[(text: str, *, limit: int = ...) -> str]
```

Here `text` is required, `limit` is keyword-only and optional, and the implementing callable supplies the actual default. `...` records permission to omit an argument; it is not a default expression evaluated by the caller.

The same signature vocabulary includes positional-only parameters and variadic positional or keyword parameters, using declaration syntax:

```incan
Callable[(text: str, /, *, limit: int = ...) -> str]
Callable[(*values: int, **options: str) -> str]
```

The latter accepts additional positional integers and additional keyword string values. Types describe each variadic element, following the declaration rules. These are proposed function-type extensions, shared with declarations rather than implemented solely inside `Callable`.

### Methods, receivers, and signature metadata

For a method declared as `def format(self, text: str, *, limit: int = 80) -> str`, the bound value `formatter.format` has caller-facing signature `(text: str, *, limit: int = ...) -> str`. The object already supplies `self`.

| Callable form | Receiver treatment |
| --- | --- |
| Bound instance method | Instance supplies `self`; absent from caller-facing parameters |
| Bound class method | Class supplies `cls`; absent from caller-facing parameters |
| Static method | No implicit receiver |
| Callable object | Object supplies the receiver of `__call__` |
| Unbound instance method, where supported | Caller must supply the receiver |

A bound method declared with `mut self` retains its mutable receiver requirement and borrow lifetime. Removing a bound receiver from the parameter list does not remove those constraints. Receiver roles come from declaration and binding semantics, not simply from a parameter being named `self` or `cls`.

`formatter.format.__signature__` exposes compiler-derived, read-only signature metadata. Conceptually it describes `text` and `limit`, their types and parameter kinds, whether each is required, the return type, and the separately recorded bound receiver contract. A callable object's `__signature__` describes calling that object, rather than its constructor. Metadata cannot be assigned to in order to change a signature or bypass receiver checks. The concrete metadata types and runtime availability policy remain to be specified below.

### Holding a callable in a model

Because the return type is spelled, a nominal declaration can bound its own type parameter and store the value:

```incan
from std.traits.callable import Callable


model Validator[Check with Callable[(str) -> bool]]:
    name: str
    check: Check

    def run(mut self, value: str) -> str:
        mut check = self.check
        if check(value):
            return f"{self.name}: ok"
        return f"{self.name}: rejected"


def main() -> None:
    mut not_empty = Validator(name="not-empty", check=(value) => len(value) > 0)
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
- The required method is `__call__`, with caller-facing parameters and result compatible with the function-type argument. Both `self` and `mut self` receivers must be supported; the receiver is recorded separately from explicit parameters.
- Parameter names, kinds, defaults, variadic element types, and return types must use the same signature rules as function and method declarations. Named parameters promise keyword availability; unnamed entries promise positional access only. `/`, `*`, `*args`, and `**kwargs` must retain their declaration meanings. Optional parameters are written `name: T = ...`; a type contract must not evaluate or replace the implementation's default.
- A parameter marker such as the `mut` marker proposed in #1790 must carry over unchanged. `Callable[(mut Box, int) -> int]` describes mutation permission for an explicit argument, independently of whether `__call__` has `self` or `mut self`.
- The argument may mention any type in scope, including the type parameters of the declaration that owns the bound, and may itself contain function types.

### Where `Callable` may appear

- As a `with` bound on a type parameter of a function, method, model, class, enum, trait, newtype, or type alias.
- In the adoption list of a model, class, enum, or newtype.
- As a supertrait in a trait declaration's adoption list (RFC 042).
- In annotation position, where RFC 042 gives every trait instantiation its abstract meaning: some value that satisfies the bound, whether a matching function, a matching closure, or an adopter.

### Satisfying a bound

A type `T` satisfies `Callable[(P1, ..., Pn) -> R]` when one of these holds:

1. `T` is a function type, as for a named function or a closure, whose accepted calls cover the required signature, and a value of `T` is assignable to a variable annotated `(P1, ..., Pn) -> R`.
2. `T` is a declaration that adopts `Callable[G]` with `G` compatible with the bound's argument under the same rule.
3. `T` is a type parameter whose own bounds include a compatible `Callable`.

Any other type must be refused at the call, construction, or assignment that supplies it, and the diagnostic must name the bound and the supplied type. A closure passed where a `Callable` bound is expected must take its parameter types from the bound.

### Calling a bounded value

A value whose type is a `Callable`-bounded type parameter must be callable with call syntax. Argument binding and result typing must use the shared signature rules. Every call allowed by the required signature must be accepted by the supplied callable: keyword names and kinds must be compatible, promised optional parameters must be omittable, and an implementation must not introduce additional required arguments. A callable may provide additional defaults or accept more calls without requiring callers to use them. The same compatibility relation must apply to bare function types and `Callable` arguments.

Receiver access and invocation multiplicity must also be checked. `mut` is required to invoke a mutating receiver. A read-only generic parameter must have a proven read-only call capability; a signature-only bound must not be treated as such proof. If a consuming callable is supplied, ownership analysis must prove each invocation legal; repeated invocation cannot be assumed merely from the parameter and result types. These requirements must survive generic forwarding and library metadata. A function body requiring repeatable or read-only invocation must have that requirement checked against supplied callables before lowering, including across package boundaries.

### Adoption

- A declaration that adopts `Callable[G]` must define `__call__` with a signature compatible with `G` under the shared signature rules. It must accept every call promised by `G`; additional optional parameters are permitted. A missing `__call__` or an incompatible parameter contract, mutation marker, or return type must be refused at the declaration, and the diagnostic must show the required signature. A different parameter count alone is not a mismatch.
- A declaration must not adopt `Callable` more than once, directly or through supertraits.
- Adoption does not change call syntax: RFC 068 already resolves `value(args)` through a compatible `__call__`.

### The `Fn`, `FnMut`, and `FnOnce` markers

- Each marker carries the corresponding `Callable[(P1, ..., Pn) -> R]` parameter/result contract, with `R` determined from the supplied callable, plus a distinct receiver guarantee. A bare marker names zero parameters; there must be no arity limit.
- `Fn` permits repeated calls through read-only access. `FnMut` permits repeated calls through mutable access and requires source permission to mutate. `FnOnce` permits consuming invocation; generic code with only that guarantee must not assume a second call is legal.
- These are source checking guarantees, not merely backend choices. Read-only repeatable callables can satisfy mutable or consuming invocation requirements; mutable repeatable callables can satisfy consuming requirements. The reverse must not be assumed. Forwarding through a signature-only bound must not erase the applicable restrictions.
- On a type parameter of any other declaration, a marker must be refused with `INCAN-T0106`. The message must name the replacement `Callable[(P1, ..., Pn) -> R]` with the written parameter types filled in.

### Removed names

`Callable0`, `Callable1`, and `Callable2` must not resolve. An import, bound, adoption, or annotation that names one of them must be refused with a diagnostic that shows the replacement built from the written type arguments: `Callable1[int, str]` becomes `Callable[(int) -> str]`.

### Retired function-type shorthand

The RFC 035 `Callable[Params, R]` function-type shorthand must be retired without a deprecation period. A legacy function-type annotation such as `Callable[int, str]` must be refused with a diagnostic directing the author to `(int) -> str`, preserving its function-type meaning. It must not be silently migrated to the trait annotation `Callable[(int) -> str]`. The diagnostic must use the annotation context to distinguish legacy shorthand from a malformed new trait bound.

### Diagnostics

- A `Callable` with no type argument, with more than one, or with one that is not a function type must be refused under one new stable `INCAN-T` code. The message must use the source context to preserve the intended meaning: in a bound or adoption, malformed `Callable[int, str]` points to `Callable[(int) -> str]`; in a legacy function-type annotation, it points to `(int) -> str` under the retirement rule above. For a single argument that is not a function type, such as `Callable[int]`, either `Callable[(int) -> R]` or `Callable[() -> int]` may be meant, so the message must offer both.
- The removed-name diagnostic may share that code or take its own; either way it must have a catalog entry and an `incan explain` text.
- `INCAN-T0106` keeps its code and covers only a marker on a nominal declaration. Its parameter-count case must be removed from the catalog entry, the `incan explain` text, and the CLI reference.

### Library metadata

A library's checked metadata must preserve the complete signature, receiver requirements, invocation guarantees, and generic invocation requirements so consumers check the same contract. Original nominal type identity and other trait bounds must survive export, import, and forwarding.

### `__signature__`

Functions, methods, closures, and callable objects must expose a compiler-derived, read-only signature description. This description must use the shared signature model and distinguish caller-facing parameters from receiver binding. It must represent parameter order, available keyword names, kinds, types, required versus optional status, variadic element types, return type, and applicable receiver access and consumption requirements. Generic signatures must retain their type parameters and reflect substitutions when specialized.

The metadata must not be an independently editable source of truth. Reading it must not invoke the callable, evaluate defaults, consume the receiver, or acquire permission to mutate it. Metadata need not expose actual default values: omission support is the contract, and default evaluation remains owned by the declaration. The exact runtime data types and availability policy are unresolved; this RFC does not yet promise a runtime object attached to every compiled function.

## Design details

### Syntax

The outer bound and adoption syntax is unchanged. Function-type parameters gain the richer declaration-style forms described above. `Callable[(A, B) -> R]` is a trait name applied to one type argument, and `(A, B) -> R` already parses as a type in that position.

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
- **RFC 041:** its recorded lowering of the markers, under which each marker becomes the fixed-arity callable trait of its arity, a marker names at most two parameters, and all three markers lower to the same bound. The markers now carry a shared signature without an arity limit while preserving their distinct read-only, mutable, and consuming invocation guarantees in source checking. The Incan-facing marker names, their use in `with` clauses, and the refusal of a marker on a nominal declaration's type parameter remain.
- **RFC 115:** the statement that callback parameters "use the canonical fixed-arity callable traits" with "explicit `Callable1` / `Callable2` adopters", the `Callable1[...]` and `Callable2[...]` spellings in its combinator signatures, the paragraph that relies on RFC 068's fixed-arity traits and on "the backend's callable-value bridge", and the non-normative note that a backend may bridge function and closure values to the canonical fixed-arity traits. The combinator set, their semantics, their `Clone` requirements, and the promise that callbacks accept functions, capturing closures, compatible enum variant constructors, and adopting models, without narrowing them to function pointers, remain.
- **RFC 035**: the `Callable[Params, R]` type-position shorthand and its desugaring table. First-class function values and the arrow function type, which RFC 035 already calls canonical, remain.

### Compatibility and migration

This is a breaking change on the 0.6 line. The removed names get no aliases and no deprecation period; the diagnostics carry the exact replacement, so the trait-name migration is mechanical. Call sites and generic helpers must also satisfy the receiver permissions and invocation guarantees specified above:

- `Callable0[R]` to `Callable[() -> R]`, `Callable1[A, R]` to `Callable[(A) -> R]`, `Callable2[A, B, R]` to `Callable[(A, B) -> R]`, and `from std.traits.callable import Callable1, Callable2` to `from std.traits.callable import Callable`.
- In the standard library: `std.traits.callable` itself, the standard and trait preludes, the feature inventory in `std.features`, and the `FallibleIterator` combinators and their state models in `std.derives.collection`.
- Examples, test programs, and documentation that name the old traits, including the callable, traits, and collection-protocol reference pages, the Rust interop how-to, the CLI reference entry for `INCAN-T0106`, and the release notes.

Uses of the retired function-type shorthand in standard-library signatures, examples, tests, documentation, and active RFCs must migrate to arrow function types. Closed RFC 035 remains unchanged as a historical record; its first-class named function behavior remains valid.

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

- Every use of `Callable0`, `Callable1`, and `Callable2` must be rewritten, and every use of `Callable[A, R]` in a type position as well.
- `Callable` is a compiler-known trait. `std.traits.callable` documents it but cannot state its requirement in source, which makes it a special case beside the ordinary source traits.
- Retiring the shipped RFC 035 shorthand requires migration of existing function-type annotations as well as callable bounds.
- Rich signatures require consistent argument binding, compatibility, and metadata across all callable forms. Backend adapters must preserve nominal identity and receiver effects, including through stored and nested values.

## Implementation architecture

This section is non-normative. The shared signature model is the source of truth for declarations, function types, callable checking, diagnostics, and `__signature__`. Binding a method derives its caller-facing view while retaining receiver ownership and access requirements. Separate implementations of keyword binding or signature compatibility for `Callable` would risk divergent language behavior.

The backend must preserve the original source type of an adopting object. Converting an adopter to a closure and substituting that closure as the source generic type is rejected: it loses fields, additional trait bounds, and explicit types such as `Holder[Counter]`. Implementation-only adapters are permitted when they preserve all source-visible behavior, identity, state, and ownership. Nested values in lists or options do not justify a source-level refusal or type substitution.

One possible representation is a canonical SDK callable capability over an argument tuple, with adapters for function and closure values. It must support receiver access and consumption separately and must not acquire an observable arity limit through tuple trait implementations. Keyword binding and omitted arguments must retain the original declaration's behavior. Rust function traits may be used where suitable, but do not replace the source contract.

Per-package traits cannot stand in for one canonical capability, and a fixed family of SDK arity traits cannot satisfy the no-limit rule. The detailed lowering is implementation work constrained by these contracts, including combined traits, supertraits, borrowing, and cross-package behavior; it is not permission to narrow the source language.

## Layers affected

- **Parser / AST**: function-type syntax for named parameters, parameter kinds, omission markers, and variadics, sharing the declaration signature model.
- **Typechecker / Symbol resolution**: signature compatibility, nominal adoption, receiver binding, mutable invocation, consuming calls, generic forwarding, and signature metadata access.
- **IR Lowering / Emission**: preserve concrete types, receiver effects, default dispatch, and canonical callable identity without an arity limit.
- **Stdlib / Library metadata**: export `Callable`, migrate fixed-arity names, describe signatures and receiver requirements, and preserve checked contracts across packages.
- **Formatter**: print richer function types consistently with declaration parameters.
- **LSP / Tooling**: present concrete callable types, signatures, keyword completions, receiver requirements, and actionable mismatch diagnostics.

## Inspectability and tooling surface

- **Artifact or metadata:** checked library metadata preserves signatures, receiver requirements, and original nominal identities; `__signature__` presents the same information under the availability policy still to be specified.
- **Inspection command:** `incan check` reports contract violations before backend compilation; `incan explain` describes callable diagnostics; hover shows the original type and callable signature.
- **Diagnostics:** malformed signature arguments, incompatible parameter names or kinds, missing defaults, adoption mismatch, missing mutable access, and illegal repeated consuming calls must identify the relevant source operation.
- **Provenance:** mismatch messages name the required contract and supplied declaration. Binding a receiver must not hide its access requirements or manufacture a closure type in source-facing output.

## Acceptance criteria

This RFC is done when:

- the checker accepts `Callable` bounds with zero, one, three, and five parameters and with a generic return type, and refuses a non-function argument, a missing argument, and extra arguments under the new stable code with the corrected spelling;
- adoption with a compatible `__call__` is accepted, including an implementation with additional optional parameters; an additional required parameter or another incompatible parameter or return contract is refused with the required signature, and a second `Callable` adoption on one declaration is refused;
- each nominal declaration kind (model, class, enum, trait, newtype, type alias) accepts a `Callable`-bounded type parameter;
- `Fn[...]`, `FnMut[...]`, and `FnOnce[...]` with three or more parameters are accepted on functions and methods, and a marker on a nominal owner is refused with `INCAN-T0106` pointing at `Callable[(...) -> R]`;
- every use of `Callable0`, `Callable1`, and `Callable2` is refused with its replacement, and none remains in the standard library, examples, tests, or documentation;
- behavior fixtures run: a three-argument callback passed as a named function, a closure, and an adopting model; a model holding a `Callable[(int) -> str]` field constructed with a closure; a zero-argument callable; a callable with thirteen parameters, past twelve, where the host's standard library stops implementing its traits for tuples; an adopting model from one package passed to a `Callable`-bounded function declared in another; each marker preserving its invocation guarantees; a mutable `Counter` retaining state through a generic helper; and rejection of mutating invocation through read-only access;
- named and keyword-only calls, omitted defaults, positional-only parameters, and variadics have matching behavior through direct calls, function types, methods, closures, and `Callable` bounds, with negative fixtures for incompatible contracts;
- bound instance and class methods exclude the bound receiver from caller-facing parameters while preserving receiver restrictions; static and supported unbound methods expose the appropriate parameter list;
- explicit nominal generic arguments, combined trait bounds, supertraits, and callable objects nested in options or lists retain their types and capabilities across packages;
- signature metadata reflects parameter kinds, omission support, return types, generic substitutions, and receiver binding without invoking the callable or evaluating defaults, and attempts to overwrite it are refused under the finalized metadata API;
- the reference pages for callable objects, traits, and collection protocols, the Rust interop how-to, the CLI reference, and the release notes describe `Callable[(A, B) -> R]` and the removal;
- a type-position `Callable[A, R]` is refused with its arrow spelling and no use remains.

## Design Decisions

- **One trait, one argument.** `Callable` takes exactly one type argument, an ordinary function type written with `->`. `=>` stays closure-only.
- **No arity limit.** Neither the language nor the generated code imposes one.
- **The return type is spelled.** Nominal owners can bound a type parameter with `Callable[...]` and hold the value in a field.
- **Adoption is checked.** `__call__` is derived from the argument and checked at the declaration.
- **A trait around a function type, not a bare function type.** A function type is a value type; `with` names a capability, and adoption reads right only with a capability.
- **The old names are removed.** `Callable0`, `Callable1`, and `Callable2` get no aliases; diagnostics carry the replacement, and every use in the standard library and documentation is migrated.
- **The markers keep their spelling and guarantees.** They share signature types but retain read-only, mutable, and consuming invocation distinctions. There is no parameter limit; nominal owners use `Callable[...]`.
- **Identity and receivers.** Callable objects remain their original types. `mut self` is supported and requires mutable access; bound receivers remain part of the access contract.
- **Shared signatures and metadata.** Named, optional, keyword-only, and variadic parameters use declaration rules. `__signature__` is compiler-derived and read-only.
- **Legacy shorthand is retired.** Function types use arrow syntax; `Callable[...]` consistently names the callable trait. RFC 035 is explicitly superseded only for its shorthand, with migration diagnostics preserving the old function-type meaning.
- **Inherited features.** `Callable` expresses whatever function types express, including the `mut` parameter marker of #1790 once it lands, with no rule of its own.
- **Compiler-known.** `std.traits.callable` declares and documents `Callable`; the checker supplies its requirement.
- **One identity.** A callable value satisfies the same bound in every package; a backend that breaks this is not acceptable.

## Unresolved questions

- **What concrete API and availability policy does `__signature__` expose?** The shared model, read-only behavior, and receiver treatment are settled. The metadata types, how programs query them, and whether runtime metadata is always available or explicitly retained still need a concrete contract before this RFC advances. This is distinct from compiler and checked-library metadata, which must always retain the information needed for checking.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
