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
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

Oven already reuses compiled work safely: a sealed plan carries a receipt whose identity covers every input, and a build that matches a receipt reuses its output without recompiling. The unit of that reuse is the whole plan. Two projects whose plans compile the same fourteen dependency units store fourteen units twice, a unit compiled in one worktree cannot satisfy a plan in another, and nothing can tell that two ten-gigabyte build directories hold the same artifacts. This RFC makes the **compiled unit** the store's addressable object. Every unit Oven produces carries a unit identity derived from its effective compilation inputs, including the identities of the units it links against, kept distinct from the publication provenance that names where its source came from and from the digest of the bytes it produced. Plans reference units by identity; the store holds each identity once, shares it across plans, projects, and worktrees on a machine, and collects units no receipt or lease reaches. The same identity is what a registry asset carries under RFC 125, so a unit downloaded from `incan.pub` and a unit baked locally are one store entry. The existing rule that a plan must never link two distinct compiled instances of one package becomes checkable at plan time instead of at link time.

## Core model

1. **The unit is the object.** A compiled unit is one compiler invocation's durable output for one unit of a Loaf's build graph in one domain: a library, a procedural macro, a build-script executable and its recorded output, a binary, or a test harness. In rustc's own vocabulary a unit is a crate; this RFC uses the RFC 119 term throughout. The store addresses units, not plans.
2. **Three identities, kept apart.** A unit has a *publication provenance* (which registry checksum, source Loaf, or path it came from, and who signed it), a *unit identity* (a digest over its effective compilation inputs: the semantic digest of the source it compiles, the manifest facts that reach compilation, toolchain, target, domain, codegen facts, features, artifact kind, and the external identities of every dependency it links), and a *payload digest* (the bytes it produced). Reuse is decided by unit identity alone. Provenance is recorded and checked for trust; it is not an identity input, so a republication that changes only version metadata or documentation does not invalidate unchanged code. Because dependency identities are inputs, identity is a Merkle root over the closure.
3. **Identity completes in stages.** Units that generate inputs for others, build scripts and procedural macros, carry a base input identity that is known before execution and a receipt identity that is known after. Downstream identities are finalised from admitted provider receipts, as RFC 119 already requires, and no unit compiles before its identity is final.
4. **Equal identity means interchangeable; nothing less does.** Oven may substitute one unit for another only when their unit identities are equal and their payload digests agree. Same package and version with different identity is a miss, and if both would enter one link, it is a refusal that no ownership declaration waives.
5. **A plan is a set of unit references plus a receipt.** The plan receipt records which unit identities satisfy which units of which Loaves and how each was obtained: baked here, reused from the store, or imported from a registry asset. Reusing a plan output remains possible and is now a special case of every unit hitting.
6. **The store is content-addressed with a name index.** Unit payloads live once under their identity. A separate index maps package name, version, and plan facts to identities so a planner can ask "what do I already have for this Loaf unit under these facts" without enumerating the store.
7. **Sharing crosses every local boundary.** Plans, projects, worktrees, and profiles share units through identity. A unit compiled for a release plan in one checkout satisfies a release plan in another checkout of the same or a different project.
8. **Collection is by reachability.** A unit is live while a receipt, lease, or policy pin reaches it. Everything else is collectable, and collection is a store operation a user can invoke and inspect.
9. **Source enters identity by meaning, not by bytes.** The source input is the RFC 106 semantic digest of what the unit compiles, not a hash of its file contents. A comment, a docstring, a reordering, or a move between files does not change what the compiler produces and must not change the unit's identity. This RFC does not define that digest; RFC 106 does, and this RFC consumes it, consistent with introducing no second analysis path.
10. **A unit is identified twice: internally and externally.** The *unit identity* covers everything the unit compiles and decides whether the unit itself is rebaked. The *external identity* covers only what a consumer can observe — public signatures, plus the bodies of anything a consumer can instantiate or inline, plus whatever those reach — and decides whether the unit's dependents are rebaked. A change confined to a unit's internals rebakes that unit and nothing beyond it. For units that pass through `rustc` this is not yet reachable on a stable compiler, and the reason is the compiler's rather than this model's; see *What stock `rustc` does not yet allow* below.
11. **Local and registry are the same identity space.** RFC 125 assets are unit-identity-addressed. Importing an asset is a store insert; publishing one is a store export. The registry never needs a second notion of what makes two compiled artifacts the same.

## Motivation

The current Oven store is correct and conservative in the right way: nothing is reused without a receipt match, and the receipt covers the inputs that matter. What it lacks is granularity, and the cost of that was measured during the 0.6 dependency audit. Two freshly generated projects with identical dependency closures each baked their own debug and release Loafs, four sealed artifacts of 51 to 54 MB holding the same fourteen compiled rlibs. On a maintainer's machine, twenty build directories under one development root totalled 52 GB, two of them the compiler itself at 10.8 GB and 10.4 GB a few commits apart, indistinguishable to any tool because the artifacts inside carry no identity that another directory could recognise.

The same audit recorded why the naive fix is wrong. Two independently resolved closures of identical source produced ABI-incompatible rlibs for the same tokio version, and linking both into one executable produced a runtime panic because a runtime object created through one instance was invisible to code compiled against the other. Oven now detects that case at link time by comparing digests and refusing. Sharing units by package and version would reintroduce the hazard everywhere; sharing them by an identity that includes the whole compilation context removes it by construction and moves the check to planning, where it can name both plans in the diagnostic.

Every ecosystem that shares compiled artifacts solved this with an identity over inputs rather than over names; the prior art section records what each got right. Oven starts from a stronger position than most because its receipts already encode those inputs, and because RFC 125 needs exactly this identity to exchange units across machines. Without unit-level identity, a registry can only offer whole plans, and whole plans almost never match another project.

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

- Changing what a unit compiles to, how the compiler is invoked, or which artifact kinds exist. RFC 119 owns the unit graph; this RFC owns how its outputs are stored and reused.
- A remote or distributed store protocol. That is RFC 125.
- Reproducible-builds guarantees in rustc. This RFC depends on the compiler's determinism where identity must be portable and states that dependency; it does not fix compiler nondeterminism.
- Sharing incremental-compilation state or other transient compiler caches.
- Any change to project receipts' role as the record of what a project selected. Receipts gain unit references; they do not lose anything.

## Guide-level explanation

### The second project is fast

A developer creates a second Incan project on a machine that already has one. Both depend on the same standard library components and runtime Loaves at the same toolchain.

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

### Two instances of one Loaf are refused before linking

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

A compiled unit is the durable output of one compiler invocation for one unit of a Loaf in one domain, together with the metadata needed to compile and link against it. Unit kinds are library, procedural macro, build-script executable with its captured output, binary, test harness, and metadata-only (the check-only output of `oven check`, which carries the compiler's metadata and no code; its identity includes the check-only emission mode so it never substitutes for a full unit, while a full unit may satisfy a later check). Every unit must record its host or target domain as RFC 119 defines them. A build-script's captured output (emitted cfgs, link directives, environment) is part of the unit that consumes it, not a separate shared object.

### Unit identity

A unit has three distinct identities, and the store must name them separately:

- **Publication provenance**: the registry checksum, source Loaf digest, or path source, together with the signer or attestation that vouches for it. Provenance must be recorded with every unit and checked against trust policy; it must not enter the unit identity.
- **Unit identity**: the digest over effective compilation inputs defined below. It decides reuse.
- **External identity**: the digest over the subset of those inputs a consumer can observe, defined below. It decides whether *dependents* are rebaked. A unit always has both; they differ whenever a change is confined to the unit's internals.
- **Payload digest**: the `sha256:` of the produced bytes, uncompressed. It verifies storage and transport. Compression is a storage and transport encoding chosen per store or per registry and never enters any identity, so a unit compressed at one level locally and at another for publication is one unit.

A unit identity must be a `sha256:` digest over a canonical serialization of at least:

- the **semantic digest** of the source the unit actually compiles, as RFC 106 defines it, excluding sources the compilation does not read (documentation, manifests, tests not built, publication metadata). This is a digest over checked meaning, not over file bytes: a comment, a docstring, a reordering of declarations, or a move between files does not change what the compiler produces and must not change the identity. It is language-neutral — an Incan-authored and a Rust-authored unit contribute the same kind of fact — and it subsumes path remapping, because a physical location is provenance and never an identity input;
- the manifest facts that reach compilation: the unit name, edition, and any package fact the code or the compiler observes, such as a version string exposed through an environment macro or embedded in the compiler's metadata output. A fact the compilation does not observe must not enter the identity;
- the toolchain identity (compiler version and host, as Oven records it);
- the target triple and the domain (host or target);
- the profile facts that affect codegen: optimisation level, debug-info level, codegen units, panic strategy, LTO mode, target features, and any other flag Oven passes that changes output;
- the resolved feature set of the unit;
- the artifact kind and edition;
- the **external identities** of every unit it links against, in canonical order. Folding the external rather than the full identity is what keeps a dependency's internal-only change from rebaking its dependents; folding the full identity would be sound but would propagate every private edit through the closure;
- for units that consume provider outputs, the receipt identity of each admitted provider (build script or procedural macro) whose generated inputs, cfgs, environment values, or link directives it consumes;
- for units with native inputs, the digests of the native libraries and headers the unit links or includes.

A unit identity must not include absolute paths, timestamps, hostnames, user names, publication signatures, or the identity of the plan that requested it. Oven must remap source paths so that the same source at two locations produces the same identity, and must derive the compiler's unit disambiguator (the metadata hash it embeds) from the *external* identity rather than from the package version, so that a version-only republication of unchanged code yields the same unit. Deriving it from the unit identity would defeat the two-identity split outright: the disambiguator reaches exported symbol names, so a unit rebaked for an internal-only change would export differently named symbols, and a dependent that was deliberately not rebaked names the old ones and cannot link. Whatever decides symbol names must move exactly when dependents are expected to move.

**External identity.** A unit's external identity must be a `sha256:` digest over the same inputs as its unit identity, with the source input narrowed to the unit's *resilience boundary*: the public declarations it exports, plus the bodies of those a consumer can instantiate or inline, plus everything reachable from those bodies whatever its visibility. For a Rust unit that set is **whatever the compiler exports in the unit's metadata**, not an annotation list. Generic, `#[inline]`, and `const` items are the obvious members, but `rustc` also exports small function bodies for cross-crate inlining on its own heuristics, with no annotation and no stability promise. An external identity computed from annotations would call a body unobservable that the compiler had in fact exported, which is a wrong hit — precisely what the lower-bound rule below forbids. Visibility marks the roots; reachability decides membership, and RFC 106's graph is what computes it. A private declaration a public generic calls is part of the external identity — an instance of the rule, not an exception to it.

Every other identity input — toolchain, target, domain, profile facts, features, artifact kind, provider receipts, native inputs — enters the external identity unchanged, because all of them are observable by a dependent.

Where the boundary cannot be determined, a unit's external identity must equal its unit identity. Erring toward equality means erring toward rebaking, which costs time; erring the other way yields a wrong build. This is the same lower-bound discipline RFC 106 imposes on reachability, applied to identity.

**Acceptance case.** A package republished at a new version whose compiled sources, observed manifest facts, features, dependencies, and provider outputs are byte-identical must resolve to the same unit identity and reuse the existing unit. A package whose new version is observed by its own code, for example through a version macro, legitimately produces a new identity, and the identity inputs make that visible.

**Acceptance case.** Editing a comment or a docstring, reordering two declarations, or moving a declaration between files within one unit, with no other change, must yield the same unit identity and reuse the existing unit.

#### What stock `rustc` does not yet allow

The two-identity split needs the compiler to agree that a dependency's internals are not part of its interface. Stable `rustc` does not agree, and the model must say so rather than describe a saving it cannot deliver.

When `rustc` compiles B against A it records A's strict version hash in B's metadata, and that hash covers all of A, function bodies included. Anything later compiled against B loads A again and checks that the A it finds carries the hash B recorded; a different hash is a hard error. This is why Cargo rebuilds every dependent when a dependency changes at all, and it is not a conservatism Cargo could choose to drop.

Take the acceptance case above literally against that compiler. A's private body changes, so A's unit identity moves and Oven rebakes it, and the rebaked A has a new strict version hash because `rustc` hashed the new body. A's external identity did not move, so B is deliberately not rebaked. Now plan C, which depends on B: `rustc` loads B, reads the hash B recorded for A, finds the rebaked A, and refuses. The plan this model calls valid does not compile. The saving the split exists to deliver is exactly the case the compiler rejects.

**Therefore, on a stable `rustc`, a Rust unit's external identity must equal its unit identity.** This is not a new rule; it is the lower-bound rule above applied honestly — the boundary cannot be determined, because the compiler does not expose one, so the two identities coincide and every dependent is rebaked. Oven must not compute a narrower external identity for such a unit merely because RFC 106's graph can describe one.

**What lifts it.** The compiler must expose an interface hash that is insensitive to body-only changes, so a dependency can change internally and its dependents relink rather than recompile. The Rust project's *Relink, don't Rebuild* work is aimed at exactly this, and a body-insensitive metadata hash is the shape of the answer. When a stable `rustc` offers one, it becomes the compiler-side counterpart of the external identity, the fallback above stops applying to Rust units, and rule 10 becomes reachable. Until then the split is a model that is correct and an optimisation that is dormant, and this RFC claims only the first.

**What the dormancy costs, which is close to nothing.** Every saving this RFC was written to deliver rests on the *unit* identity, not the external one. Two projects with identical closures sharing one set of rlibs instead of baking fourteen each; twenty build directories that no longer hold indistinguishable copies of the same artifact; a version-only republication of unchanged code resolving to the same unit; a registry handing a consumer a unit its plan already demands. In all of those the consumer receives the *same* A, so the hash its dependents recorded still matches and `rustc` is satisfied. The external identity gates one narrower case: a dependency changing internally while its dependents are left alone, inside a single chain being rebuilt. That case falls back to rebaking dependents, which is exactly what Cargo does today, so the floor here is the status quo rather than a regression. It is worth specifying now because it is dormant rather than wrong, and because the fallback keeps it safe while it sleeps.

**A note on why waiting is cheap for Oven specifically.** Incan provisions and pins the exact `rustc` its toolchain was built against rather than using whatever a machine has. Adopting a body-insensitive interface hash is therefore a decision Oven can take when the feature is usable, without waiting for it to reach the stable channel and without asking users to change toolchains. That is a structural advantage over a build system that must work with whatever compiler it finds, and it is the reason this RFC treats the gap as timing rather than as a permanent limit.

**Incan-facet units are better placed but not exempt.** The compiler service owns their checked metadata and can compute the boundary RFC 106 describes without asking `rustc` anything. But those units are still compiled through `rustc` and still linked against by units that record a strict version hash, so the same fallback governs them until the compiler side exists. The place the split pays first is wherever Oven, not `rustc`, decides what a consumer links against.

**Acceptance case.** Changing the body of a private function that no public generic or inlinable item reaches must change the unit's identity and not its external identity: the unit is rebaked, its dependents are not. This case is gated on the compiler for any unit that passes through `rustc`, per the next section, and is not claimed on a stable toolchain.

**What "a move between files" requires.** The acceptance case above is not met by digesting checked meaning alone, and the reason is worth stating rather than leaving for an implementer to discover. A declaration's canonical identity carries the module that declares it, and a module path is derived from a file path, so moving a declaration to another file changes its identity and therefore the semantic digest that contains it. Measured directly: one function digested under two module names produces two digests.

Meeting the acceptance case requires the source input to scope a declaration to its **namespace** rather than to its declaring module. A module whose name marks it internal is a detail of the nearest enclosing namespace, not a namespace of its own, so moving a declaration between two internal modules of one namespace does not move it at all. RFC 106 defines the namespace and how the graph carries it. Scoping is required rather than flattening: dropping location from identity entirely collides declarations that a real standard library deliberately gives one name and one signature across many modules, and nothing left in the identity separates them.

**The projection is a precondition, not a separate concern.** Even with the identity scoped correctly, the acceptance case cannot be observed while the emitted symbol encodes the declaring module path and the declaration span, as RFC 120's `incan-v1` projection requires. A move between files would then leave the unit identity unchanged and the produced bytes different — which this section's payload rule classifies as compiler nondeterminism, retaining both payloads, marking the identity machine-local, and making the unit ineligible for export. An ordinary reorganisation would disable sharing for that unit rather than being invisible. A projection derived from a reorganisation-stable identity is therefore a prerequisite for this RFC's own acceptance cases, and superseding that clause of RFC 120 is the work it depends on.

Two units with equal unit identity must be treated as interchangeable, subject to the payload rule below. Oven must never substitute units whose identities differ, regardless of package name, version, or apparent compatibility.

**Payload conflicts.** If two bakes with equal unit identity produce different payload digests, the compiler was not deterministic for those inputs. The store must retain both payloads under the one identity, must mark the identity machine-local and ineligible for export, and every plan receipt must record the payload digest it actually linked, so that a consumer is never given a payload other than the one its dependents were compiled against. Interchangeability is therefore promised only for identities with a single observed payload; the nondeterminism question remains open below.

### Plans and reuse

Identity completion is staged along the RFC 119 unit graph; Oven must not introduce a second graph to do it. Units that produce inputs for others (build-script and procedural-macro providers) are planned first with the base input identity RFC 119 defines: permitted input tree, features, host, target, provider and toolchain identities, and approved environment. A provider whose base identity matches a stored receipt is reused; otherwise it is executed under RFC 119 admission and its receipt recorded. Only then are the identities of the units that consume its outputs finalised from the receipt's generated-input digests, cfgs, environment values, and link directives. No unit may compile before its identity is final, and executing an admitted provider is provider execution under RFC 119, not resolution; the no-execution rule for resolution in RFC 117 and RFC 125 is untouched. For each final identity the store is consulted; a present, verified unit is reused, and an absent one is baked and inserted. A reused unit that depends on a provider must identify the provider receipt it reused, as RFC 119 already requires. Where RFC 119 marks a provider result uncacheable because an input it depends on could not be observed, every unit whose identity includes that receipt is likewise uncacheable: it may be baked and used by the plan that produced it, but must not be inserted for reuse and must not be exported to a registry. The plan receipt must record, per unit, the identity and the satisfaction kind: `baked`, `reused`, or `imported`, with the importing attestation reference where applicable.

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

RFC 125 assumes a store that shares units and states so only in non-normative text. This RFC is the normative source for that property. Registry assets are units: the object RFC 125 exchanges is the unit defined here, addressed by its unit identity, and a package version's asset manifest is the only bundling construct, so import and export are store insert and store export with no repackaging. RFC 125's applicability rule, that an asset is usable only when the plan's facts equal the receipt's facts, is the unit identity rule stated at the granularity of a published bundle.

### Relationship to RFC 020

Offline and locked modes resolve entirely from the store and the lockfile. Unit-level storage makes offline mode strictly more capable: any unit any prior plan produced on the machine is available to a new plan that shares its identity.

### Migration

The store gains a new layout version. Existing entries are not rewritten; they remain readable for whole-plan reuse until collected, and new plans populate the unit-addressed layout as they bake. No user action is required, and a `gc` after the first few bakes under the new layout reclaims the old entries.

## Prior art

- **Nix binary caches.** A derivation's store path is a hash over every input, including the store paths of its dependencies. That Merkle-over-the-closure identity is the model this RFC adopts, and it is what lets a cache serve outputs to strangers safely.
- **Bazel, Buck, and Go's build cache.** Content-addressed action caches keyed by an action identity over inputs and dependency outputs, with remote sharing. Go's build cache is the closest single-language example: per-action identity, content-addressed storage, automatic collection.
- **Conan.** C++ has no stable ABI either. Conan computes a package identity from compiler, version, architecture, options, and dependency versions; one recipe has many binaries; a consumer's profile selects a matching one and otherwise builds from source. The store described here is that model with a stricter identity.
- **uv and npm's caches.** uv keeps one content-addressed cache and links artifacts into every environment rather than copying; npm's cache stores each blob once under its integrity hash with a separate index from names to hashes and verifies on read. This RFC's payload-plus-index shape and its "install by linking" materialisation follow both.
- **sccache.** A compiler-wrapper cache keyed on the reconstructed command line. It demonstrates demand for cross-project reuse in Rust and the limits of inferring identity after the fact rather than from the plan.
- **The Rust project's own direction.** A long-open request for pre-built dependencies, a per-user compiled-artifact cache issue, a 2025 split of the build directory by unit, and a 2026 goal of a cross-workspace cache that may later be pre-populated remotely. Oven arrives at the same shape from a stronger starting point because its receipts already carry the inputs that work has to reconstruct.
- **The tokio incident recorded in this repository.** Two independently resolved closures of identical source produced ABI-incompatible units for the same version, and linking both left a runtime object invisible to half the program. It is the concrete case the identity rule and the two-instance refusal exist to prevent.

## Alternatives considered

- **Keep plan-level Loafs as the unit of reuse.** Rejected. It is the status quo the audit measured; plans almost never match across projects, so nothing is shared and nothing can be safely collected below the plan.
- **Deduplicate at the file level only.** A content-addressed store for payload bytes with no unit identity, hardlinking identical files. The Rust project's early shared-cache experiment took this shape and reported saving half a gigabyte of thirty. Rejected as the primary mechanism: it saves disk where bytes happen to coincide but cannot decide reuse, cannot refuse the two-instance hazard, and cannot feed a registry. It is retained as the store's physical layout beneath unit identity.
- **Key units by package, version, features, and target only.** Rejected. This is the identity that produced two incompatible tokio instances; it omits dependency identities and codegen facts and is exactly what makes sharing unsafe.
- **A compiler-wrapper cache keyed on the rustc command line.** The sccache model. Rejected as the basis: it reconstructs identity from the invocation rather than from the plan, shares nothing the planner can reason about, and cannot express satisfaction kinds or attestations.
- **Compile shared dependencies as dynamic libraries.** Proposed for the Rust toolchain in 2021. Rejected. It changes codegen and linking semantics to solve a storage problem, and does not remove the identity requirement.

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
- How are native inputs of system-binding units (the `-sys` convention) identified when they come from the system rather than from a package: by digest of the resolved library files, by a declared toolchain fact, or by marking such units machine-local?
- Should build-script executables be shared units in their own right, or always rebaked and only their captured output made part of the consuming unit's identity?
- What is the default collection policy: explicit only, capacity-triggered, age-based, or a combination, and how is it configured?
- When two bakes of equal identity produce different payloads, is retaining both and marking the identity machine-local sufficient, or should Oven attempt to classify the nondeterminism (debug information, symbol ordering) and treat some classes as benign?
- Which manifest facts count as observed by compilation? A version string read through an environment macro clearly does; the exact list of facts the compiler embeds in its metadata output without the code asking for them needs enumerating so the identity neither misses one nor includes ones that never reach output.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
