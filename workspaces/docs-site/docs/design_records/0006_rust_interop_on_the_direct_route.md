---
id: DD-0006
title: Rust interop values and calls on the direct route
status: Draft
type: design-decision
date: 2026-10-19
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-19
  - https://github.com/encero-systems/incan/issues/2256
  - https://github.com/encero-systems/incan/issues/1698
  - DD-0004
  - RFC 005
  - RFC 130
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0006: Rust interop values and calls on the direct route

## Context

0.6.0-dev.8 removes the Rust source generator, so the direct route (DD-0004) is the only route from checked Incan to an executable. The generator compiled RFC 005 interop by spelling Rust tokens and letting rustc infer the rest. The direct route instead hands rustc a plan of exact types and exact callees. Before this record, that plan held a Rust value only for a few allowlisted native items, had no callee for a call into a Rust crate, and had no path to a Rust value's own trait implementations. The checker represented a Rust type as a display string.

## Decision

Rust interop compiles on the direct route from checked facts alone. The facts come from rustc's metadata (DD-0009); the accepting checker freezes native storage and declaration identities, and Body IR copies those structural facts. Nothing is re-resolved from source spellings or display strings, and nothing is listed by hand.

1. **Values.** Body IR carries a Rust type structurally: its item path and its type arguments, each an ordinary Body IR type. Each closed instantiation a program uses becomes one plan declaration. An item whose public fields or variants the facts expose has that layout, so public fields are readable, variants can be matched and constructed, and named construction infers the owner's type arguments. Any other item is opaque. Lifetime arguments are erased: rustc infers them.
2. **Calls.** The checker records the resolved callee of every Rust call it admits (a free function, an inherent associated function or method, or a trait method with its trait), with the instantiated signature, the receiver's mode and the type arguments it inferred, using the same unifier as Incan calls. Inherent methods come before trait methods, as in Rust. Body IR carries that record; the lowering calls it through the external-call contract that Loaf calls use, and the driver keeps rustc's signature and predicate check. A call the checker cannot resolve refuses at the call site.
3. **Traits.** `Display`, `Debug`, `Clone` and equality on a Rust value call that value's own Rust implementation through rustc trait resolution. Native Copy receivers require a checked implementation and adapter verification.
4. **Futures and iteration.** A Rust value that implements `Future`, or `IntoFuture`, can be awaited; `await` yields its `Output`. A Rust value that implements `IntoIterator` can be iterated in loops and comprehensions; the element type is its `Item`.
5. **Borrowed text and slices.** `str` and `[T]` behind a reference stay distinct from owned strings and lists, and become owned only where an owned value is required.

Source models and newtypes may derive Rust traits (`Serialize`, `Deserialize`, `Default`, `Eq`, `Hash`); the derive becomes the Rust derive on the model's declaration.

## Consequences

- Programs using `from rust::... import ...` compile on the only remaining route, beyond the stdlib facets and Incan-declared externs.
- The checker's display strings remain for diagnostics only; no consumer parses them.
- The allowlisted native items are ordinary instances of part 1, and the handwritten method tables go away.
- Compiler-owned runtime ABI contracts stay separate from third-party Rust facts.

## Non-goals

- Layouts with hidden fields, and construction of items whose fields are not all public.
- Const-generic arguments.
- Borrowed Rust types in Incan signatures and fields, and Rust statics, until a lifetime or storage contract exists.
- Changing RFC 005's source surface.
- Rust statics without a storage contract, borrowed aliases in source signatures/fields without retained lifetime contracts, and unsafe or unsupported calls without complete checked facts.

## Revisit condition

Revisit when a consumer needs layouts with hidden fields, or when a lifetime contract for borrowed Rust types in Incan signatures is designed.

## Provenance

Derives from DD-0004 (the pinned driver), DD-0009 (Rust facts), RFC 005 (Rust interop surface) and RFC 130 (Incan as a second front end to rustc). Supersedes nothing.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- [Issue #1698](https://github.com/encero-systems/incan/issues/1698)
- DD-0004, DD-0009, RFC 005, RFC 130
