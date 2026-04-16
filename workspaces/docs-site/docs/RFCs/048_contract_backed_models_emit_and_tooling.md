# RFC 048: Contract-backed models, Incan emit, and interrogation tooling

- **Status:** Draft
- **Created:** 2026-03-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 005 (Rust interop)
    - RFC 015 (project lifecycle and CLI tooling)
    - RFC 034 (`incan.pub` registry)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/205
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC specifies how Incan treats canonical, machine-readable descriptions of row-shaped types as first-class structural contract metadata that can survive into built artifacts as well as feed compilation. Incan `model` is the language's human-facing universal data-shape surface, so the toolchain must be able to take that canonical structural description, materialize an equivalent nominal type during compilation, guarantee the same interrogation and reflection story as handwritten models for the covered subset, and emit formatted Incan source as the readable view of that contract. The same metadata therefore supports both authoring-time workflows and artifact-time inspection in places such as registries, marketplaces, or package browsers, without requiring original source files to be present. This RFC does not standardize mutable governance or SLA policy; those remain separate runtime concerns.

## Core model

1. **Canonical model description**: a versioned, machine-readable bundle names
one row type and provides a complete, ordered field list with Incan field types, nullability, and optional field-level metadata aligned with RFC 021 where applicable.
2. **Artifact contract**: supported build and packaging flows may persist that
canonical bundle into a built artifact so downstream tooling can inspect the contract without requiring the original `.incn` source checkout.
3. **Materialization**: at compile time, the implementation registers a
nominal type derived from that bundle so uses of the type behave like a handwritten `model` of the same shape for typing, lowering, and reflection within the guarantees of this RFC.
4. **Emit (round-trip to source)**: given the same canonical bundle, the
implementation must be able to produce valid, formatted Incan declaring a
   `model` whose source re-parses and typechecks to the same logical shape for
the covered subset.
5. **Tooling**: the same emit pipeline used for standalone output must be
available to CLI, artifact-inspection tooling, and LSP, or equivalent editor integration, through documented commands so users can preview or insert emitted source without a separate ad hoc formatter.
6. **V1 scope boundary**: this RFC defines the canonical bundle contract plus
how Incan consumes, embeds, interrogates, and emits it once available. It does not introduce a new `.incn` syntax for declaring bundles inline, and producer-specific ways to obtain bundles remain companion-spec territory.

## Motivation

Handwritten `model` types are the most readable contract Incan offers, but many systems already carry row shape in serialized or generated form, such as schemas, plan outputs, and registry artifacts. Turning that shape into a real Incan type often means duplicate maintenance or external codegen that drifts from the canonical bundle.

That readability point matters because `model` is not merely a storage-schema helper. It is Incan’s universal structural data shape across pipelines, APIs, events, and other typed boundaries. If the canonical contract exists only as machine metadata, then the language loses its best human-facing representation exactly where people need to review and trust it.

That problem becomes sharper once source code is no longer the thing being distributed. In a marketplace or registry flow, users may browse or download built artifacts rather than a source repository. If the contract only exists in handwritten source or ad hoc sidecar files, then the most important review surface disappears exactly where discovery and trust matter most.

Authors, reviewers, and consumers therefore need a human-readable view of the structural contract inside the language, not only in YAML or binary interchange, so diffs, code review, marketplace inspection, and governance workflows stay idiomatic Incan.

Finally, when a user is iterating on a pipeline-shaped or dataset-shaped surface, they should be able to materialize the output row type as Incan with minimal friction, including from the editor, to validate shape, attach tests, or align with policy, while that same canonical metadata remains suitable for later post-build interrogation.

The RFC also needs a clear lifecycle boundary. Structural schema changes require rebuilds because they affect typing and code generation. Governance and contract policy changes such as PII tags, retention windows, or SLAs often do not. Those are expected to evolve independently and should not be frozen into the artifact-level structural contract defined here.

## Goals

- Make canonical model metadata durable enough to be embedded in, or shipped
alongside, supported built artifacts so downstream tools can inspect model contracts without original source.
- Define normative guarantees for contract-backed nominal types: typing,
lowering, and interrogation must match equivalent handwritten models for the covered field subset.
- Require deterministic emit: the same canonical input and emitter version must
yield the same formatted Incan output within the rules this RFC fixes for naming and field order.
- Require tooling parity: CLI, artifact-inspection tools, and LSP, or a
documented editor protocol, must expose actions to emit or preview Incan model source from the same canonical bundle contract.
- Align field-level metadata in the canonical bundle with RFC 021 semantics
where both apply, so governance and aliases do not fork between “source” and “contract” paths.
- Keep the structural contract layer narrow and stable enough that mutable
runtime governance or SLA systems can enrich it later without redefining the artifact format.

## Non-Goals

- Specifying the full type inference algorithm for arbitrary relational
pipelines in host libraries. Companion specifications may define how a host produces a canonical bundle for a given pipeline. This RFC defines what Incan does once a bundle is available and how tooling surfaces emit.
- Reconstructing model source from arbitrary machine code, backend types, or
stripped binaries that do not carry this RFC’s canonical contract metadata.
- Standardizing runtime governance, classification, retention, ownership, or
SLA refresh semantics. Those are related but distinct concerns and may be supplied by external systems or future RFCs.
- Perfect round-trip of comments, import organization, or author-only
formatting that is not represented in the canonical bundle.
- **Runtime-only** row types with **no** compile-time registration: this RFC targets **compiled** nominal types.
- Replacing handwritten `model` as the primary authoring style.
Contract-backed materialization is an additional path.

## Guide-level explanation

Authors and platform integrators treat a canonical row description as the source of truth for identity and interchange. The Incan toolchain can use that description in two main places: during compilation, to materialize a real nominal type, and after packaging, to inspect the contract carried by a built artifact.

When someone needs to read or review the shape, they run emit through the CLI, an artifact browser, or an editor command. The tool prints or inserts formatted Incan, the same `model` surface they already use for pipeline, application, and contract authoring, instead of a parallel YAML dialect.

That means a registry or marketplace can show a user the contract for a published binary by reading embedded canonical metadata and rendering it as Incan `model` source. This is not reverse-engineering arbitrary binaries. It is a stable metadata contract that the build pipeline chose to ship.

This view is intentionally the structural contract view. A registry or governance portal may choose to enrich that rendered model with live policy data such as PII classification, retention, freshness SLAs, or ownership, but those are adjacent runtime layers, not the embedded structural bundle itself.

In v1, the guaranteed editor workflow starts from a materialized model symbol already known to the compiler. A companion producer such as InQL may later define richer contexts, such as “generate output model from selected pipeline,” but those host-specific entry points are extensions on top of this RFC’s core contract, not prerequisites for it.

## Reference-level explanation (precise rules)

### Canonical model description

- A canonical description must include a logical type name, a format or schema
version, and an ordered list of fields.
- Each field must carry a field name, an Incan type, or a documented mapping
into an Incan type before registration, and nullability consistent with Incan’s model rules.
- Field entries may carry metadata keys and values compatible with RFC 021. If
present, materialized types must expose the same metadata through the same reflection APIs as handwritten models.
- A canonical description must be complete for the fields it claims to
describe. Bundles with unknown, opaque, or host-only field types are not supported by this RFC’s materialization path and must be rejected with a diagnostic rather than partially registered.
- The bundle format may include optional provenance or lineage metadata, but
such metadata is non-semantic for type identity unless a companion specification explicitly says otherwise.
- The canonical description defined by this RFC is a structural contract only.
It describes schema shape and stable field metadata, not mutable runtime governance or operational SLA state.

### Artifact introspection contract

- Supported build or packaging flows may embed the canonical bundle into a
produced artifact or package payload in a documented location and encoding.
- Any artifact that claims support for RFC 048 introspection must carry the
canonical bundle verbatim, or in a losslessly recoverable container form.
- Artifact-level inspection must operate on embedded bundle metadata, not on
reverse-engineering emitted machine code or inferred backend layout.
- If an artifact does not carry RFC 048 model metadata, tooling must report
that the artifact is not introspectable under this contract rather than fabricating a best-effort reconstruction.
- Artifact introspection under this RFC must not be interpreted as a promise
that live governance, ownership, or SLA policy can be recovered from the artifact unless another specification explicitly embeds such runtime layers.

### Materialization

- For every supported canonical bundle in scope of a compilation, the
implementation must introduce a nominal type that:
  - participates in name resolution and generic instantiation like a declared
    `model` of the same field layout;
  - lowers with the same structural guarantees as an equivalent handwritten
    model for those fields;
  - supports the same interrogation APIs, such as field lists and
    schema-oriented accessors, as documented for handwritten models for the
    covered subset.
- If a bundle is ill-typed or incompatible with the containing program, the
implementation must emit diagnostics at compile time and must not silently drop fields.
- If a bundle’s logical type name collides with a user-declared type or
another materialized type visible in the same compilation scope, the implementation must raise a hard compile-time error. V1 does not define automatic mangling, shadowing, or hidden aliases.

### Emit (decompile to Incan)

- Emit must produce syntactically valid Incan declaring a `model` whose field
set and types correspond to the bundle.
- Emit must use the project formatter conventions so output matches `make fmt`,
or documented formatter behavior, for the same Incan version.
- Field order in emitted source must follow the canonical order in the bundle.
- Emit must not invent or rewrite semantic metadata that is not present in the
canonical bundle.
- Emit need not preserve comments or non-contract attributes. Documentation
should list what is lossy.

### Determinism

- For a fixed canonical bundle, emitter version, and formatter version,
repeated emit must yield identical output, including stable naming, spacing, and field order under the chosen rule.

### Tooling (LSP)

- Implementations must provide:
  - at least one CLI command that emits Incan source for a named
    contract-backed model available to the build;
  - at least one CLI, or equivalently documented tooling path, that emits
    Incan source from a supported built artifact carrying RFC 048 metadata;
  - at least one editor-accessible command that invokes the same emit pipeline
    for a selected or resolved materialized model symbol.
- Companion specifications may define additional editor contexts that first
compute a canonical bundle from a host surface and then feed that bundle through the same emit pipeline. Such extensions must not weaken this RFC’s determinism or diagnostics rules.
- When emit is not available for the current context, whether because of an
unsupported construct, an ambiguous symbol, an unavailable bundle, or an artifact without embedded metadata, the implementation must surface a clear diagnostic rather than fail silently.
- Commands that accept external bytes must document trust boundaries. Default
behavior should prefer in-memory bundles that are already validated by the compiler or by a trusted host.

### Interop

- Materialized types must follow the same interop rules as equivalent
handwritten models, within the limits of the represented field set.

## Design details

### Relationship to handwritten `model`

- Handwritten `model` remains the authoring default. Contract-backed types are
additional symbols that must not change the meaning of existing declarations.
- If a name collision occurs between a materialized type and a user-declared
type, the language must issue a hard error.

### Authoring surface

- This RFC introduces no new Incan source syntax for inline bundle
declarations, external bundle includes, or contract-backed `model` stubs.
- V1 bundle ingress is an implementation and tooling concern. A build, host
integration, compiler-facing API, or artifact metadata reader makes canonical bundles available to the compilation or inspection tool, and this RFC specifies the behavior after that point.
- Future RFCs may add explicit source-level declaration syntax, but such syntax
is not required to implement materialization, emit, or tooling parity under this RFC.

### Identity and versioning

- Canonical bundles should carry a logical identity, such as a hash or
versioned id, for platform use. This RFC does not mandate a particular identity scheme, but it does require that emitter and materialization do not silently ignore version fields when they affect field layout.

### Semantics

- Type identity for contract-backed models in v1 is determined by the
compilation-visible logical type name plus the accepted bundle contents under the active bundle format version.
- Optional provenance metadata may help tooling explain where a bundle came
from, but it must not change emitted field order, emitted field spelling, or reflection results for represented fields.
- Emitted Incan source is a readable projection of canonical bundle metadata.
It is not the source of truth and does not imply that the original authored source file existed or is available.
- Structural contract metadata is expected to be build-stable. Runtime
governance or SLA metadata is expected to evolve on a different lifecycle and is therefore outside the type identity and emit guarantees of this RFC.

### Interaction with existing features

- **RFC 021**: field metadata present in a bundle must surface through the same
reflection APIs and emitted syntax used for handwritten models.
- **RFC 005**: interop behavior for materialized models must match equivalent
handwritten models for the represented field set.
- **RFC 015**: any CLI exposure for emit should fit existing project
lifecycle and tooling conventions rather than inventing a disconnected formatter path.
- **RFC 034**: registry or marketplace workflows may expose emitted Incan as
the human-readable contract view for published artifacts that carry RFC 048 metadata.
- **Governance / policy layers**: runtime classifications, retention rules,
ownership, and SLAs may enrich the structural model view in higher-level products, but they are not part of this RFC’s embedded structural contract unless a future RFC says otherwise.
- **Companion producer specs**: systems such as InQL may define how canonical
bundles are derived, named, and validated before they reach Incan. Those specs are upstream of this RFC and must produce bundles that satisfy this RFC’s completeness and determinism requirements.

### Compatibility / migration

- The feature is additive for existing handwritten Incan source.
- Projects adopting contract-backed models should treat emitted Incan as a
reviewable artifact, not as a second source of truth. The canonical bundle remains authoritative for the materialized path.
- Projects that want artifact-time inspection must ensure their packaging flow
preserves RFC 048 metadata in supported build outputs.
- Because V1 rejects partial or opaque bundles, existing producer integrations
may need to tighten their schema export before they can participate in materialization.
- Teams that already maintain separate runtime governance systems do not need
to freeze those systems into RFC 048 bundles. They can continue treating artifact introspection and live policy enrichment as separate layers.

### Companion specifications

- Host libraries or pipeline surfaces that produce canonical bundles should
reference this RFC for Incan-side behavior and may define producer rules separately.

## Alternatives considered

1. **YAML (or JSON) as the only human-readable contract**
   - Familiar for infra, but **not** Incan: review and diffs **leave** the language ecosystem; duplicate mental models.

2. **External codegen only**
   - Works without language changes but forks formatting rules, drifts from
     compiler upgrades, and weakens editor integration.

3. **Reflection-only “anonymous” row types without nominal materialization**
   - Insufficient for generic APIs, interop, and stable naming in large
     codebases.

## Drawbacks

- **Two paths** to the “same” shape, handwritten versus contract-backed,
require discipline and clear diagnostics to avoid drift.
- **Artifact metadata retention** increases packaging responsibility. Published
outputs must carry canonical bundles if they want marketplace-grade introspection.
- **Layer separation** means users may encounter both an embedded structural
contract and separate live governance overlays. Products need to present that distinction clearly.
- **Deterministic emit** can surprise authors who expect pretty custom ordering
unless the rules are explicit.
- **Tooling surface area** grows (commands, context detection, error messages).

## Implementation architecture

*(Non-normative.)* A single shared “bundle -> normalized model -> formatter”
pipeline feeding materialization, CLI, artifact inspection, and LSP reduces divergence. Materialization should reuse the same normalized model shape used for declared models wherever practical. Artifact inspection should reuse that same post-bundle path after extracting canonical metadata from packaged output.

## Layers affected

- **Language surface**: v1 should not require new user-facing syntax; contract-backed models remain a tooling and artifact capability built around ordinary `model` semantics.
- **Build / packaging**: supported artifact formats that claim RFC 048 introspection must preserve canonical bundle metadata in a documented, versioned form so downstream tools can recover it losslessly.
- **Type system**: registration of contract-backed nominal types must enforce completeness, collision errors, and parity with handwritten model interrogation for represented fields.
- **Shared model pipeline**: materialization and emitted source should reuse the same normalized model shape used for handwritten `model` declarations wherever practical.
- **Formatter**: emitted `model` text **must** be idempotent under the project formatter.
- **CLI / tooling**: tooling must expose deterministic emit for named materialized model symbols and for supported artifacts carrying embedded bundle metadata, and it must document trust boundaries for any external bundle ingress.
- **LSP / tooling**: editor commands must call the shared emit path and must produce clear diagnostics when the current symbol or context cannot provide a valid bundle.
- **Registry / marketplace consumers**: downstream viewers should treat emitted Incan as a rendered projection of embedded metadata and must not assume it was reconstructed from full source.
- **Governance / runtime policy consumers**: any higher-level system that overlays live classifications or SLAs onto an RFC 048 model view should identify those overlays as runtime data distinct from the embedded structural contract.
- **Stdlib / Runtime**: reflection and metadata surfaces **must** stay consistent with RFC 021 for represented fields.

## Unresolved questions

1. **Artifact classes**: which shipped artifact classes must carry RFC 048
metadata in v1? Is introspection guaranteed for packaged Incan artifacts only, for compiled binaries directly, or for both?
2. **Model selection for embedding**: which models belong in the artifact
contract surface? All materialized models, only public or exported models, or only models explicitly marked for publication?
3. **Artifact discovery contract**: what is the v1 documented location and
encoding for embedded bundles so CLI, registry, and third-party tooling can interoperate without implementation-specific conventions?
4. **Logical identity**: is the logical type name alone sufficient for
artifact-facing identity, or should v1 require an additional stable model identifier to support registry diffing and compatibility views?
5. **Companion producer boundary**: what is the minimum producer-side contract
a host such as InQL must satisfy before its derived bundles are considered publishable artifact metadata rather than editor-only transient output?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
