# RFC 121: Unified Incan/Rust type substrate

- **Status:** Draft
- **Created:** 2026-08-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 024 (extensible derive protocol)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 097 (Rust-hosted Incan caller)
    - #975 (Oven: Cargo-free Incan/Rust toolchain)
    - #1077 (unified Incan/Rust type substrate)
- **Issue:** https://github.com/encero-systems/incan/issues/1077
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** —

## Summary

Incan and Rust are one type substrate, expressed through two surface syntaxes — not two independently-typed systems bridged by a conversion layer at every boundary. Incan's built-in and stdlib types are defined to use the same underlying Rust representation, so crossing between Incan-authored and Rust-authored code, in either direction, is representation-identical rather than converted. This is the same relationship Kotlin has to Java: Kotlin's `String` is `java.lang.String`, not a wrapper around one, because both languages target the same underlying object model.

## Motivation

Before Oven, this was not a real option. Rust was built by a separate toolchain (Cargo), so Incan's own type system had to be treated as independent — something that got translated to Rust as a backend implementation detail, at every boundary crossing, because there was no single build authority that could guarantee both sides stayed representation-compatible. Once Oven owns the full build graph for both Incan-authored and Rust-authored code in one project (RFC 119), that structural constraint disappears. There is no longer a reason to keep them as two systems needing a bridge.

This surfaced directly out of RFC 097 (Rust-hosted Incan caller). Its "Type projection" section describes checked, sometimes-costly conversions for crossing into Rust — "checked `i64` boundary," borrowed-versus-owned string decisions, generated adapter code per type. Once RFC 097's caller boundary stopped being framed as bridging two independently-built binaries (the same realization that removed its constructed handle-object pattern), the conversion framing for *types* stopped making sense for the same reason: there is nothing to check or convert if the two sides were never different representations to begin with.

RFC 097 intentionally does not take this on fully — it states the caller-boundary consequences of this idea without owning the underlying substrate claim, because the claim affects more than one RFC. RFC 005, RFC 041, and RFC 043 govern the reverse direction (Incan importing and using Rust) and would need the same treatment for the model to be symmetric. This RFC exists to state that shared claim once, in one place, rather than have each interop RFC independently reinvent a piece of it.

## Goals

- State the substrate-identity principle formally: Incan and Rust share one type substrate.
- Define which categories of Incan types are covered — primitives, stdlib/compound types, user-authored models/enums/newtypes — and how each relates to its Rust representation.
- Define how canonical shared identity works for stdlib/compound types, so two different Incan libraries using the same stdlib type don't produce incompatible generated copies.
- Establish that this principle holds symmetrically for both interop directions: Rust calling Incan-authored code, and Incan importing and using Rust.
- Leave room for named, explicit exceptions where full representation identity turns out not to be achievable.

## Non-Goals

- This RFC does not require implementation in v0.6. It is the intended 1.0 north star, not scoped work for the current milestone; RFC 097 already states its caller-boundary consequences without needing this RFC to land first.
- This RFC does not itself rewrite RFC 097's "Type projection" section or RFC 005/041/043's conversion rules — those are follow-up revisions once this RFC's model is accepted.
- This RFC does not claim every Incan type can be made representation-identical to a Rust type. It states the principle and the target; specific exceptions, if any prove necessary, are named explicitly rather than silently assumed away.
- This RFC does not define a new intermediate ABI, FFI boundary, or serialization format. It is the opposite: for representation-identical types, there is no boundary left to define.
- This RFC does not change Incan's own authoring syntax. `model`, `enum`, and `newtype` keep meaning what they mean to an Incan author; this RFC is about what those declarations compile to, not how they're written.
- This RFC does not extend this claim to Incan's trait/dispatch system (RFC 025's multi-instantiation dispatch, RFC 043's Rust trait implementation) — see Unresolved questions.

## Guide-level explanation

Take the same running example RFC 097 already uses: an Incan-authored `policy` library, defining a request/decision pair and a policy function, called from Oven's Rust-authored core.

```incan
pub model ProviderRequest:
    pub target: str
    pub requested_capabilities: List[str]

pub def evaluate_policy(request: ProviderRequest) -> Result[ProviderDecision, str]:
    ...
```

**Before this RFC (conversion framing):** `request.requested_capabilities`, a `List[str]`, crosses into Rust through a projection step — RFC 097's original "Type projection" table describes this as producing a `Vec<String>` via a conversion, with `int` similarly requiring a "checked `i64` boundary." The mental model is two type systems, bridged.

**Under this RFC (identity framing):** `List[str]` *is* `Vec<String>`. There is no conversion step, because Incan's `List[str]` was never a different representation from `Vec<String>` — it is defined, in the compiler's own type system, to be that Rust type. The same is true of `int` and `i64`: nothing is checked at a boundary, because there is no boundary between two different representations to check.

This is already true today for one category of type: RFC 097 already states that a `model` projects to "a Rust caller struct," used directly. This RFC generalizes that same relationship — already correct for user-authored models — to primitives and to Incan's stdlib types as well.

The stdlib case needs one more piece: canonical sharing. Suppose two different Incan libraries, `policy` and `billing`, both use Incan's stdlib `Duration` type in a caller-visible signature:

```incan
# policy/sources/incan/lib.incn
pub def evaluate_policy(request: ProviderRequest, window: Duration) -> Result[ProviderDecision, str]:
    ...
```

```incan
# billing/sources/incan/lib.incn
pub def charge_for(amount: int, window: Duration) -> Result[Receipt, str]:
    ...
```

Without canonical sharing, each library's caller artifact could regenerate its own `Duration` struct — two Rust types with the same name, structurally identical, but nominally incompatible, so a Rust caller couldn't pass one library's `Duration` value to the other's function. With canonical sharing, both `policy::caller::incan::Duration` and `billing::caller::incan::Duration` resolve to the *same* Rust type, defined once in a shared location, re-exported rather than regenerated. This is the same principle RFC 097 already requires for `rusttype` values crossing the caller boundary — "the generated caller type and host type refer to the same Rust path and compatible version" — extended from "Rust types Incan wraps" to "Incan's own stdlib types."

User-authored types (`ProviderRequest`, `ProviderDecision`) don't need this canonical-sharing treatment the same way — each library legitimately owns its own domain types, and there is no ambiguity about which library's `ProviderRequest` a given signature means.

## Reference-level explanation

An Incan implementation adopting this substrate model must satisfy the following, for every type crossing between Incan-authored and Rust-authored code, in either direction:

- A caller-visible or interop-visible Incan primitive type (`int`, `bool`, `float`) must compile to its named native Rust type directly — `int` to `i64`, `bool` to `bool`, `float` to `f64` — with no generated wrapper type and no runtime check at the crossing point. `str` remains a policy choice between `String` and borrowed forms (RFC 097 already establishes this), not a representation check; whichever form is chosen, it is a real `String`/`&str`, not a wrapped copy.
- Every Incan stdlib/compound type (`List[T]`, `Dict[K, V]`, `Option[T]`, `Result[T, E]`, and any other stdlib-owned type) must have exactly one canonical Rust definition, shared by reference across every consumer, never regenerated per library or per caller artifact. Two Incan libraries using the same stdlib type in a caller-visible or interop-visible signature must produce mutually compatible Rust values.
- A user-authored `model`, `enum`, or `newtype` declaration must compile to a genuine Rust struct or enum that the declaration is sugar over, not a separate internal representation later projected into one. Incan's own checks (validation, invariants) attach to that same real type; they do not require a shadow representation to enforce.
- This must hold symmetrically: the same identity relationship applies whether Rust-authored code is calling into Incan-authored code (RFC 097) or Incan-authored code is importing and using a Rust type (RFC 005, RFC 041, RFC 043). Neither direction may define its own, different relationship between an Incan type and its Rust representation.
- Where full representation identity is not achievable for some type or case, that gap must be explicit and diagnosable — a named, documented exception — not a silent fallback to conversion behavior that the rest of this contract assumes doesn't exist.

## Design details

### Primitive identity

Incan's primitive types are declared, at the language's own type-system level, to use exactly the Rust representation already named for them — not merely mapped to that representation at emission time. This is a frontend/type-system commitment, not a backend/codegen optimization: the guarantee needs to hold before lowering, so that both RFC 097's caller boundary and RFC 005/041/043's Rust-interop boundary can rely on it without re-deriving it independently.

### Canonical stdlib/compound type identity

Incan's stdlib-owned compound types need one canonical Rust definition, in one shared location every consumer references rather than regenerates. The exact crate/module this lives in (a dedicated shared crate, a module of the RFC 097 support crate, or elsewhere) is an open question (see Unresolved questions) — the requirement this RFC states is the property, not the specific location: canonical, not per-consumer.

### User-authored domain types

A `model`, `enum`, or `newtype` is sugar over an ordinary Rust struct/enum declaration, with Incan's own validation and derive machinery (RFC 024) attaching directly to that real type. Because the derive protocol already targets real Rust traits (Debug, Eq, Clone, and user-authored derives per RFC 024), a model's derives were already, in effect, real Rust derives on a real Rust struct — this RFC states that relationship as the general rule rather than a caller-boundary-specific behavior.

### Relationship to RFC 097

RFC 097 states the caller-boundary consequences of this model (free functions instead of a handle object, no runtime version check, models already projecting directly to structs) without asserting the general substrate claim this RFC makes. Once this RFC is accepted, RFC 097's "Type projection" section should be revised to describe identity rather than conversion for the categories this RFC covers — a follow-up pass, not a change required to unblock RFC 097 itself.

### Relationship to RFC 005 / RFC 041 / RFC 043

These RFCs own the reverse interop direction — Incan importing and using Rust — and currently describe that direction in their own terms. For the model in this RFC to be symmetric rather than a one-directional caller-boundary optimization, each needs an equivalent follow-up revision once this RFC's principle is accepted. This RFC does not perform those revisions itself.

### Migration/convergence path

Closing any gap between Incan's current internal type representations and this RFC's target is compiler-owned type-system work, not something this RFC schedules or gates on a deadline. Where a gap is real (see Unresolved questions), it should be tracked as an explicit, named exception until closed — not treated as failure to adopt this RFC.

## Alternatives considered

- **Keep Incan and Rust as two independently-typed systems, bridged by conversion at every boundary (status quo).** This is what RFC 097 and the Rust-interop RFCs currently describe piecemeal. It works, but implies ongoing translation and checking cost, and per-boundary duplicated logic, that a single-toolchain, Oven-owned build graph no longer requires.
- **Apply this only to RFC 097's caller boundary, leaving the reverse direction conversion-based.** Rejected as the long-term target — it would leave the model asymmetric: the same substrate calling one direction, two systems calling the other.
- **Write a fully formal, ABI-level unification spec now, in this RFC.** Rejected as premature. This RFC establishes the target principle and what it requires of each category of type; the detailed mechanics (canonical module location, frontend representation changes) belong to implementation-phase follow-up work once actually undertaken, not to this RFC.

## Drawbacks

- Requires the compiler's own type system (HIR/Body IR, frontend) to converge toward representation identity for primitives and stdlib types — real engineering work, not free, and not scoped or scheduled by this RFC.
- Symmetric treatment means RFC 005, RFC 041, and RFC 043 each need a follow-up revision pass; this RFC creates that obligation without discharging it.
- A canonical shared stdlib-type module is new shared infrastructure to design, own, and version.
- Removes the "checked boundary" safety net for primitives in favor of relying on the frontend's own correctness to guarantee Incan's type semantics genuinely match Rust's — raising the bar on that guarantee rather than backstopping it with a runtime check.

## Layers affected

- **Typechecker / language type system**: primitive and stdlib type definitions must commit to Rust-representation identity at the type-system level, not only at emission time.
- **IR Lowering / Emission**: no conversion/adapter emission for representation-identical types; lowering passes a real value through rather than constructing a projected copy.
- **Stdlib (`incan_stdlib`)**: owns the canonical shared definitions for stdlib/compound types.
- **RFC 097's caller boundary**: "Type projection" section requires a follow-up revision from conversion framing to identity framing.
- **RFC 005 / RFC 041 / RFC 043's Rust-interop boundary**: require an equivalent follow-up revision for the reverse direction.

## Inspectability and tooling surface

- **Artifact or metadata:** the canonical Rust path for every Incan stdlib/compound type should be a checked, versioned fact — the same kind of identity record RFC 097 already defines for caller artifacts — so tooling can confirm two consumers agree on the same underlying type rather than silently diverging.
- **Inspection command:** `incan inspect types` (exact spelling not normative) should report, for a given Incan type, its concrete Rust representation and, for stdlib types, the canonical path every consumer resolves to.
- **Diagnostics:** a named, explicit exception (see Unresolved questions) must produce a diagnostic when relied upon incorrectly — for example, code assuming representation identity for a type that has a documented exception should fail clearly, not silently misbehave.
- **Provenance:** the frontend's commitment to a given type's Rust representation should be traceable to one canonical declaration (in the compiler's own type-system source, or the stdlib's canonical module for stdlib types), not scattered per-boundary.
- **Not implicit:** which types are covered by this identity guarantee, and which (if any) are named exceptions, must be an explicit, documented list — not left for a reader to infer from behavior.

## Unresolved questions

- Are there any Incan type-system semantics that cannot be made representation-identical to a natural Rust type — different overflow/panic semantics, different string-encoding guarantees, or similar — and if so, how should those exceptions be named and diagnosed?
- What is the actual canonical location for the shared stdlib-type module — part of `incan_stdlib`, a new dedicated crate, or something else?
- Does `str`'s owned-versus-borrowed (`String` vs `&str`) choice remain a genuine per-call-site policy decision under this model, or does it need its own resolution to fit cleanly under "representation identity"?
- What is the actual sequencing for revisiting RFC 005/041/043 and RFC 097's "Type projection" section once this RFC is accepted — one combined follow-up, or a separate pass per RFC?
- Does this RFC's principle extend to Incan's trait/dispatch system (RFC 025's multi-instantiation dispatch, RFC 043's Rust trait implementation), or is it scoped to data/type-shape identity only?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
