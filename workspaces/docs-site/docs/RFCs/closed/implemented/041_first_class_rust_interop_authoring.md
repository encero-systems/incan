# RFC 041: First-Class Rust Interop Authoring

- **Status:** Implemented
- **Created:** 2026-03-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 013 (Rust crate dependencies)
    - RFC 023 (Compilable stdlib & Rust module binding)
    - RFC 026 (User-defined trait bridges)
    - RFC 029 (Union types)
    - RFC 030 (`std.collections`)
    - RFC 039 (`race` for awaitable concurrency)
- **Issue:** [#175](https://github.com/dannys-code-corner/incan/issues/175)
- **RFC PR:** [#183](https://github.com/dannys-code-corner/incan/pull/183)
- **Written against:** v0.1
- **Shipped in:** v0.2

## Summary

This RFC proposes that once a `rust::...` import resolves, the imported Rust item must behave like an ordinary Incan symbol of the corresponding kind, with compiler-managed provenance replacing handwritten bridge ceremony. Concretely, it makes Rust-origin members and associated items available through ordinary lookup and rebinding, introduces compiler-managed built-in boundary coercions, establishes `rusttype` as the direct Rust-backed interop root above which ordinary `newtype` wrappers remain optional, and allows Incan-authored capability bounds to lower to Rust predicates, all while keeping `rust::` as the explicit dependency boundary and leaving async semantics owned by Incan.

## Core model

Read this RFC as one foundation plus four mechanisms:

**Foundation**: after `rust::...` import resolution succeeds, imported Rust items become first-class compiler symbols.

**Mechanisms**:

1. Rust methods and associated items participate in ordinary Incan lookup and rebinding.
2. Built-in Incan types cross explicit Rust boundaries through compiler-owned coercion matrices with canonical lowerings, admitted target types, and per-target policies.
3. Direct Rust-backed non-builtin types use `rusttype` as the interop-root declaration form, while `newtype` remains the ordinary wrapper syntax above that root.
4. Incan-written capability bounds lower to Rust predicates without exposing raw Rust syntax.

Everything else in the RFC follows from that model: optional wrappers, interop metadata placement, diagnostics, and the reduced need for handwritten Rust adapter layers.

## Motivation

### Today Rust interop is explicit, but still awkward to author against

RFC 005 gave Incan a clear and explicit Rust import surface:

```incan
from rust::serde_json import from_str
from rust::std::time import Instant
```

That solved an important problem: users can reach Rust crates without pretending Rust is part of Incan's standard library. However, the current model still treats Rust interop more like a boundary crossing than like ordinary language authoring.

Put differently, too much of the current Rust interop still comes with unnecessary "bridge ceremony": adapter rituals authors perform only because imported Rust symbols are not yet first-class enough, even when the compiler already has enough information to model the relationship.

Pain points today:

- imported Rust functions are much easier to use than imported Rust types with rich method surfaces
- imported Rust types often need handwritten Rust aliases or shims before they become pleasant to use from Incan
- stdlib and user libraries often end up writing a parallel adapter layer in Rust just to expose constructors, methods, associated helpers, or capability bounds in an Incan-shaped API
- wrapper types become mandatory even when the user only wants to use a Rust type directly

This is exactly the awkward boundary that showed up during the RFC 023 stdlib migration. The public `std.async` surface now lives in `.incn`, but several modules still depend on Rust adapter files purely because Rust items are not yet first-class enough inside Incan's own authoring model.

### The real problem is not the runtime substrate

This RFC is not about replacing Tokio, replacing Rust, or pretending that async runtimes, operating system access, or framework integrations are language-internal.

The real problem is narrower:

- the runtime substrate can remain Rust
- the dependency boundary can remain explicit through `rust::`
- but library authors should not have to keep falling back to handwritten Rust just to make imported Rust APIs usable in Incan

### Rust and Incan both lower into the same backend world

After lowering, imported Rust items and Incan-authored items both live in the same generated Rust program. That means the language should exploit that fact where it can:

- imported Rust types should be usable as types
- imported Rust methods should be reachable through ordinary member lookup
- imported Rust constructors and associated items should be reachable through ordinary associated-item lookup
- Rust-facing capability bounds should be expressible in Incan syntax and lowered to Rust predicates

The goal is not to erase the fact that something came from Rust. The goal is to stop making users manually reconstruct that fact in boilerplate the compiler could track itself.

## Goals

- Retain the explicit `rust::` prefix as the dependency-resolution boundary for Rust imports.
- Make imported Rust types, functions, constants, and modules first-class symbols in Incan.
- Make imported Rust methods and associated items reachable through ordinary Incan lookup and rebinding rules.
- Make wrappers optional for API design, not mandatory for interop.
- Introduce a compiler-managed interop coercion matrix for built-in Incan types at explicit Rust boundaries.
- Introduce `rusttype` for direct Rust-backed interop roots and let those declarations carry only the extra compiler-relevant interop edges that are not implied by the direct backing relation.
- Allow Incan-authored generic bounds to lower to Rust constraints such as `Send`, `'static`, and callable traits.
- Keep async semantics Incan-owned while still allowing Rust runtimes and Rust futures to plug into that model.

## Non-Goals

- Removing the `rust::` prefix. This RFC keeps `rust::` as the explicit Rust dependency and import boundary.
- Making Rust dependencies implicit or guessing whether an import is Rust-backed.
- Replacing RFC 023's `rust.module()` mechanism. `rust.module()` remains valuable for Incan-authored module surfaces backed by Rust implementations.
- Replacing RFC 026's wrapper/newtype trait preservation story. This RFC makes wrappers optional; RFC 026 remains the place to preserve trait behavior when wrappers are chosen.
- Introducing arbitrary semantic conversions or unlimited implicit coercions between Incan values and foreign Rust types.
- Requiring authors to restate Rust backing information already implied by `type X = rusttype Y`.
- Defining a separate out-of-band registry for non-builtin interop metadata instead of attaching it to the Incan type declaration.
- Defining Incan's async semantics. Async remains core language territory; RFC 039 and related work define that model.
- Promising that every Rust language feature or crate API will become directly expressible on day one.

## Guide-level explanation (how users think about it)

The guide-level sections below follow the same reading order as the core model above: first-class imports, ordinary Rust member use, built-in boundary adaptation, `rusttype` interop roots, and capability bounds.

### Ordinary use: import a Rust type and use it directly

The simplest rule is the most important one: if you import a Rust item, it should behave like the corresponding kind of Incan symbol.

```incan
from rust::regex import Regex

pattern = Regex.new("^user_[0-9]+$")
ok = pattern.is_match("user_42")
```

No shim file should be required just to make `Regex.new(...)` or `pattern.is_match(...)` visible in Incan.

### Optional wrapper: curate docs and naming when you want to

Wrappers still matter, but they should be optional:

```incan
from rust::regex import Regex as RustRegex

pub type Regex = rusttype RustRegex:
    """
    A docs-rich Incan wrapper around `rust::regex::Regex`.
    """

    matches = is_match
```

This wrapper exists because the author wants:

- custom docs
- an Incan-first name
- possibly extra convenience methods

It should not exist because the interop model forces it.

### Method rebinding should feel like ordinary Incan

If a wrapper wants to expose a Rust method under a different name or keep an internal alias, it should use ordinary Incan syntax:

```incan
from rust::tokio::sync::mpsc import Sender as RustSender

pub type Sender[T] = rusttype RustSender[T]:
    __host_send = RustSender.send
    send_now = try_send
```

The compiler should understand that both aliases refer to Rust members. Users should not need a special interop-only decorator or method-binding mini-language for common cases.

### A `rusttype` is also a Rust-backed API surface

`rusttype` is not only a boundary declaration. It also says that the declared Incan type exposes the backing Rust API surface through ordinary Incan lookup:

```incan
from rust::regex import Regex as RustRegex

type Regex = rusttype RustRegex

pattern = Regex.new("^user_[0-9]+$")
ok = pattern.is_match("user_42")
```

In other words, `Regex.new(...)` and `pattern.is_match(...)` work because `Regex` is a Rust-backed API surface, not because the author manually rebound those members one by one. Rebinding remains available when the author wants to curate or rename that surface, but ordinary access should work by default.

### Capability bounds should look like Incan, not raw Rust

Library authors must be able to express Rust-lowered constraints in Incan source:

```incan
from std.rust import Send, Static, FnOnce

pub def spawn_blocking[T with Send + Static, F with FnOnce[T] + Send + Static](task: F) -> T:
    ...
```

The user writes Incan-facing capability bounds. The compiler lowers them to the appropriate Rust predicates.

### Built-in values cross a compiler-owned coercion matrix at Rust boundaries

Built-in Incan types should also cross explicit Rust boundaries without pushing conversion ceremony into user code:

```incan
from rust::my_crate import takes_f32

takes_f32(1.0)
```

If the Rust boundary expects a target type such as `f32`, the compiler may insert the appropriate host-side coercion from the built-in type's canonical Rust lowering. The user does not write `.into()` or `as` casts in Incan code for these compiler-managed interop coercions.

The important design claim is that this is not an ad hoc bag of call-site exceptions. Each built-in type owns a compiler-managed interop coercion matrix consisting of:

- its canonical Rust lowering
- the Rust boundary target types the language admits for that built-in
- a per-target policy such as exact, lossless, sanctioned lossy, or reject

The compiler consults that matrix only when crossing an explicit Rust boundary. The initial matrix for this RFC is:

<!-- markdownlint-disable MD060 -->
| Incan built-in  | Canonical Rust lowering | Admitted Rust boundary targets | Initial policy                            |
| --------------- | ----------------------- | ------------------------------ | ----------------------------------------- |
| `int`           | `i64`                   | `i64`                          | exact only                                |
| `float`         | `f64`                   | `f64`, `f32`                   | exact to `f64`; sanctioned lossy to `f32` |
| `bool`          | `bool`                  | `bool`                         | exact only                                |
| `str`           | `String`                | `String`, `&str`               | exact to `String`; borrow to `&str`       |
| `bytes`         | `Vec<u8>`               | `Vec<u8>`, `&[u8]`             | exact to `Vec<u8>`; borrow to `&[u8]`     |
| `None` / unit   | `()`                    | `()`                           | exact only                                |
<!-- markdownlint-restore -->

Structural built-ins follow the same idea recursively:

<!-- markdownlint-disable MD060 -->
| Incan built-in           | Canonical Rust lowering                     | Admitted Rust boundary targets                               | Initial policy                                         |
| ------------------------ | ------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------ |
| `Option[T]`              | `Option[T_rust]`                            | `Option[U]` when `T -> U` is admitted                        | recursive slot-wise adaptation only                    |
| `Result[T, E]`           | `Result[T_rust, E_rust]`                    | `Result[U, F]` when `T -> U` and `E -> F` are admitted       | recursive slot-wise adaptation only                    |
| `Tuple[A, B, ...]`       | `(A_rust, B_rust, ...)`                     | same-arity tuple when each position's adaptation is admitted | recursive positional adaptation only                   |
| `List[T]`                | `Vec[T_rust]`                               | `Vec[U]` when `T -> U` is admitted                           | recursive element-wise adaptation only                 |
| `Dict[K, V]`             | `std::collections::HashMap<K_rust, V_rust>` | `HashMap[K2, V2]` when key/value adaptations are admitted    | recursive key/value adaptation only                    |
| `Set[T]`                 | `std::collections::HashSet<T_rust>`         | `HashSet[U]` when `T -> U` is admitted                       | recursive element-wise adaptation only                 |
| `FrozenList[T]`          | `Vec[T_rust]`                               | `Vec[U]` when `T -> U` is admitted                           | same as `List[T]`; immutable at the Incan API level    |
| `FrozenDict[K, V]`       | `std::collections::HashMap<K_rust, V_rust>` | `HashMap[K2, V2]` when key/value adaptations are admitted    | same as `Dict[K, V]`; immutable at the Incan API level |
| `FrozenSet[T]`           | `std::collections::HashSet<T_rust>`         | `HashSet[U]` when `T -> U` is admitted                       | same as `Set[T]`; immutable at the Incan API level     |
<!-- markdownlint-restore -->

Notes:

- `int` is intentionally conservative in the initial matrix; other integer widths should be written explicitly via RFC 009-sized types rather than reached through implicit `int` coercion
- const-only `FrozenStr` and `FrozenBytes` follow the same Rust-boundary matrix entries as `str` and `bytes`
- structural built-ins recurse through the matrices of their component types, but do not implicitly change container kind in the initial matrix
- this table is intentionally conservative: built-ins do not implicitly adapt to semantic host types just because some Rust API happens to accept them
- future sized numeric types from RFC 009 are exact-lowering types in their own right; this RFC does not need `int -> i32` or `int -> u16` style implicit coercions to make those usable

That means the matrix is bounded interop, not arbitrary semantic conversion:

```incan
from rust::my_crate import takes_f32, takes_duration

takes_f32(1.0)      # allowed when `float -> f32` is admitted by the built-in matrix
takes_duration(1.0) # not a built-in coercion; needs an explicit adapter or `rusttype`
```

The compiler may adapt built-in values to admitted Rust boundary targets in ways the language defines as meaningful; it should not silently guess conversions to unrelated domain types.

### `rusttype` marks the Rust boundary once

For host-backed non-builtin types, `rusttype` is the direct Rust-backed declaration form:

```incan
from rust::std::collections import HashMap as RustHashMap

type Counter[T] = rusttype RustHashMap[T, usize]:
    def total(self) -> int:
        ...
```

The compiler should infer the exact wrap/unwrap relation from that declaration alone. Authors should not need to restate the same canonical backing in a separate `interop` block just to make exact Rust-boundary adaptation work.

An `interop` block exists only for extra edges that are not already implied by the `rusttype` itself:

```incan
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
    def parse(raw: str) -> Email:
        ...

    interop:
        from str try Email.parse

type WorkEmail = newtype Email
type PersonalEmail = newtype Email
```

Here, `Email` is the interop root because it is a `rusttype`: it directly wraps the Rust type. The fallible `str -> Email` rule is declared once on that root, and the qualified form makes the adapter source explicit: the compiler is using `Email.parse`. The important mental model is that `interop:` points at an ordinary callable on the `Email` type surface; it does not create that callable. A short form such as `from str try parse` is equivalent when the name resolves unambiguously on the `Email` surface. Domain wrappers above it use ordinary `newtype` syntax, inherit the representation chain, and can rely on the same root-defined interop behavior when the expected target type makes the path unambiguous.

### Async stays core Incan

Even when using Rust-backed runtimes or futures, Incan still owns async semantics:

```incan
from rust::tokio::task import yield_now

async def pause() -> None:
    await yield_now()
```

The point is not that Tokio defines async for Incan. The point is that imported Rust async items should plug into Incan's async model cleanly once that model is fully specified. Type coercion at Rust boundaries is a separate concern handled by the compiler-managed interop coercion rules for built-in types.

## Reference-level explanation (precise rules)

### Import syntax

This RFC does not change the syntactic shape introduced by RFC 005:

```incan
import rust::CRATE[::PATH...] [as ALIAS]
from rust::CRATE[::PATH...] import ITEM[, ITEM2 ...]
```

`rust::` remains mandatory for Rust imports.

### Symbol classification

When a `rust::...` import resolves successfully, the imported item is recorded by the compiler as a Rust-origin symbol with:

- its canonical Rust path
- its item kind
- its type-level metadata
- its value-level metadata
- its associated items and methods, when applicable

The compiler must classify imported Rust items into language-relevant symbol categories such as:

- type
- function
- constant
- module

This classification is compiler-managed provenance, not user-authored scaffolding.

### First-class item behavior

Imported Rust items behave like their corresponding Incan-level kinds:

- an imported Rust type is valid in type positions
- an imported Rust function is valid in call/value positions
- an imported Rust constant is valid in value/constant positions
- an imported Rust module participates in module/member resolution

The important rule is that the origin of a symbol being Rust must not make it second-class after import resolution succeeds.

### Member and associated-item resolution

For imported Rust types, the compiler must support member and associated-item resolution using Rust metadata.

This means:

- `TypeName.associated_item(...)` resolves against Rust associated items
- `value.method(...)` resolves against Rust methods
- wrappers and `rusttype`s may rebind imported Rust members through ordinary Incan aliasing rules

Inside a `rusttype` body over a Rust type, the design target is that imported Rust members are available for rebinding through normal syntax:

```incan
from rust::tokio::sync::mpsc import Sender as RustSender

pub type Sender[T] = rusttype RustSender[T]:
    send_now = try_send
    __host_send = RustSender.send
```

The exact scoping rule is specified as follows:

- if a `rusttype` body is built over an imported Rust type, members visible on the backing type are in scope for alias declarations
- fully qualified rebinding through `BackingType.member` is always valid
- short-form rebinding through `member` is valid when the name resolves unambiguously to a member of the backing type

### Optional wrappers and newtypes

Wrapping a Rust type in an Incan type is always optional unless the author wants one of these:

- a curated public API
- custom docs
- convenience helpers
- constrained visibility
- additional invariants
- trait/delegation behavior via RFC 026

The compiler must not require a wrapper merely to make an imported Rust type usable.

When an author does choose an Incan type over a Rust item, the direct Rust-backed declaration form is `rusttype`. Ordinary `newtype` remains available for wrappers over existing Incan types, including wrappers over a `rusttype`. Only when the author wants the compiler to understand more than "this directly wraps that Rust backing type" should an `interop` declaration surface come into play, and that surface belongs on the `rusttype` declaration itself. In other words, the authoritative place to declare non-builtin host interop metadata is the Incan interop-root declaration, not a side registry and not Rust-only glue.

### Compiler-managed interop coercions for built-in types

First-class Rust symbol resolution is only half of the interop story. Built-in Incan values must also be able to cross explicit Rust boundaries in a principled way.

For every built-in Incan type, the compiler maintains:

- a canonical Rust lowering
- a set of admitted Rust boundary target types
- a per-target coercion policy

This coercion matrix is compiler-owned and conceptually lives with the built-in type definitions themselves rather than as scattered call-site heuristics. These are compiler-managed lowering rules over admitted Rust boundary targets, and not user-callable methods.

The initial built-in matrix for this RFC is:

| Incan built-in  | Canonical Rust lowering | Admitted Rust boundary targets | Initial policy                            |
| --------------- | ----------------------- | ------------------------------ | ----------------------------------------- |
| `int`           | `i64`                   | `i64`                          | exact only                                |
| `float`         | `f64`                   | `f64`, `f32`                   | exact to `f64`; sanctioned lossy to `f32` |
| `bool`          | `bool`                  | `bool`                         | exact only                                |
| `str`           | `String`                | `String`, `&str`               | exact to `String`; borrow to `&str`       |
| `bytes`         | `Vec<u8>`               | `Vec<u8>`, `&[u8]`             | exact to `Vec<u8>`; borrow to `&[u8]`     |
| `None` / unit   | `()`                    | `()`                           | exact only                                |

Structural built-ins follow recursive structural rules rather than one row per concrete instantiation:

| Incan built-in     | Canonical Rust lowering                      | Admitted Rust boundary targets                               | Initial policy                                         |
| ------------------ | -------------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------ |
| `Option[T]`        | `Option<T_rust>`                             | `Option<U>` when `T -> U` is admitted                        | recursive slot-wise adaptation only                    |
| `Result[T, E]`     | `Result<T_rust, E_rust>`                     | `Result<U, F>` when `T -> U` and `E -> F` are admitted       | recursive slot-wise adaptation only                    |
| `Tuple[A, B, ...]` | `(A_rust, B_rust, ...)`                      | same-arity tuple when each position's adaptation is admitted | recursive positional adaptation only                   |
| `List[T]`          | `Vec<T_rust>`                                | `Vec<U>` when `T -> U` is admitted                           | recursive element-wise adaptation only                 |
| `Dict[K, V]`       | `std::collections::HashMap<K_rust, V_rust>`  | `HashMap<K2, V2>` when key/value adaptations are admitted    | recursive key/value adaptation only                    |
| `Set[T]`           | `std::collections::HashSet<T_rust>`          | `HashSet<U>` when `T -> U` is admitted                       | recursive element-wise adaptation only                 |
| `FrozenList[T]`    | `Vec<T_rust>`                                | `Vec<U>` when `T -> U` is admitted                           | same as `List[T]`; immutable at the Incan API level    |
| `FrozenDict[K, V]` | `std::collections::HashMap<K_rust, V_rust>`  | `HashMap<K2, V2>` when key/value adaptations are admitted    | same as `Dict[K, V]`; immutable at the Incan API level |
| `FrozenSet[T]`     | `std::collections::HashSet<T_rust>`          | `HashSet<U>` when `T -> U` is admitted                       | same as `Set[T]`; immutable at the Incan API level     |

`int` is intentionally conservative in the initial matrix; other integer widths should be written explicitly via RFC 009-sized types rather than reached through implicit `int` coercion. Const-only `FrozenStr` and `FrozenBytes` follow the same Rust-boundary entries as `str` and `bytes`. Structural built-ins recurse through the matrices of their component types, but do not implicitly change container kind in the initial matrix. Future sized numeric types from RFC 009 are exact-lowering types in their own right; this RFC therefore does not rely on implicit `int -> i32` or `int -> u16` style coercions in the initial matrix.

The rules are:

- exact canonical lowering match wins first
- if there is no exact match, the compiler may apply a compiler-known lossless coercion for that built-in type
- if no lossless coercion exists, the compiler may apply a compiler-sanctioned lossy coercion for that built-in type only when the target pair is explicitly admitted by the language
- if the Rust target type is not one of the compiler's known admitted targets, the compiler may attempt a single-step fallback conversion from the canonical lowered Rust type only when that fallback is compiler-approved and unambiguous
- coercions do not chain arbitrarily
- fallible conversions are not inserted implicitly
- semantic conversions to unrelated domain types are out of scope

Examples of the distinction:

- adapting an Incan `float` to an admitted Rust float width is in scope when the coercion rule is part of the built-in type's matrix
- adapting an Incan `str` to a Rust string-facing boundary type may be in scope when the compiler already owns that interop rule
- adapting an Incan numeric value to a semantic host type such as `Duration` is not a built-in implicit coercion; it requires an explicit `rusttype` interop root or adapter API

Diagnostics must reflect which coercion step failed and what the user can do next. When a built-in value cannot be adapted to the requested Rust boundary type, the compiler should suggest one of:

- use a different Rust API overload or boundary type
- introduce an explicit adapter/helper
- wrap the host type in an Incan-facing abstraction

### Interop metadata on `rusttype` definitions

Built-in types are not the only kinds of types that need compiler-understood interop behavior. Stdlib and user-authored host-backed Incan types, such as the `std.collections` shapes described in RFC 030, may also need to describe how they map onto Rust.

The key design rule is:

- for built-ins, interop metadata is compiler-intrinsic
- for non-built-in host-backed types, direct Rust-backed interop roots use `type ... = rusttype ...`, and extra interop metadata is declared there

This keeps the interop story Incan-authored even when the lowering target is Rust. It also lets stdlib and third-party library authors participate in the same system rather than forcing them back into Rust-only bridge code.

Normative rules:

- `type X = rusttype Y` declares the direct representation relation between `X` and the Rust type `Y`
- exact Rust-boundary wrap/unwrap behavior implied by that declaration must not require restating `Y` in an `interop` block
- extra boundary edges such as parsing, serialization, or other declared adapters belong on that `rusttype` declaration
- `type Z = newtype X` wraps an existing Incan type and does not itself become a new Rust interop root
- `newtype` wrappers over a `rusttype` inherit the representation chain and may rely on root-defined edges when the expected target type makes the path unambiguous
- when the target is a raw Rust type, adaptation should resolve through the nearest matching `rusttype` root rather than searching arbitrary wrapper chains

### Rust-backed API surface on `rusttype`

`type X = rusttype Y` has two effects at once:

1. It establishes `X` as the interop root for the Rust backing `Y`.
2. It establishes `X` as an Incan-visible API surface over the Rust members and associated items of `Y`.

Normative rules:

- `X.method(...)` and `value.method(...)` resolve against members of the backing Rust type using ordinary lookup rules
- Rust associated items visible on `Y` are available through `X.associated_item(...)`
- aliases declared in the `rusttype` body may rename or curate that Rust-backed API surface
- `interop:` does not define ordinary methods; it defines boundary adaptation edges

This distinction matters: the Rust-backed API surface answers "what members can I call on this type?", while `interop:` answers "how may values cross into or out of this type at Rust boundaries?"

### Full `interop:` specification

The `interop:` block is a required keyword when declaring non-obvious boundary edges on a `rusttype`. Its syntax is:

```incan
type Name[Params...] = rusttype RustBacking[Params...]:
    interop:
        from SourceType via adapter_ref
        from SourceType try adapter_ref
        into TargetType via adapter_ref
        into TargetType try adapter_ref
```

Normative syntax rules:

- `interop:` may appear at most once on a `rusttype` declaration
- each line inside the block declares exactly one directed adaptation edge
- `from S ...` declares an edge from Incan type `S` into the declaring `rusttype`
- `into T ...` declares an edge from the declaring `rusttype` into Incan type `T`
- `SourceType` and `TargetType` use ordinary Incan type-expression syntax
- union types from RFC 029 are therefore valid in `SourceType` and `TargetType`
- `via ref` declares an infallible adapter
- `try ref` declares a fallible adapter
- `adapter_ref` may be written either as a short-form name such as `parse` or as a qualified callable reference such as `Email.parse`
- `interop:` is only valid on `rusttype` declarations, not on ordinary `newtype` wrappers
- `interop:` references an existing callable; it does not itself declare a new method or function

Normative semantic rules:

- the direct backing relation implied by `type X = rusttype Y` is not spelled inside `interop:`
- adapter references resolve against the declaring `rusttype`'s API surface
- short-form `parse` inside `type Email = rusttype RustEmailAddress` is read as `Email.parse`
- fully qualified callable references such as `Email.parse` are always valid
- if a short-form adapter name is ambiguous, the compiler must reject it and require a qualified reference
- adapter reference lookup is ordinary callable lookup on the declaring `rusttype` surface; this RFC does not introduce a separate overload-resolution system for adapters
- `from S via f` means the compiler may adapt an `S` into the declaring `rusttype` by calling `f`
- `from S try f` means the same adaptation is permitted, but may fail and therefore carries the failure behavior of that adapter
- `into T via f` means the compiler may adapt the declaring `rusttype` into `T` by calling `f`
- `into T try f` means the same adaptation is permitted, but may fail
- semantically, `from S ...` reads as "use this callable to build the declaring `rusttype` from `S`", while `into T ...` reads as "use this callable to project the declaring `rusttype` into `T`"
- `from int | float try f` declares one union-typed edge whose admitted source domain is `int | float`; it is not sugar for two separate edges
- likewise, `into A | B via f` declares one edge whose target is the union `A | B`
- multiple declared edges must not be chained together arbitrarily
- at most one declared interop edge may participate in a single adaptation path, optionally alongside the implied wrap/unwrap steps of the `rusttype` chain
- if multiple adapter paths are valid and the expected target type does not disambiguate them, the compiler must reject the adaptation as ambiguous

Conceptually, a host-backed `rusttype` should be able to declare compiler-relevant information such as:

- admitted Rust boundary targets
- coercion hooks or policies
- fallible adapter edges such as parse/serialize/deserialize
- compiler-relevant structural capabilities

Illustrative examples:

```incan
type Email = rusttype RustEmailAddress:
    def parse(raw: str) -> Email:
        ...

    interop:
        from str try Email.parse

type WorkEmail = newtype Email
type PersonalEmail = newtype Email
```

This example intentionally does not restate a `rust canonical = RustEmailAddress` (or something similar) line, because the `rusttype` declaration already says that. The `interop` block only adds the non-obvious `str -> Email` edge, and the qualified form makes it explicit that the compiler is using the already-declared `Email.parse` callable.

Additional illustrative direction:

```incan
type Json[T] = rusttype RustJsonValue:
    interop:
        from T try Json.serialize
        into T try Json.deserialize
```

```incan
type Duration = rusttype RustDuration:
    interop:
        from int via milliseconds
        from float via seconds
```

```incan
type SomeExample = rusttype SomeRepresentativeRustType:
    def from_number(value: int | float) -> SomeExample:
        ...

    interop:
        from int | float try SomeExample.from_number
```

Read these examples as references to callables on the declaring `rusttype` surface: `from T try Json.serialize` means "adapt a `T` into `Json[T]` using `Json.serialize`", while `into T try Json.deserialize` means "adapt a `Json[T]` into `T` using `Json.deserialize`". Likewise, `from int | float try SomeExample.from_number` means "adapt any value assignable to `int | float` into `SomeExample` using one union-typed adapter edge". In all of these examples, the attachment point and ownership model are normative: this metadata belongs to the Incan-side `rusttype` declaration, not to a disconnected compiler registry.

This is orthogonal to RFC 025. RFC 025 governs compile-time dispatch among same-name trait methods; `interop:` adapter refs are just ordinary callable references on the `rusttype` surface. In other words, `interop:` does not introduce general overloading. It simply points at a callable that the language already knows how to resolve.

This also composes directly with RFC 029. Union types describe the shape of the adapter's accepted input or produced output; `interop:` still describes boundary conversion. A union-typed adapter edge is therefore one conversion rule over a union-shaped type, not an overloaded family of separate rules.

This is especially important for types like RFC 030's `Deque[T]` and `Counter[T]`. They are not compiler built-ins, but they are also not "just some Rust type." They are Incan types with public Incan APIs, docs, and semantics, and the compiler should be able to understand their lowering and boundary behavior from Incan-authored declarations.

### Rust-lowered capability bounds

This RFC introduces Rust-lowered capability bounds in Incan syntax.

These are Incan-facing bound names that lower to Rust predicates at code generation time. Obvious examples include:

- `Send`
- `Sync`
- `Static`
- `Fn[T]`
- `FnMut[T]`
- `FnOnce[T]`

The initial supported set must be large enough to make real Rust-backed library authoring viable in pure Incan. The capability markers listed above are the obvious starting set, and the semantics below are normative:

- these bounds are written in Incan source
- they participate in generic `with` clauses
- they are checked and carried through the frontend as semantic constraints
- lowering emits the corresponding Rust predicates

These are not ordinary runtime traits in the same sense as user-authored Incan traits. They are compiler-recognized capability bounds whose purpose is to express Rust backend requirements in an Incan-shaped contract.

### Async interaction

This RFC does not define async semantics.

However, it requires that imported Rust async items be able to participate in Incan's async model once that model is specified. In practice this means:

- imported Rust futures or future-producing functions must be mappable into Incan's awaitability rules
- wrapper authors must not need handwritten Rust shims merely to make ordinary Rust async APIs available to Incan async code

RFC 039 remains the place to define Incan's awaitable semantics and composition model.

### Dependency resolution

`rust::` imports remain tied to Rust dependency declarations as specified by RFC 013 and RFC 005.

Normative rules:

- `rust::std::...` refers to Rust's standard library and does not create a Cargo dependency
- any non-`std` crate root in a `rust::...` import must correspond to an allowed Rust dependency declaration path
- unresolved Rust crate roots must produce a clear dependency-resolution diagnostic

This is the main reason the `rust::` prefix remains desirable: it keeps the dependency boundary explicit and auditable.

### Diagnostics

This RFC requires improved diagnostics for Rust-origin symbols. At minimum, the compiler should produce clear errors for:

- unknown Rust crates
- unknown Rust items in a resolved crate/module
- unknown Rust members or associated items
- unsupported Rust constructs not yet representable in Incan
- unsupported or ambiguous interop coercions at Rust boundaries
- missing or invalid interop metadata on a host-backed `rusttype` definition
- ambiguous wrapper-mediated adaptation paths that do not identify a unique interop root
- misuse of Rust-lowered capability bounds
- ambiguous short-form member rebinding inside wrappers

Diagnostics should make it clear when a symbol came from `rust::...` resolution and, where useful, include the canonical Rust path that failed.

## Design details

### Syntax

This RFC deliberately keeps syntax changes narrow:

- `rust::...` import syntax stays as defined by RFC 005
- `rusttype` is introduced as the direct Rust-backed declaration form
- `newtype` remains the ordinary wrapper syntax for Incan-to-Incan wrapping
- `rusttype` declarations may grow an optional `interop:` block for non-obvious boundary edges
- rebinding should use ordinary aliasing syntax
- capability bounds use Incan `with` clauses

The goal is to extend meaning, resolution, and lowering while making the interop root explicit in syntax, not to introduce a sprawling interop-specific mini-language.

### Semantics

The semantic center of this RFC is:

1. `rust::...` remains the explicit Rust import boundary.
2. After import resolution succeeds, imported Rust items become first-class compiler symbols.
3. Compiler-managed provenance tracks how those symbols lower to Rust.
4. Member and associated-item lookup can resolve against Rust-origin metadata.
5. Wrappers are optional and only exist when the author wants a better API surface.
6. Built-in types own compiler-managed interop coercion matrices for explicit Rust boundaries.
7. `type X = rusttype Y` marks a direct Rust-backed interop root and already implies the exact backing relation.
8. `newtype` over a `rusttype` remains an ordinary Incan wrapper that inherits the representation chain.
9. Optional interop metadata on `rusttype` declarations declares only extra edges beyond that implied exact relation.
10. Capability bounds let Incan express Rust backend requirements without exposing raw Rust syntax.

### Interaction with existing features

#### async/await

This RFC does not define async semantics. It only requires that Rust-origin async APIs become consumable within whatever async model Incan defines. RFC 039 remains the owner of Incan's awaitability semantics.

#### traits/derives

Imported Rust items becoming first-class does not eliminate the need for wrapper trait preservation. If an Incan `rusttype` or `newtype` wrapper must preserve or expose Rust trait behavior, RFC 026 remains relevant.

#### imports/modules

This RFC keeps `rust::` as the only Rust import prefix. It extends what imported items can do after resolution rather than changing how the dependency boundary is spelled.

#### error handling

This RFC improves the ergonomics of wrapping and reusing Rust error/result surfaces, but it does not mandate automatic conversion of arbitrary Rust error types into Incan model types. Library authors may still wrap errors intentionally when they want a curated API.

#### Rust interop

This RFC is an extension of RFC 005, not a replacement. RFC 005 established explicit imports and core type mapping. RFC 041 makes the imported symbols themselves first-class enough to support native-feeling library authoring.

### Compatibility / migration

This RFC is designed to be source-compatible where possible.

Existing code that uses:

- `rust::...` imports
- handwritten Rust shims
- explicit wrapper/newtype layers

continues to work.

What changes is that many of those shims and mandatory wrappers can gradually become unnecessary. Migration is opt-in:

- keep current shims if they still provide value
- remove them when direct Rust imports plus wrapper rebinding become sufficient
- temporarily accept direct Rust-backed `newtype` declarations as a compatibility alias while emitting a migration diagnostic toward `rusttype`

## Alternatives considered

### Keep mandatory Rust shim layers

This is the status quo. It is workable, but it makes interop authoring feel heavier than it should and forces library authors to duplicate compiler-manageable information in Rust glue.

### Drop the `rust::` prefix and use plain imports

This was rejected for this RFC. The explicit `rust::` prefix remains useful for dependency resolution, `incan.toml` declaration clarity, diagnostics, and keeping the Rust dependency boundary explicit.

### Introduce an interop-specific decorator or binding language

This was rejected as the default direction. Ordinary use should rely on ordinary imports, lookup, and aliasing. Special interop-only syntax should be reserved for cases that genuinely cannot be expressed through the normal language model.

### Reuse `newtype` for direct Rust-backed interop roots

This would work semantically, but it would hide an important distinction in the syntax: wrapping a Rust type directly is not the same authoring move as wrapping an existing Incan type. `rusttype` makes the interop root explicit, keeps `newtype` focused on ordinary wrappers, and gives the compiler and user a clearer shared model.

### Always wrap imported Rust types in hidden compiler-generated wrappers

This would reduce some visible boilerplate, but it would keep the mental model indirect and would make interop harder to reason about. The better model is direct first-class imports, with explicit wrappers only when chosen.

## Drawbacks

- The compiler becomes significantly more sophisticated in how it models imported Rust items.
- The compiler must own and maintain a coherent coercion matrix for built-in Incan types.
- Rust metadata loading and caching become central implementation concerns.
- Diagnostics must explain not only Incan semantics but also how Rust-origin items were resolved.
- The boundary between language semantics and backend semantics becomes more subtle, especially around capability bounds.
- Some Rust constructs will still remain out of scope, which means the compiler must be explicit about what “first-class” does and does not cover initially.

## Implementation architecture (non-normative)

This RFC intentionally specifies the language model more strongly than any particular internal module layout, but the implementation should still keep interop policy centralized and predictable. The core risk in a feature like this is not only complexity; it is drift, where the typechecker, lowering, emitter, runtime helpers, and tooling each grow their own partial idea of what an interop adaptation means.

The recommended shape is:

- shared pure interop policy lives in `incan_core`
- parser, typechecker, and adaptation planning live in compiler crates
- runtime-only helper glue lives in `incan_stdlib`

In practice, that means builtin coercion matrices, adaptation recipe kinds, canonical names, and other pure interop metadata should sit beside the rest of the language's shared semantic policy rather than being redefined ad hoc in frontend and backend code. The compiler should then resolve `rusttype` declarations, validate `interop:` edges, and compute a single adaptation plan for a given Rust boundary crossing. Lowering and emission should consume that plan rather than each re-deciding conversion rules locally.

This is also why a separate dedicated interop crate is not the initial recommendation. `incan_core` already exists to hold pure shared semantics and registries, while `incan_stdlib` exists to hold runtime glue for generated programs. Adding another crate too early would likely scatter the model rather than simplify it. A separate crate only becomes attractive later if the interop machinery genuinely outgrows `incan_core` or needs to be consumed independently by tooling beyond the compiler/runtime split described in the repository's layering docs.

## Layers affected

- **Parser / AST**: new `rusttype` declaration form with optional `interop:` block; short-form and qualified member rebinding syntax inside `rusttype` bodies.
- **Typechecker / Symbol resolution**: Rust-origin provenance on imported items; first-class classification of imported Rust types, functions, constants, and modules; member and associated-item resolution against Rust metadata; built-in interop coercion matrix validation; `interop:` edge validation and adapter reference resolution; Rust-lowered capability bounds in `with` clauses.
- **IR Lowering**: direct lowering of Rust-origin symbols without shim intermediaries; coercion insertion at explicit Rust boundaries from the compiler-owned built-in matrix; lowering of `rusttype` wrap/unwrap steps from declared interop roots; emission of Rust predicates from capability bound annotations.
- **Emission**: Rust member calls, associated-item calls, and interop coercion output driven by frontend-carried provenance rather than call-site heuristics.
- **Stdlib / Runtime (`incan_stdlib`)**: migration of `std.async` and similar modules away from handwritten Rust adapter layers where those layers existed solely for symbol exposure.
- **Formatter**: stable formatting for `rusttype` declarations, `interop:` blocks, and wrapper rebinding syntax.
- **LSP / Tooling**: completions and docs for imported Rust members and associated items; improved diagnostics that surface the canonical Rust path in error messages.

## Implementation Plan

### Phase 1: Parser + AST

- Add `rusttype` declaration syntax, optional `interop:` blocks, and rebinding forms described in this RFC.
- Extend the AST and formatter so the new surface round-trips stably.

### Phase 2: Typechecker + symbol resolution

- Model every resolved `rust::...` binding with explicit Rust provenance and import shape (crate root, rooted path, or `from`-imported leaf).
- Thread provenance through identifier and type resolution; replace opaque `Unknown` placeholders where a canonical Rust path is known.
- Resolve members and associated items against one shared Rust semantic model; emit span-precise diagnostics for unsupported shapes and for invalid uses (for example crate-root imports in type position).
- Validate the built-in interop coercion matrix, `interop:` edges, and Rust-lowered capability bounds in `with` clauses as the surfaces land.

### Phase 3: Lowering + emission

- Lower Rust-origin symbols using frontend-carried provenance instead of rediscovering paths at emit time.
- Insert coercions at explicit Rust boundaries per the compiler-owned matrix; lower `rusttype` wrap/unwrap and capability predicates.

### Phase 4: Stdlib + runtime

- Migrate modules such as `std.async` off Rust adapter layers that exist only to re-expose imported Rust items.

### Phase 5: LSP, tests, and docs

- Improve completions and diagnostics using the same provenance as the typechecker.
- Add parser, typechecker, snapshot, and integration tests per phase; update docs-site and release notes when behavior is user-visible.

## Implementation log

### Spec / design

- [x] Capture any normative edge cases discovered during implementation in **Design Decisions**.

### Parser / AST

- [x] Lex/parse `rusttype` and `interop:` blocks per RFC.
- [x] AST nodes with correct spans; formatter round-trip.
- [x] Parser negative tests for invalid interop syntax (missing `via`/`try`, missing `from`/`into`).
- [x] Minimal `rusttype` (no body) parses correctly.

### Typechecker

- [x] Rust import symbols carry `RustItem` provenance and binding kind (crate root / rooted path / from-import leaf).
- [x] Expression and type resolution use `ResolvedType::RustPath` where a canonical path is known (not `Unknown`).
- [x] Diagnostic: crate-root `import rust::cr` cannot be used as a type; hint `from rust::cr import ...`.
- [x] Member and associated-item resolution consume unified Rust metadata (inherent methods and associated functions resolved; permissive fallback for trait-provided methods pending full extraction).
- [x] Explicit diagnostics for unsupported Rust-backed shapes (enum variants, macros, etc. emit `rust_item_shape_not_supported`).
- [x] `std.rust` capability imports are recognized as trait symbols during stdlib import collection.
- [x] Rust-path missing-method and non-public-item diagnostics are wired for metadata-backed paths.
- [x] Rust free-function / associated-function calls validate arity when metadata reports zero parameters (extra arguments must error; regression covered by unit tests).
- [x] Method argument validation against `RustMethodSig.signature.params` with coercion tracking (mirrors function call validation).
- [x] Scalar and structural coercion matrix (`Option`, `Result`, `List`, `Dict`, `Set`, `Tuple`, frozen variants) wired through `rust_arg_boundary_match` via `incan_boundary_type_display` + `admitted_builtin_coercion`.
- [x] `interop:` edge validation: `via` infallible / `try` fallible semantics, adapter signature checks, duplicate detection, short-form ambiguity rejection, input type mismatch.
- [x] Rebinding resolution: `send_now = try_send` on `rusttype` resolves to backing method in typechecker.
- [x] `RustPath` field access permissive when metadata is non-Module (no false-positive `missing_field` for types, traits, etc.).

### Lowering / IR

- [x] Preserve provenance through IR for Rust-origin bindings (`ResolvedType::RustPath` lowered to path-bearing IR types).
- [x] Lower coercions at Rust boundaries: `InteropCoerce` IR node with `Builtin` / `AdapterCall` / `RustTypeUnwrap` kinds; inserted for both function calls and method calls via `wrap_with_rust_arg_coercion`.
- [x] `rusttype` type-alias lowering with `interop_edges` carried through IR.
- [x] Capability bounds folded correctly (`T with Send, Sync` -> single type param with `Send + Sync`; `Static` -> `'static`).
- [x] Rebinding lowering: `type_method_rebindings` registered from AST; `resolve_method_rebinding` substitutes alias -> target name at method call sites.
- [x] Interop edge lowering handles both `Into` and `From` directions via `lower_rusttype_interop_adapter`.

### Emission

- [x] Emit calls and coercions from resolved provenance: `InteropCoerce` emits `.to_string()`, `.to_vec()`, `as f32`, borrow `&`, identity per `CoercionPolicy`; adapter calls emit `adapter(inner)` for `via` and `adapter(inner)?` for `try`.
- [x] Capability bounds emit as Rust `where`-style predicates (`Send`, `Sync`, `'static`, `Fn<T>`, `FnMut<T>`, `FnOnce<T>`).

### Stdlib / runtime

- [x] Stdlib `.incn` surfaces use RFC 041 `std.rust` capability bounds (`Send`, `Static`) where applicable (`task.incn`, `time.incn`). Remaining Rust `.rs` adapter modules contain genuine trait implementations (`Future`, `From<JoinError>`, `FnOnce` blanket impls) that Incan cannot express; these stay as justified Rust runtime glue, not redundant shims.

### LSP / tooling

- [x] Document symbols include `Newtype` and `TypeAlias` declarations.
- [x] Hover shows underlying Rust path for `rusttype` symbols.
- [x] Go-to-definition handles `TypeAlias` and `Newtype`.
- [x] Canonical Rust paths surfaced in diagnostic messages for `RustPath`-origin symbols.

### Tests

- [x] Parser tests for `rusttype` / `rust::` imports (positive: minimal, rebinding+interop, rust imports; negative: missing via/try, missing from/into).
- [x] Typechecker tests for provenance, invalid crate-root-as-type, unsupported-shape diagnostics, interop edge validation (try/via semantics, adapter mismatch, duplicates, ambiguity), structural coercion, capability bounds (Send, Sync, Static, Fn, FnMut, FnOnce), rebinding resolution, permissive RustPath access.
- [x] Codegen snapshots for end-to-end Rust interop: capability bounds (`rfc041_std_rust_capability_bounds`, `rfc041_capability_bounds_full`), rusttype + interop (`rfc041_rusttype_interop`), rebinding (`rfc041_rusttype_rebinding`), scalar coercions (`rfc041_rust_coercions`), structural coercions (`rfc041_structural_coercion`), interop edges (`rfc041_interop_from_try`, `rfc041_interop_into_via`), field access, associated functions.
- [x] Integration tests: `test_rfc041_rusttype_interop_typechecks_end_to_end`, `test_rfc041_rusttype_with_methods_typechecks`, `test_rfc041_rust_coercion_codegen_smoke`, `test_rfc041_structural_coercion_codegen_smoke`.

### Docs

- [x] Docs-site `rust_interop.md` updated with `rusttype`, `interop:` blocks, capability bounds, and coercion model.
- [x] Release notes entry in `0_2.md` for RFC 041 surface.

## Design Decisions

- **Rust function call arity:** For bindings whose provenance is a Rust-origin `RustFunctionSig`, the typechecker must compare `args.len()` to `sig.params.len()` for every call, including when `sig.params` is empty. Excess arguments must produce the same arity diagnostic as builtins (`expects N argument(s), got M`) and must not be dropped because `zip` yields no pairs.

- **Rust metadata source:** The compiler must use one authoritative Rust semantic source for item resolution, member lookup, signatures, docs, and visibility. RFC 041 does not mandate whether that is implemented through rustc's own type engine, compiler queries, or another equivalent semantic integration, but the frontend, tooling, and lowering must consume the same Rust interpretation rather than inventing separate ones.
- **Initial built-in coercion scope:** RFC 041 deliberately keeps the initial built-in interop matrix conservative. After RFC 009 lands, exact-lowering sized numeric types should be the preferred path rather than automatically broadening `int` or `float` coercions, and any additional implicit boundary targets beyond `float -> f32` and borrow-based `str` / `bytes` forms should require an explicit follow-up RFC or amendment.
- **Migration spelling:** RFC 041 does not define a compatibility alias for direct Rust-backed declarations. `rusttype` is the direct Rust-backed declaration form, while `newtype` remains the wrapper form for existing Incan types, including wrappers over a `rusttype`.
- **Fallible interop edges:** `try` interop edges may participate only when the expected target type is already known and there is a single unambiguous adaptation path at an explicit boundary site such as argument passing, assignment or return coercion, or Rust-boundary lowering. The compiler must not speculate with fallible adapters during ordinary inference, and failures must surface through ordinary Incan fallible-expression semantics rather than hidden panics or implicit control flow.
- **Initial `std.rust` capability set:** The initial Rust-lowered capability set must be sufficient for real Rust-backed library authoring in pure Incan, especially async, callable, and concurrency-facing surfaces. `Send`, `Sync`, `Static`, `Fn[T]`, `FnMut[T]`, and `FnOnce[T]` are the obvious starting set, but RFC 041 is not complete if additional capability bounds are still required to make first-class Rust interop authoring viable.
- **Short-form rebinding:** Short-form rebinding such as `send_now = try_send` should be allowed only when the name resolves unambiguously against one backing type surface. When the lookup is ambiguous, authors must use the qualified form.
- **Permissive field/method access on incomplete metadata:** When `rust-inspect` provides type information but the specific field or method is not in the extracted inherent method surface (e.g. trait-provided methods, constants, associated types), the typechecker must return `Unknown` rather than emitting a false-positive `missing_field` or `missing_method` diagnostic. Only module-child membership from rust-analyzer is authoritative enough to reject unknown names.
- **Structural coercion boundary display:** Structural Incan types are serialized to boundary display strings (e.g. `Option[int]`, `List[str]`, `Dict[str, int]`) for coercion matrix lookup. The display format must match the `incan_core` parser's expectations; frozen variants use their canonical names (`FrozenList`, `FrozenDict`, `FrozenSet`).
- **Method argument coercion parity:** `validate_rust_method_call` must record `rust_arg_coercions` on the same terms as `validate_rust_function_call` so that method call lowering can insert `InteropCoerce` IR nodes. Without this parity, method calls at Rust boundaries would bypass coercion insertion.
- **Capability bound folding:** The parser emits `T with Send, Sync` as two separate type parameters (`T with Send` and bare `Sync`). The lowering stage must fold trailing bare capability markers back into the prior bounded type param so codegen emits `T: Send + Sync`, not `T: Send, Sync`. This folding only applies when the trailing name is a recognized `RUST_CAPABILITY_BOUNDS` entry and the prior param already has at least one capability-origin bound.
- **Stdlib trait impl boundary:** RFC 041 makes imported Rust items first-class for symbol resolution, method lookup, and boundary coercion, but it does not enable Incan to express Rust trait implementations (`Future`, `From`, blanket impls). Stdlib Rust adapter modules that contain genuine trait impls (`JoinHandle: Future`, `TaskJoinError: From<JoinError>`, `RuntimeFuture` blanket) remain justified Rust glue; only symbol-exposure-only shims are candidates for removal.
- **Initial associated-type and generic-method scope:** The first implementation must support the subset of inherent methods, associated items, generic methods, and associated-type-bearing APIs required to make real Rust-backed library authoring viable in pure Incan. Rust shapes beyond that subset may remain unsupported initially, but they must fail with explicit diagnostics rather than silently degrading the model.
- **Privacy and visibility diagnostics:** Diagnostics must distinguish between "does not exist", "exists but is not visible here", and "exists but is not yet supported by Incan interop". Where useful, the diagnostic should include the canonical Rust path and the privacy or support boundary that caused the rejection.
- **Docs precedence:** When wrapper or rebinding docs exist in Incan, they are the primary docs surfaced by the LSP and docs tooling. Tooling should retain Rust provenance and may append normalized Rust-origin docs as secondary or inherited context; if no Incan-authored docs exist, the normalized Rust docs should be surfaced directly.
