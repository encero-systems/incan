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

## Checked call inference

Rust function and associated-function inference uses the same structural unifier as Incan functions. Only parameters declared by Rust facts are inference variables. Argument types and an expected result constrain those variables together; contradictory bindings are errors. Owner arguments include the defaults recorded on the Rust type. The checker substitutes the resulting arguments into the callable signature before recording parameter and result types, and retains inferred arguments in declaration order for Body IR. Inference does not make an unresolved Rust callee executable: retaining and lowering the callee remain separate requirements of the calls decision above.

For metadata-selected safe synchronous free functions, the checker retains the public callee path, instantiated parameters, result, and native type arguments. Body IR projects that record into its named call and external callable declaration machinery, retaining reference passing modes in the parameter types. The driver preserves those external declarations when joining project modules. A nongeneric Rust free function therefore uses the same external signature verification and call emission as a linked Loaf function. An absent checked call record grants no foreign execution behavior.

Inherent associated functions and methods retain the enclosing impl's native type parameters and self type from Rust metadata. Their native instantiation orders impl arguments before method arguments; it does not infer the impl's arity from the owner's type declaration. A value receiver becomes an explicit first parameter with the checked shared, mutable, or owned passing mode. Closed Rust signatures use the existing external callable records, selected by item identity and native arguments, and lower through the same instantiated external call machinery as linked Loaf functions. The adapter resolves the item and checks the complete instantiated native signature before MIR construction. Trait methods, async calls, unsafe calls, and signatures with unsupported carriers continue to require their own complete checked facts.

Declared reference arguments retain the caller's place through ordinary argument binding; preparing a reference never copies its referent into a value snapshot. Mutable receiver signatures also require a mutable source binding. Opaque Rust item paths remain type identities, while their plan layout names use identifier-safe encoding.

A free function reached through an imported Rust module uses that module's public child item metadata, then the same free-function inference and retained callee record as a directly imported function. A module binding's source spelling does not become an executable target.

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
