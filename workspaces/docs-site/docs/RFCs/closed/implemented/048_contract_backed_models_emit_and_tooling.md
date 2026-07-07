# RFC 048: Checked contract metadata, Incan emit, and interrogation tooling

- **Status:** Implemented
- **Created:** 2026-03-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 005 (Rust interop)
    - RFC 015 (project lifecycle and CLI tooling)
    - RFC 031 (library system phase 1)
    - RFC 034 (`incan.pub` registry)
    - RFC 082 (checked API documentation generation)
- **Issue:** https://github.com/encero-systems/incan/issues/205, https://github.com/encero-systems/incan/issues/438
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3.0-dev.20

## Summary

This RFC defines a shared checked metadata inspection layer for Incan contracts. It covers two related surfaces: canonical, machine-readable descriptions of row-shaped types that can materialize as nominal `model` types, and checked public API metadata extracted from source declarations for documentation and downstream tooling. Incan `model` is the language's human-facing universal data-shape surface, so the toolchain must be able to take canonical structural descriptions, materialize equivalent nominal types during compilation, guarantee the same interrogation and reflection story as handwritten models for the covered subset, and emit formatted Incan source as the readable view of those model contracts. The same inspection architecture must also expose checked package API structure such as public declarations, signatures, decorators, aliases, docstrings, field metadata, safe constant metadata, and stable anchors as machine-readable JSON. This RFC does not standardize mutable governance or SLA policy, hosted documentation generation, or product-specific renderers; those remain separate concerns.

## Core model

1. **Checked metadata families**: RFC 048 metadata includes canonical model bundles and checked API metadata documents. Both are compiler-validated, versioned, machine-readable descriptions of Incan-facing contracts.
2. **Canonical model description**: a versioned bundle names one row type and provides a complete, ordered field list with Incan field types, nullability, and optional field-level metadata aligned with RFC 021 where applicable.
3. **Checked API metadata**: a versioned document describes checked public source structure for documentation and tooling: package/module paths, public declarations, signatures, decorators with resolved arguments, aliases with resolved targets, parsed docstrings, relevant safe constants or metadata values, field docs/metadata, and stable anchors.
4. **Artifact contract**: supported build and packaging flows may persist RFC 048 metadata into built artifacts so downstream tooling can inspect public contracts without requiring the original `.incn` source checkout.
5. **Materialization**: at compile time, the implementation registers a nominal type derived from a supported model bundle so uses of the type behave like a handwritten `model` of the same shape for typing, lowering, and reflection within the guarantees of this RFC.
6. **Projection**: model bundles must be projectable to valid formatted Incan `model` source. Checked API metadata must be projectable to a stable JSON document that renderers and downstream tools can consume. RFC 082 defines checked API documentation generation on top of this metadata, but JSON remains the stable machine-readable layer owned by this RFC.
7. **Tooling**: the same checked metadata pipeline must be available to CLI, artifact-inspection tooling, and LSP, or equivalent editor integration, through documented commands so users can preview, emit, inspect, or render contracts without separate ad hoc extractors.
8. **Scope boundary**: this RFC defines the checked metadata contract plus how Incan consumes, embeds, interrogates, and projects it once available. It does not introduce a new `.incn` syntax for declaring bundles inline, does not make Incan own every documentation renderer, and leaves producer-specific ways to obtain bundles in companion-spec territory.

## Motivation

Handwritten `model` types are the most readable contract Incan offers, but many systems already carry row shape in serialized or generated form, such as schemas, plan outputs, and registry artifacts. Turning that shape into a real Incan type often means duplicate maintenance or external codegen that drifts from the canonical bundle.

That readability point matters because `model` is not merely a storage-schema helper. It is Incan’s universal structural data shape across pipelines, APIs, events, and other typed boundaries. If the canonical contract exists only as machine metadata, then the language loses its best human-facing representation exactly where people need to review and trust it.

That problem becomes sharper once source code is no longer the thing being distributed. In a marketplace or registry flow, users may browse or download built artifacts rather than a source repository. If the contract only exists in handwritten source or ad hoc sidecar files, then the most important review surface disappears exactly where discovery and trust matter most.

Authors, reviewers, and consumers therefore need a human-readable view of the structural contract inside the language, not only in YAML or binary interchange, so diffs, code review, marketplace inspection, and governance workflows stay idiomatic Incan.

The same gap exists for public APIs. Incan already has public declarations, typed signatures, decorators, docstrings, exports, aliases, constants, and library manifests, but without a first-class checked metadata surface, downstream tools either scrape source text, inspect generated Rust, or duplicate compiler logic. That is fragile for ordinary documentation tooling and worse for higher-level query, catalog, or package-browser consumers, which need checked answers about registered functions, decorator arguments, aliases, and parsed docstrings without baking product-specific catalog concepts into the compiler.

Finally, when a user is iterating on a pipeline-shaped or dataset-shaped surface, they should be able to materialize the output row type as Incan with minimal friction, including from the editor, to validate shape, attach tests, or align with policy, while that same canonical metadata remains suitable for later post-build interrogation.

The RFC also needs a clear lifecycle boundary. Structural schema changes require rebuilds because they affect typing and code generation. Governance and contract policy changes such as PII tags, retention windows, or SLAs often do not. Those are expected to evolve independently and should not be frozen into the artifact-level structural contract defined here.

## Goals

- Make canonical model metadata durable enough to be embedded in, or shipped alongside, supported built artifacts so downstream tools can inspect model contracts without original source.
- Define normative guarantees for contract-backed nominal types: typing, lowering, and interrogation must match equivalent handwritten models for the covered field subset.
- Require deterministic emit: the same canonical input and emitter version must yield the same formatted Incan output within the rules this RFC fixes for naming and field order.
- Require tooling parity: CLI, artifact-inspection tools, and LSP, or a documented editor protocol, must expose actions to emit or preview Incan model source from the same canonical bundle contract.
- Define a stable checked API metadata JSON output for public API documentation and inspection tooling.
- Include enough checked structure for downstream renderers to describe public declarations, signatures, decorators with resolved arguments, aliases, relevant safe constants or metadata values, parsed docstrings, field metadata, and stable source anchors.
- Require mechanically checkable documentation validation, so stale docstring parameter, return, or field sections produce actionable diagnostics rather than silently drifting from checked source.
- Align field-level metadata in the canonical bundle with RFC 021 semantics where both apply, so governance and aliases do not fork between “source” and “contract” paths.
- Keep the structural contract layer narrow and stable enough that mutable runtime governance or SLA systems can enrich it later without redefining the artifact format.

## Non-Goals

- Specifying the full type inference algorithm for arbitrary relational pipelines in host libraries. Companion specifications may define how a host produces a canonical bundle for a given pipeline. This RFC defines what Incan does once a bundle is available and how tooling surfaces emit.
- Reconstructing model source from arbitrary machine code, backend types, or stripped binaries that do not carry this RFC’s canonical contract metadata.
- Standardizing runtime governance, classification, retention, ownership, or SLA refresh semantics. Those are related but distinct concerns and may be supplied by external systems or future RFCs.
- Owning full hosted documentation-site generation in the compiler. The compiler/tooling responsibility is stable checked metadata extraction; RFC 082 owns checked API documentation generation, while hosted sites and product renderers remain downstream concerns.
- Baking query-language-specific catalog concepts into Incan. Downstream query tools may consume RFC 048 metadata, but their catalog models are outside this RFC.
- Copying rustdoc, Griffe, or another documentation tool wholesale. Prior art motivates the direction, but Incan needs its own checked metadata contract.
- Executing user code to discover documentation metadata. Extraction must use parsed, resolved, typechecked, and compiler-safe constant metadata only.
- Perfect round-trip of comments, import organization, or author-only formatting that is not represented in the canonical bundle.
- **Runtime-only** row types with **no** compile-time registration: this RFC targets **compiled** nominal types.
- Replacing handwritten `model` as the primary authoring style. Contract-backed materialization is an additional path.

## Guide-level explanation

Authors and platform integrators treat a canonical row description as the source of truth for identity and interchange. The Incan toolchain can use that description in two main places: during compilation, to materialize a real nominal type, and after packaging, to inspect the contract carried by a built artifact.

When someone needs to read or review the shape, they run emit through the CLI, an artifact browser, or an editor command. The tool prints or inserts formatted Incan, the same `model` surface they already use for pipeline, application, and contract authoring, instead of a parallel YAML dialect.

A producer might provide a canonical bundle for a public order-summary row. The emitted Incan view is ordinary model source:

```incan
model OrderSummary:
    order_id [description="Stable order identifier"] as "orderId": str
    customer_id [description="Stable customer identifier"] as "customerId": str
    total_cents: int
    coupon_code: Option[str]
```

That source is reviewable, can be pasted into tests or documentation, and should format the same way as a handwritten `model`. It is still a projection of the canonical bundle: changing the emitted source alone does not update the producer's contract unless the producer deliberately accepts that source as input through some companion workflow.

That means a registry or marketplace can show a user the contract for a published package artifact by reading embedded canonical metadata and rendering it as Incan `model` source. This is not reverse-engineering arbitrary binaries. It is a stable metadata contract that the build pipeline chose to ship.

The same inspection path should let documentation tooling ask checked questions about public API declarations. For example, a package might define a reusable aggregate function with a docstring, aliases, and a decorator argument that resolves to a checked constant:

```incan
const AVG = "avg"

@aggregate(AVG)
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        values: Input values.

    Returns:
        Mean value.
    """
    ...
```

The checked API metadata JSON should describe `avg` as a public function, include its signature, identify the `aggregate` decorator, record that `AVG` resolves to the checked constant value `"avg"` when that value is safe to expose, include aliases that resolve to `avg`, and carry the parsed docstring sections. A downstream renderer can turn that JSON into Markdown, HTML, a query-language function catalog, or a package-browser card without reimplementing Incan name resolution or scraping generated Rust.

This view is intentionally the structural contract view. A registry or governance portal may choose to enrich that rendered model with live policy data such as PII classification, retention, freshness SLAs, or ownership, but those are adjacent runtime layers, not the embedded structural bundle itself.

The guaranteed editor workflow starts from a materialized model symbol already known to the compiler. A companion producer such as a query or pipeline tool may later define richer contexts, such as “generate output model from selected pipeline,” but those host-specific entry points are extensions on top of this RFC’s core contract, not prerequisites for it.

## Reference-level explanation (precise rules)

### Canonical model description

- A canonical description must include a logical type name, a format or schema version, and an ordered list of fields.
- A publishable canonical description must also include an artifact-facing stable model id. The logical type name is the Incan spelling; the stable model id is the cross-version handle used by artifacts, registries, and compatibility tooling.
- Each field must carry a field name, an Incan type, or a documented mapping into an Incan type before registration, and nullability consistent with Incan’s model rules.
- Field entries may carry metadata keys and values compatible with RFC 021. If present, materialized types must expose the same metadata through the same reflection APIs as handwritten models.
- A canonical description must be complete for the fields it claims to describe. Bundles with unknown, opaque, or host-only field types are not supported by this RFC’s materialization path and must be rejected with a diagnostic rather than partially registered.
- The bundle format may include optional provenance or lineage metadata, but such metadata is non-semantic for type identity unless a companion specification explicitly says otherwise.
- The canonical description defined by this RFC is a structural contract only. It describes schema shape and stable field metadata, not mutable runtime governance or operational SLA state.

### Checked API metadata description

- A checked API metadata document must be produced from parsed and typechecked Incan semantics, not from generated Rust and not from source-text scraping alone.
- The document must include a metadata schema version, package identity when available, module paths, and stable declaration anchors for public API items.
- The document must include public functions and methods, public models/classes/traits/enums/newtypes/type aliases, exported constants or statics that are relevant to exposed metadata, and public aliases with their resolved targets.
- Function and method entries must include checked signatures: parameter names and types, return type, type parameters, and bounds where available.
- Type entries must include checked field/member information relevant to documentation and inspection. Model fields must include RFC 021-compatible field metadata and field documentation where available.
- Decorator entries must include the resolved decorator path or identity, and must include checked argument structure where the argument can be represented without executing user code.
- Constants and metadata values exposed through this document must be limited to compiler-known safe values such as literals, checked constant expressions, and structured metadata already accepted by the frontend. The extractor must not run arbitrary code to compute documentation output.
- Docstrings must be parsed according to Incan documentation standards. Mechanically checkable sections, such as documented parameters, returns, and fields, must be validated against checked signatures or fields.
- If a docstring claims parameters, returns, fields, aliases, or decorator metadata that contradict checked source structure, the command must report actionable diagnostics. Strict extraction modes should fail on those diagnostics; non-strict renderers may still consume partial metadata when explicitly configured to do so.

### Artifact introspection contract

- Artifact introspection is required for Incan library and package artifacts that claim RFC 048 support. It is not required for arbitrary standalone compiled binaries unless another RFC defines how those binaries embed the same metadata contract.
- Supported build or packaging flows may embed RFC 048 metadata into a produced artifact or package payload in a documented location and encoding.
- For package-style artifacts, the `.incnlib` manifest is the discovery document. RFC 048 model bundles and checked API metadata must be carried inside that manifest, either directly as typed entries or through manifest entries that losslessly reference embedded payloads. A second sidecar file must not be required for ordinary package introspection.
- Publishable artifact metadata must include public or exported contract-backed models plus models explicitly selected for publication by a producer or build integration. Private compiler-temporary materialized models must not be published by default.
- Any artifact that claims support for RFC 048 introspection must carry the relevant metadata verbatim, or in a losslessly recoverable container form.
- Artifact-level inspection must operate on embedded RFC 048 metadata, not on reverse-engineering emitted machine code or inferred backend layout.
- If an artifact does not carry RFC 048 metadata, tooling must report that the artifact is not introspectable under this contract rather than fabricating a best-effort reconstruction.
- Artifact introspection under this RFC must not be interpreted as a promise that live governance, ownership, or SLA policy can be recovered from the artifact unless another specification explicitly embeds such runtime layers.

### Materialization

- For every supported canonical bundle in scope of a compilation, the implementation must introduce a nominal type that:
  - participates in name resolution and generic instantiation like a declared `model` of the same field layout;
  - lowers with the same structural guarantees as an equivalent handwritten `model` for those fields;
  - supports the same interrogation APIs, such as field lists and schema-oriented accessors, as documented for handwritten models for the covered subset.
- If a bundle is ill-typed or incompatible with the containing program, the implementation must emit diagnostics at compile time and must not silently drop fields.
- If a bundle’s logical type name collides with a user-declared type or another materialized type visible in the same compilation scope, the implementation must raise a hard compile-time error. This RFC does not define automatic mangling, shadowing, or hidden aliases.
- A bundle without an artifact-facing stable model id may still be materialized for compiler-validated transient workflows, but it is not publishable artifact metadata under this RFC.

### Projection and emit

- Emit must produce syntactically valid Incan declaring a `model` whose field set and types correspond to the bundle.
- Emit must use the project formatter conventions so output matches `make fmt`, or documented formatter behavior, for the same Incan version.
- Field order in emitted source must follow the canonical order in the bundle.
- Emit must not invent or rewrite semantic metadata that is not present in the canonical bundle.
- Emit need not preserve comments or non-contract attributes. Documentation should list what is lossy.
- Checked API metadata extraction must produce stable JSON. RFC 082 may define Markdown, local HTML, or package docs artifacts over that metadata, but those formats are renderers over the checked metadata model rather than the normative metadata contract.
- API metadata projection must preserve Incan source concepts: public/exported declaration identity, checked signatures, docstrings, decorators, aliases, and safe metadata values. It must not substitute generated Rust names or lowering artifacts as the primary API contract.

### Determinism

- For a fixed canonical bundle, emitter version, and formatter version, repeated emit must yield identical output, including stable naming, spacing, and field order under the chosen rule.

### Tooling (LSP)

- Implementations must provide:
  - at least one CLI command that emits Incan source for a named contract-backed model available to the build;
  - at least one CLI, or equivalently documented tooling path, that emits Incan source from a supported built artifact carrying RFC 048 metadata;
  - at least one CLI command or documented tooling path that extracts checked public API metadata as JSON from a package or source root after parsing and typechecking;
  - at least one editor-accessible command that invokes the same emit pipeline for a selected or resolved materialized model symbol.
- Implementations should expose editor-accessible checked metadata for selected public declarations where practical, so LSP clients can preview docs, decorators, aliases, source anchors, and signatures through the same model as CLI extraction.
- Companion specifications may define additional editor contexts that first compute a canonical bundle from a host surface and then feed that bundle through the same emit pipeline. Such extensions must not weaken this RFC’s determinism or diagnostics rules.
- When emit is not available for the current context, whether because of an unsupported construct, an ambiguous symbol, an unavailable bundle, or an artifact without embedded metadata, the implementation must surface a clear diagnostic rather than fail silently.
- Commands that accept external bytes must document trust boundaries. Default behavior should prefer in-memory bundles that are already validated by the compiler or by a trusted host.

### Interop

- Materialized types must follow the same interop rules as equivalent handwritten models, within the limits of the represented field set.

## Design details

### Relationship to handwritten `model`

- Handwritten `model` remains the authoring default. Contract-backed types are additional symbols that must not change the meaning of existing declarations.
- If a name collision occurs between a materialized type and a user-declared type, the language must issue a hard error.

### Authoring surface

- This RFC introduces no new Incan source syntax for inline bundle declarations, external bundle includes, or contract-backed `model` stubs.
- This RFC does not introduce new documentation syntax beyond parsing existing Incan docstrings and field metadata according to documented standards.
- Bundle ingress is an implementation and tooling concern. A build, host integration, compiler-facing API, or artifact metadata reader makes canonical bundles available to the compilation or inspection tool, and this RFC specifies the behavior after that point.
- Future RFCs may add explicit source-level declaration syntax, but such syntax is not required to implement materialization, emit, or tooling parity under this RFC.

### Identity and versioning

- Canonical bundles must carry a logical type name and a format version.
- Publishable canonical bundles must carry a stable model id that remains the same across compatible revisions of the same conceptual model. The stable model id may be producer-supplied or derived from package identity plus exported model path, but the derivation must be documented and deterministic.
- Artifact-facing compatibility tooling should use the stable model id to compare the same model across package versions, and should use bundle content or field-level metadata to determine the actual shape delta.
- Checked API metadata documents must carry their own schema version and should provide stable anchors derived from package/module/declaration identity rather than line numbers alone.
- Emitter and materialization must not silently ignore version fields when they affect field layout.

### Semantics

- Type identity for contract-backed models is determined by the compilation-visible logical type name plus the accepted bundle contents under the active bundle format version.
- Optional provenance metadata may help tooling explain where a bundle came from, but it must not change emitted field order, emitted field spelling, or reflection results for represented fields.
- Emitted Incan source is a readable projection of canonical bundle metadata. It is not the source of truth and does not imply that the original authored source file existed or is available.
- Checked API metadata is an inspection contract, not a runtime reflection promise. Runtime APIs such as `__fields__()` may expose a subset of the same facts, but RFC 048 extraction is allowed to include compile-time-only data such as docstrings, source anchors, resolved decorators, and checked constants.
- Structural contract metadata is expected to be build-stable. Runtime governance or SLA metadata is expected to evolve on a different lifecycle and is therefore outside the type identity and emit guarantees of this RFC.

### Interaction with existing features

- **RFC 021**: field metadata present in a bundle must surface through the same reflection APIs and emitted syntax used for handwritten models.
- **RFC 005**: interop behavior for materialized models must match equivalent handwritten models for the represented field set.
- **RFC 015**: any CLI exposure for emit should fit existing project lifecycle and tooling conventions rather than inventing a disconnected formatter path.
- **RFC 031**: package-style artifact inspection should extend the `.incnlib` manifest contract rather than inventing a separate package sidecar for ordinary metadata discovery.
- **RFC 034**: registry or marketplace workflows may expose emitted Incan as the human-readable contract view for published artifacts that carry RFC 048 metadata.
- **RFC 082**: checked API documentation generation consumes RFC 048 metadata. The compiler should own correctness of extracted checked facts and diagnostics, while RFC 082 owns documentation output contracts and hosted documentation remains downstream.
- **Governance / policy layers**: runtime classifications, retention rules, ownership, and SLAs may enrich the structural model view in higher-level products, but they are not part of this RFC’s embedded structural contract unless a future RFC says otherwise.
- **Companion producer specs**: query, catalog, or pipeline systems may define how canonical bundles are derived, named, rendered, and validated before they reach or after they leave Incan. Those specs are upstream or downstream of this RFC and must consume/produce metadata that satisfies this RFC’s completeness and determinism requirements when they participate in RFC 048 tooling.

### Compatibility / migration

- The feature is additive for existing handwritten Incan source.
- Projects adopting contract-backed models should treat emitted Incan as a reviewable artifact, not as a second source of truth. The canonical bundle remains authoritative for the materialized path.
- Projects that want artifact-time inspection must ensure their packaging flow preserves RFC 048 metadata in supported build outputs.
- Projects adopting checked API metadata extraction should treat generated docs as derived output from the JSON inspection model, not as a replacement for checked source.
- Because this RFC rejects partial or opaque bundles, existing producer integrations may need to tighten their schema export before they can participate in materialization.
- Teams that already maintain separate runtime governance systems do not need to freeze those systems into RFC 048 bundles. They can continue treating artifact introspection and live policy enrichment as separate layers.

### Companion specifications

- Host libraries or pipeline surfaces that produce canonical bundles should reference this RFC for Incan-side behavior and may define producer rules separately.
- Documentation renderers and catalogs that consume checked API metadata should reference this RFC for the extraction contract and RFC 082 for checked documentation generation behavior.

## Alternatives considered

1. **YAML (or JSON) as the only human-readable contract**
   - Familiar for infra, but **not** Incan: review and diffs **leave** the language ecosystem; duplicate mental models.

2. **External codegen only**
   - Works without language changes but forks formatting rules, drifts from compiler upgrades, and weakens editor integration.

3. **Reflection-only “anonymous” row types without nominal materialization**
   - Insufficient for generic APIs, interop, and stable naming in large codebases.

4. **Docs renderer as the compiler-owned contract**
   - Rejected because Markdown, HTML, package cards, and query/catalog views are presentation concerns. The stable compiler/tooling contract is checked metadata extraction.

5. **Library manifests only**
   - Rejected because manifests are necessary for dependency/import semantics, but public API documentation also needs docstrings, decorators, aliases, safe metadata values, and source anchors that should not be inferred by downstream source scraping.

## Drawbacks

- **Two paths** to the “same” shape, handwritten versus contract-backed, require discipline and clear diagnostics to avoid drift.
- **Artifact metadata retention** increases packaging responsibility. Published outputs must carry canonical bundles if they want marketplace-grade introspection.
- **API metadata stability** adds compatibility pressure. Once downstream docs tooling consumes a JSON shape, schema evolution needs versioning and migration guidance.
- **Docstring diagnostics** can block documentation workflows if the checker is too strict. Implementations need a clear strictness policy so stale docs are caught without making exploratory work painful.
- **Layer separation** means users may encounter both an embedded structural contract and separate live governance overlays. Products need to present that distinction clearly.
- **Deterministic emit** can surprise authors who expect pretty custom ordering unless the rules are explicit.
- **Tooling surface area** grows (commands, context detection, error messages).

## Implementation architecture

*(Non-normative.)* A single shared checked-metadata pipeline should feed materialization, model emit, API JSON extraction, CLI, artifact inspection, and LSP. Model bundle ingestion can normalize through a “bundle -> normalized model -> formatter/materializer” path, while source inspection can normalize through a “checked program -> public API metadata -> JSON/renderers” path. Both paths should share declaration/type/metadata vocabulary wherever practical so docs tooling, artifact browsers, and editor features do not diverge.

## Layers affected

- **Language surface**: this RFC should not require new user-facing syntax; contract-backed models remain a tooling and artifact capability built around ordinary `model` semantics.
- **Build / packaging**: supported artifact formats that claim RFC 048 introspection must preserve RFC 048 metadata in a documented, versioned form so downstream tools can recover it losslessly.
- **Type system**: registration of contract-backed nominal types must enforce completeness, collision errors, and parity with handwritten model interrogation for represented fields.
- **Shared model pipeline**: materialization and emitted source should reuse the same normalized model shape used for handwritten `model` declarations wherever practical.
- **Checked API metadata extractor**: tooling must extract public declarations, signatures, decorators, aliases, safe constants/metadata values, docstrings, field metadata, and stable anchors from checked Incan semantics.
- **Docstring validation**: documentation-aware checks must compare mechanically checkable docstring sections against checked signatures and fields and emit actionable diagnostics on drift.
- **Formatter**: emitted `model` text **must** be idempotent under the project formatter.
- **CLI / tooling**: tooling must expose deterministic emit for named materialized model symbols, supported artifacts carrying embedded RFC 048 metadata, and checked public API metadata JSON extraction; it must document trust boundaries for any external bundle ingress.
- **LSP / tooling**: editor commands must call the shared emit/inspection paths and must produce clear diagnostics when the current symbol, declaration, or context cannot provide valid metadata.
- **Registry / marketplace consumers**: downstream viewers should treat emitted Incan and rendered docs as projections of embedded metadata and must not assume they were reconstructed from full source.
- **Documentation consumers**: docs generators and downstream catalogs should consume the JSON inspection model rather than scraping Incan source or generated Rust.
- **Governance / runtime policy consumers**: any higher-level system that overlays live classifications or SLAs onto an RFC 048 model view should identify those overlays as runtime data distinct from the embedded structural contract.
- **Stdlib / Runtime**: reflection and metadata surfaces **must** stay consistent with RFC 021 for represented fields.

## Implementation Plan

### Phase 1: Metadata contract and shared vocabulary

- Define the RFC 048 metadata schema families for canonical model bundles and checked API metadata documents, including schema versions, package identity, stable anchors, safe value representation, and diagnostics.
- Establish one compiler-facing normalized metadata vocabulary that can be serialized to JSON, persisted into package artifacts, and consumed by CLI and editor tooling.
- Keep bundle ingress out of the user-facing language syntax; host integrations, package readers, and compiler-facing APIs may supply validated bundles.

### Phase 2: Canonical model validation and materialization

- Validate canonical model bundles for logical type name, stable model id where publishable, format version, complete ordered fields, Incan type mappings, nullability, and RFC 021-compatible field metadata.
- Register accepted bundles as nominal model types in checked program state, with collision diagnostics and parity with handwritten model interrogation for represented fields.
- Reuse existing model lowering, interop, and reflection guarantees wherever practical so materialized models do not fork handwritten model behavior.

### Phase 3: Artifact embedding and inspection

- Extend supported package/library artifact metadata so `.incnlib` is the ordinary discovery document for RFC 048 model bundles and checked API metadata.
- Preserve publishable public/exported contract-backed models and explicitly selected producer models in a lossless artifact representation.
- Add artifact inspection behavior that reports missing RFC 048 metadata clearly instead of attempting best-effort reconstruction from generated code.

### Phase 4: Deterministic model emit

- Implement deterministic Incan model emit for validated canonical bundles, named materialized model symbols, and supported artifacts carrying RFC 048 metadata.
- Route emitted source through the project formatter path and preserve canonical field order, metadata syntax, logical names, aliases, and type spelling required by this RFC.
- Document lossy projection behavior for comments, imports, author-only formatting, and metadata that is not present in the canonical bundle.

### Phase 5: Checked API metadata extraction

- Extract checked public API structure from parsed and typechecked Incan semantics, including public declarations, signatures, models/classes/traits/enums/newtypes/type aliases, field metadata, docstrings, decorators, aliases, and stable anchors.
- Represent resolved decorator arguments, aliases, and relevant constants only when the compiler can expose checked safe values without executing user code.
- Validate mechanically checkable docstring sections against checked parameters, returns, fields, aliases, and decorator metadata, with strict and non-strict extraction behavior documented.

### Phase 6: CLI and editor tooling

- Add CLI commands or documented tooling paths for model emit from compiler-visible symbols, model emit from supported artifacts, and checked API metadata JSON extraction from a package or source root.
- Add editor-accessible commands that invoke the same emit and inspection paths for resolved materialized model symbols and, where practical, selected public declarations.
- Surface diagnostics for unsupported contexts, ambiguous symbols, missing bundles, non-introspectable artifacts, and unsafe external bytes.

### Phase 7: Tests, docs, and release integration

- Add focused tests for schema validation, materialization, collision diagnostics, reflection parity, artifact persistence, deterministic emit, checked API JSON shape, decorator/constant/alias resolution, docstring diagnostics, CLI behavior, and at least one multi-module package.
- Update user-facing reference and tooling documentation for the new inspection and emit commands; do not treat the RFC alone as documentation.
- Bump the active development version and add release notes when implementation work lands on the dev line.

## Implementation log

### Spec / lifecycle

- [x] Review RFC 048 after the checked API metadata scope was merged in and RFC 082 was split out as the documentation-generation layer.
- [x] Establish the active implementation boundary: RFC 048 owns checked metadata extraction, model emit, artifact inspection, CLI/editor tooling, and docstring diagnostics; RFC 082 owns generated documentation output contracts.
- [x] Keep issue #205 as the implementation tracker for RFC 048 and issue #438 as the adjacent checked API metadata extraction context.

### Metadata contract

- [x] Define serializable schema types for canonical model bundles.
- [x] Define serializable schema types for checked API metadata JSON.
- [x] Define schema versioning and stable anchor rules.
- [x] Define safe value representation for decorator arguments, constants, and metadata values.
- [x] Add diagnostics for incomplete, unknown, opaque, or unsafe metadata values.

### Canonical model materialization

- [x] Validate bundle logical type names, stable model ids, format versions, ordered fields, field types, nullability, and RFC 021-compatible metadata.
- [x] Register accepted bundles as nominal model types in checked program state.
- [x] Reject name collisions with user-declared or already materialized visible types.
- [x] Preserve model interrogation and reflection parity for represented fields.
- [x] Reuse existing model lowering and Rust interop behavior for represented fields.

### Artifact introspection

- [x] Extend package/library artifact metadata so `.incnlib` discovers RFC 048 metadata without ordinary sidecar discovery.
- [x] Persist publishable model bundles and checked API metadata losslessly in supported artifacts.
- [x] Add artifact inspection behavior for supported artifacts carrying RFC 048 metadata.
- [x] Report non-introspectable artifacts with clear diagnostics.

### Emit and formatter

- [x] Emit syntactically valid Incan `model` source from canonical bundles.
- [x] Preserve canonical field order, type spelling, aliases, and field metadata in emitted source.
- [x] Route emitted source through the project formatter path and prove idempotence.
- [x] Add deterministic output tests for repeated emit.
- [x] Document lossy projection behavior.

### Checked API metadata extraction

- [x] Extract public declarations, signatures, type parameters, bounds, and model field metadata from checked program state.
- [x] Extract decorators with resolved paths and checked safe argument structures.
- [x] Extract aliases with resolved targets.
- [x] Extract relevant safe constants and metadata values without executing user code.
- [x] Parse Incan-standard docstrings into structured sections.
- [x] Validate documented parameters, returns, fields, aliases, and decorator metadata against checked source structure.
- [x] Emit stable JSON suitable for RFC 082 and downstream renderers.

### CLI / LSP

- [x] Add a CLI path for model emit from a compiler-visible contract-backed model.
- [x] Add a CLI or documented tooling path for model emit from a supported artifact.
- [x] Add a CLI path for checked public API metadata JSON extraction.
- [x] Add editor-accessible emit for selected or resolved materialized model symbols.
- [x] Expose checked metadata previews for selected public declarations where practical.
- [x] Document trust boundaries for external bytes and unsupported contexts.

### Tests

- [x] Add schema validation tests for valid and invalid bundles.
- [x] Add typechecker/materialization tests for nominal behavior and collisions.
- [x] Add reflection/interrogation parity tests against handwritten models.
- [x] Add artifact persistence and inspection tests.
- [x] Add CLI tests for checked API JSON extraction.
- [x] Add CLI tests for model emit.
- [x] Add tests for checked API JSON shape, decorator argument resolution, safe constant extraction, and model field metadata.
- [x] Add tests for alias target extraction and multi-module package metadata.
- [x] Add tests for docstring parsing and docstring drift diagnostics.
- [x] Run targeted tests plus the repository pre-commit gate before closeout.

### Docs / release

- [x] Update user-facing CLI/tooling documentation for checked API metadata extraction.
- [x] Update user-facing CLI/tooling documentation for artifact inspection and model emit.
- [x] Update reference documentation for checked API metadata structure and parsed docstring handling.
- [x] Update reference documentation for docstring validation once validation lands.
- [x] Add release notes for checked API metadata extraction.
- [x] Bump the active `0.3.0-dev.N` version for the full RFC 048 implementation.

## Design Decisions

1. **Artifact classes**: this RFC guarantees RFC 048 introspection for Incan library and package artifacts that claim support for it. Arbitrary standalone compiled binaries are explicitly deferred unless a later RFC defines how they embed the same checked metadata contract.
2. **Model selection for embedding**: artifact metadata includes public or exported contract-backed models plus models explicitly selected for publication by a producer or build integration. Private compiler-temporary materializations are not published by default.
3. **Artifact discovery contract**: package-style artifacts use the `.incnlib` manifest as the discovery document. The manifest must either carry RFC 048 bundles directly or reference an embedded payload losslessly from a documented manifest entry, so CLI, registry, LSP, and third-party tooling do not need implementation-specific sidecar discovery.
4. **Logical identity**: logical type name alone is not sufficient for artifact-facing identity. Publishable bundles require a stable model id in addition to the Incan logical type name, so registry diffing and compatibility views can track one conceptual model across versions even when its visible type name changes or its field set evolves.
5. **Companion producer boundary**: a producer-derived bundle is publishable only after it supplies a complete ordered field list, maps every field type into Incan, records nullability and RFC 021-compatible metadata, provides a stable model id, declares its bundle format version, and passes the same validation required for compiler materialization. Bundles that are complete enough for editor preview but lack stable identity or publishable validation may remain transient and must not be embedded as artifact contract metadata.
6. **Checked API metadata scope**: RFC 048 includes checked API metadata extraction for documentation and downstream tooling. The stable contract is the extracted metadata model; RFC 082 owns generated documentation contracts built on that model.
7. **Docs tooling boundary**: Incan tooling must expose enough checked metadata for consumers such as RFC 082, query/catalog tools, and package browsers to render their own views, but it must not encode product-specific concepts in the compiler metadata schema.
8. **Safe metadata values**: decorator arguments and constants may be exposed only when the compiler can represent their checked structure without executing user code. Unsupported values must produce diagnostics or structured opaque entries rather than best-effort stringification.
