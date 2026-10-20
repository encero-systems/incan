---
id: DD-0006
title: Rust interop values and calls on the direct route
status: Draft
type: design-decision
date: 2026-10-19
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-19 (dev.8 fix stage)
  - https://github.com/encero-systems/incan/issues/2256
  - https://github.com/encero-systems/incan/issues/1698
  - DD-0004
  - RFC 005
  - RFC 130
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0006: Rust interop values and calls on the direct route

## Context

0.6.0-dev.8 removes the Rust source generator, so the direct route (DD-0004) is the only route from checked Incan to an executable. The generator compiled RFC 005 interop by spelling Rust tokens and letting rustc infer the rest. The direct route instead hands rustc a plan of exact types and exact callees, and today that plan covers Rust interop only partially:

- A Rust value has no plan layout unless it is one of a few allowlisted native items (`axum` response and extractor carriers, the validation error, `FieldInfo`). A local of type `core::str::Utf8Error` or a field of type `std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, F>>>` refuses.
- A call into a Rust crate other than a stdlib facet has no Body IR callee: `String::from_utf8_lossy`, `Arc::new`, `value.lock()` and a trait method on a Rust value refuse. Stdlib facet calls and Incan-declared externs already work.
- No path calls a Rust value's `Display`, `Debug`, `Clone` or `PartialEq` implementation.

The checker represents a Rust type as a display string, with generic arguments rendered inside it, and records Rust call facts for generated-code emission rather than the resolved callee.

## Decision

Rust interop compiles on the direct route from checked facts alone, in three parts.

1. **Values.** Body IR carries a Rust type structurally: its item path and its type arguments, each argument an ordinary Body IR type. Each closed instantiation a program uses becomes one opaque plan declaration (no fields, no derives). The driver resolves the item through rustc (struct, enum or type alias), fills omitted defaulted type parameters from the item's own defaults (`HashMap`'s `S = RandomState`, `Arc`'s `A = Global`), and treats `std`, `core` and `alloc` as always available. Field access on an opaque Rust value is refused.
2. **Calls.** The checker records the resolved Rust callee of every Rust call site it admits: a free function, an inherent associated function or method, or a trait method with its trait, together with the receiver's borrow mode and the explicit type arguments the checker inferred. Body IR lowers that record to a Rust callee; the driver resolves it and keeps its exact signature check. A call the checker cannot fully resolve refuses at the call site; nothing is re-resolved from source spellings.
3. **Traits.** `Display`, `Debug`, `Clone` and equality on a Rust value call that value's own Rust implementation through rustc trait resolution, the same way the runtime's generic display helper does for type parameters.

## Consequences

- Programs using `from rust::... import ...` compile on the only remaining route; interop is no longer limited to stdlib facets and Incan-declared externs.
- The checker's display-string Rust types remain for diagnostics; the structured form is built once at the Body IR boundary from checked facts, so no consumer parses a display.
- The allowlisted native items become ordinary instances of part 1.

## Non-goals

- Field access, pattern matching on variants, or construction by fields of opaque Rust values.
- Lifetime-generic Rust items and const-generic arguments.
- Changing RFC 005's source surface.

## Revisit condition

Revisit when a consumer needs field or variant access on Rust values, or when rustc's metadata can supply the callee record directly.

## Provenance

Derives from DD-0004 (the pinned driver), RFC 005 (Rust interop surface) and RFC 130 (Incan as a second front end to rustc). Supersedes nothing.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- [Issue #1698](https://github.com/encero-systems/incan/issues/1698)
- DD-0004, RFC 005, RFC 130
