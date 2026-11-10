# RFC 125: `loaves.pub` Loaf registry and baked asset distribution

- **Status:** Planned
- **Created:** 2026-09-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 034 (`incan.pub` package registry; superseded by this RFC)
    - RFC 076 (project mutation policy and recovery)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 079 (`loaves.pub` artifact graph)
    - RFC 082 (checked API documentation generation)
    - RFC 114 (compiled providers, SDK components, and package features)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 123 (package executable representation)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
    - RFC 159 (native C library Loaves and the managed Linux C toolchain)
- **Issue:** [#2706](https://github.com/encero-systems/incan/issues/2706)
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

`loaves.pub` is the registry for Oven-built projects. Its unit of publication is the Loaf, whatever facets the Loaf carries, so an Incan library, a Rust-only crate that adopted `loaf.toml`, and a mixed project publish and resolve the same way. The registry stores two tiers of immutable, content-addressed artifacts: **source Loaves**, which are the authoritative publication signed by their publisher, and **baked assets**, which are target-bound compiled units, each carrying its receipt and an attestation that binds it to a source digest and a toolchain. A consumer whose plan matches an available asset downloads it instead of compiling; when no asset matches, the consumer bakes from source. Discovery goes through a static sparse index of scoped names; integrity through per-artifact checksums; trust through keyless signing and trusted publishing; state changes through signed registry events. crates.io is a source the registry adopts from: `loaves.pub` turns a crates.io package version into a Loaf once, hosts its sources, and records its build facts, so a project the registry can serve never contacts crates.io, and nothing on any path runs Cargo. What the registry has not adopted, a local provider can adopt as its own visible step. Oven reaches every registry, remote or hosted locally, through one provider surface. This RFC supersedes RFC 034, which was written before Loaves existed.

## Core model

1. **The Loaf is the package.** A published package is a project as RFC 117 defines it, identified by `loaf.toml`. The registry does not distinguish Incan-facet, Rust-facet, or mixed Loaves; RFC 119 defines what a Rust facet is, and this RFC only requires that whatever a Loaf declares is what gets published.
2. **Two tiers, one identity chain.** A source Loaf is the publication of record. A baked asset is derived from exactly one source Loaf under one recorded toolchain, target, profile, and feature closure, and says so in its receipt. Nothing in the registry is authoritative unless it is reachable from a signed source Loaf; for a package adopted from crates.io, that source Loaf is the registry-signed **adopted Loaf**, which binds the upstream registry, package, version, and checksum it was adopted from.
3. **Assets are offered, never substituted.** An asset is usable only when every unit it carries has exactly the RFC 124 unit identity the consumer's plan requires. Toolchain, target, profile, and features are how candidates are found; unit identity is how they are admitted. A near match is a miss, and a miss means baking from source. Two compiled instances of one crate at the same version are not interchangeable, and the registry must never make Oven believe otherwise.
4. **Versions are immutable; state is events.** No publisher operation can overwrite, replace, or delete a published version. Adoption, bound build facts, equivalence attestations, yank, unyank, ownership, advisory, and supersession are signed registry events. The index, the rendered manifest of an adopted Loaf, and the web site are projections over the event stream and the immutable artifacts, never a database that can drift from them.
5. **The first part of a name is its owner.** Community packages live under the scope of the account or organization that owns them, such as `acme/widgets`. Reserved scopes are operated by the project: `incan` for native packages the project provides, `crates-io` and `c` for adopted ones.
6. **Resolution executes nothing.** Resolving, fetching, verifying, and staging never run package-provided code, as RFC 117 already requires. Build-time code runs only through an explicitly selected host provider under RFC 119, and filling a miss locally is a separate step, never part of resolving.
7. **The registry is a key, not a hostname.** A registry is identified by a root public key and an id derived from it. Every registry statement, from events to index files to a periodic checkpoint, is signed by a key delegated from that root, and clients pin the id and root key through RFC 117 configuration. Hostnames are transport: any endpoint whose content verifies under the pinned root is the registry, and mirrors need no separate trust.
8. **Assets are optional; the rest is not.** A build must be resolvable and bakeable from the index, manifests, event log, and source publications alone. An asset that cannot be fetched, for whatever reason, is inapplicable, and inapplicable means baking from source. No asset outage is a build failure.
9. **One provider surface, any endpoint.** Oven speaks one surface to every Loaf registry: index lines, records, Loaf archives by digest, build facts by binding, and compiled units by unit identity. A remote HTTPS endpoint, a mirror, a git checkout, and Oven's local store (RFC 117) are interchangeable once their content verifies, and Oven's behavior, output, and user experience are the same against each. A miss is the provider's business: a local provider may fill it as its own visible step, and a remote one answers "not published".
10. **Loaves, not crates.** There is no crate dependency kind and no second index format. A crates.io package reaches consumers only as an adopted Loaf. Adoption belongs to the registry's tool, so Oven itself never reads crates.io, a crate's `Cargo.toml`, or a `Cargo.lock`, and nothing on any path runs Cargo.

## Motivation

RFC 034 described a registry for Incan libraries in the shape the project had in early 2026: a bespoke `.incanpkg` archive around a checked type manifest, a flat namespace, tokens first and signatures later, and a resolver that fed downloaded package facts into a Cargo-generated project. Every one of those assumptions has since changed. RFC 117 made `loaf.toml` the only authored manifest and `oven.lock` the resolved graph. RFC 118 gave Oven its own command surface with `oven publish`. RFC 119 gave Rust-only projects a first-class Loaf shape and removed Cargo as the hidden backend. RFC 123 defined what a package publishes so that its exports are executable by any consumer. RFC 079 reframed `loaves.pub` as an artifact graph. A registry RFC that still speaks of `.incnlib` files and generated `Cargo.toml` cannot be implemented against that foundation, and patching it would leave the package format, the trust model, and the resolution flow all pointing at a world that no longer exists.

Two further motivations are new. First, Oven produces compiled, target-bound, receipt-carrying artifacts as its normal output, which is something Cargo's source-only registry never had to work with. A registry that can distribute those artifacts changes the cost of using Incan: a bare `incan new` project today compiles fourteen dependency rlibs into a local Loaf before it can print a line, and every project on a machine repeats that work. The standard library's own base Loaf families face the same problem on the release side, where one immutable Loaf has to satisfy every optional module and therefore carries the web stack for every user. Assets published per target and closure make both of these download-and-verify operations instead of compile operations.

Rust itself has asked for this since 2015: the Cargo issue for pre-built dependencies is still open, the Cargo team's 2026 project goal is a cross-workspace build cache that may later be pre-populated from a remote cache, and the one time a major crate shipped a precompiled artifact without a source of record, a verifiable link to it, or a way to decline it, the ecosystem rejected it within a month and its maintainer asked for a first-class mechanism instead. The two-tier design here is that mechanism.

Second, the ecosystems Incan learns from have spent a decade discovering what a registry must not do. Install-time scripts, mutable version pointers, oversized metadata documents, flat namespaces, and long-lived publish tokens each caused an incident class of their own. Content addressing, per-artifact integrity, keyless signing with a public transparency log, trusted publishing from CI, and scoped names are the settled answers. A registry designed now should start from those answers rather than arrive at them.

## Goals

- Define the Loaf as the unit of publication, so Rust-only, Incan-only, and mixed projects publish and resolve identically.
- Define the two artifact tiers, source Loaves and baked assets, with the identity chain that binds them.
- Define the static sparse index, the per-version asset manifest, and the scoped naming rules.
- Define integrity, signing, trusted publishing, and consumer-side verification as normative behavior.
- Define registry state as signed events over immutable artifacts, consistent with RFC 079.
- Define adoption: how the registry turns a crates.io package version into a hosted Loaf, without Cargo, and records its build facts per binding.
- Define the provider surface every Loaf registry endpoint serves, local or remote, including compiled units looked up and published by unit identity.
- Define version selection: the lowest version that satisfies a project's requirements by default, and newer versions only on request.
- Define what the registry publishes for people to read, as a projection of the same data.
- Define the degradation rule: an unavailable asset is a miss, never a build failure.
- Define the registry's own identity and signing model, so that state assertions are verifiable independently of the host serving them.
- Preserve the hosting constraints from RFC 034: predictable capped cost, EU hosting, provider portability, and static distribution of everything immutable.

## Non-Goals

- Publishing Loaves or compiled units to crates.io, or presenting an Oven-native package as a crates.io package. RFC 119 already rules this out.
- Proxying or mirroring crates.io. The registry adopts a package version once and serves the adopted Loaf; it hosts only the versions it has adopted, and it never forwards a consumer's request to crates.io.
- An on-demand compiler in the public registry. A remote registry serves what was published or attested; on-demand adoption and building happen only in Oven's local store (RFC 117).
- The full artifact-graph vocabulary of RFC 079 beyond packages, their versions, their assets, and the events that govern them. This RFC is the package substrate that RFC 079 builds on.
- The unit archive format or receipt schema. Those belong to the Oven RFCs; this RFC consumes them.
- Hosting vendor selection, cost figures, and deployment topology beyond the constraints stated as goals.
- Web site visual design and search ranking.
- A hosted private-registry product. Private registries are registered compatible sources under RFC 117; the protocol here is what they implement.

## Guide-level explanation

### Publishing a Loaf

A library author with a `loaf.toml` publishes from the project root:

```text
$ oven publish
  Planning acme/widgets 0.4.0 ...
  Baking source Loaf ...                          ok
  Baking asset aarch64-apple-darwin/release ...   ok (from this machine; attested as local)
  Signing (Sigstore, github:acme/widgets) ...     ok
  Uploading source Loaf ...                       ok  sha256:7c1d…
  Uploading 1 asset ...                           ok
  Published acme/widgets 0.4.0
```

`oven publish` from a project's own CI does the same thing without a stored credential. The CI job presents its OpenID Connect identity, the registry checks that identity against the trusted publisher the scope owner configured, and the resulting attestation names the workflow that produced the artifact. Assets baked in CI for other targets upload alongside the source Loaf and are marked as publisher-built.

A Rust-only project publishes identically. Its `loaf.toml` declares a Rust facet under RFC 119, `oven publish` bakes it, and the registry stores a source Loaf whose facet happens to be Rust. Consumers see a Loaf, not a crate.

### Consuming a published Loaf

Every dependency has one form, and the first part of the name says who owns or provides it:

```toml
# loaf.toml
[dependencies]
widgets = { loaf = "acme/widgets", version = "^0.4" }     # published by its owner
query   = { loaf = "incan/query", version = "0.1" }       # native, provided by the project
serde   = { loaf = "crates-io/serde", version = "1.0.228", features = ["derive"] }  # adopted from crates.io
```

A facet decides how source imports the package. `rust::` reaches a Rust facet, `pub::` reaches an Incan facet, and a mixed Loaf offers both:

```incan
from rust::serde import Serialize
from pub::widgets import Widget
```

Several Loaves can share a short name, so `oven add` never guesses. `oven add tokio` lists every match, asks which one, and records the full name. Without a terminal, it refuses and prints the full names.

```text
$ oven bake
  Resolving ...
    acme/widgets 0.4.0         (loaves.pub)
    crates-io/serde 1.0.228    (loaves.pub, adopted)
  acme/widgets 0.4.0: asset aarch64-apple-darwin/release matches plan    → downloading
    integrity sha256:9a2f… ok · attestation github:acme/widgets@refs/tags/v0.4.0 ok
  crates-io/serde 1.0.228: no asset for this closure                    → baking from source
  Baked my-app (release)
```

The consumer never chose between "download" and "compile". Oven computed the plan, asked the registry which assets exist for exactly that plan, took the ones that matched, and baked the rest. The lockfile records, for every unit, where it came from, its checksum, and whether an asset or a source bake satisfied it.

### Which version you get

Oven selects the lowest version that satisfies every requirement on a package, so a project gets what it asked for, and nothing changes until someone asks for more. `oven add` records the newest published version as the requirement, so a new dependency still starts current:

```text
$ oven add crates-io/serde
  Added crates-io/serde, version = "1.0.228"

$ oven update crates-io/serde
  crates-io/serde 1.0.228 → 1.0.230
```

A new release never reaches a project by itself. `oven update` raises a requirement on request, and `oven audit` reports a locked version with an advisory (RFC 076), so staying put never means silently missing a security fix. Because the requirement is what gets built, `version = "1.2"` is a tested claim rather than an untested one.

An author can ask for something else:

```text
$ oven lock --versions newest
```

`newest` takes the highest match everywhere, and `lowest-direct` takes the lowest for a project's own dependencies and the highest for everything else. The lockfile records the mode, so a later `oven lock` cannot quietly switch it back.

### When an adoption is wrong

The registry adopts `crates-io/some-crate 1.2.3` from crates.io. If that translation turns out to be wrong, say it dropped a license file, the registry cannot publish `1.2.4`, because that number belongs to the upstream author. It adopts again under a new **revision**:

```text
adopt  crates-io/some-crate 1.2.3  revision 1  sha256:aaa…
adopt  crates-io/some-crate 1.2.3  revision 2  sha256:bbb…  supersedes 1: "license file missing"
```

An author writing `version = "1.2"` sees nothing different and resolves to revision 2. A project locked to revision 1 keeps building and sees a warning. Community and native packages have no revisions, because their authors publish a new version.

### Yanking

```text
$ oven yank acme/widgets 0.4.0 --reason "panics on empty input, use 0.4.1"
  Signed yank event recorded. Existing locks still resolve 0.4.0; new resolutions skip it.
```

The archive stays. Locked consumers keep building and see the advisory through the recovery surfaces RFC 076 defines. New resolutions select 0.4.1.

### What `loaves.pub` shows

A package page lists its versions, the facets each version carries, the scope and its owners, the trusted publishers configured for the scope, the asset matrix (which targets, toolchains, and profiles have attested assets, and who built them), the declared dependencies, the package's documents and API reference, and advisories and yank events with their reasons. All of it is static content derived from the catalog, the event log, and the artifacts; nothing on the page requires a request to a live service.

### Where the standard library fits

The standard library ships with the toolchain, as ordinary Loaves the toolchain places in the local store. It is ambient: no manifest declares it, and it is not resolved from the index. Its Loaves are named under the reserved `std` scope after the package each provides, so `std.primitives` is `std/primitives`, and that name gives their models and enums canonical identities (RFC 165).

## Reference-level explanation

### Package identity

A package name is written `<scope>/<name>`. Community scopes and names must match `[a-z0-9][a-z0-9-]*` and must be at most 64 characters each. A scope is owned by an account or an organization, recorded by the source forge's immutable numeric id for it, not by its name. A renamed or deleted forge account therefore never transfers a scope, and an account later registered under the same name is a different owner. A deleted scope or package name is tombstoned and never reissued. Bare names must not be published. No scope or name may contain a capital letter, under any scope, and the registry and clients must refuse one rather than fold its case.

The scopes `incan`, `crates-io`, and `c` are reserved, as are `std` and `oven`. A registry's published policy may reserve further scopes, and the registry must reject a publish to a reserved scope from an identity it has not authorized for that scope. `incan` holds native packages the project provides. An adopted Loaf is named under the reserved scope of the source it was adopted from: a crates.io package `serde` becomes `crates-io/serde`. Names under an adopted scope keep their upstream spelling in lowercase, including underscores a community name may not use, so `crates-io/ar_archive_writer` keeps its underscores and the crates.io package `Inflector` becomes `crates-io/inflector`. crates.io treats names as equal regardless of case, so lowercasing cannot make two packages collide. The Loaf's Rust crate name stays the upstream crate name (RFC 119), so the name a consumer's code uses does not change.

Versions must be SemVer. A `(package, version)` pair is published at most once, and no publisher operation can overwrite, replace, or delete it. The registry must reject a publish for an existing pair. An adopted version may have several revisions (see "Adoption from crates.io").

A package name is the Loaf's registry identity, not its code namespace. The `[dependencies]` key names the namespace: `spark = { loaf = "apache/spark", version = "3" }` is imported as `from pub::spark.sql import SparkSession`. A Loaf's `pub` namespace is virtual: its own direct published dependencies fill it, a dependency's dependencies don't appear in it, and two Loaves may use one key for different packages, since types are compared by identity and not by key. A key can name only one package, so a collision is settled in `loaf.toml` by choosing another key. `oven add` must write the last part of the name as the key, with `-` as `_`, and must ask for a key when that one is already taken.

Naming policy belongs to `loaves.pub`. A registered compatible registry sets its own, and a client must never treat a name as the same package across two registries. A `loaf.toml` dependency names its package and registry explicitly as RFC 117 defines; the registry must not disambiguate bare names across registries.

### Source Loaves

A source Loaf is the archive `oven publish` produces from a project root. It must contain the project's `loaf.toml` and the sources the manifest's facets select. When the Loaf publishes an RFC 123 executable representation for an Incan facet, the source Loaf carries it; a Loaf may publish none, and the routes that need one refuse it as RFC 123 specifies rather than the registry rejecting the publication. It must not contain generated Rust as its compatibility contract; generated Rust may be present only as an inspection artifact with provenance, as RFC 034 already required. The source Loaf's identity is the SHA-256 digest of the archive.

Every source Loaf, published or adopted, must carry a license file and a README at its root. A license file is a file at the Loaf root whose name starts with `license`, `licence`, `copying` or `unlicense`, or the exact path the manifest names as its license file; a README is a file at the Loaf root whose name starts with `readme`, or the exact path the manifest names as its readme. Name matching is case-insensitive and by prefix, and applies to root files only: a nested file such as `licenses/MIT` counts only when the manifest names it. A boolean `readme` (`true` or `false`) names no file. The registry must refuse a publication that carries neither form of either; adoption supplies what the upstream package lacks (see "Adoption from crates.io"). Neither file is read by resolution, compilation, or any unit identity.

The archive is a deterministic, uncompressed POSIX tar: entries are sorted by path bytes, paths are relative to the Loaf root with `loaf.toml` at the root, and every entry is a regular file with mode `0644` or `0755`, modification time `0`, owner and group ids `0`, and empty owner and group names. The digest covers the tar bytes; compression applies only in transport, so the digest never depends on a compressor and an identical re-adoption or re-publication produces the same identity.

Every source Loaf must be signed by its publisher. Signing uses keyless certificates issued against an OpenID Connect identity and recorded in a public transparency log. The registry must verify on publish that the certificate identity is authorized for the scope, that the signature covers the archive digest, and that the transparency-log inclusion is valid. The registry must reject unsigned source Loaves.

### Baked assets

A baked asset is a deterministic archive holding one compiled unit, produced by Oven from exactly one source publication: a signed source Loaf, publisher-signed or, for an adopted package, registry-signed. Its receipt must record the source digest, the toolchain identity, the target triple, the profile, the host and target domains, and for every unit it carries the RFC 124 unit identity and payload digest. An asset is one compiled unit: the exchanged object is the RFC 124 unit, and the per-version asset manifest is the only bundle. A Loaf version therefore has as many assets per target and closure as it has units (typically one library unit and a few companions such as a host proc-macro build or a metadata-only check unit), each addressed by its unit identity, and a consumer downloads exactly the units its plan misses. This is the mainstream registries' model of one object per package per variant, with the variant tag replaced by the identity Rust requires. A registry may offer packs, several units in one archive still addressed by their unit identities, as a cold-start optimization; a pack changes no identity and no admission rule. Two digests name an asset and must not be conflated: the **archive digest** is the SHA-256 of the asset archive and is what the asset manifest, attestation, and transport verify; the **unit identity** and payload digest inside it are what admission compares against a plan.

Every asset must carry an attestation that binds the asset digest to the source Loaf digest and the toolchain identity, and names the builder. Builder kinds are:

- **publisher**: built in the publisher's trusted CI; the attestation identity is the workflow identity.
- **registry**: built by the registry's own bakery from the signed source Loaf; the attestation identity is the registry's build workflow.
- **local**: built on the publishing machine; the attestation identity is the publisher's keyless identity.

Consumers may apply different trust policy to each builder kind through RFC 117 registry configuration. The registry must publish which kind produced each asset and must not relabel one kind as another. All three builder kinds are part of this design. A registry-built asset must be admitted to the index only when two independent builds of it, meaning separate build roots in separate runs, are byte-identical. Whether a given registry-built asset was produced for a publisher on request, from a curated public list, or as part of a paid service is registry operating policy and is outside this RFC; the attestation, admission rule, and trust treatment are the same in every case.

Toolchain identity, target triple, profile, and feature closure are **discovery facts**: the asset manifest is filtered by them to find candidates cheaply. An asset is **admitted** to a consumer plan only when, for every unit the plan would take from it, the asset's recorded RFC 124 unit identity equals the identity the plan computed, and the asset's attestation satisfies the project's trust policy for its builder kind. Discovery facts alone are never sufficient: two assets can share all of them and differ in dependency identities, provider outputs, or native inputs. Oven must treat any unit-identity inequality as inapplicable for that unit and bake it from source, and must never admit an asset by version alone. A unit RFC 124 marks machine-local must not be present in a published asset.

A binary is an ordinary compiled unit, so running a published tool through RFC 078's `oven tool run` needs no separate kind of artifact.

**Retention.** No retention rule and no publisher operation removes a source Loaf, event, or manifest. An asset must be kept while its toolchain is supported, which the registry's published policy defines, and an asset's record must name its full toolchain identity so that the rule can be computed. After that its bytes may be removed, recorded by a signed `retire` event naming its archive digest, and a removed asset is an ordinary miss. Nothing is removed because of age or total size.

### Index

The registry must publish a sparse index: one file per package at `index/<scope>/<name>`, containing one JSON object per line, one line per published version and revision. Each line must contain:

| Field | Meaning |
| --- | --- |
| `name` | the full package name |
| `vers` | the version |
| `cksum` | the source Loaf digest, `sha256:` prefixed |
| `facets` | the facet kinds the Loaf declares, such as `incan`, `rust` |
| `rust` | for a Loaf with a Rust facet, its `[rust]` table (`name`, `type`, `edition`, `build-script`), so the resolver can tell a host edge, and a binding that needs a fact, from index lines alone |
| `deps` | the version's Loaf dependencies as the manifest declares them, each `{name, loaf, req, features, default-features, optional, target}`: `name` is the extern key, `req` the RFC 119 version requirement, and `target` absent when unconditional; a key declared once per target appears once per declaration |
| `features` | the version's feature table in the RFC 119 grammar, so the whole graph resolves from index lines before any record or archive is fetched |
| `adopted` | for an adopted Loaf, `{registry, package, version, checksum, revision}` with a `sha256:` prefixed checksum of the upstream archive, and `package` in the upstream's exact spelling; absent for a published Loaf |
| `requires` | toolchain requirements, at least the compiler and Oven ranges |
| `yanked` | whether a yank event is in effect |
| `publisher` | the signing identity of the source Loaf |
| `manifest` | the path of the Loaf's rendered `loaf.toml` |
| `assets` | the number of attested assets |
| `facts` | for each build fact, its binding, the owner identities of any executables it names, and its status: `harvested` or `attested` |
| `advisories` | the number of advisory events in effect for this version |

Index lines must stay small: no descriptions, readmes, or asset tables. A version's records must live under `<scope>/<name>/<version>/`, with each revision's records addressable on their own, and the asset manifest there must list every attested asset with its archive digest, its discovery facts, the RFC 124 unit identity and payload digest of every unit it carries, its builder kind, its attestation reference, and where to fetch it, so that admission is decided from the manifest before any asset is downloaded. A location's tag is transport, not authority: clients must fetch by digest. Both files must be served as static, cacheable content, and the registry should serve them from a CDN.

A consumer may refuse `harvested` bindings by policy and must be able to decide that from the index line, before it fetches the record.

### Version selection

The resolver must filter candidates by the dependency's version requirement and by `requires` against the installed toolchain, and must skip yanked versions unless the lockfile already pins them. It must record the selected version, revision, source, digest, and satisfaction kind (asset or source) in `oven.lock`.

The selection mode must be one of:

- `lowest`, the default, selects for each package the lowest published version that satisfies every requirement on it in the graph;
- `lowest-direct` selects the lowest satisfying version for the project's own dependencies, and the highest for everything else;
- `newest` selects the highest satisfying version.

`oven lock --versions <mode>` must select the mode, and `oven.lock` must record it. A later `oven lock` without the flag must keep the recorded mode. When a lower bound names a version that is not published, the resolver must use the lowest published version that satisfies the requirement, and the report of `oven lock` must show that the bound itself was not tested.

`oven add` must write the newest published version as the requirement unless the user names a version. `oven update` must raise a requirement only when asked, and nothing published must change what a locked project resolves to until then.

### Registry identity and signing

A registry must publish a root public key and must derive its id from the digest of that key, so that the id is self-certifying. The root key must sign only key-management events: delegation to online signing keys, and rotation or revocation of any key including itself. Every registry event, every index file, every asset manifest, and a periodic **checkpoint** naming the event-log head, its length, and a timestamp must be signed by a key delegated from the root. The registry must publish a checkpoint at least daily.

Clients must pin a registry's id and root key through RFC 117 registry configuration and must verify the signature on any index file, manifest, or event before using it. Clients must verify the checkpoint and must refuse content whose checkpoint is older than one already observed for that registry, or older than a staleness bound, so that a stale or frozen copy cannot suppress a yank or an advisory. The default bound is seven days, configurable per registry, and it must not apply in offline mode (RFC 020). Clients must learn endpoints only from configuration and must treat every endpoint whose content verifies as equivalent; a mirror is therefore any endpoint serving the registry's signed content and requires no trust of its own.

A root key rotation must reach clients in two ways: a client must accept a rotation event signed by the root it has pinned, and every toolchain release must ship the newest root, so that a client pinned to a lost or compromised root has a way forward. Key rotation and revocation must be signed events, and the registry should anchor its events in a public transparency log so that a compromised online key can be bounded to the events it signed.

Two signatures therefore appear on registry state, and they answer different questions. A publisher-originated change (a publish, a yank, an ownership or trusted-publisher change) must be authorized by an identity the scope permits, and that authorization must be recorded in the event; the registry must then sign the event into the log with a delegated key. The first signature says who asked; the second says the registry accepted it and when. Publisher signatures on source Loaves are unchanged by this section.

### Encrypted artifacts

A registry may serve artifacts encrypted for its authorized clients, for example a private registry protecting its assets against disclosure of the storage behind it. When it does, unit identities, payload digests, receipts, and attestations remain over the plaintext, so that verification and reuse are identical to the unencrypted case, and the asset manifest must additionally carry the digest of the ciphertext as the transport digest. Clients must verify the transport digest before decryption and the payload digest after it. Key distribution is outside this RFC.

### Registry events

Every state change is a signed event: `publish`, `adopt`, `fact`, `attest`, `asset`, `retire`, `yank`, `unyank`, `ownership`, `trusted-publisher`, `advisory`, `supersede`, `reference`, `reference-check`, and the key-management events `delegate`, `rotate`, and `revoke`. Events must be signed by an identity authorized for the scope at the time of the event, must be append-only, and must be publicly readable. The index, the asset manifests, and the web site must be derivable from the event log plus the immutable artifacts alone. This is the package-level substrate the RFC 079 artifact graph projects from.

**Bound build facts.** An adopted Loaf accrues its RFC 119 build facts one at a time, as a version accrues assets: the registry records one exact selection — toolchain, target, profile, and the complete enabled feature set — and a `fact` event declares the answers that hold under that selection alone. The selection's toolchain is the Rust compiler identity, because a build script's answers depend on it and not on the rest of the toolchain. A fact is published once per binding and, where it carries a `link` or `tool` recipe, per owner identity of the executables it names, so one binding can hold several facts. A later `fact` for the same key must repeat its declared facts exactly, and is otherwise refused, so a record is immutable per binding rather than per version; a record that must state something new states it under a new binding. The rendered `loaf.toml` a consumer reads is a projection of a record's `fact` events, exactly as the index line is a projection of the event log, and neither is authored. A consumer applies the record whose binding equals its own selection exactly and whose executable owners are present, and refuses otherwise; it never interpolates between bindings and never falls back to executing what the record replaced.

**Equivalence attestations.** An `attest` event names one existing binding, the archive digest and RFC 124 unit identity of an Oven-baked unit for it, and the attestation that binds them. It is a different claim from the attestation an `asset` event carries: an asset attestation binds an artifact to a source publication, while an `attest` event says that two independent recordings of the binding agree on its facts and that Oven's bakes from those facts agree. Until one exists, a binding's status is `harvested`, resting on its harvest evidence alone; once one exists, it is `attested`.

### Publishing protocol

Publishing must be possible through two authentication paths:

- **Trusted publishing.** A CI job presents an OpenID Connect token; the registry verifies it against the trusted-publisher configuration of the scope and accepts the publish without any stored credential. This must be available.
- **Publisher tokens.** A scope owner may mint tokens for publishing outside CI. Tokens must be scoped to a set of packages and an expiry, and the registry should warn on tokens that never expire.

A scope must be claimed by proving control of the source-forge organization or account with that name, compared in lowercase, through the OpenID Connect identity signing already requires. The first valid proof of a name wins, and an `ownership` event must record it with the forge it came from. A scope must not be claimable without that proof.

On publish the registry must: verify the caller is authorized for the scope; reject an existing `(name, version)`; verify the declared digest against the uploaded archive; verify the signature and transparency-log inclusion; parse `loaf.toml` and reject a Loaf whose manifest does not describe the archive's contents; verify each uploaded asset's attestation binds it to the uploaded source digest; store the artifacts; append the index line and asset manifest; and emit a `publish` event. Publish must be atomic from the consumer's point of view: a version is either fully visible or absent.

**Adding assets after publication.** Source publication is immutable, but the set of attested assets for a version grows: CI jobs for other targets finish later, and the registry bakery runs after the fact. The registry must accept an `asset` event for an existing `(name, version)` from an identity authorized for the scope or from the registry bakery, carrying the asset archive, its receipt, and an attestation bound to the version's source digest. The registry must verify that binding, must reject an asset whose source digest does not match the published version, and must project the new entry into the asset manifest atomically. Publishing an asset whose archive digest already exists is idempotent. Publishing an asset that claims a unit identity already present in the manifest with a different payload digest must be refused and recorded as a determinism conflict for that unit identity, so that consumers are never offered two payloads for one identity.

### Fetching and verification

Oven must fetch over HTTPS with a certificate policy that behaves identically on every supported host, and should offer an explicit opt-in to the operating system's certificate store for environments that require it. Oven must verify every downloaded artifact's digest against the index or asset manifest before using it, must fail closed on mismatch, and must verify attestations according to the active trust policy. A source Loaf a lockfile pins that cannot be fetched must fail the build, with the reason from any advisory in effect for it. Oven must not execute any content of a downloaded artifact during resolution or fetching.

The Loaf store is the only cache and the only offline source. An offline mode must satisfy resolution from the store and the lockfile alone and must fail fast on anything absent, as RFC 020 requires. Assets are addressed by the unit identity RFC 124 defines, and importing an asset must insert its units into the store through the same path a local bake uses, so a downloaded unit and a locally baked unit with equal identity are one entry. A unit RFC 124 marks machine-local must not be published as an asset.

### Adoption from crates.io

crates.io is a source the registry adopts from, never a source consumers read. Adoption turns one crates.io package version into a Loaf, once:

1. **Inputs.** The registry reads the package version's crates.io sparse index entry (dependencies with requirement, kind, target predicate, rename, optional, default-features and features; the feature table; the checksum) and downloads the published package archive, which it must verify against that checksum before reading it. From the archive's published manifest it reads the descriptive fields step 4 records and, for the Loaf itself, only the edition, the library target (crate name, root, and whether it is a procedural macro), and whether the package has a build script (a declared `build` path, or a `build.rs` at the package root when none is declared), which it writes as `[rust] build-script`; a package that declares no library target uses the conventional root. Adoption runs no Cargo.
2. **Translation.** The registry writes the Loaf manifest in the RFC 119 Rust facet grammar under these fixed rules: development dependencies and build dependencies are dropped; a dependency on a procedural-macro package stays and is a host edge; `links` is dropped; every implicit optional-dependency feature becomes an explicit feature; a feature member that names an optional dependency's feature without `?` becomes `dep:<name>` plus `<name>/<feature>` (whether the feature `<name>` also turns on depends on the target and is the resolver's rule, RFC 119); a weak member stays `<name>?/<feature>`, and a member naming a non-optional dependency's feature is unchanged; a renamed dependency keeps its rename as its dependency key. A package version whose manifest cannot be stated in the grammar is refused with the construct named.
3. **The adopted Loaf.** The Loaf archive contains the generated `loaf.toml` and the package's sources unchanged; the upstream manifest and any build script travel in it as inert files nothing reads. The package's own license and README files are kept unchanged. Where the package ships no license file, adoption adds a root `LICENSE` stating the SPDX expression its published manifest declares; where it ships no README, adoption adds a root `README.md` with its published description and links to its crates.io page, repository, and documentation. These additions are the only files adoption authors besides `loaf.toml`, and an added file must say that the registry wrote it. An added file is part of the source archive, so it determines the archive digest. It is not a compilation input, so it changes no unit identity (RFC 124). The registry signs it and hosts it; consumers fetch it from the registry, never from crates.io. Its identity is its archive digest, and its index line carries the `adopted` provenance.
4. **The `adopt` event.** Adoption is recorded as one signed `adopt` event per revision of a `(name, version)`, carrying the manifest digest and provenance, and an `about` record copied from the published manifest: `license`, `license-file`, `readme`, `description`, `authors`, `rust-version`, `repository`, `homepage`, and `documentation`, each absent when the manifest omits it. `about` itself never reaches `loaf.toml`, an index line, or any unit identity; a file adoption adds from it is part of the archive (step 3). A revision is immutable: an identical repetition is a no-op, a different one is refused, and a correction is a new revision. The registry must refuse a new `adopt` event whose manifest doesn't state `[rust] build-script`, so that a package with a real build script can't read as having none.
5. **Closure under default dependencies.** An `adopt` event is refused unless every dependency active under the version's default features, on any target, is satisfied by an already adopted or published version: every non-optional declaration, target-conditional ones included, and every optional declaration the default feature set enables through `dep:<name>`, a non-weak `<name>/<feature>`, or a feature that does either. Every pin therefore resolves its default graph on its own. A dependency reachable only through a feature beyond the defaults may be absent from a pin; a remote provider answers it "not published" and the local store adopts it on demand (provider rule 2). Closure over every optional declaration is not required: optional dependencies of optional dependencies reach versions that cannot be adopted at all, such as a package version with no library target.
6. **Build facts.** For a package with a build script, the registry records build facts per binding (see "Registry events"): it compiles the script once with the pinned `rustc` against the build-dependency Loaves, executes it sandboxed under the binding's declared environment, and records its directives in the RFC 119 vocabulary. A package without a build script needs no facts. A consumer the registry can serve executes nothing.
7. **Determinism.** Each revision records the adoption tool version that produced it. Rebuilding an adopted archive from the same upstream input with that version must reproduce exactly the digest the index names, and the registry must not serve an archive that does not.
8. **Revisions.** A faulty adoption must be corrected by a new `adopt` event with the next revision, naming the revision it supersedes and the reason. A revision may change only `loaf.toml` and the files adoption adds, never the package's sources, so build facts carry across the revisions of a version. A revision has its own archive digest, and its compiled units change only where its compilation inputs change, so a revision that only adds a license file needs no new bake. An adoption recorded before revisions existed is revision 1, and its tool version is unrecorded. Only adopted scopes have revisions; a community or native package publishes a new version. New resolutions must select the highest revision of a version, and a lockfile pinned to an older revision must keep resolving to it and must surface the supersession through RFC 076.

**Selection hazards.** Adoption, and a local fill-in for a missing version, must not select a version younger than the registry's published release-age floor. Two signals block automatic selection until the registry reviews them: a patch release that adds a dependency edge or a build dependency, and a burst of yanks by one owner. An upstream yank or removal, and a published advisory against an adopted source, must become a `yank` or `advisory` event keyed on the source digest.

**Archive intake.** Adoption must extract a package archive under the archive rules its source enforces, without relying on that source to have enforced them. It refuses links, device entries, entries whose recorded sizes disagree, names that collide under case folding or Unicode normalization, and decompressed sizes beyond a bound. Adoption, `check` and Oven read archives through one shared reader.

A C library adopted from an upstream release follows the same rules under the `c` scope (RFC 159).

### Provider surface

Every Loaf registry endpoint, remote or local, serves one surface, and Oven uses no other:

| Request | Answer |
| --- | --- |
| index line by Loaf name | the signed index file for that Loaf |
| record by name, version and revision | the manifest, events, and asset manifest of that revision, the highest when none is named |
| Loaf archive by digest | the archive whose digest is requested |
| build facts by binding | the facts recorded for exactly that toolchain, target, profile, and feature set, each with the executable owners it requires, or "not recorded" |
| compiled unit by unit identity | the asset carrying a unit with exactly that RFC 124 identity, or "not published" |

1. Every answer is content-addressed or signed and is verified by Oven before use. Because of that, a remote HTTPS endpoint, a mirror, a git checkout of the registry, and Oven's local store are interchangeable providers; Oven's behavior, output, diagnostics, and lock contents must not depend on which one answered.
2. A miss is the provider's business. A remote provider answers a missing record, binding, or unit with "not published". A local provider may instead fill the miss by running the registry's adoption tool: it adopts the package version or records the binding, then answers as if it had been present. Filling must be a separate step, never part of resolving, must run sandboxed, and must say in Oven's output what it is adopting or running. It is on by default, and project or organization policy can turn it off. A version is adopted locally only when no published version satisfies the requirement, and `oven.lock` must mark it as locally adopted. Adoption is deterministic, so another machine that fills the same miss with the same adoption tool version gets the same digest. Oven's code path is otherwise the same in both cases.
3. Before compiling a unit, Oven asks the provider for it by unit identity. A unit Oven compiles because the provider did not have it is recorded in the local store as an `asset` event with builder kind `local`, its archive stored by digest; a remote provider accepts assets only through the publishing protocol and the trust policy for their builder kind.
4. The local store is a cache in front of the authority, never a second authority (RFC 117): it keeps what `loaves.pub` served and what Oven had to adopt or bake itself, under the same unit identities, and it cannot be selected as a registry.

Registered compatible registries under RFC 117 configuration implement this surface. A private registry is a registered compatible registry with its own root key and id; it implements the same surface and is not a variant of it.

### Asset unavailability

Oven must treat any failure to obtain an asset, including a registry answering that the asset is unavailable, throttled, or over quota, exactly as it treats an inapplicable asset: it bakes the affected units from source, records the fallback in the receipt as `baked` with the reason, and neither fails the build nor retries in a way that amplifies load. A registry may require per-client credentials at the manifest to reach asset locations, and anonymous access must mean no credential at all. Clients must fetch by digest and must not assume asset paths are stable. Index files, asset manifests, the event log, and source publications, adopted Loaves included, must be served and must be sufficient on their own to resolve and bake any project.

### Documents, references and the web surface

Each published version may carry **documents**, such as its README, license, license notice, and changelog, recorded by digest. A **reference** is a separate derived publication, bound to the source Loaf digest and the binding, so that a published archive never changes to gain one. It holds the declarations and documentation a compiler extracts, and the calling contract an Incan consumer checks against. A `reference` event publishes it and a `reference-check` event records its verification. It must be produced from ordinary compiled output, with no unstable compiler mode, and RFC 082's documentation bundle is the handoff from it to any renderer.

The registry must publish a **catalog**: an index of all packages and one document per package, generated from the index, event log, and artifacts. The web surface must be generated from the catalog and must show, per package: versions and yank state with reasons, facets, scope owners and trusted publishers, the asset matrix with builder kinds, the declared dependencies of each version, advisories, documents, and the reference where one exists. It may show descriptive metadata such as license, keywords, and repository, and download counts where the host can provide them as a published projection. It must not depend on a live service for reads.

The registry must also publish, beside each package's index file, a per-package README projection at `<scope>/<name>/README.md` (for adopted packages, `crates-io/<name>/README.md`). It lists the package's upstream page when adopted and every version with its adoption or publication date, license, recorded build facts, and admitted units, and is regenerated from the event log like every other projection. Package hosts that cannot show a per-package README, such as an OCI registry, link to it from the package description.

### Mirrors

Because every artifact and index file is immutable or event-derived, a mirror is a copy of the index, asset manifest, artifact, and event directories. Oven must accept a mirror as a registered compatible source and must verify against the same digests and signatures, so a mirror can never alter content.

## Design details

### One name, and the records written before it

`loaves.pub` is the registry's only name. Its repository, its tools, the container-registry paths that hold published artifacts, the event schema `loaves.pub/event/1`, the event actor and the catalog schemas all carry it.

Records written while the registry was named `incan.pub` keep the strings they were written with. That covers events under `incan.pub/event/1` and their actor, the attestation identities of builds that ran under the earlier repository name, and the notice adoption wrote into the archives it published then. The event strings are hashed into event ids, and the notice sits inside archives that published digests name, so rewriting either would change identities that lockfiles rest on. Readers must accept both forms as the same records, and tools must not treat the earlier ones as stale. Artifacts are identified by their digests, not their paths, so moving them to the `loaves.pub` paths changes no identity a lockfile records.

### Interaction with RFC 117

`loaf.toml` declares every package dependency with the `loaf` key and lets a project name a registered Loaf registry; RFC 117 retires the `crate` key. This RFC adds no manifest syntax; an adopted Loaf's manifest uses RFC 119's Rust facet grammar. The `requires` index field maps to the requirement ranges RFC 117 lets a project declare. Registry trust policy, credentials, and registered sources stay in Oven-controlled user or organization configuration, never in `loaf.toml`.

### Interaction with RFC 118

`oven publish`, `oven add`, `oven lock`, `oven update`, `oven yank`, and the `oven dependency` family are RFC 118's surfaces; this RFC defines what they exchange with the registry, and the `--versions` selection for `oven lock`.

### Interaction with RFC 078

RFC 078's `oven tool run` is how a published tool runs without being added to a project. This RFC only requires that binaries are ordinary assets.

### Interaction with RFC 119

The Rust facet is what makes Rust-only publication possible, and adoption is how the Rust ecosystem reaches it. Asset applicability reuses RFC 119's host and target domains and per-unit feature closures verbatim; this RFC introduces no new notion of a build unit. A Cargo project is not Loaf-native and cannot publish here until it converts to `loaf.toml`.

### Interaction with RFC 159

A C library adopted from an upstream release is adopted under this RFC, into the `c` scope, with its C facet written by the registry.

### Interaction with RFC 124

RFC 124 is the normative source for unit identity, the separation of publication provenance from compilation identity, staged identity completion, and the store's sharing and collection behavior. This RFC depends on it: asset admission is unit-identity equality, discovery facts only narrow the search, and the store that receives imports is the store RFC 124 defines. Assets are units and manifests are the only bundles, as decided in both RFCs.

### Interaction with RFC 123

The executable representation is part of the source Loaf for any Incan facet, so a consumer never depends on generated Rust reaching it through the registry. RFC 123 owns its format and versioning.

### Interaction with RFC 076 and RFC 079

Yank, advisory, and supersede events are the source of the recovery information RFC 076 requires Oven to surface. RFC 079's artifact graph is a projection over the event stream and artifacts defined here.

### Compatibility with RFC 034

Nothing was published under RFC 034, so there is no migration. The constraints RFC 034 established for hosting remain in force as goals of this RFC. The `.incanpkg` format, `.incnlib` manifest dependency, flat namespace, and token-first phasing are withdrawn.

## Prior art

- **crates.io and Cargo.** The sparse index (one small static file per package) and the flat, identity-keyed lockfile with a checksum per artifact are copied here almost verbatim; immutability with yank is kept. crates.io added trusted publishing in 2025, so the trust posture this RFC starts from is arriving on the Rust side too. What crates.io never had is a compiled tier, and this RFC exists to add one beside it, not to replace it.
- **uv.** One HTTPS client with pure-Rust TLS and an opt-in for the platform certificate store; retries and caching in the client; a content-addressed cache that installs by linking; one universal lockfile valid on every platform; static metadata first and isolated builds second; no package code executed during resolution. The client and cache design here follows it, and its `--resolution` modes, `highest`, `lowest` and `lowest-direct`, name the same three choices offered here.
- **Go modules.** Minimal version selection builds with the lowest version that satisfies every requirement, and upgrades are explicit. It is the closest precedent for the default here.
- **Cargo's minimal-versions experiments.** Selecting the lowest version everywhere exposes other packages' wrong lower bounds, which is why a direct-only mode exists beside the full one.
- **Debian and conda-forge.** A packaging revision beside the upstream version (`1.2.3-2`, a build number) is the established way to fix a packaging mistake without claiming an upstream release.
- **npm.** Content-addressed cache with integrity on every lock entry; scoped names as organizations; granular expiring tokens; provenance through Sigstore and trusted publishing; a typo guard before running anything not local. Its history also supplies the cautions: install-time scripts, mutable tags, oversized metadata documents, flat namespaces, and long-lived tokens each caused an incident class, and each is excluded here by design.
- **PyPI wheels, Maven Central, Homebrew bottles, Nix binary caches.** Four ecosystems that added a compiled or prebuilt tier above a source ecosystem without displacing it. Nix is the closest in identity model; wheels are the closest in how a source-first registry grew an artifact tier.
- **Conan.** Binary packages selected by an identity over compiler, settings, options, and dependencies, with build-from-source as the fallback, in an ecosystem with no stable ABI. The asset applicability rule is that model stated over RFC 124 unit identities.
- **TUF, Go's checksum database, and Sigstore.** Offline root and online delegated keys, a signed timestamp against freeze and rollback, and proxies that are deliberately untrusted caches because every statement is signed; keyless publisher identity with a public transparency log. The registry identity and signing section is assembled from these three.
- **warg.** A registry for compiled WebAssembly components with content addressing and a Certificate-Transparency-style log; the same tamper-evidence goal for compiled artifacts.
- **The serde_derive episode of 2023.** A widely used Rust package shipped a precompiled artifact with no source of record, no verifiable link to source, and no way to decline; the ecosystem rejected it within a month and its maintainer asked for a first-class mechanism. The two-tier design, builder-kind attestations, and bake-from-source fallback here are that mechanism.

## Alternatives considered

- **Amend RFC 034 in place.** Rejected. The package format, resolution flow, and phasing all rest on pre-Loaf assumptions; a rewrite is smaller than the amendment.
- **Source-only registry, as crates.io.** Rejected. It discards the one property that distinguishes Oven, receipt-bound compiled artifacts, and keeps every consumer compiling every dependency on every machine.
- **Refusing anything the registry hasn't adopted.** Rejected. A registry can't adopt every package and feature combination, and a remote one never compiles on demand, so uncommon packages would simply not work. Filling the miss locally, as a visible and sandboxed step, keeps them usable.
- **The newest version by default, as Cargo and npm do.** Rejected. Every new upstream release changes what a fresh resolve produces, and a project's declared minimum is never what gets tested.
- **Fixing a faulty adoption by replacing it in place.** Rejected. A lockfile pinned to the old digest would fail with no explanation, while a revision keeps it building and tells it why.
- **Flat namespace.** Rejected. Rust-only Loaves guarantee collisions with crate names, flat spaces are where squatting happens in every registry that has one, and a flat name cannot tell an adopted package from a native one.
- **A scope named after the registry for native packages.** Rejected. It says nothing about who provides a package, and scope names are close to permanent because they enter every manifest and lockfile.
- **Claiming a scope freely on first publish.** Rejected. It invites squatting, and a mix of proven and unproven scopes would be a second trust tier to explain.
- **Short-lived signed URLs per plan for assets.** Rejected. They need a live service, and they show the registry each project's plan.
- **`@scope/name` spelling.** Rejected in favor of `scope/name`. The `@` character is the conventional version separator in tool-run syntax (`tool@1.2`), and the slash form matches container and source-forge conventions.
- **Assets built only by publishers.** Rejected as the sole path. Most publishers will not bake for every target; a registry bakery over signed source is what makes assets common rather than rare. Kept as one of three builder kinds with distinct trust.
- **Consumers read crates.io directly, with `loaves.pub` offering only compiled assets for crates.** Rejected (an earlier revision of this RFC). It kept Cargo's model on the consumer side: a crates.io index reader and a crate-manifest reader in Oven, Cargo as a download tool, and two sources of truth for one dependency. Adopting each package version once into a hosted Loaf costs the registry storage for adopted sources, and in exchange every consumer reads one format from one surface and no build path ever contacts crates.io.
- **Translate crate metadata with Cargo at adoption.** Rejected. The sparse index entry and the published package manifest already hold everything adoption needs, so a Cargo invocation would add a dependency without adding information.
- **Tokens first, signatures later.** Rejected. Long-lived publish tokens are the documented origin of the largest registry compromises elsewhere; trusted publishing is cheaper to start with than to retrofit.
- **A mutable registry database as the source of truth.** Rejected, consistent with RFC 079: signed events over immutable artifacts are auditable, mirrorable, and static-site friendly.

## Drawbacks

- **Asset matrix growth.** Toolchain × target × profile × closure can explode. The mitigation is that only closures the toolchain pins, publishers bake, or the bakery selects ever exist; the index never promises completeness, and a miss is a bake, not a failure.
- **Bakery cost.** Registry-built assets are compute the project pays for, against a hard cost cap. What the registry chooses to build is operating policy.
- **Trust complexity.** Three builder kinds and per-kind policy are more to explain than "download the tarball". The default policy should make the safe choice invisible.
- **Scoped names are longer.** `acme/widgets` is less pretty than `widgets`, and every registry that started flat has wished it had not.
- **Filling a miss runs third-party code on a consumer's machine.** Recording a build fact compiles and runs that package's build script. It is sandboxed, visible, and can be turned off by policy, and it only happens for what the registry doesn't have.
- **Fixes do not arrive by themselves.** A project stays on what it asked for until someone runs `oven update`, which is why `oven audit` reports advisories on locked versions.
- **A fresh lock depends on what has been published.** Adopting an older version of a package lowers what a fresh lock selects for any dependency whose requirement allows it, and that version may never have been built with those dependents. Locked projects are unaffected, and `oven add` raising a project's own requirements limits this to indirect dependencies.
- **Other packages' lower bounds get tested.** A dependency whose declared minimum is wrong fails under the default, where a newest-first resolver would hide it. The failure is loud, and `lowest-direct` or a raised requirement resolves it.
- **Adoption makes the registry responsible for its translations.** A mistake is the registry's to fix, and an upstream release is unavailable from a remote registry until it is adopted.
- **A registry is infrastructure.** Even a mostly static one needs a publish service, an event log, and operations. RFC 034's cost and hosting constraints are carried forward for this reason.

## Implementation architecture

Non-normative. The registry client belongs in Oven's build-system ring rather than anywhere the compiler links: one HTTPS client with a pure-Rust TLS stack and an opt-in for the platform certificate store, one retry and caching layer, and one provider-surface client whose endpoints may be remote, mirrored, or the local store. The adoption tool belongs to the registry, not to Oven, and a local provider runs that same tool. The Loaf store is the cache for index files, asset manifests, source Loaves, and assets alike, keyed by payload digest with a separate name-to-digest index, which is also what lets two projects share one compiled unit. The publish service is small: authentication, validation, storage, index append, and event emission; everything read-side is static files behind a CDN. The bakery is an Oven invocation over a signed source Loaf under a pinned toolchain, producing assets whose attestations name the registry. The web surface is a static-site generator over the event log.

## Layers affected

- **Oven resolver and lockfile**: one provider surface for every registry endpoint, compiled-unit lookup and local publication, toolchain-requirement filtering, asset applicability, version selection modes, revisions, satisfaction kind and attestation facts in `oven.lock`.
- **Oven store**: content-addressed storage of index files, asset manifests, source Loaves, and assets; offline mode over the store alone.
- **Oven publication**: `oven publish` bakes, packages, signs, attests assets, and uploads; `oven yank` emits signed events; trusted-publishing and token flows.
- **Oven configuration**: registered sources, trust policy per builder kind, certificate-store opt-in.
- **Registry service**: publish validation, adoption from crates.io (index entry, verified package archive, translation, sandboxed build-script facts per binding), artifact and event storage, index projection, the provider surface, static web surface.
- **Compiler**: no parser, typechecker, or emission changes; the source Loaf carries the RFC 123 executable representation the compiler already produces.
- **Docs / Tooling**: publishing, consuming, verification, and failure-mode documentation; asset matrix and provenance in the web surface.

## Design Decisions

- **Every dependency is a Loaf.** Adoption is how other ecosystems arrive, and the `crate` dependency kind does not exist.
- **`loaves.pub` is the registry's only name,** in its repository, tools, artifact paths and schemas. Records written under `incan.pub` keep their strings, because their identities rest on them, and readers accept both forms.
- **A miss can be filled locally,** as a separate, sandboxed, visible step that runs the registry's tool, on by default and off by policy. The registry can't cover every package and feature combination, so refusing would leave uncommon packages unusable.
- **Faulty adoptions are corrected by revisions,** and only adopted scopes have them. Authors never set or see one.
- **The first part of a name is its owner,** with `incan`, `crates-io`, `c`, `std` and `oven` reserved, plus any scopes a registry's published policy adds.
- **No name ever has a capital letter.** Adopted names keep their upstream spelling in lowercase.
- **A scope is claimed by proving control of the source-forge organization or account with that name, and is held by that account's immutable numeric id.** A name can be registered again after a rename or deletion, but an id cannot, so ownership never passes to whoever registers a name next. Deleted scopes and names are tombstoned.
- **`oven add` never guesses between packages that share a short name.**
- **The lowest version is the default,** so nothing changes until someone asks. `newest` and `lowest-direct` are opt-ins recorded in the lockfile, `oven add` records the newest published version, and `oven update` raises it on request.
- **`rust::` reaches a Rust facet and `pub::` an Incan facet,** and a mixed Loaf offers both.
- **Assets are kept while their toolchain is supported,** then may be retired by a signed event, never because of age or size. An asset's record names the full toolchain identity, so that rule can be computed, while a build fact keeps only the Rust compiler identity, so a toolchain release doesn't invalidate every fact.
- **A published tool runs through RFC 078's `oven tool run`,** and binaries are ordinary assets.
- **The site renders documents and references,** with RFC 082's bundle as the handoff to renderers. There is no separate documentation asset.
- **A root rotation reaches clients both ways:** signed by the old root, and shipped with each toolchain release. Checkpoints are at least daily, with a seven-day default bound that does not apply offline.
- **Asset locations are protected by optional per-client credentials at the manifest,** anonymous by default, and fetched by digest.
