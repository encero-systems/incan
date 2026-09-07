# RFC 123: Package executable representation

- **Status:** Planned
- **Created:** 2026-09-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 034 (`incan.pub` package registry and the `.incanpkg` format)
    - RFC 118 (Oven API and operational core)
    - RFC 120 (canonical source symbol identity)
    - RFC 097 (Rust-hosted Incan caller)
    - #989 (executable public-boundary evidence)
    - #1260 (package and local-module import execution)
    - #1261 (facade identity on the replacement route)
- **Issue:** [#1339](https://github.com/encero-systems/incan/issues/1339)
- **RFC PR:** [#1324](https://github.com/encero-systems/incan/pull/1324)
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A package is a unit of distribution, not a unit of semantics. What a package exports should mean the same thing, and behave the same way, whether a consumer reached it through an import or wrote it themselves. This RFC establishes what a package publishes so that its public surface is executable by any route that can execute equivalent local code: a versioned executable representation, keyed by the canonical identities the package's manifest already exports, produced by the compilation that declares them.

## Core model

1. **A package's public surface has one identity space.** Canonical identity is minted by the compilation that declares a symbol, and every consumer resolves against those identities rather than against spellings.
2. **Execution is a projection of that surface, not a second description of it.** What a consumer executes for an imported declaration is reachable from the identity it already resolved — never from a name, a path, or a re-derivation.
3. **A representation is produced once, by the declaring compilation.** One declaration has one executable meaning, established where it is written.
4. **Representations are versioned and refusable.** A consumer that cannot interpret a package's representation says so in terms of the package and version it could not use, before it produces a result.

## Motivation

Incan's execution model is becoming plural. Compiling a program to Rust and linking it is one route; interpreting a program's checked representation directly is another, and more will follow as the compiler learns to answer questions about a program without building it. Each route is a different way of asking what a program means, and they should agree.

A package boundary should not decide which of them can answer. Moving a declaration into a package changes how it is distributed, versioned, and depended upon; it does not change what the declaration means. A model in which extracting a module into a package silently removes execution routes makes packaging a semantic act, and pushes authors toward keeping code in one project for reasons that have nothing to do with design.

The same principle carries the evidence the public boundary needs. Establishing that a contract survives a boundary requires exercising it across that boundary, and a route that cannot cross one cannot supply that evidence. A package that publishes what it exports, and how it runs, can be checked as a consumer actually experiences it.

Identity is what makes this coherent rather than duplicative. A package already names its public surface by canonical identity, and a consumer already resolves imports against those identities. An executable representation keyed by the same identities is the same surface seen from a second angle — not a parallel description that can drift from the first.

## Goals

- Define what a package publishes so a non-Rust route can execute its public surface.
- Keep identity minting in the declaring compilation, so a consumer never re-derives what the package already established.
- Version the representation independently of the manifest, so a consumer can refuse an interpretation it does not support without refusing the package.
- Make an unusable or absent representation an explicit, source-owned refusal rather than a silent fallback.
- Let a package ship no representation at all and remain a valid package for the routes that do not need one.

## Non-Goals

- Defining the exact wire encoding. This RFC fixes the contract and constrains the encoding to be binary; it does not select a specific binary format.
- Replacing the compiled Rust library. Packages continue to ship one, and it remains what the Rust-linking route uses.
- Making a package's private implementation executable. Only the public surface is in scope.
- Cross-version execution guarantees. A representation produced by one compiler release need not be interpretable by another; it must only say so clearly.
- Rust-hosted callers, which RFC 097 owns, and the interop surface a package may expose to Rust.

## Guide-level explanation

Building a library produces its manifest and its compiled artifact as it does today, and additionally an executable representation of the declarations it exports. The author does not write it or name it; it is a product of the same build that produced the manifest, and it describes the same public surface.

Consuming a package is unchanged. An import resolves against the package's public contract exactly as before, and a call into it typechecks the same way. What changes is that a route which does not link Rust can now execute that call, because the package shipped something it can execute.

When a route cannot execute a call, the reason is specific. A consumer that finds no representation, or one it cannot interpret, reports the package and version it could not use and what it required of it. That is a packaging or version fact, and a reader can act on it — republish the dependency, or take a route that does not need the representation. Reporting the same condition as an unsupported construct would send them to the language instead.

A package that ships no representation stays usable. Routes that link Rust behave as they always have; only the routes that need a representation refuse, and only for the calls that actually cross into that package.

## Reference-level explanation

A package **may** publish an executable representation of its public surface. A package that does not **must** remain valid for every route that does not require one.

A published representation **must** identify every declaration it covers by the canonical identity that declaration carries in the package's own manifest. A representation **must not** identify a declaration by source spelling, by module path, or by any generated Rust name.

A representation **must** be produced by the compilation that declares the symbols it covers. A consumer **must not** synthesise a representation for a dependency, including when that dependency's source is available to it.

A representation **must** carry a version distinct from the manifest's. A consumer **must** refuse a representation whose version it does not support, and that refusal **must not** invalidate the manifest or the package.

A representation **must** be self-consistent with the manifest it accompanies: every identity it covers **must** appear in that manifest's public surface, and a representation **must not** cover a declaration the package does not export.

A consumer that requires a representation and cannot obtain a usable one **must** refuse before producing any result or publishing an execution receipt. The refusal **must** name the package, the version, and the requirement that was not met. It **must not** silently select another route, and it **must not** report the condition as an unsupported language construct.

A consumer **must** resolve an imported declaration to its representation through canonical identity alone. Where a package's declaration is a projection of another — an alias, a re-export, or a facade — the consumer **must** resolve to the declaration the identity names rather than to the projection's spelling.

A representation **must** use a binary encoding. A self-describing text encoding **must not** be used: measured against the same content, JSON was an order of magnitude larger and several times slower to decode than a binary encoding, and a representation is read by every consumer of the package on every route that needs it.

A representation **must** be addressable by canonical identity without decoding the whole of it. A consumer that calls three declarations of four hundred **must not** be required to load four hundred.

A representation **must not** expose a package's private declarations, its compiler-session state, or its generated Rust layout. A consumer **must not** derive compatibility from any of those, and **must** derive it from the declared version and requirements alone.

## Design details

The representation is a third product of a library build, beside the manifest and the compiled artifact. It is keyed by canonical identity, which is already the manifest's currency, so the two are joined by the identity space rather than by file naming or ordering.

Versioning is separate from the manifest deliberately. A consumer that understands a package's exports but not its representation is in a recoverable position: it can typecheck, report precisely, and continue on a route that does not need the representation. Collapsing the two versions would turn an execution-route limitation into a packaging incompatibility.

Coverage is permitted to be partial. A package may publish a representation for some of its public surface and not the rest, and a consumer refuses per call rather than per package. This keeps the contract usable while the set of executable constructs grows, and it avoids an all-or-nothing gate on a package that exports one construct a route cannot yet execute.

Projections resolve to their target. A consumer calling through an alias, a re-export, or a facade resolves to the declaration the canonical identity names, so a chain of projections does not multiply representations and a facade does not need one of its own.

The compiled Rust artifact is unaffected. It remains the product the Rust-linking route consumes, and nothing here changes how that route selects or verifies it.

### Where it ships, and why a prebuilt Loaf is not the same thing

RFC 034 already reserves the slot. A `.incanpkg` carries `semantic/` for "optional semantic package fragments" beside `artifacts/` for target artifacts, and states that generated Rust "must not be the public compatibility contract." This representation is what `semantic/` holds; a Loaf, when one is published, is an entry under `artifacts/`. Both travel in one signed archive.

They are not substitutes, because they answer different halves of one question:

| | any target | no local compilation |
| --- | --- | --- |
| source snapshot | yes | no |
| prebuilt Loaf | no -- one target, toolchain and profile | yes |
| this representation | yes | yes |

A registry can publish Loaves for the combinations it chooses to precompile, and every one of them serves the Rust-linking route on one platform. The representation is a single artifact that serves every route that does not link, on every platform. Publishing more Loaves does not reduce the need for it, and publishing a representation does not reduce the value of a Loaf.

### Cost

The shape of the cost is known rather than assumed. Measured over a module's Body IR in a binary encoding, a twenty-five-declaration surface was roughly 16 KB and under a millisecond to load; a four-hundred-declaration surface was roughly 293 KB and about fourteen milliseconds. Loading was consistently cheaper than compiling the same declarations from source. The same measurement demonstrated the contract end to end: a representation round-tripped exactly, and a call executed from the decoded copy produced the result the compiled-from-source module produced.

Those numbers are why addressing by identity is normative rather than advisory. Whole-surface loading is affordable at the small end and wasteful at the large end, and the difference is entirely in declarations the consumer never calls.

## Alternatives considered

**Re-derive the representation in the consumer.** Rejected. A declaration's executable meaning would then be established twice, in two compilations, and the two results would carry identities from different spaces. Identity-based reasoning about the public surface — which is the basis of RFC 120 — holds only while one declaration has one identity, so a consumer-side derivation would return the language to comparing spellings.

**Extend the manifest to carry executable content.** Rejected. The manifest is the public contract and is read for typechecking, inspection, and compatibility. Loading executable content for every consumer that only wants a signature makes the common path pay for the rare one, and couples two things that need to version independently.

**Ship the representation as a separate package.** Rejected. Two packages describing one public surface can drift, and a consumer would have to reconcile two identity spaces at exactly the boundary this RFC exists to keep single.

**Require every package to publish one.** Rejected. It would make packages invalid for routes that never needed a representation, and would gate publication on a route's current capability.

**Interpret the compiled artifact.** Rejected. It is the output of one backend for one target and profile, and reading it back would make every route depend on that backend's layout — the opposite of a route-neutral contract.

## Drawbacks

A package build produces more, and a published package is larger, for a benefit only some routes use. The partial-coverage rule and the optionality of the representation limit that cost but do not remove it.

Two independently versioned products describing one surface can disagree. The self-consistency rule constrains what a valid package may publish, but a consumer must still handle the case, and "the manifest says this exists but the representation does not cover it" becomes a state that has to be reported well.

A representation is a second thing to keep correct as the language grows. A construct that becomes executable is not executable across a package boundary until the representation covers it, and that lag is visible to users as a route difference. Declared coverage makes the lag legible rather than removing it: a consumer can see what a package supports on its route, but it still has to wait for a republish.

## Implementation architecture

*Non-normative.*

The natural producer is the same library build that emits the manifest, because it already holds the checked public surface and the identities the representation must use. The natural consumer boundary is wherever a route resolves an imported declaration, so that a missing or unusable representation is discovered at resolution rather than part-way through execution.

Coverage is likely to grow along the same axis as executable constructs generally, which is one reason the decision below records it explicitly rather than inferring it from what happens to be present.

Consumers will want the decoded form to outlive one invocation, and the natural shape for that is an identity-keyed local store rather than a cache private to this contract. Nothing here requires one, and nothing here should prevent one: a consumer that keeps decoded declarations addressed by the same canonical identities the representation uses needs no format of its own.

## Layers affected

- **Typechecker / Symbol resolution**: resolving an imported declaration to a published representation through canonical identity, and reporting when none is usable.
- **Emission**: producing the representation for a library's public surface as part of the same build that produces its manifest.
- **Tooling**: publishing and locating the representation beside a package's other products, and surfacing version and coverage in inspection output.

## Design Decisions

**Coverage is declared explicitly, not inferred from the identities present.** A consumer must be able to answer "can this route execute this call?" before decoding the declaration, and a reader must be able to see what a package supports without executing anything. Inferring coverage from content also conflates two different states -- "this declaration is not covered" and "this declaration is covered and its body happens to be empty" -- which a refusal has to tell apart.

**A consumer may require a representation, and the requirement is resolved at resolution time.** A route that cannot proceed without one reports at the point the dependency is resolved, naming the package and version, rather than at whichever call happens to cross the boundary first. Deferring to first call makes the failure depend on control flow: the same program reports in different places on different inputs, and a consumer cannot tell whether a dependency is usable without running it. Routes that do not require a representation are unaffected and continue to resolve packages that ship none.

**The representation carries one version for the whole representation.** Integrity and identity are already settled at the archive: RFC 034 covers the package with one checksum and one signature, so the representation's version answers only "can this compiler interpret this?" That is a property of the encoding and the fact vocabulary, not of individual constructs, and per-construct versioning would multiply a compatibility surface that the coverage declaration already expresses more directly.

**A representation and the Rust-linking route must agree by construction.** They are produced by one compilation of one package version and ship inside one signed archive, so a disagreement between them is not two artifacts drifting apart -- it is one publisher being internally inconsistent within a single signed unit. Permitting divergence would also make a package's meaning depend on how a consumer reached it, which is the outcome this RFC exists to prevent. Where a construct cannot be represented for a route, the answer is to leave it uncovered and refuse, not to represent it differently.

