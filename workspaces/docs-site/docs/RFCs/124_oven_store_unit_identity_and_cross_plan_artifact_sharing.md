# RFC 124: Oven store unit identity and cross-plan artifact sharing

- **Status:** Draft
- **Created:** 2026-09-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 123 (package executable representation)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution)
- **Issue:** —
- **RFC PR:** [#1477](https://github.com/encero-systems/incan/pull/1477)
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

Oven already reuses compiled work safely: a sealed plan carries a receipt whose identity covers every input, and a build that matches a receipt reuses its output without recompiling. The unit of that reuse is the whole plan. Two projects whose plans compile the same fourteen dependency units store fourteen units twice, a unit compiled in one worktree cannot satisfy a plan in another, and nothing can tell that two ten-gigabyte build directories hold the same artifacts. This RFC makes the **compiled unit** the store's addressable object. Every unit Oven produces carries a unit identity derived from its effective compilation inputs, including the identities of the units it links against, kept distinct from the publication provenance that names where its source came from and from the digest of the bytes it produced. Plans reference units by identity; the store holds each identity once, shares it across plans, projects, and worktrees on a machine, and collects units no receipt or lease reaches. The same identity is what a registry asset carries under RFC 125, so a unit downloaded from `incan.pub` and a unit baked locally are one store entry. The existing rule that a plan must never link two distinct compiled instances of one package becomes checkable at plan time instead of at link time.

## Core model

1. **The unit is the object.** A compiled unit is one rustc invocation's durable output for one crate in one domain: a library, a procedural macro, a build-script executable and its recorded output, a binary, or a test harness. The store addresses units, not plans.
2. **Three identities, kept apart.** A unit has a *publication provenance* (which registry checksum, source Loaf, or path it came from, and who signed it), a *unit identity* (a digest over its effective compilation inputs: the source bytes it compiles, the manifest facts that reach compilation, toolchain, target, domain, codegen facts, features, crate type, and the unit identities of every dependency it links), and a *payload digest* (the bytes it produced). Reuse is decided by unit identity alone. Provenance is recorded and checked for trust; it is not an identity input, so a republication that changes only version metadata or documentation does not invalidate unchanged code. Because dependency identities are inputs, identity is a Merkle root over the closure.
3. **Identity completes in stages.** Units that generate inputs for others, build scripts and procedural macros, carry a base input identity that is known before execution and a receipt identity that is known after. Downstream identities are finalised from admitted provider receipts, as RFC 119 already requires, and no unit compiles before its identity is final.
4. **Equal identity means interchangeable; nothing less does.** Oven may substitute one unit for another only when their unit identities are equal and their payload digests agree. Same package and version with different identity is a miss, and if both would enter one link, it is a refusal that no ownership declaration waives.
5. **A plan is a set of unit references plus a receipt.** The plan receipt records which unit identities satisfy which crates and how each was obtained: baked here, reused from the store, or imported from a registry asset. Reusing a plan output remains possible and is now a special case of every unit hitting.
6. **The store is content-addressed with a name index.** Unit payloads live once under their identity. A separate index maps package name, version, and plan facts to identities so a planner can ask "what do I already have for this crate under these facts" without enumerating the store.
7. **Sharing crosses every local boundary.** Plans, projects, worktrees, and profiles share units through identity. A unit compiled for a release plan in one checkout satisfies a release plan in another checkout of the same or a different project.
8. **Collection is by reachability.** A unit is live while a receipt, lease, or policy pin reaches it. Everything else is collectable, and collection is a store operation a user can invoke and inspect.
9. **Local and registry are the same identity space.** RFC 125 assets are unit-identity-addressed. Importing an asset is a store insert; publishing one is a store export. The registry never needs a second notion of what makes two compiled artifacts the same.

## Motivation

The current Oven store is correct and conservative in the right way: nothing is reused without a receipt match, and the receipt covers the inputs that matter. What it lacks is granularity, and the cost of that was measured during the 0.6 dependency audit. Two freshly generated projects with identical dependency closures each baked their own debug and release Loafs, four sealed artifacts of 51 to 54 MB holding the same fourteen compiled rlibs. On a maintainer's machine, twenty build directories under one development root totalled 52 GB, two of them the compiler itself at 10.8 GB and 10.4 GB a few commits apart, indistinguishable to any tool because the artifacts inside carry no identity that another directory could recognise.

The same audit recorded why the naive fix is wrong. Two independently resolved closures of identical source produced ABI-incompatible rlibs for the same tokio version, and linking both into one executable produced a runtime panic because a runtime object created through one instance was invisible to code compiled against the other. Oven now detects that case at link time by comparing digests and refusing. Sharing units by package and version would reintroduce the hazard everywhere; sharing them by an identity that includes the whole compilation context removes it by construction and moves the check to planning, where it can name both plans in the diagnostic.

The Rust project has arrived at the same conclusion independently. The Cargo team's 2026 goal is a cross-workspace build cache, preceded in 2025 by splitting the target directory and regrouping artifacts by build unit, and early experiments with a content-addressed store behind it are under way. Oven starts from a stronger position because its receipts already encode the inputs Cargo has to reconstruct, and because RFC 125 needs exactly this identity to exchange units across machines. Without unit-level identity, a registry can only offer whole plans, and whole plans almost never match another project.

## Goals

- Define the compiled unit as the store's addressable object and the unit identity as its key.
- Define what the identity must and must not include, so it is exact enough to guarantee interchangeability and portable enough to match across machines.
- Define how plans reference units, how reuse is decided, and how the two-instance hazard is refused at plan time.
- Define store obligations: content addressing, the name index, reference tracking, leases, a verification lifecycle that checks digests at admission and trust boundaries without rehashing warm paths, and collection by reachability.
- Keep publication provenance, unit identity, and payload digest distinct, so unchanged code republished under a new version is reused and every consumer records the exact payload it linked.
- Stage identity completion along the RFC 119 unit graph so provider outputs are admitted before the units that consume them are identified, with no second graph.
- Define the observability a user needs: per-unit hit and miss in `oven plan`, satisfaction kind in receipts, and a store inspection surface.
- Make the identity the one RFC 125 assets carry, so registry import and local bake populate one store.

## Non-Goals

- Changing what a unit compiles to, how rustc is invoked, or which crate types exist. RFC 119 owns the unit graph; this RFC owns how its outputs are stored and reused.
- A remote or distributed store protocol. That is RFC 125.
- Reproducible-builds guarantees in rustc. This RFC depends on the compiler's determinism where identity must be portable and states that dependency; it does not fix compiler nondeterminism.
- Sharing incremental-compilation state or other transient compiler caches.
- Any change to project receipts' role as the record of what a project selected. Receipts gain unit references; they do not lose anything.

## Guide-level explanation

### The second project is fast

A developer creates a second Incan project on a machine that already has one. Both depend on the same standard library components and runtime crates at the same toolchain.

```text
$ oven bake
  Planning app-two (release) ...
  14 units: 14 reused from store, 0 to bake
    tokio 1.53.1        sha256:4b1c…  reused (baked by app-one, 2026-09-09)
    serde_json 1.0.149  sha256:9e02…  reused
    …
  Baking app-two ...   1 unit
  Sealed plan sha256:c0ff…
```

Nothing about the first project leaked into the second. The units matched because their identities matched, which means every input matched.

### A near miss is a miss, and it says why

```text
$ oven bake --profile dev
  14 units: 3 reused, 11 to bake
    tokio 1.53.1  sha256:77aa…  miss: profile differs from stored sha256:4b1c… (release)
```

### Two instances of one crate are refused before linking

```text
$ oven bake
  error: plan would link two compiled instances of tokio 1.53.1
    sha256:4b1c…  via app-three → tokio (features rt-multi-thread, macros)
    sha256:d2e9…  via query-provider → tokio (features rt-multi-thread, macros, net)
  hint: unify the feature closure in loaf.toml so both resolve to one unit, or place the provider behind an
        explicit isolation boundary (RFC 119) so no Rust type or runtime state crosses between the two
```

The same condition today is caught when the executable is linked and diagnosed from symbol names. Under this RFC it is caught when the plan is computed and named in terms the user wrote.

### The store is inspectable and collectable

```text
$ oven store status
  units 1,312 · 6.1 GB payload · 4.4 GB physical (hardlinked)
  reachable from 9 receipts, 2 leases · 380 MB collectable

$ oven store gc
  removed 41 units (380 MB); kept everything a receipt or lease reaches
```

### Registry assets land in the same place

When RFC 125 is in place, a unit that arrives from `incan.pub` shows in `oven store status` with `imported` as its origin and the attestation that vouched for it. A later local bake of a unit with the same identity is recognised as already present and is not recompiled.

## Reference-level explanation

### Compiled units

A compiled unit is the durable output of one rustc invocation for one crate in one domain, together with the metadata needed to link against it. Unit kinds are library, procedural macro, build-script executable with its captured output, binary, and test harness. Every unit must record its host or target domain as RFC 119 defines them. A build-script's captured output (emitted cfgs, link directives, environment) is part of the unit that consumes it, not a separate shared object.

### Unit identity

A unit has three distinct identities, and the store must name them separately:

- **Publication provenance**: the registry checksum, source Loaf digest, or path source, together with the signer or attestation that vouches for it. Provenance must be recorded with every unit and checked against trust policy; it must not enter the unit identity.
- **Unit identity**: the digest over effective compilation inputs defined below. It decides reuse.
- **Payload digest**: the `sha256:` of the produced bytes, uncompressed. It verifies storage and transport. Compression is a storage and transport encoding chosen per store or per registry and never enters any identity, so a unit compressed at one level locally and at another for publication is one unit.

A unit identity must be a `sha256:` digest over a canonical serialization of at least:

- the content digest of the source files the unit actually compiles, after path remapping, excluding files the compilation does not read (documentation, manifests, tests not built, publication metadata);
- the manifest facts that reach compilation: crate name, edition, and any package fact the code or the compiler observes, such as a version string exposed through an environment macro or embedded in crate metadata. A fact the compilation does not observe must not enter the identity;
- the toolchain identity (compiler version and host, as Oven records it);
- the target triple and the domain (host or target);
- the profile facts that affect codegen: optimisation level, debug-info level, codegen units, panic strategy, LTO mode, target features, and any other flag Oven passes that changes output;
- the resolved feature set of the unit;
- the crate type and edition;
- the unit identities of every unit it links against, in canonical order;
- for units that consume provider outputs, the receipt identity of each admitted provider (build script or procedural macro) whose generated inputs, cfgs, environment values, or link directives it consumes;
- for units with native inputs, the digests of the native libraries and headers the unit links or includes.

A unit identity must not include absolute paths, timestamps, hostnames, user names, publication signatures, or the identity of the plan that requested it. Oven must remap source paths so that the same source at two locations produces the same identity, and must derive the compiler's crate disambiguator from the unit identity rather than from the package version, so that a version-only republication of unchanged code yields the same unit.

**Acceptance case.** A package republished at a new version whose compiled sources, observed manifest facts, features, dependencies, and provider outputs are byte-identical must resolve to the same unit identity and reuse the existing unit. A package whose new version is observed by its own code, for example through a version macro, legitimately produces a new identity, and the identity inputs make that visible.

Two units with equal unit identity must be treated as interchangeable, subject to the payload rule below. Oven must never substitute units whose identities differ, regardless of package name, version, or apparent compatibility.

**Payload conflicts.** If two bakes with equal unit identity produce different payload digests, the compiler was not deterministic for those inputs. The store must retain both payloads under the one identity, must mark the identity machine-local and ineligible for export, and every plan receipt must record the payload digest it actually linked, so that a consumer is never given a payload other than the one its dependents were compiled against. Interchangeability is therefore promised only for identities with a single observed payload; the nondeterminism question remains open below.

### Plans and reuse

Identity completion is staged along the RFC 119 unit graph; Oven must not introduce a second graph to do it. Units that produce inputs for others (build-script and procedural-macro providers) are planned first with the base input identity RFC 119 defines: permitted input tree, features, host, target, provider and toolchain identities, and approved environment. A provider whose base identity matches a stored receipt is reused; otherwise it is executed under RFC 119 admission and its receipt recorded. Only then are the identities of the units that consume its outputs finalised from the receipt's generated-input digests, cfgs, environment values, and link directives. No unit may compile before its identity is final, and executing an admitted provider is provider execution under RFC 119, not resolution; the no-execution rule for resolution in RFC 117 and RFC 125 is untouched. For each final identity the store is consulted; a present, verified unit is reused, and an absent one is baked and inserted. A reused unit that depends on a provider must identify the provider receipt it reused, as RFC 119 already requires. The plan receipt must record, per unit, the identity and the satisfaction kind: `baked`, `reused`, or `imported`, with the importing attestation reference where applicable.

If two units in one link closure share a source-qualified package identity and linkage role (the same package from the same source kind, both linked as Rust libraries into one artifact) but have different unit identities, the plan must fail before execution with a diagnostic naming both units, the requirement paths that introduced them, and the first differing identity input. No ownership or provenance declaration waives this rule: declaring who owns a closure does not make two instances' types identical or their runtime state shared. The two permitted resolutions are a **coherent closure**, in which the provider's dependencies are recompiled against the consumer's chosen dependency identities so that one unit results, or an **explicit isolation boundary** under RFC 119, in which the second instance lives behind a process or foreign-function boundary across which no Rust type or runtime object passes; the isolation must be declared and receipt-visible. Two units of the same package in different linkage roles, for example a host proc-macro build and a target library build, are not a collision.

Whole-plan reuse remains permitted: a plan whose receipt identity matches a sealed plan may reuse the sealed outputs without consulting units individually. This is an optimisation over the unit rules, not an exception to them.

### Store obligations

The store must hold each unit payload once, addressed by unit identity and payload digest. Payloads are immutable once inserted; the index and referrer tables change only by append and tombstone. The store must verify a payload's digest at admission (insert, import, or first use after the store detects external change to the file) and whenever a unit crosses a trust boundary such as import from a registry. Within one operation, a unit that has been verified and is held by a lease must be treated as immutable and must not be rehashed again; Oven must not recompute the digest of a transitive closure on every logical lookup. Cheap consistency checks (size, modification time, inode) may guard the warm path, with a full rehash on `oven store verify` or on policy. A payload that fails verification must be evicted and the unit treated as absent. The store must keep a name index from package name, version, domain, target, profile facts, and feature set to identities, so a planner can find candidates without scanning payloads. The store must track referrers: every receipt and lease that reaches a unit. Leases keep in-progress plans and explicitly pinned entries live. The store should materialise units into build directories by hard link or reflink where the filesystem allows and must fall back to copying where it does not.

Collection must remove only units no receipt, lease, or policy pin reaches, must be explicit or governed by a stated policy, and must report what it removed. The existing bounded-capacity behaviour continues to apply: the store must refuse an insert that capacity policy cannot satisfy rather than silently evicting live units.

### Registry parity

RFC 125 assets must be addressed by unit identity as defined here. Importing an asset must verify its digest and attestation and then insert its units into the store with satisfaction kind `imported`. Exporting for publication must serialise units with their identities and receipt facts unchanged. A unit whose identity is marked machine-local must not be exported.

### Observability

`oven plan` must report, per unit, whether it will be reused, imported, or baked, and for a miss the first identity input that differs from the nearest stored candidate. Receipts must expose satisfaction kinds through the build report. The store must offer a status view (unit count, logical and physical bytes, reachable and collectable totals) and a per-unit view (identity, package, origin, referrers). Command spellings belong to RFC 118.

## Design details

### Relationship to today's receipts

The current receipt already computes a build-unit identity from intent, compatibility envelope, and frozen inputs, and a separate identity for the project's own source. This RFC refines the first into a per-unit identity that includes dependency identities, and adds unit references and satisfaction kinds to the receipt. The project identity is unchanged. Existing plan-level reuse continues to work through the whole-plan rule.

### Relationship to RFC 117 and RFC 119

RFC 117 defines the project, the lock, and the store as the only cache. RFC 119 defines units, their roles, and the host and target domains. This RFC gives those units a durable identity and defines the store's contract for them. It introduces no manifest syntax and no new unit kinds.

### Relationship to RFC 106 and RFC 123

A native unit in the store is evidence of a compilation, not of source or semantic authority. The checked analysis behind the RFC 106 context graph and the RFC 123 executable representation remain the authority for what a package means and exports; a plan must reach those facts through the representation and the analysis, never infer them from the presence of a compiled unit. The store must keep the semantic and inspection evidence a unit was produced from alongside the native payload, so that a consumer working from reused units still has complete source and generated-output authority. This RFC reuses the RFC 119 unit graph and the RFC 106 and RFC 123 analysis; it introduces no second graphing or analysis path.

### Relationship to RFC 125

RFC 125 assumes a store that shares units and states so only in non-normative text. This RFC is the normative source for that property. RFC 125's applicability rule, that an asset is usable only when the plan's facts equal the receipt's facts, is the unit identity rule stated at the granularity of a published bundle.

### Relationship to RFC 020

Offline and locked modes resolve entirely from the store and the lockfile. Unit-level storage makes offline mode strictly more capable: any unit any prior plan produced on the machine is available to a new plan that shares its identity.

### Migration

The store gains a new layout version. Existing entries are not rewritten; they remain readable for whole-plan reuse until collected, and new plans populate the unit-addressed layout as they bake. No user action is required, and a `gc` after the first few bakes under the new layout reclaims the old entries.

## Alternatives considered

- **Keep plan-level Loafs as the unit of reuse.** Rejected. It is the status quo the audit measured; plans almost never match across projects, so nothing is shared and nothing can be safely collected below the plan.
- **Deduplicate at the file level only.** A content-addressed store for payload bytes with no unit identity, hardlinking identical files. Cargo's early cache experiment took this shape and reported saving half a gigabyte of thirty. Rejected as the primary mechanism: it saves disk where bytes happen to coincide but cannot decide reuse, cannot refuse the two-instance hazard, and cannot feed a registry. It is retained as the store's physical layout beneath unit identity.
- **Key units by package, version, features, and target only.** Rejected. This is the identity that produced two incompatible tokio instances; it omits dependency identities and codegen facts and is exactly what makes sharing unsafe.
- **A compiler-wrapper cache keyed on the rustc command line.** The sccache model. Rejected as the basis: it reconstructs identity from the invocation rather than from the plan, shares nothing the planner can reason about, and cannot express satisfaction kinds or attestations.
- **Compile shared dependencies as dynamic libraries.** Proposed for Cargo in 2021. Rejected. It changes codegen and linking semantics to solve a storage problem, and does not remove the identity requirement.

## Drawbacks

- **Identity is only as portable as the compiler is deterministic.** Known nondeterminism, notably in Windows debug information, forces some units to be marked machine-local, which limits registry export on those targets until the compiler improves.
- **Planning does more work.** Computing Merkle identities and consulting the index adds a step before any compilation. It is small against the compilation it avoids, but it is not free for tiny plans.
- **Refusal surfaces earlier.** Plans that today link two compiled instances of one package and happen to work will be refused at plan time until they either unify the closure or declare an isolation boundary. This is the intended behaviour and will feel like a regression to anyone relying on the accident.
- **Collection needs a policy.** A shared store that is never collected grows without bound; one collected too eagerly rebakes. Defaults are an unresolved question.
- **A new store layout is a migration**, even a lazy one, and two layouts coexist until old entries age out.

## Implementation architecture

Non-normative. The identity computation belongs in the planner, computed bottom-up over the resolved unit graph so dependency identities are available when each unit's identity is formed. The store's payload area is a flat content-addressed directory; the index and referrer tables are small structured files or an embedded database keyed by identity, updated atomically with the payload insert. Materialisation into a build directory is a link where possible. The two-instance check is a single pass over the link closure grouping units by package and version. The registry client of RFC 125 reads and writes the same store through the same insert path as the executor, so `imported` and `baked` differ only in their satisfaction kind and attestation reference.

## Layers affected

- **Oven planner**: bottom-up unit identity computation, store consultation, two-instance refusal, per-unit hit and miss reporting.
- **Oven store**: unit-addressed layout, name index, referrer tracking, verification on read, collection, capacity interaction with leases.
- **Oven executor**: insert baked units by identity; materialise reused units by link or copy.
- **Receipts and build reports**: unit references, satisfaction kinds, attestation references.
- **Oven CLI (RFC 118)**: store status, per-unit inspection, collection; plan output changes.
- **Registry client (RFC 125)**: import and export through the store's unit insert and read paths.
- **Compiler**: no parser, typechecker, or emission changes; path remapping flags Oven already controls.

## Unresolved questions

- Which codegen flags are identity inputs, and should they enter the identity verbatim or as a normalised profile description so that spelling differences do not defeat sharing?
- How are native inputs of `-sys` crates identified when they come from the system rather than from a package: by digest of the resolved library files, by a declared toolchain fact, or by marking such units machine-local?
- Should build-script executables be shared units in their own right, or always rebaked and only their captured output made part of the consuming unit's identity?
- Should RFC 125 offer assets per unit, per package bundle, or both, given that per-unit assets maximise hits and per-package bundles minimise index size?
- What is the default collection policy: explicit only, capacity-triggered, age-based, or a combination, and how is it configured?
- When two bakes of equal identity produce different payloads, is retaining both and marking the identity machine-local sufficient, or should Oven attempt to classify the nondeterminism (debug information, symbol ordering) and treat some classes as benign?
- Which manifest facts count as observed by compilation? A version string read through an environment macro clearly does; the exact list of facts the compiler embeds in crate metadata without the code asking for them needs enumerating so the identity neither misses one nor includes ones that never reach output.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
