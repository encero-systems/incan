# RFC 171: Retire `@rust.allow`, `@rust.extern` and `rust.module`

- **Status:** In Progress
- **Created:** 2026-11-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 022 (stdlib namespacing and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 024 (extensible derive protocol)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 048 (checked contract metadata)
    - RFC 057 (`@rust.allow`), which this RFC supersedes
    - RFC 125 (Loaf registry and baked asset distribution)
    - RFC 127 (Incan lint catalog)
- **Issue:** https://github.com/encero-systems/incan/issues/2682
- **RFC PR:** [#2688](https://github.com/encero-systems/incan/pull/2688)
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Remove three interop forms whose reason for existing has gone. `@rust.allow(...)` suppressed lints in generated Rust, and the compiler no longer generates Rust source. `@rust.extern` marked a function whose body lives in a Rust module, and `rust::` imports now name that function directly. `rust.module(...)` said which Rust module a file's bodies and macros came from, and `rust::` imports now say that at each name. `rust::` remains the one spelling for reaching Rust from Incan.

## Motivation

`@rust.allow(...)` was added so an author could silence a Rust lint on the code generated for one declaration. Native compilation lowers checked Incan directly, so no generated Rust exists to carry the attribute and no Rust lint runs over it. The decorator is validated and then does nothing. An author who writes it believes they have suppressed something, and they haven't.

`@rust.extern` came from a time when Rust items couldn't be imported as ordinary names. An author wrote an Incan signature with no meaningful body, bound the file to a crate with `rust.module(...)`, and the compiler connected the two. With first-class `rust::` imports, the same function can be imported by name, so a signature written by hand is a second statement of a fact that the Rust item already states.

`rust.module(...)` has one other job: it lets a marker trait stand for a Rust derive macro, so `@derive(IntoResponse)` reaches the macro in `incan_web_macros`. That is a third route to a Rust item, and a `rust::` import can name the macro directly.

`@rust.extern` has also been carrying something that has nothing to do with Rust. `std.testing` declares what a test marker means through `@rust.extern(metadata={...})`, and `incan test` reads it from there.

## Goals

- Remove `@rust.allow(...)`, superseding RFC 057.
- Remove `@rust.extern`, so a Rust-backed function is either a `rust::` import or an ordinary Incan function that calls one.
- Remove `rust.module(...)`, so a Rust derive macro is reached through a `rust::` import.
- Give test markers a declaration that doesn't depend on Rust.
- Refuse all three forms with a diagnostic that names the replacement.

## Non-Goals

- Changing `@rust.derive`, `rusttype` or other interop forms.
- Adding an Incan lint-suppression surface. RFC 127 owns Incan lints.
- Changing what any test marker does.

## Guide-level explanation

### Lint suppression

Delete the decorator. Nothing it did remains to be replaced.

```incan
@rust.allow("dead_code")   # refused: the compiler generates no Rust to carry this
def helper() -> int:
    return 1
```

### Rust-backed functions

Import the Rust function by name, and publish it the way any other name is published:

```incan
pub from rust::incan_std_testing import fail_t
```

When the public function needs Incan policy or its own documentation, write an ordinary function that calls the import:

```incan
from rust::incan_std_testing import fail_t as native_fail


pub def fail_t[T](msg: str) -> T:
    """Fail the current test with `msg`."""
    return native_fail(msg)
```

### Rust derive macros

Import the macro by name, and `@derive` uses it like any other derive:

```incan
pub from rust::incan_web_macros import IntoResponse
```

```incan
from std.web import IntoResponse


@derive(IntoResponse)
model Greeting:
    message: str
```

### Test markers

A marker declares its meaning with `@marker`, a decorator from `std.testing`, and `incan test` reads it through checked metadata:

```incan
from rust::incan_std_testing import skip as native_skip


@marker(kind="skip", runner_only=true)
pub def skip(reason: str = "") -> None:
    """Skip a test unconditionally."""
    native_skip(reason)
```

## Reference-level explanation

- A declaration decorated with `@rust.allow(...)` must be refused. The diagnostic must say that native compilation generates no Rust source and that the decorator can be deleted.
- A declaration decorated with `@rust.extern` must be refused. The diagnostic must name a `rust::` import as the replacement.
- A `rust.module(...)` statement must be refused. The diagnostic must name a `rust::` import as the replacement.
- A `rust::` import must keep its current checking and its canonical identity through re-exports, so `pub from rust::... import name` and a wrapper both present a checked signature to consumers.
- A `rust::` import of a generic Rust function must be callable like a generic Incan function, including a type parameter that appears only in its return type.
- A `rust::` import must be able to name a Rust derive macro, and `@derive` must accept that name.
- `std.testing` must declare marker semantics with `@marker(...)`. `incan test` must read them through checked metadata, and they must not depend on any Rust interop form.
- Generated references and the feature inventory must not list any of the three forms as available.

## Design details

### Interaction with existing features

A method body supplied by Rust, such as a comparison on a Rust-backed type, becomes an ordinary method that calls the imported function. Decorators other than `@rust.allow` and `@rust.extern` are unaffected.

### Compatibility

This breaks source that uses any of the three forms. Incan has not reached 1.0, and the diagnostics state the mechanical change. A published Loaf that uses one declares the toolchain range it builds with (RFC 125), so a toolchain that refuses the form doesn't select it.

## Alternatives considered

- **Keep `@rust.allow(...)` as a no-op.** It would keep telling authors that it does something.
- **Keep `@rust.extern` beside `rust::` imports.** That leaves two ways to reach one Rust function, and a handwritten signature that can disagree with the Rust item.
- **Keep `rust.module(...)` for derive macros.** That leaves a second route to Rust for one kind of item, and a marker trait with no behavior of its own.
- **Move marker metadata to a renamed decorator in the `rust` namespace.** Nothing about a test marker involves Rust.

## Drawbacks

- A wrapper is more text than a decorated signature when a function needs its own documentation.
- Standard library modules that use these forms have to be rewritten.

## Layers affected

- **Typechecker / Symbol resolution**: refuse all three forms, remove the extern declaration path, resolve `rust::` imports of derive macros, and check `@marker`.
- **IR Lowering**: remove extern delegation.
- **Stdlib / Runtime**: replace existing uses with `rust::` imports or wrappers, and declare test markers with `@marker`.
- **Test runner**: read marker semantics through checked metadata.
- **Docs / Tooling**: decorator reference, generated feature inventory, LSP completion.

## Progress Checklist

- [x] Retire the three Rust interop forms and diagnose their replacements. Direct Rust imports and checked test markers are covered by the implementation on `0.6.0-dev.9` ([#2688](https://github.com/encero-systems/incan/pull/2688)).
- [x] Verify the supported compiler and installed toolchain paths. The implementation PR records its contract census, Oven tests and Rust facet suite results.
- [ ] Record the release containing these changes when it ships.

## Design Decisions

- **`rust.module(...)` is retired too.** It has one use beyond `@rust.extern`, reaching derive macros, and a `rust::` import can do that, which keeps `rust::` as the one spelling.
- **Test markers use `@marker(...)` from `std.testing`,** read through checked metadata, because nothing about them involves Rust.
