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

1. **Values.** Body IR carries a Rust type structurally: its item path and its type arguments, each argument an ordinary Body IR type. Each closed instantiation a program uses becomes one native plan declaration. Uninspected layouts remain opaque, with no fields or derives. A selected public struct layout retains its ordered fields and their canonical identities. The driver resolves the item through rustc (struct, enum or type alias), fills omitted defaulted type parameters from the item's own defaults (`HashMap`'s `S = RandomState`, `Arc`'s `A = Global`), and treats `std`, `core` and `alloc` as always available.
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

Native parameter facts distinguish Rust's unsized `str` referent from Incan's owned string carrier before generic substitution. A literal Rust `&str` parameter receives a text slice; a generic `&T` instantiated with Incan `str` receives a reference to its owned `String`. The plan borrows owned strings for `&String` and obtains a slice through `String.as_str()` for `&str`, preserving the owner across both calls.

A free function reached through an imported Rust module uses that module's public child item metadata, then the same free-function inference and retained callee record as a directly imported function. A module binding's source spelling does not become an executable target.

Inspected native signatures take priority over metadata-free fallback contracts. Owner type arguments may omit trailing metadata defaults; those defaults substitute earlier arguments in declaration order. A value imported as a constant selects methods from its resolved type's metadata. Retained callees keep their exact native return carrier independently of source result conversions. Shared scalar parameters borrow caller storage or a materialized literal, and mutable scalar parameters require an exclusive borrow fact and update the caller's storage. A retained Rust usize call returned from an Incan int function uses the existing explicit int conversion at that boundary.

Public Rust constants retain their metadata-selected item path and checked carrier through Body IR and the plan. This includes a constant selected through an imported module. The adapter resolves the public constant, verifies its exact type, and leaves evaluation to rustc; the frontend never copies a guessed value into the program. Rust statics require a separate storage contract.

After inherent lookup, extracted trait impl templates can select a closed trait reference. The checker retains the trait item path, Self followed by trait and method arguments, the receiver's passing mode, and the instantiated signature. Competing equally ranked references remain unresolved. Native carrier comparison is stricter than source compatibility: permissive opaque-path compatibility cannot select `From<char>` for a string. Conditional impl templates are candidates; the adapter verifies the selected closed callee's predicates before MIR construction. This uses the existing external-call machinery, including for blanket `ToString` and expected-result-directed `Into`.

A checked Rust result retains its native item identity when its source type projects to `str` or a collection. Canonical local bindings preserve that provenance independently of their source type and storage. Subsequent native method lookup reads this retained identity. Callable predicates retain input tuples and associated output equalities, so a selected owner's concrete parameters can contextually type a closure expression; its native callable representation must still survive the executable boundary before the foreign call can run.

Generic signature types substitute the checker's structured argument types before Body IR projection. This retains an Incan model's declaration identity across calls such as `swap` and `size_of_val`; a rendered Rust display of that model is not a replacement identity. A source-local declaration supplies its canonical carrier even when an inferred value never required a written type annotation.

Rust lifetime arguments carry no layout and are erased before Body IR carrier construction. The adapter supplies erased regions for metadata lifetime parameters without consuming a type-argument slot. Native carrier aliases take their exact type from rustc metadata through the alias type query, using the same argument binding and defaults as calls. Their AST shells declare only the alias identity; they do not reconstruct a second foreign generic type or invent source lifetime binders. Rustc checks the actual borrowed values in MIR.

Rust tuple elements retain shared and mutable scalar reference carriers explicitly. Mutable references cannot be copied. Nested callable signatures use the same carrier projection as ordinary parameters, and structural collection displays enter the same inference representation whether they come from Rust metadata or an Incan annotation. Executable representation version 22 appends the retained Rust constant form; version 21 payloads are refused before decoding and must be rebuilt.

## Native public layouts and derives

Checking retains a selected public struct's exact field templates and identities. Named construction infers the native owner arguments from already checked field arguments and retains the ordinary constructor slot binding. Body IR uses the same aggregate and field-projection machinery as source nominals. The driver gathers native layouts into their own declaration modules and verifies complete field count, names, public visibility, and instantiated types against rustc before emitting MIR. Missing fields, private fields marked public, and mismatched types refuse; an empty opaque layout still grants no projection.

Source models and newtypes retain supported `@rust.derive(...)` selections before admission. Builtin derives and canonical serde Serialize/Deserialize paths use the existing native derive machinery; automatic and explicit selections are deduplicated. An unknown macro expansion does not grant a native layout or implementation, and serde still requires its exact native dependency artifact.

## Non-goals

- Partial layouts containing hidden native fields, default-filled native field construction, and pattern matching on Rust variants.
- Const-generic arguments.
- Changing RFC 005's source surface.

## Revisit condition

Revisit when a consumer needs partial hidden-field layouts or variant access on Rust values, or when rustc's metadata can supply the callee record directly.

## Provenance

Derives from DD-0004 (the pinned driver), RFC 005 (Rust interop surface) and RFC 130 (Incan as a second front end to rustc). Supersedes nothing.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- [Issue #1698](https://github.com/encero-systems/incan/issues/1698)
- DD-0004, RFC 005, RFC 130
