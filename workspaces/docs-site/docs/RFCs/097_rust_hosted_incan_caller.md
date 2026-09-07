# RFC 097: Rust-hosted Incan caller

- **Status:** Planned
- **Created:** 2026-05-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 013 (Rust crate dependencies)
    - RFC 031 (Incan library system phase 1)
    - RFC 034 (`incan.pub` package registry)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 121 (unified Incan/Rust type substrate)
    - #652 (v0.6 replacement-backend cutover)
    - #656 (Rust-facing ABI and Incan package compatibility direction)
    - #975 (Oven: Cargo-free Incan/Rust toolchain)
- **Issue:** https://github.com/encero-systems/incan/issues/569
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC defines a Rust-hosted Incan caller model for **self-hosted, Oven-orchestrated interop**: within one Oven-managed project, Rust-authored code should be able to call a curated, typed Rust-facing API backed by Incan-authored behavior, without reverse-engineering compiler output, manually wiring Incan runtime helpers, or treating every public Incan export as a stable Rust API. The motivating case is Incan's own toolchain: Oven's operational core is a deliberate, narrow Rust exception (RFC 118), while providers, generators, and policy logic are meant to be ordinary typed Incan functions (RFC 104, RFC 117) — this RFC defines how that Rust core calls into that Incan-authored code reliably, in the same repo, same bake, same toolchain.

This Draft is framed around a Rust-facing caller facet and caller ABI metadata. A caller artifact is built by Oven's Rust facet compiling the Rust-host caller projection carrier (RFC 119) through direct `rustc` invocation, within the same Oven-managed build graph as the calling Rust-authored unit — there is no path to a Rust-hosted caller artifact that does not go through Oven. Rust source produced this way is a deliberate, normative build mechanism for the selected facet, not incidental compiler output; it is separate from the ordinary default-path Rust emission that #652's backend cutover demotes to an optional migration/debugging/inspection projection with no compatibility promise. The caller boundary is exposed as plain typed Rust free functions, with compatibility guaranteed by construction since Oven compiles the Rust-authored caller and the Incan-authored callee together in one bake. The caller boundary is the stable host API shape; it is backed by checked Incan metadata, ABI metadata, generated adapters where needed, and a small support crate that owns conversions, async/runtime policy, diagnostics, and panic/error containment.

Cargo-compatible external consumption — a Rust project outside Oven's own build graph depending on a published Incan-authored package — is out of scope for this RFC. Divorcing Incan's own toolchain from Cargo entirely is the stated 1.0 goal (#975); this RFC does not reopen that boundary to accommodate external Cargo consumers. If a Cargo-facing consumption story is ever wanted, that is a separate proposal with its own tradeoffs to make explicitly, not a loosening of the design this RFC defines.

## Core model

1. **Rust-hosted consumption is a first-class direction:** Incan already lets Incan code call Rust; this RFC defines the reverse direction where Rust code deliberately calls Incan-authored behavior.
2. **The caller artifact is built by Oven, not by ambient compiler output:** a caller artifact only exists because Oven's Rust facet (RFC 119) selected and built the Rust-host caller projection carrier through direct `rustc` invocation, within the same Oven-managed build graph as its caller. The public compatibility promise is the caller facet and ABI metadata that gate that build, not whatever Rust text the ordinary default compiler path happens to produce for debugging.
3. **Default-path implementation artifacts remain inspectable but carry no promise:** generated Rust from the ordinary (non-caller) build path, object code, IR snapshots, or other backend artifacts should be inspectable and debuggable where emitted, but they are not the host-facing semantic contract. They are a separate build output from the caller carrier's own Rust build, produced by a separate build request (an ordinary build versus a selected Rust-hosted caller build) with its own, unguaranteed stability — even though both draw on the same upstream parsing, typechecking, and Body IR/semantic-facts stages, and only diverge at backend/carrier selection.
4. **The caller boundary is plain typed Rust functions:** the Rust-authored caller calls free functions directly, backed by caller helpers and support traits that make those calls feel natural from Rust while preserving Incan semantics.
5. **`pub` alone is sufficient; the caller facet is derived from usage, not authored as a separate declaration:** an ordinary `pub` Incan export is already eligible to be called from a Rust-authored unit in the same Oven-managed build graph. Oven determines the actual caller facet by observing which `pub` exports are referenced across every Rust-authored unit in the project during the bake, unioned into one stable facet per Incan library — an Incan author does not maintain a second, hand-curated list of Rust-visible exports alongside the ordinary `pub` surface.
6. **Types cross through reusable helpers:** primitive values, models, newtypes, enums, `Result`, `Option`, collections, and Rust-backed types should cross through explicit, versioned conversion helpers that can also simplify emitter responsibilities.
7. **Runtime policy is explicit:** async execution, logger/telemetry hooks, host capabilities, and panic handling must be part of the caller contract rather than incidental generated code behavior, even without a constructed object to attach them to — ambient wiring or explicit parameters, not implicit assumptions.
8. **Cargo independence is the design, not a temporary restriction:** the caller contract targets Oven-orchestrated interop within one build graph, same toolchain end to end, and does not shape itself around a foreign build system's constraints. A Cargo-consumable bridge is not a later phase of this RFC; it would be a distinct proposal with its own tradeoffs, layered outside this contract rather than replacing it.

## Motivation

Incan's current interop story is strong in one direction: Incan source imports Rust crates, wraps Rust types, and can implement Rust traits for Incan-owned types. That is necessary, but it does not answer the reverse question Incan's own toolchain already needs answered: how does Rust-authored code in the same project call into Incan-authored behavior?

That question is not hypothetical. RFC 118 already commits Oven's operational core to staying Rust — "a justified Rust exception," narrow but real. RFC 117 and RFC 104 already expect providers, generators, and policy logic to be "ordinary typed Incan functions." Put together, those two decisions already require a Rust-authored core to call Incan-authored code, in the same repository, compiled in the same bake. This RFC exists because that call has no defined contract yet: no stable namespace, no typed conversions, no error boundary, no compatibility guarantee. Without it, Oven's Rust core would have to either avoid calling Incan-authored logic at all (undermining the entire "generators are ordinary typed Incan functions" premise) or reach into whatever internal implementation shape the compiler happens to produce that week.

RFC 031 created the first library artifact foundation: an Incan library can build a semantic manifest and implementation artifacts. The missing product-level answer is the shape above those artifacts: which public exports are intended for Rust callers, which helper types make calls feel Incan-like from Rust, which support code owns repeated boundary mechanics, and which metadata defines compatibility without exposing default-path implementation internals as API.

Oven completes the delivery boundary without replacing this caller model. A selected Rust-hosted export facet is a target-neutral contract within Oven's own project; Oven turns it into target-specific build units by selecting the Rust-host caller projection carrier and compiling it through its Rust facet's direct-`rustc` build graph (RFC 119), in the same bake as the Rust-authored caller. RFC 117 owns the authored-manifest, resolved-graph, asset-identity, and receipt model that this RFC consumes.

The missing piece is not only a command. It is a boundary. Rust-authored code embedding Incan behavior needs to know which calls are stable, how values convert, how errors surface, whether async calls need a runtime, whether panics are contained, and which compiler/runtime version produced the artifact. Without that boundary, callers either treat compiler output as hand-authored Rust or avoid Rust-hosted Incan entirely.

The end-state is Incan's own toolchain proving the model on itself: Oven's Rust core calls into Incan-authored providers, generators, and policy logic as typed dependencies within one build graph, with no Cargo anywhere in that loop — the first real, self-hosted, bidirectional Rust↔Incan interop.

## Goals

- Define a Rust-hosted caller model for Rust-authored code, within an Oven-managed project, calling Incan-authored behavior.
- Define a stable Rust-facing caller surface backed by ABI metadata.
- Keep default-path implementation artifacts inspectable without making them the public compatibility path.
- Define how the caller facet is derived from actual Rust-side usage of `pub` exports, without requiring a second, separately authored declaration.
- Define conversion requirements for primitives, collections, models, enums, newtypes, results, options, and Rust-backed values.
- Define reusable caller helpers that can reduce bespoke emitter output for common boundary shapes.
- Define version, diagnostics, panic, async, logging, telemetry, and host capability responsibilities at the caller boundary, without relying on a constructed handle object to hold them.
- Define how a selected caller facet becomes a target-compatible Oven caller artifact with inspectable build and source-map receipts, consumed entirely within Oven's own build graph.
- Keep Rust integration Rust-shaped enough to feel natural to a Rust caller without making Incan source adopt Rust's full API design model.

## Non-Goals

- This RFC does not define Cargo-compatible package publication, crates.io distribution, or consumption by a Rust project outside Oven's own build graph. That is out of scope for this design, not a gap it leaves open; see Alternatives considered.
- This RFC does not make default-path generated Rust source the public package compatibility path.
- This RFC does not require the ordinary (non-caller) build path to emit Rust source.
- This RFC does not make every default-path generated Rust module a stable public API where such output is still emitted.
- This RFC does not replace `rust::` imports or Rust interop from Incan source.
- This RFC does not define a C ABI, dynamic plugin ABI, `extern "C"` boundary, or cross-language FFI story.
- This RFC does not require a Rust application to run the Incan compiler at runtime.
- This RFC does not eagerly build a caller wrapper for every `pub` export regardless of whether anything calls it from Rust; only exports actually referenced by a Rust-authored unit in the same build graph enter the caller build (see "Public export profiles").
- This RFC does not define registry publication mechanics beyond compatibility with RFC 034 and RFC 079.
- This RFC does not define `loaf.toml`, `oven.lock`, or the `*.loaf` asset format; it consumes the project, asset-identity, and receipt contract from RFC 117.
- This RFC does not define the full implementation of async runtime internals, host capability enforcement, or telemetry backends.
- This RFC does not guarantee that every Rust type imported through `rust::` can automatically cross back into a caller-visible Rust type without an adapter.

## Guide-level explanation

Take the motivating case directly: Oven's Rust-authored core needs to evaluate a capability grant, and per RFC 104/117 that policy logic is meant to be an ordinary typed Incan function, not hand-written Rust. Both live in the same workspace, as sibling Loaves under RFC 117's workspace model:

```text
oven/
├── loaf.toml              # [workspace] members = ["core", "policy"]
├── core/
│   ├── loaf.toml          # [project] name = "oven_core"
│   └── sources/rust/plan.rs
└── policy/
    ├── loaf.toml          # [project] name = "policy"
    └── sources/incan/lib.incn
```

`core` is Oven's own Rust-authored operational core (RFC 118's justified Rust exception); `plan.rs` is where the Rust-authored caller in this example lives. `policy` is an ordinary Incan-authored library Loaf, with its logic in `lib.incn`. An Incan author writes the policy logic there as an ordinary library export:

```incan
pub model ProviderRequest:
    pub target: str
    pub requested_capabilities: List[str]

pub model ProviderDecision:
    pub granted: List[str]
    pub denied: List[str]

pub def evaluate_policy(request: ProviderRequest) -> Result[ProviderDecision, str]:
    if len(request.requested_capabilities) == 0:
        return Err("no capabilities requested")
    granted = [cap for cap in request.requested_capabilities if cap != "process.exec"]
    denied = [cap for cap in request.requested_capabilities if cap == "process.exec"]
    return Ok(ProviderDecision(granted=granted, denied=denied))

pub async def evaluate_policy_remote(request: ProviderRequest) -> Result[ProviderDecision, str]:
    # unlike evaluate_policy above, this checks a remote-backed policy service
    return await fetch_remote_decision(request)
```

Nothing further is required on the Incan side to make `evaluate_policy` callable from Rust. `pub` is already sufficient — there is no separate declaration to write. From `plan.rs`, Oven's Rust-authored core simply calls it, as a plain function:

```rust
use policy::caller::incan::{evaluate_policy, ProviderRequest};

fn plan_provider_grant(request: ProviderRequest) -> Result<(), Box<dyn std::error::Error>> {
    let decision = evaluate_policy(request)?;
    // Oven's Rust core uses `decision` to finish planning the provider grant.
    Ok(())
}
```

The crate path starts with `policy`, not an invented prefix — it is the `policy` Loaf's own project name, following the same rule RFC 119 already states for the Rust facet generally ("the derived crate name follows the project name only when that mapping is unambiguous"). The caller artifact this RFC defines is projected under that same crate name, at the `caller` module this RFC's caller boundary owns.

Compatibility holds by construction: Oven compiles this Rust-authored core unit and the Incan-authored `policy` library together, in the same bake, keyed by source digest (RFC 119), so a call can never reach a caller artifact built against a stale or incompatible callee. `evaluate_policy` and `ProviderRequest` both live under the reserved `incan` submodule of the caller namespace, since both are caller-projected Incan items — a function projects the same way a model does, and reserving that submodule keeps every projected name away from whatever compiler-generated infrastructure (such as the caller error type) still needs its own space at the namespace's top level (see "Caller facet and artifact shape").

`oven bake` (exact command spelling not normative) resolves the whole build graph — both the Rust-authored core unit and the Incan-authored policy library — in one pass. During that resolution, Oven observes that the core unit references `evaluate_policy`, and that observation is what makes it part of the caller facet: no separate declaration authored it there. Oven then checks that the reference has a representable Rust signature and, if so, selects the Rust-host caller projection carrier and materializes a target-compatible caller artifact through its Rust facet's direct-`rustc` build graph (RFC 119) — this is the only way the artifact comes into existence; there is no `incan build` path that produces it without Oven. There is no manifest dependency line to write for this either — the exact in-workspace linkage mechanism is Oven planning state, not authored configuration, matching how RFC 119 already links host/target Rust units within one graph. The artifact carries caller metadata, ABI metadata, target compatibility, source-map references, and its verified Oven build receipt, inspectable for diagnostics and provenance even though nothing checks it at call time.

If a later change removes the only Rust-side reference to `evaluate_policy`, the next bake simply stops building a caller wrapper for it — the caller facet tracks live usage, not a list someone has to remember to prune. And when the default projection genuinely isn't enough — an alias, a non-default type projection, a forced blocking wrapper around an async export — attaching that policy to the declaration itself remains an open question (see Unresolved questions); it is override metadata for the uncommon case, not a gate the common case has to pass through.

`evaluate_policy_remote` is declared `async def` in Incan, unlike `evaluate_policy` above — and that is what determines its projection, not a separate calling-convention choice on the Rust side. An async Incan export projects to an async Rust free function, called with `.await` from an async Rust caller:

```rust
use policy::caller::incan::{evaluate_policy_remote, ProviderRequest};

async fn plan_provider_grant_remote(request: ProviderRequest) -> Result<(), Box<dyn std::error::Error>> {
    let decision = evaluate_policy_remote(request).await?;
    Ok(())
}
```

An Incan export that is never referenced by a Rust-authored unit simply never enters the caller facet — the Rust-authored caller must not rely on whatever implementation symbols happen to exist for anything it hasn't actually called. The distinction is about semantic authority: caller metadata and the caller namespace are stable; default-path implementation artifacts are not.

The author-facing model is:

```text
Incan library source
  -> checked public Incan API (ordinary `pub`)
  -> Oven observes which `pub` exports a Rust-authored unit in the same build graph actually references
  -> Rust-facing caller facet + ABI metadata, derived from that usage
  -> Oven selects the Rust-host caller projection carrier (RFC 119)
  -> Oven's Rust facet builds it via direct `rustc`, linked into the same build graph as the calling Rust-authored unit
  -> Rust-authored code within that Oven-managed project
```

## Reference-level explanation

An Incan implementation that supports Rust-hosted caller artifacts must emit a Rust-facing caller boundary for `pub` exports referenced by a Rust-authored unit in the same Oven build graph.

The caller boundary must include:

- a stable Rust module or crate-level namespace for caller APIs, structurally reserving its own top level for compiler-generated infrastructure and placing every caller-projected Incan type under a reserved `incan` submodule (see "Caller facet and artifact shape")
- typed Rust representations for caller-visible Incan input and output types
- conversion implementations or generated adapters for every caller-visible boundary type
- version and compatibility metadata, inspectable for diagnostics and provenance
- diagnostic metadata sufficient to map caller failures back to Incan export names and source spans where available
- error types that distinguish Incan `Err` values, Incan runtime errors, caller conversion errors, host capability errors, version mismatches, and contained panics

The caller boundary must not require Rust consumers to import arbitrary default-path implementation modules as the host API. Such modules may exist and should remain readable where the default path still emits them, but only the caller namespace is stable for Rust-hosted consumption.

The caller boundary must be materialized by Oven, via its Rust facet building the Rust-host caller projection carrier (RFC 119), as a target-compatible caller artifact linked into the same build graph as its Rust-authored caller. The Rust-authored caller must not need to know the compiler's internal implementation layout.

Caller-visible Incan functions must have a representable Rust signature. The compiler must reject a Rust-hosted public export when any parameter, return value, type parameter, effect, or captured dependency cannot be represented by the caller boundary.

Caller-visible synchronous functions must expose a synchronous Rust free function. Caller-visible async functions must expose an async Rust free function or an explicit, separately named blocking-wrapper free function, depending on the declared caller mode. Neither must silently create or own a runtime in a way that surprises the caller.

Compiler/runtime compatibility is a build-time guarantee: Oven compiles the caller artifact and its Rust-authored caller together in the same bake, keyed by source digest, so no call can ever reach a caller artifact built against an incompatible compiler, runtime, target, or profile. The generated artifact must still expose metadata identifying the Incan compiler version, manifest format, caller facet, caller ABI version, generated support crate version, target/profile compatibility, and the selected Oven build receipt or `*.loaf` asset identity, inspectable for diagnostics, provenance, and cache-correctness auditing.

Incan `Result[T, E]` crossing into Rust must map to Rust `Result<T, E>` when both `T` and `E` are representable. Boundary failures that occur outside the Incan function's declared return value must use the caller error type, not the Incan function's domain error type.

Incan `Option[T]` must map to Rust `Option<T>` when `T` is representable.

Incan `None`, `bool`, `int`, `float`, `str`, list, dict, tuple-like fixed records, models, enums, value enums, newtypes, and constrained newtypes must have deterministic caller conversion rules. Integer width, constrained storage carriers, and validation failures must not rely on unchecked Rust casts.

Incan models exposed to Rust callers should generate Rust structs with stable field names and derive surfaces appropriate for Rust host use when those derives are requested or safe by default. Wire aliases remain data-contract metadata and must not silently rename Rust struct fields unless the caller export explicitly chooses that projection.

Incan newtypes exposed to Rust callers must preserve validation semantics. Constructing a Rust-side caller newtype from an unchecked primitive must either be impossible or go through a checked constructor.

Rust-backed `rusttype` values may cross the caller boundary only when the backing Rust type is visible to the host crate and the artifact can prove that the generated caller type and host type refer to the same Rust path and compatible version. Otherwise, the export must require an explicit adapter.

Panics from compiled Incan code or Rust code called by Incan must not be indistinguishable from ordinary Incan `Result` values. Implementations should contain unwinding where practical and report a caller panic error with diagnostic context. If panic containment is unavailable for a target, the caller metadata must state that policy clearly.

Host capabilities used by caller-visible Incan code must be visible through metadata. If an Incan caller export requires filesystem, network, process, environment, telemetry, clock, async runtime, or other host services, those services must be provided or validated ambiently by Oven's build-time capability wiring, or through an explicit parameter on the specific caller-visible functions that need them, before use where the target supports such validation.

Caller diagnostics and capability outcomes must reference stable public source-map and receipt identities. They must not expose private HIR, Body IR, compiler session state, or backend implementation representations.

## Design details

### Authority and compatibility identity

The compatibility surface for a Rust-hosted caller boundary is limited to a specific set of inputs. Everything else is implementation detail that may change between compiler versions without breaking the caller contract — a real concern even within one repository, since the Rust-authored caller and the Incan-authored callee are typically maintained across separate changes.

Compatibility inputs are: Incan source and its checked public API, semantic manifests (RFC 031's library artifact model), caller-facet metadata (which exports, projections, and policy make up the selected facet), ABI metadata (caller ABI version, compiler/runtime compatibility range, target/profile compatibility), and versioned native artifacts — the caller build unit and its `*.loaf` asset.

Never part of the contract: generated Rust source (useful for inspection, debugging, migration evidence, or as an implementation backend, but never a compatibility promise), and private HIR, Body IR, compiler session state, or any other backend implementation representation.

A caller artifact must be usable by its Rust-authored caller without reading or depending on any of the non-public inputs above. A rule elsewhere in this RFC that would require the Rust-authored caller to depend on generated Rust internals, private HIR, or Body IR to use a caller-visible export correctly is incompatible with this section.

### Caller facet and artifact shape

The Rust-hosted caller facet is the target-neutral selection of Incan exports, type projections, capability requirements, caller ABI version, and source-map schema. Oven materializes that facet into target-compatible caller artifacts by selecting the Rust-host caller projection carrier and building it through its Rust facet's direct-`rustc` graph (RFC 119), linked into the same build graph as the Rust-authored caller. The normative contract is the caller facet and metadata, not the emitted source layout.

Conceptually, the caller artifact set contains:

```text
stable caller namespace
caller facet metadata
ABI metadata
semantic manifest
source-map and Oven-receipt references
implementation artifact(s)
target-compatible caller `*.loaf` asset(s)
```

The exact directory layout and static-versus-dynamic linkage are not normative. The Rust-authored caller must not need to know which files came from Incan source lowering, backend emission, support glue, ABI materialization, or Oven storage.

Caller-visible Incan functions project to plain Rust free functions.

The caller namespace must reserve its own top level for compiler-generated infrastructure — at minimum, the caller error type(s) (see "Errors") — and must never place a caller-projected Incan item (a projected function, model, enum, or newtype) directly at that top level. Every caller-projected Incan item must instead live one level deeper, under a reserved `incan` submodule of the caller namespace (for example `policy::caller::incan::evaluate_policy` and `policy::caller::incan::ProviderRequest`, side by side). This is a structural rule, not a name-collision check: an Incan export whose own name happens to match a reserved infrastructure name (including an export literally named `incan`) can never collide with it, because projected items are never eligible to land at the top level in the first place — the collision class is eliminated by construction rather than detected and diagnosed after the fact.

### Caller identity metadata

Every caller artifact Oven materializes must carry an identity record readable by its Rust-authored caller and by inspection tooling before the artifact is trusted. The record must include: `package_name` and `package_version` (the Incan package identity); `caller_facet_id` (identifies the library-scoped, usage-derived facet this artifact was built from — one per Incan library per bake, shared by every Rust-authored caller in the project, never scoped to a single calling unit); `caller_abi_version` (versioned independently of `package_version`, bumped when the caller-facing shape changes in a way that could break an existing caller); `compiler_version_range` (the range of Incan compiler versions this artifact is compatible with); `manifest_schema_version` (the version of this identity record's own format); `target` and `profile`; and a reference to the exact Oven build receipt the artifact was materialized from.

Compatibility holds by construction: Oven compiles the caller artifact and its Rust-authored caller together in the same bake, keyed by source digest (RFC 119), so a compiler/runtime/target/ABI mismatch cannot reach a call site at all — Oven's build-unit staleness check invalidates and rebuilds the caller artifact whenever the callee's caller-facing shape changes. The identity record's role is inspectability: what a diagnostic, a receipt audit, or a caching-bug investigation reads to confirm which exact build produced a given caller artifact.

An unsupported or non-representable caller-visible export must be rejected when Oven selects and bakes the caller build unit, not discovered later by its Rust-authored caller at call time. Oven must not materialize a caller artifact or `*.loaf` asset that contains an unresolved unsupported-export failure.

### Oven planning

Oven must include the selected caller facet, public feature projection, compiler and support compatibility, target/profile, native-linkage requirements, and relevant source/ABI metadata in the caller build-unit identity. The resulting `*.loaf` asset must identify the caller facet and the exact build receipt through which it was built. The Rust-authored caller consumes that selected caller artifact directly, within the same Oven build graph — there is no Cargo subprocess and no external package resolution step (see Non-Goals and Alternatives considered for why Cargo-consumable projection is out of scope for this design).

### Support crate

The Incan toolchain should provide a small Rust support crate for caller boundaries. That crate should own shared traits, error types, conversion helpers, and host context traits that would otherwise be duplicated into every generated caller artifact or embedded directly into emitter output. Oven's Rust facet builds and links it into the same build graph as any other Rust-authored unit in the project; nothing about its existence depends on Cargo.

The support crate should not become a large runtime framework. It should be boundary infrastructure: conversions, diagnostics, panic policy, host context plumbing, and reusable call-shape helpers. Those helpers should make Rust-hosted calls feel closer to Incan's domain model while giving the emitter fewer bespoke cases to print inline.

### Public export profiles

The caller facet is derived, not authored. Plain Incan `pub` exports remain the Incan package API, and every `pub` export is eligible to be caller-visible without a second declaration. The caller facet is **library-scoped, not caller-unit-scoped**: for a given Incan library, it is the union of every `pub` export referenced by *any* Rust-authored unit anywhere in the same Oven build graph, checked for a representable Rust signature. One caller module (`incan_<library>::caller::incan`) exists per Incan library, shared by every Rust-authored caller in the project — two Rust modules calling the same library see the same free functions and types, and a new caller referencing a new export only ever grows that union, never produces a differently-shaped module for a different file. Scoping the facet per calling unit instead was considered and rejected: it would make the caller module's available functions depend on which file was asking, defeat ordinary incremental caching (a bespoke module per Rust unit rather than one stable module per library), and give a brand-new caller nothing to discover from, since a per-unit facet only contains what that unit already references.

Library-scoping still leaves a real gap for the *first* caller of a library, or any caller wanting to browse what's available before writing a reference: nothing has been used yet, so the compiled caller module may not contain it. This RFC resolves that by separating two different concerns rather than picking one artifact to serve both. What actually gets **compiled** stays usage-scoped and lean, for exactly the reasons "Alternatives considered" rejects eager generation: no wasted representability-checking or generated code for exports nobody calls. What a developer can **discover** does not depend on the compiled artifact at all — Oven already determines, for every `pub` export, whether it is representable independent of whether anything currently references it (see "Reference-level explanation"), and that eligibility fact is exactly what LSP completion and an inspection command (`incan inspect caller <library>`, exact spelling not normative) should surface: the full eligible surface, not just the realized one. A developer writing new Rust-authored code gets full IDE-backed discovery of what it *could* call; the compiled artifact only ever contains what it actually does.

This must be answered by the same session-owned, incrementally-maintained semantic-facts service RFC 106 defines for LSP reference queries — the same machinery that already needs to answer "find references" at interactive latency for editor tooling answers both "which `pub` exports does any Rust-authored unit in this project reference" for the compiled union and "which `pub` exports are representable" for discovery. The usage query must be gated by the same source-digest staleness check that governs Oven build-unit reuse (RFC 119): it must run only for units whose source, or transitive dependency closure, changed since the last successful bake receipt, not as a full-project re-scan on every bake. Usage detection that does not meet this bar is not an acceptable implementation of this section.

A separate, explicitly-authored declaration remains relevant only for non-default policy — an alias, a non-default type projection, a forced blocking wrapper around an async export — layered onto the `pub def` (or equivalent) itself. Its exact shape is unresolved (see Unresolved questions); it is override metadata for the exception case, not a required gate for the default case.

### Type projection

The table below describes projection in conversion terms, which is what this RFC needs to be usable now. RFC 121 proposes the durable target this should converge toward: for primitives and stdlib types, no conversion at all, because the Incan type and its Rust representation are the same thing. That convergence is compiler-owned type-system work with its own timeline, not a prerequisite for this RFC.

Caller type projection should prefer ordinary Rust types where doing so preserves semantics:

| Incan surface | Rust caller projection |
| ------------- | ---------------------- |
| `bool` | `bool` |
| `int` | checked `i64` boundary unless a narrower constrained carrier is explicitly exposed |
| `float` | `f64` |
| `str` | `String` or borrowed input forms where explicitly generated |
| `Option[T]` | `Option<T>` |
| `Result[T, E]` | `Result<T, E>` for domain result values |
| `List[T]` | `Vec<T>` |
| `Dict[K, V]` | map type with documented ordering/hash requirements |
| `model` | Rust caller struct |
| `enum` | Rust caller enum |
| `newtype` | Rust caller newtype with checked construction |

Borrowed Rust signatures may be generated as an optimization, but the semantic contract must first be expressible with owned values. Borrowed projections must not expose Incan lifetime or ownership details as user-authored Incan concepts.

Caller artifact metadata must also declare, per export, the ownership/conversion policy (owned vs. borrowed projection) and any runtime or native-linkage requirements needed to call it, such as an async runtime, a specific allocator, or host-provided native linkage. Where 0.6 does not establish behavior — no-std compatibility, C-ABI stability, dynamic-plugin loading, or a specific allocator/panic policy — the metadata must state that status explicitly as unsupported or unknown rather than implying an unstated guarantee.

### Errors

The generated caller API should use two layers of errors:

- domain errors produced by Incan functions that return `Result[T, E]`
- caller errors produced by the boundary itself

For a function whose Incan signature returns `Result[T, DomainError]` for some domain error type `DomainError`, the Rust caller may expose a nested result or a generated convenience type, but it must preserve the distinction between `DomainError` and boundary failures such as conversion failure, missing host capability, incompatible artifact version, or contained panic.

### Async and runtime policy

Async caller exports must not assume that the caller artifact owns the process runtime. The Rust-authored caller should either provide an async context by calling async functions or explicitly opt into a blocking wrapper that documents runtime behavior.

Caller metadata should state whether an export is synchronous, async, blocking, or requires host-provided runtime services. This should compose with RFC 104 target and host capability metadata when those contracts mature.

### Diagnostics and observability

Caller failures should identify the caller export name, the Incan function name, and source-span metadata when available. Logging and telemetry should route through host-provided hooks where configured, rather than unconditionally initializing global logging from the caller package.

Public caller diagnostics must bind to the compiler-owned backend-selection and execution receipts (#986) and to the selected Oven build/`*.loaf` receipt (RFC 117), so a host or CI system can trace a caller failure back to a specific verified build without exposing private compiler representations. Diagnostics must distinguish at minimum: Incan domain errors (the function's own `Result` error type), caller boundary/conversion errors, host-capability errors, artifact version-compatibility errors, and contained panics.

### Evolution and publication posture

Caller ABI version and manifest schema version follow independent, explicit compatibility rules. Within Oven's own build graph, an incompatible compiler, runtime, target, or profile combination must produce an explicit, diagnosable bake failure — Oven refuses to materialize the caller artifact rather than silently linking a mismatched pair — never a silent partial success discovered later at runtime.

Internal, prewarmed SDK providers may consume private implementation metadata ahead of this RFC's full caller contract while it is still maturing. Publication to `incan.pub` (RFC 034, RFC 079) — Incan's own registry, not Cargo or crates.io — must wait for this identity, type/linkage, and diagnostics contract to be in place; it must not publish against an unversioned or provisional shape.

### Compatibility and migration

This RFC is additive but reframes older default-path generated-crate consumption as transitional. Existing direct default-path generated-crate consumers may continue while that path remains available, but it must be documented as a lower-level implementation-artifact path rather than the recommended Rust-hosted integration path.

Once caller artifacts exist, docs should steer Rust application authors toward caller APIs and reserve default-path backend artifacts for debugging, compiler tests, inspection, or advanced toolchain integration. Migration reporting must classify direct default-path generated-crate use as supported through the caller facet, temporarily retained for migration/debugging, or unsupported with an explicit diagnostic.

## Alternatives considered

- **Design the caller contract around external Cargo/crates.io consumption** — Rejected. The goal is self-hosted, Oven-orchestrated interop within Oven's own build graph, starting with Incan's own compiler repo, with the explicit 1.0 direction of divorcing the toolchain from Cargo entirely (#975). Shaping the caller contract around a foreign build system's resolver and publication model would compromise that goal for a consumer this design does not target. A Cargo-consumable bridge is not a future phase of this RFC; it would be a separate proposal, built as a distinct projection on top rather than a change to this contract.
- **Tell Rust callers to depend on the default-path generated crate directly** — Rejected because it makes default-path implementation internals the compatibility path. A caller needs a stable caller ABI contract, distinct from whatever the ordinary (non-caller) build path happens to emit.
- **Use a dynamic plugin or C ABI boundary** — Rejected for this RFC because the Rust-host caller projection carrier already compiles to genuine Rust through direct `rustc` (RFC 119), within the same Oven build graph as its caller, and should get normal Rust type checking rather than a new FFI boundary.
- **Use only a `build.rs` helper in the Rust-authored caller** — A development helper may hydrate or validate a selected caller artifact, but it is insufficient as the whole model because it re-introduces a build-time discovery step Oven's own build graph already owns.
- **Require a separate, hand-curated declaration for every caller-visible export** — Rejected. It duplicates the `pub def` that already exists, adds ceremony to the common case, and creates a list someone has to remember to keep in sync as usage changes. Superseded by usage-driven derivation (see "Public export profiles").
- **Eagerly build a caller wrapper for every `pub` export regardless of Rust-side usage** — Rejected as distinct from the chosen design. It would flatten every public Incan symbol into the same Rust-hosted contract whether or not anything calls it from Rust, growing the checked/versioned caller surface with dead weight and doing needless representability-checking work on every bake.
- **Scope the derived caller facet per Rust-authored calling unit rather than per Incan library** — Rejected. It would make the caller module's available functions depend on which file was asking, defeat incremental caching by generating a bespoke module per Rust unit instead of one stable module per library, and leave a brand-new caller nothing to discover from. Superseded by library-scoped union derivation plus tooling-based discovery of the full eligible surface (see "Public export profiles").
- **Expose the caller boundary as a constructed handle object (`Caller::new()` with instance methods) rather than plain free functions** — Rejected. That pattern defends against runtime drift between two independently-built, decoupled binaries — a risk that doesn't exist when Oven compiles the Rust-authored caller and the Incan-authored callee together in one bake, keyed by source digest. Free functions get the same guarantee at build time instead, without the construction ceremony, the invented fixed entry-point name, or the multi-library aliasing question a shared type name would raise.
- **Generate only untyped stringly dynamic calls** — Rejected because it gives up the main benefit of compiling Incan into the Rust ecosystem: typed, auditable integration.

## Drawbacks

- The proposal introduces another artifact boundary and support crate.
- Caller projection rules can become complex around generics, constrained newtypes, borrowed data, async values, and Rust-backed types.
- A library's realized caller surface is implicit — discoverable through tooling (LSP, `incan inspect`) or by reading Rust-side call sites, rather than a single authored list a maintainer can review directly in Incan source.
- Rust host ergonomics may pressure Incan APIs toward Rust-shaped design unless the boundary keeps projections separate from source semantics.
- Version and compatibility metadata add maintenance burden to the build pipeline.

## Implementation architecture

The recommended architecture is to extend library builds with a caller adapter generation pass that consumes checked public API metadata, semantic facts, ABI metadata, and the derived caller-usage facts (see "Public export profiles"). Oven plans that adapter, its implementation dependencies, target/profile requirements, and native linkage as one caller build-unit graph, alongside the Rust-authored caller's own build unit, then builds it by selecting the Rust-host caller projection carrier through its Rust facet's direct-`rustc` invocation (RFC 119) — this build-unit graph is the only mechanism that produces a caller artifact. The adapter calls backend-owned implementation artifacts through compiler-owned internal paths or ABI entrypoints while exposing only the caller namespace to the Rust-authored caller.

The support crate should remain narrow and versioned. Caller artifacts should declare the caller ABI version they were emitted against, inspectable for diagnostics and provenance (see "Caller identity metadata"). Metadata, source maps, and Oven receipts should be inspectable by docs, LSP, and tooling so the caller boundary can be documented and discovered without building the artifact.

If a Cargo-consumable bridge is ever proposed, it would be a separate compatibility projection layered over this same caller artifact — not a redefinition of the caller boundary itself. This RFC does not design that bridge (see Non-Goals, Alternatives considered).

Borrowed non-`Copy` callable parameters illustrate why implementation artifacts are not enough as the contract. The caller adapter must generate a compatible caller projection or reject the unsupported export before producing a caller artifact. It must not publish a boundary whose generated Rust shape is incompatible with its declared caller signature.

## Layers affected

- **Library artifact model**: library builds must be able to include caller metadata, ABI metadata, caller adapters, and semantic manifests alongside backend implementation artifacts.
- **Typechecker / API metadata**: caller export validation must prove that selected entrypoints and boundary types are representable for Rust-hosted calls.
- **IR Lowering / Emission**: backend output must preserve a stable caller namespace or ABI entrypoint and avoid making internal generated modules part of the Rust-hosted contract.
- **Stdlib / Runtime (`incan_stdlib`)**: caller-facing runtime hooks, errors, logging, telemetry, async, and capability surfaces may need caller-compatible contracts.
- **Oven / Tooling**: Oven must plan, materialize, inspect, and verify caller build units and `*.loaf` assets under RFC 117's project/asset contract, including the caller identity record and its Rust facet's link into the calling Rust-authored unit's own build unit (RFC 119); it must reject rather than publish a caller artifact with an unsupported export, and tooling must report diagnostics traceable to #986 execution receipts and the selected Oven build receipt.
- **LSP / Docs tooling**: tooling should surface caller-visible exports, Rust-facing signatures, compatibility metadata, and unsupported-boundary diagnostics.
- **Registry / Package metadata**: `incan.pub` (RFC 034, RFC 079) should advertise whether a published Incan package provides a Rust-hosted caller surface, which caller ABI version it requires, and its compiler/runtime/target compatibility range — not a Cargo/crates.io concern.

## Design decisions

- **Non-default caller policy reuses standard namespace aliasing, not new syntax:** renaming a caller-visible export is ordinary Incan import aliasing (`from policy import evaluate_policy as check_grant`, RFC 083), which already preserves semantic identity through the rename. There is no separate caller-specific declaration for this. The other originally-considered override cases dissolve into other decisions below: non-default type projection isn't supported as a per-export override — the default projection rules in "Type projection" are the only behavior — and forced blocking wrappers are usage-derived, not manually opted into (see the sync-wrapper decision below).
- **The support crate's first stable shape is boundary infrastructure only, with no host-context or initialization API:** a shared `CallerError` enum (variants for conversion failure, host-capability error, version-compatibility mismatch, and contained panic, matching "Errors" and the Reference-level explanation's error-type requirements), shared conversion helper functions/traits implementing the "Type projection" rules, and a panic-containment helper wrapping `catch_unwind` semantics. It does not own initialization or compatibility-check responsibilities, since neither applies without a constructed handle object.
- **Blocking wrappers around async exports are usage-derived, not default, opt-in, or disallowed as a manual choice:** Oven generates a blocking wrapper for an async caller-visible export only when a synchronous Rust-authored unit in the same build graph actually references it — the same usage-driven principle that derives the rest of the caller facet (see "Public export profiles"). If only async callers ever reference an async export, no blocking wrapper is generated at all.
- **Domain and boundary errors flatten into one per-export error enum, not nested `Result`s:** a caller-visible export whose Incan signature returns `Result[T, DomainError]` projects to `Result<T, PerExportCallError>` in Rust, where the generated `PerExportCallError` enum carries a domain variant wrapping `DomainError` and a boundary variant wrapping the shared `CallerError`. One `?` unwinds either; a `match` distinguishes domain from boundary when needed, avoiding the double-unwrap ceremony a nested `Result<Result<T, DomainError>, CallerError>` shape would impose.
- **Caller-projected models and enums receive exactly the derives already declared on the Incan side, mapped 1:1:** RFC 024's derive protocol already targets real Rust traits (Debug, Eq, Clone, and user-authored derives), so a caller-projected struct or enum's derives mirror whatever `@derive(...)` the Incan declaration already carries. There is no separate caller-specific derive policy; a declaration with no derives projects with none beyond what's structurally required to compile.
- **Generic Incan functions and generic model types project to genuine Rust generics, not per-usage monomorphization:** because every concrete type a generic parameter could be is already a real Rust type (per "Type projection" and RFC 121's identity principle), a caller-visible generic Incan function projects directly to a generic Rust free function, with Incan's `with` bounds mapped 1:1 to Rust trait bounds — both systems are intersection-only (RFC 025), so the mapping is direct. Oven does not track or generate a separate monomorphized function per concrete type actually used.
- **Host capability wiring is RFC 104's model, consumed directly, not a separate bridging mechanism:** RFC 097 does not define its own host-capability connection; caller-visible calls that need host capabilities get them through whatever ambient wiring or explicit-parameter mechanism RFC 104 defines (see "Host capabilities" in Reference-level explanation). Which RFC's implementation lands first is a sequencing concern for implementation planning, not a design question this RFC answers differently.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
