# RFC 124: `incan.pub` Loaf registry and baked asset distribution

- **Status:** Draft
- **Created:** 2026-09-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 034 (`incan.pub` package registry; superseded by this RFC)
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 076 (project mutation policy and recovery)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 114 (compiled providers, SDK components, and package features)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 123 (package executable representation)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

`incan.pub` is the registry for Oven-built projects. Its unit of publication is the Loaf, whatever facets the Loaf carries, so an Incan library, a Rust-only crate that adopted `loaf.toml`, and a mixed project publish and resolve the same way. The registry stores two tiers of immutable, content-addressed artifacts: **source Loaves**, which are the authoritative publication signed by their publisher, and **baked assets**, which are the target-bound `*.loaf` outputs Oven already seals, each carrying its receipt and an attestation that binds it to a source digest and a toolchain. A consumer whose plan matches an available asset downloads it instead of compiling; one that does not bakes from source. Discovery goes through a static sparse index of scoped names; integrity through per-artifact checksums; trust through keyless signing and trusted publishing from the first release; state changes through signed registry events. crates.io remains a consumption-only source reached through the same client, cache, and lockfile. This RFC supersedes RFC 034, which was written before Loaves existed.

## Core model

1. **The Loaf is the package.** A published package is a project as RFC 117 defines it, identified by `loaf.toml`. The registry does not distinguish Incan-facet, Rust-facet, or mixed Loaves; RFC 119 defines what a Rust facet is, and this RFC only requires that whatever a Loaf declares is what gets published.
2. **Two tiers, one identity chain.** A source Loaf is the publication of record. A baked asset is derived from exactly one source Loaf under one recorded toolchain, target, profile, and feature closure, and says so in its receipt. Nothing in the registry is authoritative unless it is reachable from a signed source Loaf.
3. **Assets are offered, never substituted.** An asset is usable only when the consumer's plan facts equal the asset's receipt facts. A near match is a miss, and a miss means baking from source. Two compiled instances of one crate at the same version are not interchangeable, and the registry must never make Oven believe otherwise.
4. **Versions are immutable; state is events.** A `(name, version)` pair is published once and never overwritten. Yank, unyank, ownership, advisory, and supersession are signed registry events. The index and the web site are projections over the event stream and the immutable artifacts, never a database that can drift from them.
5. **Names are scoped.** Community packages live under an owner scope. Unscoped names are reserved for the toolchain's own Loaves.
6. **Resolution executes nothing.** Resolving, fetching, verifying, and staging never run package-provided code, as RFC 117 already requires. Build-time code runs only through an explicitly selected host provider under RFC 119.
7. **One client, several sources.** `incan.pub` and crates.io are two index formats over one transport, one TLS policy, one store, and one lockfile. Registered compatible sources from RFC 117 configuration plug into the same client.

## Motivation

RFC 034 described a registry for Incan libraries in the shape the project had in early 2026: a bespoke `.incanpkg` archive around a checked type manifest, a flat namespace, tokens first and signatures later, and a resolver that fed downloaded package facts into a Cargo-generated project. Every one of those assumptions has since changed. RFC 117 made `loaf.toml` the only authored manifest and `oven.lock` the resolved graph. RFC 118 gave Oven its own command surface with `oven publish`. RFC 119 gave Rust-only projects a first-class Loaf shape and made Cargo an explicit compatibility mode rather than the hidden backend. RFC 123 defined what a package publishes so that its exports are executable by any consumer. RFC 079 reframed `incan.pub` as an artifact graph. A registry RFC that still speaks of `.incnlib` files and generated `Cargo.toml` cannot be implemented against that foundation, and patching it would leave the package format, the trust model, and the resolution flow all pointing at a world that no longer exists.

Two further motivations are new. First, Oven produces compiled, target-bound, receipt-carrying artifacts as its normal output, which is something Cargo's source-only registry never had to work with. A registry that can distribute those artifacts changes the cost of using Incan: a bare `incan new` project today compiles fourteen dependency rlibs into a local Loaf before it can print a line, and every project on a machine repeats that work. The standard library's own base Loaf families face the same problem on the release side, where one immutable Loaf has to satisfy every optional module and therefore carries the web stack for every user. Assets published per target and closure make both of these download-and-verify operations instead of compile operations.

Second, the ecosystems Incan learns from have spent a decade discovering what a registry must not do. Install-time scripts, mutable version pointers, oversized metadata documents, flat namespaces, and long-lived publish tokens each caused an incident class of their own. Content addressing, per-artifact integrity, keyless signing with a public transparency log, trusted publishing from CI, and scoped names are the settled answers. A registry designed now should start from those answers rather than arrive at them.

## Goals

- Define the Loaf as the unit of publication, so Rust-only, Incan-only, and mixed projects publish and resolve identically.
- Define the two artifact tiers, source Loaves and baked assets, with the identity chain that binds them.
- Define the static sparse index, the per-version asset manifest, and the scoped naming rules.
- Define integrity, signing, trusted publishing, and consumer-side verification as normative behaviour from the first release.
- Define registry state as signed events over immutable artifacts, consistent with RFC 079.
- Define how the same Oven client, cache, and lockfile serve `incan.pub`, crates.io, and registered compatible sources.
- Define what the registry web surface shows, as a projection of the same data.
- Preserve the hosting constraints from RFC 034: predictable capped cost, EU hosting, provider portability, and static distribution of everything immutable.

## Non-Goals

- Publishing Loaves or `*.loaf` assets to crates.io, or presenting an Oven-native package as a crates.io package. RFC 119 already rules this out.
- Mirroring or proxying crates.io source archives. crates.io remains an external, consumption-only source.
- The full artifact-graph vocabulary of RFC 079 beyond packages, their versions, their assets, and the events that govern them. This RFC is the package substrate that RFC 079 builds on.
- The `*.loaf` archive format or receipt schema. Those belong to the Oven RFCs; this RFC consumes them.
- Hosting vendor selection, cost figures, and deployment topology beyond the constraints stated as goals.
- Web site visual design and search ranking.
- A hosted private-registry product. Private registries are registered compatible sources under RFC 117; the protocol here is what they implement.

## Guide-level explanation

### Publishing a Loaf

A library author with a `loaf.toml` publishes from the project root:

```text
$ oven publish
  Planning encero/widgets 0.4.0 ...
  Baking source Loaf ...                          ok
  Baking asset aarch64-apple-darwin/release ...   ok (from this machine; attested as local)
  Signing (Sigstore, github:encero/widgets) ...   ok
  Uploading source Loaf ...                       ok  sha256:7c1d…
  Uploading 1 asset ...                           ok
  Published encero/widgets 0.4.0
```

`oven publish` from a project's own CI does the same thing without a stored credential. The CI job presents its OpenID Connect identity, the registry checks that identity against the trusted publisher the scope owner configured, and the resulting attestation names the workflow that produced the artifact. Assets baked in CI for other targets upload alongside the source Loaf and are marked as publisher-built.

A Rust-only project publishes identically. Its `loaf.toml` declares a Rust facet under RFC 119, `oven publish` bakes it, and the registry stores a source Loaf whose facet happens to be Rust. Consumers see a Loaf, not a crate.

### Consuming a published Loaf

```toml
# loaf.toml
[dependencies]
widgets = { loaf = "encero/widgets", version = "^0.4" }
serde   = { crate = "serde", version = "1", features = ["derive"] }
```

```text
$ oven bake
  Resolving ...
    encero/widgets 0.4.0  (incan.pub)
    serde 1.0.228         (crates.io)
  encero/widgets 0.4.0: asset aarch64-apple-darwin/release matches plan  → downloading
    integrity sha256:9a2f… ok · attestation github:encero/widgets@refs/tags/v0.4.0 ok
  serde 1.0.228: no asset for this closure                              → baking from source
  Baked my-app (release)
```

The consumer never chose between "download" and "compile". Oven computed the plan, asked the registry which assets exist for exactly that plan, took the ones that matched, and baked the rest. The lockfile records, for every unit, where it came from, its checksum, and whether an asset or a source bake satisfied it.

### Yanking

```text
$ oven yank encero/widgets 0.4.0 --reason "panics on empty input, use 0.4.1"
  Signed yank event recorded. Existing locks still resolve 0.4.0; new resolutions skip it.
```

The archive stays. Locked consumers keep building and see the advisory through the recovery surfaces RFC 076 defines. New resolutions select 0.4.1.

### What `incan.pub` shows

A package page lists its versions, the facets each version carries, the scope and its owners, the trusted publishers configured for the scope, the asset matrix (which targets, toolchains, and profiles have attested assets, and who built them), the resolved dependency graph, rendered documentation when the publisher shipped a documentation asset, advisories and yank events with their reasons, and download counts. All of it is static content derived from the index, the event log, and the artifacts; nothing on the page requires a request to a live service.

### Where the standard library fits

The toolchain's own Loaves publish here under reserved unscoped names such as `std` and its components. The compiler pins the versions it expects through the toolchain manifest, so a standard library fix can ship as a registry publication without a compiler release, and the base Loaf families the compiler needs are ordinary assets in the index rather than a special release artifact.

## Reference-level explanation

### Package identity

A package name is either **scoped**, written `<scope>/<name>`, or **reserved unscoped**, written `<name>`. Scopes and names must match `[a-z0-9][a-z0-9-]*` and must be at most 64 characters each. A scope is owned by an account or an organization. All community packages must be scoped. Unscoped names are reserved for Loaves published by the toolchain itself; the registry must reject any attempt to claim an unscoped name from a non-toolchain publisher. The reserved set includes at least `std`, `incan`, and `oven`, and every standard-library component name.

Versions must be SemVer. A `(package, version)` pair is published at most once and is never overwritten, replaced, or deleted. The registry must reject a publish for an existing pair.

Package names on `incan.pub` and crate names on crates.io are distinct namespaces. A `loaf.toml` dependency selects the registry through its dependency key (`loaf` or `crate`) as RFC 117 defines; the registry must not attempt to disambiguate bare names across sources.

### Source Loaves

A source Loaf is the archive `oven publish` produces from a project root. It must contain the project's `loaf.toml`, the sources the manifest's facets select, and the executable representation RFC 123 requires for any Incan facet. It must not contain generated Rust as its compatibility contract; generated Rust may be present only as an inspection artifact with provenance, as RFC 034 already required. The source Loaf's identity is the SHA-256 digest of the archive.

Every source Loaf must be signed by its publisher. Signing uses keyless certificates issued against an OpenID Connect identity and recorded in a public transparency log. The registry must verify on publish that the certificate identity is authorised for the scope, that the signature covers the archive digest, and that the transparency-log inclusion is valid. The registry must reject unsigned source Loaves.

### Baked assets

A baked asset is a `*.loaf` produced by Oven from exactly one source Loaf. Its receipt must record the source Loaf digest, the toolchain identity, the target triple, the profile, the resolved feature closure of every unit, and the host and target domains. The asset's identity is the SHA-256 digest of the archive.

Every asset must carry an attestation that binds the asset digest to the source Loaf digest and the toolchain identity, and names the builder. Builder kinds are:

- **publisher**: built in the publisher's trusted CI; the attestation identity is the workflow identity.
- **registry**: built by the registry's own bakery from the signed source Loaf; the attestation identity is the registry.
- **local**: built on the publishing machine; the attestation identity is the publisher's keyless identity.

Consumers may apply different trust policy to each builder kind through RFC 117 registry configuration. The registry must publish which kind produced each asset and must not relabel one kind as another.

An asset is **applicable** to a consumer plan only when the plan's toolchain identity, target triple, profile, and per-unit feature closure equal the asset's receipt facts. Oven must treat any inequality as inapplicable and bake from source. Oven must not select an asset by version alone.

### Index

The registry must publish a sparse index: one file per package at `index/<scope>/<name>` for scoped packages and `index/-/<name>` for reserved unscoped packages, containing one JSON object per line, one line per published version. Each line must contain:

| Field | Meaning |
| --- | --- |
| `name` | the full package name |
| `vers` | the version |
| `cksum` | the source Loaf digest, `sha256:` prefixed |
| `facets` | the facet kinds the Loaf declares, such as `incan`, `rust` |
| `deps` | typed dependencies with their requirements and source kind |
| `requires` | toolchain requirements, at least the compiler and Oven ranges |
| `yanked` | whether a yank event is in effect |
| `publisher` | the signing identity of the source Loaf |
| `assets` | the path of the per-version asset manifest |

Index lines must stay small: no descriptions, readmes, or asset tables. The per-version asset manifest at the recorded path must list every attested asset with its digest, receipt facts, builder kind, and attestation reference. Both files must be served as static, cacheable content, and the registry should serve them from a CDN.

The resolver must filter candidates by the dependency's version requirement and by `requires` against the installed toolchain, must skip yanked versions unless the lockfile already pins them, and should select the maximal satisfying version. It must record the selected version, source, digest, and satisfaction kind (asset or source) in `oven.lock`.

### Registry events

Every state change other than a first publish is a signed event: `yank`, `unyank`, `ownership`, `trusted-publisher`, `advisory`, and `supersede`. Events must be signed by an identity authorised for the scope at the time of the event, must be append-only, and must be publicly readable. The index, the asset manifests, and the web site must be derivable from the event log plus the immutable artifacts alone. This is the package-level substrate the RFC 079 artifact graph projects from.

### Publishing protocol

Publishing must be possible through two authentication paths:

- **Trusted publishing.** A CI job presents an OpenID Connect token; the registry verifies it against the trusted-publisher configuration of the scope and accepts the publish without any stored credential. This must be available from the first release.
- **Publisher tokens.** A scope owner may mint tokens for publishing outside CI. Tokens must be scoped to a set of packages and an expiry, and the registry should warn on tokens that never expire.

On publish the registry must: verify the caller is authorised for the scope; reject an existing `(name, version)`; verify the declared digest against the uploaded archive; verify the signature and transparency-log inclusion; parse `loaf.toml` and reject a Loaf whose manifest does not describe the archive's contents; verify each uploaded asset's attestation binds it to the uploaded source digest; store the artifacts; append the index line and asset manifest; and emit a `publish` event. Publish must be atomic from the consumer's point of view: a version is either fully visible or absent.

### Fetching and verification

Oven must fetch over HTTPS with a certificate policy that behaves identically on every supported host, and should offer an explicit opt-in to the operating system's certificate store for environments that require it. Oven must verify every downloaded artifact's digest against the index or asset manifest before using it, must fail closed on mismatch, and must verify attestations according to the active trust policy. Oven must not execute any content of a downloaded artifact during resolution or fetching.

The Loaf store is the only cache and the only offline source. An offline mode must satisfy resolution from the store and the lockfile alone and must fail fast on anything absent, as RFC 020 requires.

### crates.io and other sources

`crate` dependencies resolve against the crates.io sparse index and download archives from crates.io through the same client, transport, verification, and store. Oven must parse a crate's manifest only as provider metadata under the constraints RFC 119 places on it. The registry must not mirror crates.io source. The registry may publish registry-built assets for crates.io crates at selected closures; such assets follow every rule above for registry-built assets, with the crate's crates.io checksum as the source digest.

Registered compatible sources under RFC 117 configuration must implement this index and artifact protocol for `kind = "loaf"` or the crates.io sparse protocol for `kind = "crate"`.

### Web surface

The registry must publish a web surface generated from the index, event log, and artifacts. It must show, per package: versions and yank state with reasons, facets, scope owners and trusted publishers, the asset matrix with builder kinds, the dependency graph of each version, advisories, and download counts. It should render documentation when a version ships a documentation asset. It must not depend on a live service for reads.

### Mirrors

Because every artifact and index file is immutable or event-derived, a mirror is a copy of the index, asset manifest, artifact, and event directories. Oven must accept a mirror as a registered compatible source and must verify against the same digests and signatures, so a mirror can never alter content.

## Design details

### Interaction with RFC 117

`loaf.toml` already distinguishes `loaf` and `crate` dependency keys and lets a project name a registered source. This RFC adds no manifest syntax. The `requires` index field maps to the requirement ranges RFC 117 lets a project declare. Registry trust policy, credentials, and registered sources stay in Oven-controlled user or organisation configuration, never in `loaf.toml`.

### Interaction with RFC 118

`oven publish`, `oven add`, `oven yank`, and the `oven dependency` family are RFC 118's surfaces; this RFC defines what they exchange with the registry. Running a published tool without adding it to a project is a natural extension of the asset model and is listed as an unresolved question rather than a new command.

### Interaction with RFC 119

The Rust facet is what makes Rust-only publication possible. Asset applicability reuses RFC 119's host and target domains and per-unit feature closures verbatim; this RFC introduces no new notion of a build unit. Cargo compatibility mode is orthogonal: a project in that mode is not Loaf-native and cannot publish here until it adopts `loaf.toml`.

### Interaction with RFC 123

The executable representation is part of the source Loaf for any Incan facet, so a consumer never depends on generated Rust reaching it through the registry. RFC 123 owns its format and versioning.

### Interaction with RFC 076 and RFC 079

Yank, advisory, and supersede events are the source of the recovery information RFC 076 requires Oven to surface. RFC 079's artifact graph is a projection over the event stream and artifacts defined here.

### Compatibility with RFC 034

Nothing was published under RFC 034, so there is no migration. The constraints RFC 034 established for hosting remain in force as goals of this RFC. The `.incanpkg` format, `.incnlib` manifest dependency, flat namespace, and token-first phasing are withdrawn.

## Alternatives considered

- **Amend RFC 034 in place.** Rejected. The package format, resolution flow, and phasing all rest on pre-Loaf assumptions; a rewrite is smaller than the amendment.
- **Source-only registry, as crates.io.** Rejected. It discards the one property that distinguishes Oven, receipt-bound compiled artifacts, and keeps every consumer compiling every dependency on every machine.
- **Flat namespace.** Rejected. Rust-only Loaves guarantee collisions with crate names, and flat spaces are where squatting happens in every registry that has one. Reserving unscoped names for the toolchain keeps the common standard-library spelling short.
- **`@scope/name` spelling.** Rejected in favour of `scope/name`. The `@` character is the conventional version separator in tool-run syntax (`tool@1.2`), and the slash form matches container and source-forge conventions.
- **Assets built only by publishers.** Rejected as the sole path. Most publishers will not bake for every target; a registry bakery over signed source is what makes assets common rather than rare. Kept as one of three builder kinds with distinct trust.
- **Proxying crates.io source through `incan.pub`.** Rejected. It adds a bandwidth cost the project cannot cap and a trust surface it does not need; the client can reach crates.io directly. Registry-built assets for crates are the useful subset and are permitted.
- **Tokens first, signatures later.** Rejected. Long-lived publish tokens are the documented origin of the largest registry compromises elsewhere; trusted publishing is cheaper to start with than to retrofit.
- **A mutable registry database as the source of truth.** Rejected, consistent with RFC 079: signed events over immutable artifacts are auditable, mirrorable, and static-site friendly.

## Drawbacks

- **Asset matrix growth.** Toolchain × target × profile × closure can explode. The mitigation is that only closures the toolchain pins, publishers bake, or the bakery selects ever exist; the index never promises completeness, and a miss is a bake, not a failure.
- **Bakery cost.** Registry-built assets are compute the project pays for, against a hard cost cap. The bakery is optional under this RFC and its scope is an unresolved question.
- **Trust complexity.** Three builder kinds and per-kind policy are more to explain than "download the tarball". The default policy should make the safe choice invisible.
- **Scoped names are longer.** `encero/widgets` is less pretty than `widgets`. The reserved unscoped tier keeps the standard library short, and every other registry that started flat has wished it had not.
- **A registry is infrastructure.** Even a mostly static one needs a publish service, an event log, and operations. RFC 034's cost and hosting constraints are carried forward for this reason.

## Implementation architecture

Non-normative. The registry client belongs in Oven's build-system ring rather than anywhere the compiler links: one HTTPS client with a pure-Rust TLS stack and an opt-in for the platform certificate store, one retry and caching layer, and two index readers (Loaf and crates.io sparse) behind one resolver interface. The Loaf store is the cache for index files, asset manifests, source Loaves, and assets alike, keyed by content digest with a separate name-to-digest index, which is also what lets two projects share one compiled unit. The publish service is small: authentication, validation, storage, index append, and event emission; everything read-side is static files behind a CDN. The bakery is an Oven invocation over a signed source Loaf under a pinned toolchain, producing assets whose attestations name the registry. The web surface is a static-site generator over the event log.

## Layers affected

- **Oven resolver and lockfile**: index reading for two source kinds, toolchain-requirement filtering, asset applicability, satisfaction kind and attestation facts in `oven.lock`.
- **Oven store**: content-addressed storage of index files, asset manifests, source Loaves, and assets; offline mode over the store alone.
- **Oven publication**: `oven publish` bakes, packages, signs, attests assets, and uploads; `oven yank` emits signed events; trusted-publishing and token flows.
- **Oven configuration**: registered sources, trust policy per builder kind, certificate-store opt-in.
- **Registry service**: publish validation, artifact and event storage, index projection, static web surface.
- **Compiler**: no parser, typechecker, or emission changes; the source Loaf carries the RFC 123 executable representation the compiler already produces.
- **Docs / Tooling**: publishing, consuming, verification, and failure-mode documentation; asset matrix and provenance in the web surface.

## Unresolved questions

- Should Oven offer minimal-version resolution as an opt-in mode alongside the default maximal selection, and if so should the lockfile record which mode produced it?
- Is the registry bakery in scope for the 0.6 release, or is publisher-built plus local the first shipped set of builder kinds?
- How is a Rust-facet Loaf from `incan.pub` imported in Incan source? The dependency key selects the registry; whether `rust::` or `pub::` selects the surface needs a rule.
- How are scopes claimed and verified: free claim on first publish, linked to a source-forge organisation, or both with different display?
- What is the retention policy for assets whose toolchain is no longer supported, given that source Loaves are retained forever?
- Should running a published tool without a project (`oven run <scope>/<name>@<version>`) be defined here or in an RFC 118 amendment?
- Which documentation asset format does the web surface render, and does RFC 082 define it?
- When the CDN bandwidth cap is reached, should clients fall back to the origin or fail with a clear diagnostic?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
