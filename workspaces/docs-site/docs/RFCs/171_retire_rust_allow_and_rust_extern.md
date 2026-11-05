# RFC 171: Retire `@rust.allow` and `@rust.extern`

- **Status:** Draft
- **Created:** 2026-11-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 022 (stdlib namespacing and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 057 (`@rust.allow`), which this RFC supersedes
    - RFC 127 (Incan lint catalog)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Remove two decorators whose reason for existing has gone. `@rust.allow(...)` suppressed lints in generated Rust, and the compiler no longer generates Rust source. `@rust.extern` marked a function whose body lives in a Rust module, and `rust::` imports now name that function directly. `rust::` remains the one spelling for reaching Rust from Incan.

## Motivation

`@rust.allow(...)` was added so an author could silence a Rust lint on the code generated for one declaration. Native compilation lowers checked Incan directly, so no generated Rust exists to carry the attribute and no Rust lint runs over it. The decorator is validated and then does nothing. An author who writes it believes they have suppressed something, and they haven't.

`@rust.extern` came from a time when Rust items couldn't be imported as ordinary names. An author wrote an Incan signature with no meaningful body, bound the file to a crate with `rust.module(...)`, and the compiler connected the two. With first-class `rust::` imports, the same function can be imported by name, so a signature written by hand is a second statement of a fact that the Rust item already states.

## Goals

- Remove `@rust.allow(...)`, superseding RFC 057.
- Remove `@rust.extern`, so a Rust-backed function is either a `rust::` import or an ordinary Incan function that calls one.
- Refuse both decorators with a diagnostic that names the replacement.

## Non-Goals

- Changing `rust::` imports.
- Changing `@rust.derive`, `rusttype` or other interop forms.
- Adding an Incan lint-suppression surface. RFC 127 owns Incan lints.

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

## Reference-level explanation

- A declaration decorated with `@rust.allow(...)` must be refused. The diagnostic must say that native compilation generates no Rust source and that the decorator can be deleted.
- A declaration decorated with `@rust.extern` must be refused. The diagnostic must name a `rust::` import as the replacement.
- A `rust::` import must keep its current checking and its canonical identity through re-exports, so `pub from rust::... import name` and a wrapper both present a checked signature to consumers.
- Generated references and the feature inventory must not list either decorator as available.

## Design details

### Interaction with existing features

A method body supplied by Rust, such as a comparison on a Rust-backed type, becomes an ordinary method that calls the imported function. Decorators other than these two are unaffected.

### Compatibility

This breaks source that uses either decorator. Incan has not reached 1.0, and the diagnostics state the mechanical change. Compiled libraries that were published earlier keep working, because the decorators only affect source.

## Alternatives considered

- **Keep `@rust.allow(...)` as a no-op.** It would keep telling authors that it does something.
- **Keep `@rust.extern` beside `rust::` imports.** That leaves two ways to reach one Rust function, and a handwritten signature that can disagree with the Rust item.

## Drawbacks

- A wrapper is more text than a decorated signature when a function needs its own documentation.
- Standard library modules that use `@rust.extern` have to be rewritten.

## Layers affected

- **Typechecker / Symbol resolution**: refuse both decorators, and remove the extern declaration path.
- **IR Lowering**: remove extern delegation.
- **Stdlib / Runtime**: replace existing uses with `rust::` imports or wrappers.
- **Docs / Tooling**: decorator reference, generated feature inventory, LSP completion.

## Unresolved questions

- `std.testing` declares marker semantics through `@rust.extern(metadata={...})`, and `incan test` reads them from there. Where should that metadata live? Recommendation: on an ordinary declaration the test runner reads through checked metadata.
- `rust.module(...)` exists to say where `@rust.extern` bodies live. Should it be retired here too? Recommendation: yes, unless another use is found.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
