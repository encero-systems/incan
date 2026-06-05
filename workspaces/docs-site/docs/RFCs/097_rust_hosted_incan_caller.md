# RFC 097: Rust-hosted Incan caller

- **Status:** Draft
- **Created:** 2026-05-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 013 (Rust crate dependencies)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 031 (Incan library system phase 1)
    - RFC 034 (`incan.pub` package registry)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 092 (interactive runtime stdlib contracts)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/569
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a Rust-hosted Incan caller model: a native Rust application should be able to depend on an Incan-authored library through ordinary Cargo mechanics and call a curated, typed Rust-facing API without reverse-engineering compiler output, manually wiring Incan runtime helpers, or treating every public Incan export as a stable Rust API.

This Draft is now framed around a Rust-facing caller ABI and Cargo-usable Incan package artifact. Generated Rust source may remain useful for inspection, debugging, migration, or an implementation backend, but it must not be the public package compatibility path. The caller boundary is the stable host API shape; it is backed by checked Incan metadata, ABI/package metadata, generated adapters where needed, and a small support crate that owns initialization, conversions, async/runtime policy, diagnostics, version checks, and panic/error containment.

## Core model

1. **Rust-hosted consumption is a first-class direction:** Incan already lets Incan code call Rust; this RFC defines the reverse direction where Rust code deliberately calls Incan-authored behavior.
2. **The Cargo-usable artifact is not generated Rust source as contract:** Rust hosts need a Cargo-native dependency shape, but the public compatibility promise is the caller ABI/package metadata, not compiler-emitted Rust internals.
3. **Implementation artifacts remain inspectable:** generated Rust, object code, IR snapshots, or other backend artifacts should be inspectable and debuggable where emitted, but they are not the host-facing semantic contract.
4. **The caller boundary is the stable host-facing shape:** Rust consumers should target caller helpers and support traits that make calls feel natural from Rust while preserving Incan semantics.
5. **The `pub` system should grow rather than be bypassed:** Rust-hosted exports should be modeled as a public export profile or facet, not as an unrelated side channel.
6. **Types cross through reusable helpers:** primitive values, models, newtypes, enums, `Result`, `Option`, collections, and Rust-backed types should cross through explicit, versioned conversion helpers that can also simplify emitter responsibilities.
7. **Runtime policy is explicit:** async execution, logger/telemetry hooks, host capabilities, panic handling, and initialization must be part of the caller contract rather than incidental generated code behavior.
8. **Cargo remains the host integration substrate:** Rust applications should use normal dependency declarations, build scripts, or Cargo-usable package artifacts instead of a bespoke binary loader.

## Motivation

Incan's current interop story is strong in one direction: Incan source imports Rust crates, wraps Rust types, and can implement Rust traits for Incan-owned types. That is necessary, but it does not answer the common embedding question: "how do I integrate Incan-generated code into my native Rust application code?"

That question exposes a deeper product direction. Incan should produce Rust-native integration artifacts without making generated Rust source the package contract. Generated Rust can still be valuable as an implementation artifact and inspection surface, but the durable promise to Rust hosts should be an explicit caller ABI, metadata, support crate contract, and Cargo-native package shape.

RFC 031 created the first library artifact foundation: an Incan library can build a semantic manifest and implementation artifacts. The missing product-level answer is the shape above those artifacts: which public exports are intended for Rust hosts, which helper types make calls feel Incan-like from Rust, which support code owns repeated boundary mechanics, and which metadata defines compatibility without exposing generated Rust internals as API.

The missing piece is not only a command. It is a boundary. A Rust application embedding Incan code needs to know which calls are stable, how values convert, how errors surface, whether async calls need a runtime, whether panics are contained, how logs and telemetry are connected, and which compiler/runtime version produced the artifact. Without that boundary, users either treat compiler output as hand-authored Rust or avoid Rust-hosted Incan entirely.

The end-state should be simple: an application team writes domain logic, policy, validation, transformations, routing decisions, or workflow steps in Incan, builds or publishes a Rust-facing package, and calls it from Rust as a typed dependency. The Rust app should remain in charge of process lifecycle, threading, deployment, and host resources. The Incan package should remain in charge of Incan language semantics and its exported behavior.

## Goals

- Define a Rust-hosted caller model for native Rust applications that call Incan-authored libraries.
- Define a stable Rust-facing caller surface backed by ABI/package metadata.
- Keep implementation artifacts inspectable without making generated Rust source the public compatibility path.
- Define how the `pub` system can express Rust-hosted public export profiles or facets.
- Define conversion requirements for primitives, collections, models, enums, newtypes, results, options, and Rust-backed values.
- Define reusable caller helpers that can reduce bespoke emitter output for common boundary shapes.
- Define initialization, version, diagnostics, panic, async, logging, telemetry, and host capability responsibilities at the caller boundary.
- Preserve Cargo-native Rust host ergonomics without requiring generated Rust source to be the concrete public artifact.
- Leave room for both local path development and published package consumption.
- Keep Rust integration Rust-shaped enough to feel natural in Rust applications without making Incan source adopt Rust's full API design model.

## Non-Goals

- This RFC does not make generated Rust source the public package compatibility path.
- This RFC does not require every implementation backend to emit Rust source.
- This RFC does not make every generated Rust module a stable public API where generated Rust is still emitted.
- This RFC does not replace `rust::` imports or Rust interop from Incan source.
- This RFC does not define a C ABI, dynamic plugin ABI, `extern "C"` boundary, or cross-language FFI story.
- This RFC does not require a Rust application to run the Incan compiler at runtime.
- This RFC does not require every Incan public export to be Rust-callable by default.
- This RFC does not define registry publication mechanics beyond compatibility with RFC 034 and RFC 079.
- This RFC does not define the full implementation of async runtime internals, host capability enforcement, or telemetry backends.
- This RFC does not guarantee that every Rust type imported through `rust::` can automatically cross back into a host Rust application without an adapter.

## Guide-level explanation

An Incan library author starts with ordinary library exports:

```incan
pub model OrderInput:
    subtotal: int
    customer_tier: str

pub model Quote:
    total: int
    discount_applied: bool

pub def quote_order(input: OrderInput) -> Result[Quote, str]:
    if input.subtotal < 0:
        return Err("subtotal must be non-negative")
    if input.customer_tier == "gold":
        return Ok(Quote(total=input.subtotal * 90 / 100, discount_applied=true))
    return Ok(Quote(total=input.subtotal, discount_applied=false))
```

The library then exposes the relevant export through a Rust-hosted public profile. The exact spelling is unresolved, but the design should enrich Incan's existing `pub` system rather than introduce an unrelated metadata escape hatch. One possible light-weight shape is:

```incan
pub caller from pricing import quote_order
```

That example is intentionally minimal. A synchronous function does not need to spell `mode = "sync"`, and a Rust caller name does not need to be repeated when it matches the Incan export name. Extra metadata should appear only when the Rust-hosted profile needs an alias, a projection choice, a blocking wrapper, or another non-default policy.

The library is built for Rust-hosted consumption:

```bash
incan build --lib --caller rust
```

That command emits or materializes a Cargo-usable caller artifact with caller metadata. A Rust application can then depend on it through Cargo:

```toml
[dependencies]
pricing_rules = { path = "../pricing_rules/target/lib" }
```

The Rust application calls the typed caller wrapper rather than internal implementation details:

```rust
use pricing_rules::caller::{Caller, OrderInput};

fn price() -> Result<(), Box<dyn std::error::Error>> {
    let caller = Caller::new()?;
    let quote = caller.quote_order(OrderInput {
        subtotal: 10_000,
        customer_tier: "gold".to_string(),
    })?;
    println!("{}", quote.total);
    Ok(())
}
```

For async entrypoints, the caller surface should make runtime requirements explicit:

```rust
use pricing_rules::caller::{AsyncCaller, OrderInput};

async fn price_async() -> Result<(), Box<dyn std::error::Error>> {
    let caller = AsyncCaller::new()?;
    let quote = caller.quote_order(OrderInput {
        subtotal: 10_000,
        customer_tier: "gold".to_string(),
    }).await?;
    println!("{}", quote.total);
    Ok(())
}
```

If an Incan export is not in the Rust-hosted public profile, Rust code must not rely on whatever implementation symbols happen to exist. The distinction is about semantic authority: caller metadata and caller APIs are stable; compiler implementation artifacts are not.

The author-facing model is:

```text
Incan library source
  -> checked public Incan API
  -> Rust-hosted public profile
  -> Rust-facing ABI/package metadata + caller artifact
  -> native Rust application
```

## Reference-level explanation

An Incan implementation that supports Rust-hosted caller artifacts must emit a Rust-facing caller boundary for exports selected by the Rust-hosted public profile.

The caller boundary must include:

- a stable Rust module or crate-level namespace for caller APIs
- typed Rust representations for caller-visible Incan input and output types
- conversion implementations or generated adapters for every caller-visible boundary type
- a caller initialization API
- version and compatibility metadata
- diagnostic metadata sufficient to map caller failures back to Incan export names and source spans where available
- error types that distinguish Incan `Err` values, Incan runtime errors, caller conversion errors, host capability errors, version mismatches, and contained panics

The caller boundary must not require Rust consumers to import arbitrary compiler-generated implementation modules as the host API. Internal generated modules may exist and should remain readable, but only the caller namespace is stable for Rust-hosted consumption.

The caller boundary should be generated or materialized as a Cargo-usable artifact. It may live in the same package as implementation artifacts or in a sibling package, but Rust consumers must not need to know the compiler's internal implementation layout.

Caller-visible Incan functions must have a representable Rust signature. The compiler must reject a Rust-hosted public export when any parameter, return value, type parameter, effect, or captured dependency cannot be represented by the caller boundary.

Caller-visible synchronous functions must expose a synchronous Rust call. Caller-visible async functions must expose an async Rust call or an explicit runtime-backed blocking call, depending on the declared caller mode. The generated API must not silently create or own a runtime in a way that surprises the host application.

Caller initialization must validate compiler/runtime compatibility before the first call. The generated artifact must expose enough metadata to identify the Incan compiler version, manifest format, caller ABI version, and generated support crate version.

Incan `Result[T, E]` crossing into Rust must map to Rust `Result<T, E>` when both `T` and `E` are representable. Boundary failures that occur outside the Incan function's declared return value must use the caller error type, not the Incan function's domain error type.

Incan `Option[T]` must map to Rust `Option<T>` when `T` is representable.

Incan `None`, `bool`, `int`, `float`, `str`, list, dict, tuple-like fixed records, models, enums, value enums, newtypes, and constrained newtypes must have deterministic caller conversion rules. Integer width, constrained storage carriers, and validation failures must not rely on unchecked Rust casts.

Incan models exposed to Rust callers should generate Rust structs with stable field names and derive surfaces appropriate for Rust host use when those derives are requested or safe by default. Wire aliases remain data-contract metadata and must not silently rename Rust struct fields unless the caller export explicitly chooses that projection.

Incan newtypes exposed to Rust callers must preserve validation semantics. Constructing a Rust-side caller newtype from an unchecked primitive must either be impossible or go through a checked constructor.

Rust-backed `rusttype` values may cross the caller boundary only when the backing Rust type is visible to the host crate and the artifact can prove that the generated caller type and host type refer to the same Rust path and compatible version. Otherwise, the export must require an explicit adapter.

Panics from generated Incan code or Rust code called by Incan must not be indistinguishable from ordinary Incan `Result` values. Implementations should contain unwinding where practical and report a caller panic error with diagnostic context. If panic containment is unavailable for a target, the caller metadata must state that policy clearly.

Host capabilities used by caller-visible Incan code must be visible through metadata. If an Incan caller export requires filesystem, network, process, environment, telemetry, clock, async runtime, or other host services, the Rust caller initialization must provide or validate those services before use when the target supports such validation.

## Design details

### Caller artifact shape

The caller artifact should be a Cargo-usable package backed by Incan-owned caller metadata and ABI metadata. A current implementation may materialize that as a generated Rust package, but the normative contract is the Cargo-usable caller artifact and its metadata, not the emitted source layout.

Conceptually, the package contains:

```text
stable caller namespace
caller metadata
ABI/package metadata
semantic manifest
Cargo metadata
implementation artifact(s)
```

The exact directory layout is not normative. The normative requirement is that Rust consumers do not need to know which files came from Incan source lowering, backend emission, support glue, or ABI materialization.

### Support crate

The Incan toolchain should provide a small Rust support crate for caller boundaries. That crate should own shared traits, error types, version checks, conversion helpers, and host context traits that would otherwise be duplicated into every generated package or embedded directly into emitter output.

The support crate should not become a large runtime framework. It should be boundary infrastructure: initialization, compatibility checks, conversions, diagnostics, panic policy, host context plumbing, and reusable call-shape helpers. Those helpers should make Rust-hosted calls feel closer to Incan's domain model while giving the emitter fewer bespoke cases to print inline.

### Public export profiles

The caller surface should be represented as an enrichment of `pub`. Plain Incan public exports remain the Incan package API. Rust-hosted public exports are a profile or facet of that public API intended to be stable for native Rust hosts.

The exact selection syntax is unresolved. Plausible shapes include `pub caller from ... import ...`, named export profiles, profile blocks in `src/lib.incn`, or carefully-scoped declaration metadata for non-default policy. The chosen shape must keep API intent visible in review, support docs/tooling extraction, and avoid making a second export system that fights `pub`.

### Type projection

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

### Errors

The generated caller API should use two layers of errors:

- domain errors produced by Incan functions that return `Result[T, E]`
- caller errors produced by the boundary itself

For a function whose Incan signature returns `Result[Quote, PricingError]`, the Rust caller may expose a nested result or a generated convenience type, but it must preserve the distinction between `PricingError` and boundary failures such as conversion failure, missing host capability, incompatible artifact version, or contained panic.

### Async and runtime policy

Async caller exports must not assume that the caller package owns the process runtime. The Rust host should either provide an async context by calling async functions or explicitly opt into a blocking wrapper that documents runtime behavior.

Caller metadata should state whether an export is synchronous, async, blocking, or requires host-provided runtime services. This should compose with RFC 092 target and host capability metadata when those contracts mature.

### Diagnostics and observability

Caller failures should identify the caller export name, the Incan function name, and source-span metadata when available. Logging and telemetry should route through host-provided hooks where configured, rather than unconditionally initializing global logging from the caller package.

### Compatibility and migration

This RFC is additive but reframes older generated-crate consumption as transitional. Existing `incan build --lib` consumers may continue depending directly on generated crates while that path exists, but that should be documented as a lower-level implementation-artifact path rather than the recommended Rust-hosted integration path.

Once caller artifacts exist, docs should steer Rust application authors toward caller APIs and reserve backend artifacts for debugging, compiler tests, inspection, or advanced toolchain integration.

## Alternatives considered

- **Tell Rust users to depend on the generated crate directly** — Rejected because it makes generated Rust internals the compatibility path. Rust hosts need a stable caller ABI/package contract even if the current backend happens to emit Rust.
- **Use a dynamic plugin or C ABI boundary** — Rejected for this RFC because Incan already emits Rust, and Rust-hosted applications should get normal Cargo type checking, optimization, and dependency resolution.
- **Use only a `build.rs` helper in the Rust application** — Useful for local development, but insufficient as the whole model because published artifacts and registry workflows should not require every consumer to run the Incan compiler.
- **Make every public Incan export Rust-callable automatically** — Rejected as the default because Incan's `pub` system should be enriched with host-facing profiles instead of flattening every public Incan symbol into the same Rust-hosted contract.
- **Generate only untyped stringly dynamic calls** — Rejected because it gives up the main benefit of compiling Incan into the Rust ecosystem: typed, auditable integration.

## Drawbacks

- The proposal introduces another artifact boundary and support crate.
- Caller projection rules can become complex around generics, constrained newtypes, borrowed data, async values, and Rust-backed types.
- Opt-in caller exports add ceremony for small libraries.
- Rust host ergonomics may pressure Incan APIs toward Rust-shaped design unless the boundary keeps projections separate from source semantics.
- Version and compatibility metadata add maintenance burden to the build pipeline.

## Implementation architecture

The recommended architecture is to extend library builds with a caller adapter generation pass that consumes checked public API metadata, semantic facts, ABI metadata, and caller export declarations. The adapter should call into backend-owned implementation artifacts through compiler-owned internal paths or ABI entrypoints, while exposing only the caller namespace to host Rust code.

The support crate should remain narrow and versioned. Caller artifacts should declare the caller ABI version they were emitted against and validate it at initialization. Metadata should be inspectable by docs, LSP, and registry tooling so Rust-hosted integration can be documented and discovered without building the package.

Local development may later add a build-script helper that invokes the Incan compiler from a Rust workspace, but that helper should produce the same caller boundary as a prebuilt or published package.

Current package-facing characterization shows why generated implementation artifacts are not enough as the public contract. Ordinary `incan build --lib` artifacts can already expose owned scalar callable parameters through package exports, but borrowed non-`Copy` callable parameters are not yet consumable across a `pub::` package boundary. A producer export such as `Callable[Payload, None]` currently emits a Rust signature shaped like `fn(&Payload) -> ()`, while a downstream Incan consumer observer still emits `fn(Payload)`, causing Cargo type checking to fail. The caller adapter work must either generate a compatible borrowed wrapper for that boundary or reject/document the unsupported export before producing a broken consumer build.

## Layers affected

- **Library artifact model**: library builds must be able to include caller metadata, ABI/package metadata, caller adapters, and semantic manifests alongside backend implementation artifacts.
- **Typechecker / API metadata**: caller export validation must prove that selected entrypoints and boundary types are representable for Rust-hosted calls.
- **IR Lowering / Emission**: backend output must preserve a stable caller namespace or ABI entrypoint and avoid making internal generated modules part of the Rust-hosted contract.
- **Stdlib / Runtime (`incan_stdlib`)**: host-facing runtime hooks, errors, logging, telemetry, async, and capability surfaces may need caller-compatible contracts.
- **CLI / Tooling**: build commands should expose a caller artifact mode and diagnostics for unsupported caller exports.
- **LSP / Docs tooling**: tooling should surface caller-visible exports, Rust-facing signatures, compatibility metadata, and unsupported-boundary diagnostics.
- **Registry / Package metadata**: published packages should advertise whether they provide a Rust-hosted caller surface and which caller ABI version they require.

## Unresolved questions

- What is the exact source syntax for marking caller-visible exports?
- Should caller adapters live in the same Cargo package as the implementation artifact or in a sibling package?
- What is the first stable shape of the Rust support crate API?
- Should synchronous wrappers around async Incan exports be generated by default, opt-in only, or disallowed?
- How should nested domain results and boundary errors be represented ergonomically in Rust signatures?
- Which derives should caller-projected models and enums receive by default?
- How should generic Incan functions and generic model types be projected into Rust caller APIs?
- How should host capability metadata from RFC 092 connect to caller initialization before RFC 092 is fully implemented?
- Should local Rust workspaces use a build-script helper first, or should the first implementation require explicit `incan build --lib --caller rust` before Cargo builds the host application?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
