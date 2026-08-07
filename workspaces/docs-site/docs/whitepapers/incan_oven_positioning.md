---
title: "A Cargo-free toolchain for Incan and Rust"
status: "Draft positioning paper"
snapshot_date: "2026-07-24"
authors:
  - "Danny Meijer"
audience:
  - "Incan contributors and prospective collaborators"
  - "Rust developers evaluating Incan"
scope: "Product positioning and directional architecture for a unified, interactive-first Incan and Rust toolchain."
normative: false
related_rfc:
  - "RFC 020"
  - "RFC 034"
  - "RFC 073"
  - "RFC 079"
  - "RFC 112"
  - "RFC 114"
  - "RFC 116"
research_context:
  - "Cargo package, build-script, and workspace model"
  - "Bazel and Buck explicit action-graph and cache-identity models"
  - "Nix provenance and reproducibility model"
related_whitepapers:
  - "Incan ecosystem north star"
review_after: "After Incan 0.5"
---

# A Cargo-free toolchain for Incan and Rust

--8<-- "_snippets/callouts/whitepaper_status.md"

> **Current boundary:** supported Alpha envelopes for ordinary `incan build`, `incan test`, and `incan run` select receipt-bound, store-owned direct-`rustc` execution without Cargo on the consumer path. The hidden `legacy_cargo` Loaf baker is the sole Cargo-backed compatibility publisher for those envelopes, never a normal-command fallback. The repository suite exercises the same Oven runner and records both case and root aggregates under a Cargo guard. Oven Alpha is not yet a general Cargo-free toolchain, a cross-platform performance claim, or the full design described here. This paper remains future-facing; see [Oven Alpha](../tooling/explanation/oven_alpha.md) for the implemented boundary.

## Abstract

Incan should not treat Cargo as the permanent centre of its developer experience. Cargo remains an important part of Rust's history and ecosystem, but Incan needs a project system that understands more than Rust crates: Incan source, Rust source, typed scripts and actions, semantic facts, target constraints, published artifacts, policy, provenance, live editable models, and long-lived interactive sessions. This paper proposes a direction for that system: a Cargo-free Incan 1.0 SDK, built around one inspectable project graph and a native build planner that can drive Rust compilation without shelling out to Cargo. We suggest **Oven** as a working name for that toolchain. The name is optional; the architectural promise is the point.

The central user-facing idea is simple: ordinary Rust projects should be able to enter an Incan-managed workflow without being rewritten, while Incan projects gain a toolchain designed for persistent, interactive work. Instead of downloading raw package source and discovering the build cost only when a command is run, `incan.pub` can publish ready-to-use, target-compatible artifacts. We suggest calling those artifacts **Loaves**: verified, provenance-carrying build products that arrive ready to use. Local compilation remains possible, but it should be the fallback—not the surprise first action in a notebook or data-rendering session.

This direction applies lessons from Cargo's crate and build-script model, and from the explicit action graphs and cache identities used by systems such as Bazel and Buck. It is a positioning paper, not an implementation RFC or a promise of measured performance. The proposal needs a staged delivery plan and hard compatibility boundaries. Its first demanding proof is IncQL with DataFusion: a workload where the difference between a cache and a genuinely persistent toolchain is immediately visible.

## The position

Incan should become the best toolchain for projects that combine Incan and Rust, and a compelling toolchain for Rust projects even when they contain no Incan source.

That is a stronger claim than "Incan can invoke Cargo." It says the project model itself belongs to Incan. One system should be able to answer, consistently and inspectably:

- What source, dependencies, toolchains, targets, actions, and policies make up this project?
- Which build units and artifacts are valid for this exact environment?
- What changed, what must be invalidated, and why?
- Which result is safe to reuse in this session, on this machine, or from `incan.pub`?
- How should an IDE, a notebook, CI, a command-line user, and an agent all operate on the same project?

Cargo was designed around the Rust crate and the command-line build. That is a sensible foundation for Rust, but it is not a complete model for Incan's intended workflow. Incan needs to connect language semantics, Rust interoperability, project automation, registry artifacts, and interactive execution without repeatedly crossing opaque process and cache boundaries.

The goal is not an anti-Cargo rewrite for its own sake. The goal is a better developer loop: less accidental rebuild work, clearer ownership of build state, more useful diagnostics, and a project graph that matches how Incan actually wants to be used. The experience must be both speedy and nimble: fast for a large prepared workspace, but also light enough for a one-file script, a short-lived experiment, or a notebook cell. It should not require a heavyweight project ceremony, a large local cache, or a long-lived background service merely to begin useful work.

## A proposed name: Oven

We suggest **Oven** as the working name for Incan's toolchain equivalent to Cargo. The name is memorable because it captures a different philosophy, but it is not a branding commitment. If a better name emerges, the architecture should survive unchanged.

> Incan Oven is a drop-in replacement for Rust's Cargo with a fundamentally different philosophy. Instead of raw Cargo crates, Oven deals in Loaves that arrive ready to use.

"Drop-in" needs a precise reading. It means a normal Rust project should be able to be adopted by an Incan-managed workflow without a source rewrite or a bespoke parallel ecosystem. It does not mean emulating every Cargo command-line flag, implementation quirk, or undocumented cache behaviour forever. Compatibility is a project and dependency boundary; the purpose is not Cargo impersonation.

The terms separate responsibilities cleanly:

| Term | Proposed meaning |
| --- | --- |
| **Oven** | The unified Incan/Rust project resolver, build planner, executor, and session coordinator. |
| **Loaf** | One verified, target-compatible, provenance-carrying artifact or artifact set ready for consumption. **Loaves** is the public plural. |
| **`incan.pub`** | The publishing and distribution side: where compatible Loaves are baked, verified, indexed, and made available. |
| **Local baking** | Building a Loaf locally when publication does not provide a compatible one, or when local source must be compiled. |

The metaphor is useful only if it makes the operational model clearer. A Loaf is not a vague cache entry. It has an identity, inputs, target compatibility, provenance, integrity information, and a lifecycle the toolchain can explain.

## The architectural shift: one project graph

Oven's core should be a single project graph rather than a collection of wrappers around separate tools. The graph needs to represent at least:

- Incan packages, semantic facts, and generated interfaces
- Rust packages, source dependencies, registries, and resolved versions
- typed scripts and actions
- pinned Rust toolchains and target constraints
- build units, native-link requirements, build scripts, and procedural macros
- artifacts, receipts, integrity metadata, and provenance
- policy, diagnostics, and lifecycle state
- interactive sessions and their separately managed runtime state

This is the important line between a faster Cargo invocation and a native Incan toolchain. If Cargo remains the authority for resolution, package identity, invalidation, and artifact ownership, Incan can improve the edges but cannot give the entire workflow a single coherent answer. If the project graph is Incan-owned, the compiler, CLI, IDE, registry, CI, notebook runtime, and automation tools can share one model.

`rustc` remains a useful compiler backend. Cargo need not remain the project authority. Oven should resolve the graph, select compatible artifacts, plan Rust compilation, invoke the pinned compiler, collect outputs, and report build receipts in terms Incan users can inspect.

## Compatibility direction: adopt Rust, replace Cargo's project authority

Oven should adopt the parts of Rust that make the ecosystem interoperable, and replace the parts that prevent Incan from owning the developer loop. This is not a clean-room alternative package universe. Rust source remains Rust source; crates remain crates; `rustc`, target triples, SemVer requirements, crate archives, registries, and ordinary native dependencies remain part of the world Oven must support.

| Adopt and preserve | Move under Oven's authority |
| --- | --- |
| Rust source, crate names, editions, target triples, profiles, `rustc`, crate archives, registries, and Rust's dependency vocabulary | Project identity, mixed Incan/Rust workspace topology, lock ownership, resolution receipts, build planning, artifact ownership, diagnostics, interactive sessions, and lifecycle actions |
| Existing `Cargo.toml` and `Cargo.lock` as compatibility and import inputs for ordinary Rust projects | The normal user workflow: no Cargo executable invocation, generated Cargo project, or Cargo target directory required to build, test, run, package, or work interactively |
| Cargo-compatible resolution and feature semantics wherever Oven claims compatibility | A native, inspectable representation of the resolved graph and all build-unit identities |

The adoption path should begin conservatively. An existing Rust repository keeps its Rust source and can keep its `Cargo.toml` and `Cargo.lock` intact. Oven reads them as declared compatibility inputs, resolves the equivalent project graph, and records an Incan-owned lock and receipt. No source rewrite, wrapper crate, or parallel package publishing system should be needed merely to adopt Incan. Over time, a project can express additional Incan packages, typed actions, capabilities, policy, and notebook sessions in that same graph.

This also gives `incan.toml` and `incan.lock` a clearer destination. They should evolve from a manifest plus embedded Cargo lock payload into Incan's native declaration of project intent and resolved identity. Importing existing Cargo metadata is part of compatibility; materialising a generated Cargo project is not the end state.

### The hard Rust boundary

Direct `rustc` execution is not enough. The difficult compatibility surface is build scripts, procedural macros, feature resolution, native linking, and host-versus-target compilation. A build script and a procedural macro are host executables; a cross-compiled library is a target artifact. Oven must plan and identify those domains separately.

For each supported package, Oven needs explicit build-unit inputs and receipts: crate source and features, compiler and sysroot identity, target/profile/cfg selection, build-script or macro binary identity, declared and observed environment inputs, generated outputs, native-tool and link inputs, and produced artifacts. It must execute build scripts and procedural macros under an explicit compatibility and policy contract, not hide arbitrary process execution behind package installation. A first version may need a trusted compatibility mode for widely used existing crates; it should still report what ran and what affected reuse. The long-term direction is stricter, more declarative build metadata where the ecosystem can support it.

This is why compatibility must be proven with representative projects rather than asserted from a happy-path crate. Oven should maintain a differential compatibility suite: given the same Rust project inputs, the supported resolver and build paths produce equivalent dependency selection and usable artifacts, while Oven adds its own receipts, policy, session, and provenance semantics.

### Interop from a developer's point of view

The implementation should disappear behind one project experience. A Rust developer adopts a repository and continues editing ordinary `.rs` files. An Incan developer imports a Rust crate through a declared `rust::` boundary. A mixed workspace uses one lock, one dependency explanation, one test/run surface, and one place to inspect why a build unit was selected or rebuilt.

The useful flows are:

- **Rust project under Oven:** existing Rust source and compatibility metadata enter unchanged; `incan build`, `incan test`, and `incan run` provide the normal workflow, with diagnostics and receipts expressed in project terms rather than as a second build system bolted alongside the first.
- **Incan code using Rust:** an Incan package declares the Rust package, version, features, and source it needs; Oven resolves it once, supplies typed interop metadata where available, and maps compiler failures back to the Incan and Rust locations that matter.
- **Rust code using Incan:** an Incan library exposes a stable Rust-facing ABI artifact and package metadata. A Rust consumer can use that boundary as an ordinary dependency; Oven owns how the Incan semantic package and its compatible Loaves are produced and selected.
- **Mixed interactive work:** a notebook session can call both Incan and Rust-backed capabilities while retaining the resolved graph and loaded build units. A small Incan cell, a plot, or a query edit should invalidate its semantic dependents, not re-create the Rust dependency world.

Capabilities sit above this boundary. They turn a project intent—such as a notebook, plotting stack, ML evaluation, query adapter, or governance editor—into explicit declared dependencies, interfaces, typed actions, file roles, and policy requirements. Once a project accepts a capability, those pieces become ordinary, visible inputs to Oven's graph. A capability is never an unrestricted build-system extension point.

## Loaves: publication-time preparation, not consumer-time surprise

The most visible consequence of this model is what happens to expensive dependency graphs.

For a library with a costly native or Rust dependency closure, the best experience is not a clever local cache that eventually becomes warm. The best experience is that `incan.pub` has already prepared and verified the compatible Loaf. A consumer should avoid an unplanned local compilation when a compatible Loaf exists, rather than discovering a multi-minute build only when they try to use the library.

That changes the division of labour:

- publishers bake, verify, and attest compatible Loaves;
- `incan.pub` distributes them against explicit target and toolchain identities;
- Oven selects and validates the right Loaf for the user's project and environment;
- local baking occurs only when needed, and is visible as preparation work rather than incidental runtime latency.

This does not eliminate source distributions, local compilation, cross-compilation, or platform-specific native dependencies. It makes those cases explicit. Where a compatible published Loaf exists, it should be preferred. Where it does not, Oven should explain what it needs to build, why reuse is unavailable, and which resulting Loaf it will retain or publish according to policy.

The trust decision belongs to the receiving project. A Loaf must bind to declared source and build inputs, carry verifiable publisher or rebuild evidence, and remain subject to the project's trust policy. If an artifact is withdrawn or found invalid, Oven must be able to explain which projects selected it and refuse, replace, or rebuild it according to policy. Prebuilt distribution removes surprise compilation; it cannot remove provenance or revocation responsibility.

The cache-identity, lifecycle, and CI-safety principles explored in current work remain valuable. Under this direction, they become the substrate for managed build units and Loaves rather than a permanent way to manage Cargo target directories.

### Native deployment handoff

A Loaf can also carry the target-resolved native facts needed to hand a binding or native dependency to a host platform build: artifact identities, binding-verification and shim-build receipts, and a versioned native deployment plan. That plan gives Gradle and Xcode useful, consistent input while leaving final application assembly and signing with the platform packager. The exact integration format belongs in the relevant platform and interop RFCs, including RFC 116; it is not fixed by this paper.

## Interactive work is the differentiator

The opportunity is larger than build speed. Python wins many exploratory and data-oriented workflows because its interactive loop is immediate. Rust brings a powerful ecosystem and predictable deployment, but its usual toolchain can make short iterations feel disproportionately expensive. Incan can make a different trade: retain Rust's systems strengths while offering a session model designed for notebooks, REPLs, rendering environments, and iterative data work.

That requires more than caching. A valid interactive session should:

- retain resolved project state while its lock, toolchain, and relevant inputs remain unchanged;
- retain compiled and loaded semantic/build units where their identities remain valid;
- invalidate only the dependents affected by an edit;
- keep compilation state distinct from runtime values and side effects;
- make cell diagnostics, artifacts, and provenance inspectable;
- surface preparation work intentionally, instead of hiding it behind an apparently small command.

The session should be exposed through a UI-neutral service contract rather than tied to one notebook product. Notebook, IDE, rendering, and application clients need the same persistent execution surface: typed values and artifacts, incremental updates, diagnostics, cancellation, resource limits, and a clear boundary between a transient interaction and a durable project mutation. Persistence is an optimisation a client can use, not a tax every command must pay: a small script or ephemeral notebook should start cleanly and retain only the state its workflow justifies.

The desired experience is straightforward: opening a notebook or rerunning an unchanged cell should not trigger dependency resolution or compilation just because the execution boundary is a new command. A small query edit should not discard a valid DataFusion graph. A dependency, feature, target, or toolchain change should invalidate the appropriate state—and say so.

### Notebooks are a general live environment, not just a faster terminal

An Incan notebook should be a general live environment for creating plots, exploring data, running models, training or evaluating machine-learning workflows, building operational interfaces, and editing domain models. It should retain the immediate, exploratory character people expect from notebooks while keeping the typed project model, provenance, and deployment path of an Incan system. A published Loaf can make a substantial backend—such as a compute engine, visualisation stack, or model runtime—available without turning first use into an unexpected compilation event.

The notebook session becomes an incremental, typed programming environment: it holds live values and model graphs, typechecks and evaluates changes as they are made, renders plots and interfaces from the current state, and preserves the link between an interaction and the source-level change that produced it. A governance UI is one important example. It should not need a second YAML-shaped configuration language that is later translated into the actual project model: a user can inspect and edit a typed Incan governance model, see dependent views update, and use that same model in validation, policy, automation, and deployment.

This needs a clear state boundary. A filter choice, cursor position, or transient preview can remain session-local. A change to a governance rule, data model, policy, or declared action is a project mutation: Oven must identify the affected Incan model, validate the proposed change, show a reviewable source or structured-model diff, evaluate policy, and record provenance when the project accepts it. An accepted durable edit must round-trip to a project-owned, versionable Incan declaration or structured Incan artifact; the exact surface syntax can remain a later design decision. The UI cannot become an opaque side channel that quietly owns configuration.

Capabilities make this practical without weakening the boundary. A capability can declare that a project offers a governance editor, its typed actions, required model interfaces, file roles, and policy requirements. It may provide the UI and its model-aware views. It does not get an unrestricted install hook or independent authority to mutate the model or build graph. The project remains the source of truth; the capability is an explicit, inspectable integration.

## IncQL and DataFusion: the proving ground

IncQL is a good first proof because its DataFusion backend makes the cost visible. Today, the question is not whether a warm cache can sometimes help. The question is whether an IncQL notebook or rendering environment can depend on a prepared graph without turning normal edits into a long compilation event.

The right target is a published, compatible Loaf for the expensive DataFusion closure, paired with a persistent Oven session for the user-facing project. Then the costly graph becomes a publication-time preparation cost, not a recurring tax on tests, notebook cells, or small IncQL edits.

This should be treated as a measurable product proof, not a slogan. The evaluation needs separate measurements for:

- cold environment preparation;
- first use with a compatible published Loaf;
- warm notebook open;
- unchanged cell rerun;
- a small query edit;
- a helper or dependency change;
- toolchain or feature change;
- session restart.

It should also distinguish session start, planning, execution, and rendering. A fast build with slow planning is not an interactive win; a fast query with a slow notebook bootstrap is not one either. DataFusion is the first demanding proof, not the sole target: the model should eventually support other compute engines and ordinary Rust-heavy workflows too.

## What Incan 1.0 should mean

The proposed 1.0 contract is deliberately concrete:

> An installed Incan SDK can resolve, build, test, run, package, and support editor and interactive workflows for Incan-managed projects—including ordinary Rust projects—without requiring or invoking Cargo in the normal workflow.

This does not require Incan to reject the Rust ecosystem. It requires Incan to own the interface to it. Existing Rust source, packages, registries, and compiler backends remain inputs to the system. The installed user experience no longer depends on Cargo being present, being fast, or defining the shape of project state.

Success would look like this:

- a Rust developer can adopt Incan incrementally and keep using the libraries they need;
- an Incan project has one project graph across Incan and Rust rather than split ownership;
- `incan.pub` can provide verified compatible Loaves before a consumer needs them;
- notebooks and rendering environments can remain alive through ordinary edits;
- notebooks can support plots, data exploration, ML, live interfaces, and direct editing of typed Incan models without discarding the project and session state that makes those workflows interactive;
- persistent model changes remain reviewed and governed rather than hidden in separate configuration files;
- tools can explain every reuse, rebuild, incompatibility, and artifact origin;
- CI and local development follow the same resolution and build semantics.

## Delivery direction after 0.5

This paper should sharpen the roadmap after Incan 0.5 lands; it should not substitute for that roadmap work. The natural sequence is to move the existing semantic and compiler plumbing toward an Incan-owned project/build spine, then make the boundary visible and testable.

The likely tracks are:

1. Native project identity, resolution, and lock semantics for mixed Incan/Rust workspaces.
2. An Incan-owned build planner and `rustc` executor, including explicit contracts for build scripts, procedural macros, native linking, and target selection.
3. Managed build units, receipts, and Loaf publication/selection through `incan.pub`.
4. Persistent interactive sessions with precise invalidation, runtime-state separation, and a UI-neutral execution-service contract.
5. A Cargo-free compatibility and release gate that proves the installed SDK contract against representative Rust-only, mixed, native, build-script/proc-macro, and interactive projects.

Existing work should be classified against those tracks rather than discarded. Compiler middle-end progress, HIR and ABI work, workspace support, pinned toolchains, artifact graph work, `incan.pub`, typed actions, and managed cache safety all contribute. The needed change is to make the shared end-state explicit: they should converge on an Incan-owned toolchain rather than a more sophisticated way of calling Cargo.

Broader operating-system or kernel ambitions can remain valuable proof points later. They should not be the organizing metaphor for the immediate roadmap. The first obligation is to dogfood the toolchain on Incan, Rust projects, and IncQL until it is plainly the better development environment.

## What this proposal does not claim

Several boundaries keep the proposal credible.

- It does not claim a finished Oven implementation today.
- It does not claim measured IncQL or DataFusion speedups before the relevant dependency graph and session matrix have been benchmarked.
- It does not require every Cargo behaviour to be reproduced exactly.
- It does not imply that all builds can be precompiled for every target or that local compilation disappears.
- It does not turn `incan.pub` into an opaque binary-only distribution channel; source, provenance, policy, and reproducibility remain first-class.
- It does not make a registry the authority over a local project. Local projects retain control over dependency selection, mutation, and trust policy.

Those constraints are features, not concessions. They define a toolchain that can be ambitious without hiding the difficult engineering work.

## Closing

The strategic choice is not whether Incan can make Cargo a little less painful. It is whether Incan wants to own the developer loop it is asking people to adopt.

If the answer is yes, then the toolchain needs to be designed around Incan's actual strengths: a unified semantic model, explicit project state, inspectable artifacts, controlled provenance, and persistent interactive work. Oven is a proposed name for that system. Loaves are a proposed way to make its publication model tangible. The durable idea is simpler: Incan should give Rust and Incan developers a toolchain that feels prepared for the work they are doing, rather than one that asks them to wait for a separate build tool to catch up.
