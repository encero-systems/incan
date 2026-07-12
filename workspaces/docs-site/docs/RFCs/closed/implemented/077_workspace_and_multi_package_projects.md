# RFC 077: workspace and multi-package projects

- **Status:** Implemented
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 034 (`incan.pub` package registry)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 079 (`incan.pub` artifact graph)
- **Issue:** [#405](https://github.com/encero-systems/incan/issues/405)
- **RFC PR:** [#830](https://github.com/encero-systems/incan/pull/830)
- **Written against:** v0.3
- **Shipped in:** v0.5

## Summary

This RFC defines a first-class workspace model for Incan projects that contain multiple related packages, applications, libraries, tools, examples, or generated artifacts in one repository. RFC 015 envs answer how commands run; workspaces answer what project members exist, how they share dependency resolution and policy, and how lifecycle commands operate across member boundaries.

## Core model

Read this RFC as nine foundations:

1. **Topology is explicit:** a workspace declares its root and members instead of relying on ad-hoc directory conventions.
2. **Members remain ordinary projects:** each member can have its own package metadata, source tree, dependencies, envs, scripts, capabilities, and publish settings.
3. **Root identity is structural:** when `[project]` and `[workspace]` coexist in the root manifest, the root project is a workspace member automatically; a root with only `[workspace]` is virtual.
4. **Resolution is workspace-wide:** one canonical root lockfile represents the dependency graph of every member, independent of which members a command executes.
5. **Commands select a scope:** lifecycle commands must know whether they apply to the current member, selected members, default members, or the whole workspace.
6. **Inheritance is explicit and refinable:** members opt into shared dependency and environment declarations; inherited Rust dependencies may add usage-specific features and optionality without changing shared dependency identity.
7. **Envs compose with topology:** RFC 015 envs still describe execution context, but workspace commands decide which members receive that context.
8. **Capabilities are scoped:** starters and capability packs may target one member, multiple members, or workspace-level metadata, and must show that scope in mutation plans.
9. **The workspace is publish-aware:** publication, documentation, artifact discovery, and policy may need workspace-level views without forcing every member to publish or version together.

## Motivation

RFC 015 gives Incan a project lifecycle and RFC 073 gives env matrices a way to describe execution variation. Those are necessary but not enough for repositories that naturally contain multiple packages: an application plus internal libraries, a CLI plus a core library, examples plus integration tests, or a product surface plus shared schemas. Treating those as unrelated projects loses shared locking, shared policy, shared scripts, and cross-member dependency intent.

Cargo and uv both make workspaces a central scaling primitive. Incan should have the same topology concept, but it should integrate with Incan-specific concerns: capability application, template provenance, env matrices, policy evaluation, and future artifact graph publication.

## Goals

- Define workspace root discovery and workspace member declaration.
- Allow non-root members to be declared by path or glob and resolved members to be selected by name, path, default set, or all-members mode.
- Allow an existing root project to become the implicit root member without relocating its source tree or repeating `"."` in `members`.
- Define one shared workspace lock and deterministic whole-workspace dependency resolution.
- Define explicit inheritance for shared Incan dependencies, refinable Rust dependencies, and environments.
- Define how RFC 015 envs and RFC 073 matrices apply across members.
- Define workspace-level policy and catalog/source configuration hooks for RFC 076.
- Define how RFC 075 starters and capabilities can apply to one member, selected members, or workspace-level metadata.
- Define machine-readable workspace inspection for CLI, LSP, IDEs, docs tooling, and agents.
- Leave room for `incan.pub` to understand workspace artifact relationships without requiring all members to publish together.

## Non-Goals

- Defining a monorepo build system with remote execution, distributed caching, or task graph scheduling.
- Replacing RFC 015 envs or RFC 073 matrices.
- Requiring every project to be a workspace.
- Requiring all workspace members to share one package version.
- Inferring workspace membership from path dependencies.
- Supporting nested workspaces or members outside the workspace root.
- Providing atomic multi-package publication; publication remains a per-member operation.
- Defining registry hosting behavior for workspace artifacts.

## Terminology and mental model

- A **project** or **package** is an independently named and versioned unit described by `[project]`; it can declare dependencies and may be published.
- A **member** is a project that belongs to one workspace. Membership says that projects are managed together; it does not imply that they depend on one another.
- A **workspace** is the repository-level coordination boundary for members, dependency resolution, locking, policy, and command selection. It is not itself a package or compiled artifact.
- An **environment** describes how commands run: toolchain constraints, variables, scripts, dependency overlays, and matrices. Workspace selection determines which members run; environment selection determines the execution context for each selected member.

## Guide-level explanation

### Declaring a workspace

A repository may add a workspace to an existing root project without moving that project:

```toml
[project]
name = "query_core"
version = "0.4.0"

[workspace]
members = ["packages/storage_backend", "examples/*"]
exclude = ["examples/experimental"]

[workspace.dependencies]
query_core = { path = "." }

[workspace.rust-dependencies]
serde = { version = "1", default-features = false }
```

Each non-root member remains an ordinary Incan project and opts into shared declarations explicitly:

```toml
[project]
name = "storage_backend"
version = "0.1.0"

[dependencies]
query_core = { workspace = true }

[rust-dependencies]
serde = { workspace = true, features = ["derive", "std"] }
```

The member inherits Serde's version, source, and default-feature policy from the workspace, then adds the `derive` and `std` features for its own use. A member may also mark an inherited Rust dependency as `optional`; it cannot replace the inherited version or source.

The resulting repository keeps independent package boundaries:

```text
repo/
  incan.toml
  incan.lock
  src/
    lib.incn
  packages/
    storage_backend/
      incan.toml
      src/lib.incn
  examples/
    demo/
      incan.toml
      src/main.incn
```

The root coordinates member discovery, dependency resolution, shared lock state, policy, and tooling. Because the same manifest contains `[project]` and `[workspace]`, `query_core` is the root member automatically. A manifest containing `[workspace]` without `[project]` instead defines a virtual workspace root.

### Running across members

A user can run commands against the current member, the default members, or the whole workspace:

```bash
incan test
incan test --workspace
incan test --member storage_backend
incan run --member demo
incan workspace inspect --format json
```

With no explicit selector, a command run inside a member targets that member. The workspace-root directory is the exception: it targets `default-members` when configured, otherwise the implicit root member in a rooted workspace, or every member in a virtual workspace. A command below the root but outside every non-root member belongs to the implicit root member. `--workspace` selects every member, while one or more `--member` flags select named members or member paths.

Commands that inherently operate on one project, such as `run` or `version`, must reject a scope containing multiple members and ask for an explicit member. Commands such as `check`, `build`, `test`, and `fmt` may operate across a selected member set. `lock` is workspace-wide whenever a workspace is active because the root lock represents every member.

### Sharing environments explicitly

The workspace may define reusable RFC 015 environment fragments:

```toml
[workspace.envs.ci]
requires-incan = ">=0.5,<0.6"

[workspace.envs.ci.env-vars]
CI = "true"
```

A member inherits that environment only by naming the workspace-qualified environment in its local `extends` list:

```toml
[tool.incan.envs.ci]
extends = ["workspace:ci"]

[tool.incan.envs.ci.scripts]
test = ["incan", "test"]
```

Workspace environments therefore remove duplication without becoming ambient configuration that silently changes unrelated members.

### Applying capabilities in a workspace

A capability can target one member:

```bash
incan capability add cli --member packages/cli --dry-run
```

Or a workspace-level capability can add coordinated metadata:

```bash
incan capability add workspace.ci --workspace --dry-run
```

The dry-run plan must show which member receives each file, manifest entry, dependency, script, env, policy, or agent-guidance change.

## Reference-level explanation

### Workspace manifest schema

A workspace root is a directory whose `incan.toml` contains a top-level `[workspace]` table. When the same manifest also contains `[project]`, that project is the implicit root member. A manifest with `[workspace]` but no `[project]` is a virtual workspace root.

`[workspace]` has these fields:

- `members: list[str]` contains explicit root-relative paths or glob patterns for non-root members. It may be omitted or empty when an implicit root member exists; a virtual workspace must expand at least one member.
- `default-members: list[str]` is optional and names the members selected when a command starts at the workspace root without an explicit selector.
- `exclude: list[str]` is optional and removes root-relative non-root paths from the expanded member set.

The implicit root member must have a non-empty `[project].name` and is not repeated in `members`. A literal `"."`, or a path or glob that canonicalizes to the workspace root, is invalid in `members` and `exclude`; diagnostics must explain that `[project]` already establishes root membership. The root may still be selected by project name or `"."` in `default-members`, `--member`, and machine-readable tooling.

A path dependency on a project does not make that project a workspace member, and declaring a shared dependency does not make its target a member. All non-root membership therefore comes from `members` after applying `exclude`.

Every expanded non-root member path must resolve to a directory containing an `incan.toml` with a non-empty `[project].name`. Member paths must remain beneath the canonical workspace root after resolving `.` components and symbolic links. Nested workspaces and external members are not supported by this RFC.

Member globs must be expanded in deterministic lexical order. Exclusions are applied after expansion, duplicate canonical paths are removed, and a literal member path that does not contain a manifest is an error. A glob that matches no members should produce a warning so temporary repository states remain inspectable without silently hiding a likely typo.

The implicit root member, when present, is first in deterministic workspace order. Expanded non-root members follow in canonical lexical path order.

The final workspace graph, including the implicit root member when present, must be non-empty.

Member names, including the implicit root member's name, must be unique within one workspace. Duplicate names make the workspace invalid; commands must not silently choose one by path order.

Every `default-members` entry must resolve to one member by name or root-relative path. Missing, excluded, or ambiguous defaults make the workspace invalid. When `default-members` is absent, commands started at the workspace root default to the implicit root member in a rooted workspace and to every member in a virtual workspace.

### Workspace discovery and member identity

Project discovery continues to locate the nearest `incan.toml` as defined by RFC 015. Workspace discovery then searches that manifest and its ancestors for the nearest `[workspace]` whose member graph contains the discovered project. The root project is contained structurally by `[project]` in the workspace manifest; non-root projects must be present in the expanded `members` set. An ancestor workspace that does not contain the project has no authority over it.

When a command starts at a virtual workspace root, the workspace is still a valid command context even though the root has no project identity. When the root manifest has both sections, the root has both workspace identity and member identity automatically.

Each workspace graph must record the canonical root, root manifest, ordered members, member names and manifests, root-member status, defaults, exclusions, and the relationship between each member and its path dependencies. A member must belong to at most one active workspace graph.

### Command scope

Workspace-aware commands must resolve scope before compiling, executing, locking, or mutating member state. The standard selectors are:

- `--workspace` selects every member and conflicts with `--member`.
- `--member <name-or-path>` selects one member and may be repeated by commands that support multiple members.
- no selector inside a member selects that member. The workspace-root directory is the exception: it selects `default-members` when configured, otherwise the implicit root member in a rooted workspace, or every member in a virtual workspace. A descendant not contained by a non-root member belongs to the implicit root member.

Explicit selectors take precedence over current-directory inference. Unknown names, paths outside the workspace, ambiguous selections, and an empty selected set are errors.

Commands such as `check`, `build`, `test`, and `fmt` may support multiple members and must process the selected set in deterministic workspace order. Commands whose semantics require one project, including `run` and `version`, must reject a multi-member scope with a diagnostic that requests `--member`. `incan lock` must resolve and write the complete workspace graph regardless of the current member or selector.

Diagnostics, progress output, build reports, and machine-readable results must identify the workspace root, selected scope, and member associated with each result. A command that mutates files or manifests must not silently apply to more members than the resolved scope.

### Shared dependencies and explicit inheritance

The workspace root may declare shared dependency specifications under `[workspace.dependencies]`, `[workspace.rust-dependencies]`, and `[workspace.rust-dev-dependencies]`. Paths in these tables are resolved relative to the workspace root.

A shared declaration is a reusable specification, not an activated dependency. Every member, including the implicit root member, inherits one only by declaring the same key with `{ workspace = true }` in the corresponding member table.

For Incan library dependencies under `[workspace.dependencies]`, the current dependency schema has no member-level usage refinements. An inheriting `[dependencies]` entry therefore contains only `workspace = true`.

For Rust dependencies, an inherited member entry may contain only `workspace`, `features`, and `optional`. The workspace owns dependency identity and the member owns package-local usage:

- The workspace declaration owns the version requirement, registry/git/path source, git reference, package rename, and `default-features` baseline. A member must not override those fields while inheriting.
- A member may add `features`; its feature set is the deterministic union of workspace and member features.
- A member may set `optional` because optionality belongs to the consuming package. Workspace Rust dependency declarations must not set `optional`.
- Refinement is additive. A member cannot remove a workspace feature or disable defaults enabled by the workspace. When the workspace disables default features, a member may add the dependency's `default` feature explicitly.

For example:

```toml
# Workspace root
[workspace.rust-dependencies]
serde = { version = "1", default-features = false }

# Member
[rust-dependencies]
serde = { workspace = true, features = ["derive", "std"], optional = true }
```

If a member needs a different version, source, package rename, or default-feature baseline, it must stop inheriting and declare a complete member-local dependency instead. Workspace dependency entries should therefore define the smallest baseline shared by their inheriting members.

Missing workspace keys, dependency-kind mismatches, inheritance cycles, and prohibited refinements are manifest errors. Inspection and diagnostics must distinguish workspace-owned fields, member refinements, and the effective dependency. For Rust features, tooling must report the workspace baseline, each member's additions, and the effective feature set so cross-member feature activation is attributable rather than ambient. When a selected build graph causes Cargo to unify requests from multiple members, the build report must identify every member that enabled each resulting feature.

### Shared lock state

A declared workspace has one canonical `incan.lock` at the workspace root. Lock generation must resolve the union of every member's effective dependency graph after inheritance and refinement, regardless of the execution scope of the command that triggered locking. An unused shared declaration is inspectable configuration but does not activate or resolve a package. Member selection must never change the root lock fingerprint.

Project-aware commands run from any member must consult the root lock. A member-local `incan.lock` inside a declared workspace is not authoritative and must not be read as a fallback. Workspace inspection must report stale member-local lockfiles so migration is visible, but tooling must not delete them without an explicit mutation operation.

Commands that require locked or frozen operation must fail if the root lock is absent, stale for any member, or inconsistent with any effective inherited declaration. A command targeting one member must still respect the whole-workspace lock contract.

### Environments and matrices

RFC 015 environments and RFC 073 matrices remain execution-context features. Workspaces add reusable environment definitions and member selection without changing the meaning of one resolved environment.

The workspace root may declare environment fragments under `[workspace.envs.<name>]`. A member opts into a fragment by including `workspace:<name>` in a local environment's `extends` list. Workspace environments are never inherited by name coincidence or by being present at the root.

Workspace-qualified and member-local environment layers are resolved in declared `extends` order using RFC 015 merge rules. Cycles across workspace and member environments are errors. Matrix expansion occurs after member selection and is reported separately for each selected member.

### Workspace mutation plans and policy

Any workspace-scoped mutation plan must include member scope. For each planned change, the plan must state whether it affects the workspace root, one member, selected members, or every member.

Workspace mutation plans must be compatible with RFC 076 policy. Policy may require additional approval for cross-member changes, shared dependency changes, shared environment changes, shared lock changes, or workspace-level source policy changes. When workspace and member policy both apply, the more restrictive outcome wins.

Workspace-aware capability application from RFC 075 requires, at minimum, a validated workspace graph, deterministic member selection, machine-readable inspection, scoped mutation planning, and policy evaluation. Until all five are available, capability commands must remain member-local rather than approximating whole-workspace behavior.

### Machine-readable inspection

The CLI must expose `incan workspace inspect` with a JSON output mode. The machine-readable workspace view must contain:

- a schema version;
- workspace root and manifest path;
- members, canonical paths, names, and root-member status;
- default members and exclusions;
- selected scope for the current invocation;
- shared dependency declarations, member refinements, effective specifications, and feature-enablement provenance;
- shared environments and member inheritance provenance;
- shared policy and source configuration;
- member capabilities and provenance summaries;
- root lock state, fingerprint, and lockfile location; and
- warnings such as unmatched globs, unused shared declarations, and stale member-local lockfiles.

Inspection must remain available when the active toolchain does not satisfy a member's `requires-incan` constraint, although the result must report that incompatibility.

### Publication and versions

Workspace members keep independent `[project].version` values and are published independently. A future convenience command may orchestrate publication of several selected members, but each publication remains a separately validated operation with its own artifact identity and result. This RFC does not define an atomic workspace release.

### Compatibility and migration

Manifests without `[workspace]` retain RFC 015 single-project behavior. Existing projects become implicit root members by adding `[workspace]`; they do not list `"."`, move their source tree, or change package identity.

When separate projects adopt one workspace, they must generate a canonical root lockfile. Existing member-local lockfiles cease to be authoritative and should be removed through an explicit reviewed change after workspace inspection confirms the new root lock.

## Design details

### Relationship to RFC 015

RFC 015 owns single-project lifecycle commands, manifest metadata, envs, scripts, and nearest-project discovery. This RFC preserves that project identity and adds an ancestor workspace graph when the project is the implicit root member or an explicitly listed non-root member.

### Relationship to RFC 020

RFC 020 owns locked and frozen dependency policy. This RFC changes the lock ownership boundary for a declared workspace: the root lock covers every member and is the only lock consulted by workspace members.

### Relationship to RFC 073

RFC 073 owns matrix expansion and toolchain constraints. Workspace member selection happens before env or matrix execution. Matrix expansion should be reported per selected member.

### Relationship to RFC 075

Starter and capability descriptors may be workspace-aware only after workspace graph validation, selection, inspection, scoped mutation planning, and policy evaluation are available. A descriptor that mutates multiple members must report member scope in its dry-run and machine-readable plan.

### Relationship to RFC 076

Workspace-level policy may be stricter than member-level policy. If multiple policies apply, RFC 076's conservative precedence model should be used unless this RFC later defines a more specific workspace precedence rule.

### Relationship to RFC 079

The artifact graph may represent workspace relationships: root project, member packages, examples, docs, generated artifacts, AI assets, and publishable units. This RFC defines the local topology that a future registry can mirror.

### Why root membership follows manifest identity

`[project]` already declares a project at the workspace root, so repeating `"."` in `members` adds ceremony without adding identity. More importantly, a root project outside its colocated workspace would compete with that workspace for the same root command context and canonical `incan.lock`. Treating the root as an automatic member removes that incoherent state while keeping every non-root member explicit. A virtual root remains unambiguous because it omits `[project]`.

## Alternatives considered

### Model workspaces as envs

Rejected because envs describe execution context, while workspaces describe project topology. Collapsing them would make it hard to express shared locks, member selection, cross-member dependencies, and publish topology.

### Require one package per repository

Rejected because real projects often need multiple related packages, examples, tools, and applications in one repository.

### Infer members from path dependencies

Rejected because dependency edges and workspace ownership are different relationships. Automatic membership would make an unrelated nested project join workspace commands, policy, and lock resolution merely because one package references it.

### Require `"."` for root membership

Rejected because `[project]` in the workspace manifest already declares the root project explicitly. Requiring a second marker creates an omission footgun, while permitting the omission to exclude the root creates ambiguous command and lock ownership.

### Allow a root project outside its colocated workspace

Rejected because the project and workspace would both claim the same root command context and `incan.lock` path while describing different dependency graphs. A repository that needs a virtual workspace root should omit `[project]` there and keep every project in an explicit member directory.

### Keep one lockfile per member

Rejected because independently resolved member locks cannot represent shared dependency constraints or guarantee that a workspace-wide command sees one coherent dependency graph. Per-member locks would also make results depend on command entrypoint and selection.

### Require moving the root package under `packages/`

Rejected because directory layout should not determine whether a package can participate in a workspace. Implicit root membership provides the same boundary without forcing repository-wide path churn.

### Make every project a workspace

Rejected because it would add unnecessary conceptual overhead to small projects. Single-project behavior should remain simple.

## Drawbacks

- Workspaces add another layer of command scope that users must understand.
- Shared dependency and env inheritance can become confusing without strong diagnostics.
- Additive Rust features requested by several selected members may broaden the effective feature set through Cargo feature unification; build reports must make that provenance visible.
- Cross-member mutation plans increase the importance of machine-readable output and policy review.
- Whole-workspace locking means a stale dependency declaration in an unselected member can block a locked command in another member.
- First-class workspace support requires lifecycle commands, the test runner, build reports, and editor tooling to carry member identity consistently.

## Implementation architecture

The implementation should build one validated workspace graph before command planning. The graph contains the root, ordered members, shared declarations, member refinements, effective dependency specifications, root lock state, and selected scope. Existing project commands can then operate on one or more member project contexts while preserving RFC 015 behavior for projects outside a workspace.

## Layers affected

- **Manifest schema / configuration validation:** manifests need workspace root, implicit root-member, non-root member, default-member, shared dependency, member refinement, shared env, and shared policy fields.
- **CLI / tooling:** lifecycle commands need member selection, workspace discovery, workspace inspection, and workspace-scoped mutation plans.
- **Locking / dependency resolution:** shared lockfiles and shared dependency constraints must be understood by project resolution.
- **Build / test / format orchestration:** multi-member commands must fan out deterministically while preserving member-local source roots, diagnostics, reports, and generated artifacts.
- **LSP / IDE tooling:** editor tooling should surface workspace members, default members, selected command scope, and member-specific diagnostics.
- **Agentic tooling:** agents may use workspace topology to select relevant project skills, but must respect member scope and policy.
- **Documentation:** docs must explain the difference between envs, members, workspace roots, and project packages.

## Implementation Plan

### Phase 1: Validated workspace topology

- Parse workspace declarations alongside existing project manifests without changing single-project behavior.
- Construct a deterministic workspace graph with implicit rooted membership, virtual-root support, explicit non-root paths and globs, exclusions, default members, and precise validation diagnostics.
- Make project discovery recognize only an ancestor workspace that actually contains the current project.

### Phase 2: Scope, inspection, and command routing

- Resolve current-directory, default-member, `--member`, and `--workspace` selection before commands read or mutate member state.
- Add human-readable and JSON `incan workspace inspect` output with graph identity, selected scope, inherited configuration provenance, lock state, and warnings.
- Route single-project and multi-member commands through one scope-aware orchestration boundary, rejecting multi-member use where a command intrinsically requires one project.

### Phase 3: Shared dependency, environment, and lock contracts

- Validate explicit workspace dependency inheritance, including permitted Rust feature and optional refinements, and report effective provenance.
- Resolve explicit workspace environment extensions and reject cross-root/member cycles.
- Publish and consume one root lock that represents the complete workspace graph regardless of the selected member set, using the durable publication and locking contract.

### Phase 4: Lifecycle, tooling, and documentation

- Fan out supported build, check, test, and format commands in deterministic member order with member-scoped diagnostics and machine-readable results.
- Keep capabilities member-local until the graph, selection, inspection, scoped mutation planning, and policy-evaluation prerequisites are all available.
- Make the workspace graph available to project-aware editor and tooling entrypoints without independently rediscovering topology.
- Document rooted and virtual workspace behavior, selection, inheritance, locks, and migration from member-local locks.

## Implementation log

### Spec / design

- [x] Settle rooted versus virtual workspace identity, explicit non-root membership, deterministic selection, whole-workspace locking, and additive Rust dependency refinements.
- [x] Record the constrained v0.5 boundary: no registry, task scheduler, atomic workspace publish, or implicit capability fan-out.

### Manifest and topology

- [x] Parse and validate `[workspace]` declarations without changing manifests that omit the table.
- [x] Build a deterministic graph for rooted and virtual workspaces, explicit paths, globs, exclusions, defaults, duplicate names, and invalid containment.
- [x] Resolve a workspace only when it contains the discovered project, preserving nearest-project behavior outside a workspace.

### Scope and CLI

- [x] Resolve unqualified current-directory/default-member scope and explicit `--member`/`--workspace` scope before member work begins.
- [x] Add `incan workspace inspect` in human and JSON forms with selected scope, provenance, warnings, and lock state.
- [x] Reject invalid, ambiguous, empty, or intrinsically multi-project command scopes with actionable diagnostics.

### Shared configuration and locks

- [x] Validate explicit shared Incan, Rust, and Rust dev dependency inheritance and effective Rust feature provenance.
- [x] Resolve explicit workspace environment inheritance and reject cross-layer cycles.
- [x] Read and publish only the canonical root lock for a workspace; retain member-local locks as visible, non-authoritative migration warnings.
- [x] Cover crash-safe root lock publication and concurrent lock access through the established durability primitives.

### Command and tooling integration

- [x] Fan out supported check, build, test, and format commands in deterministic member order while retaining unchanged single-project behavior.
- [x] Carry workspace/member identity into diagnostics, reports, test-batch results, LSP project context, and codegraph project discovery where observable.
- [x] Keep workspace capability operations rejected or member-local until RFC 075/RFC 076 prerequisites are implemented rather than approximating cross-member mutation.

### Tests and documentation

- [x] Add rooted and virtual fixture coverage for topology, selection, manifests, locks, inheritance, command fan-out, generated Rust, and JSON inspection.
- [x] Add Linux and macOS integration coverage for concurrent root-lock access and crash-safe publication.
- [x] Update reference documentation, generated references, feature metadata, release notes, and migration guidance.

## Design Decisions

- `[project]` in the workspace manifest creates an implicit root member; `[workspace]` without `[project]` creates a virtual root.
- All non-root membership comes only from explicit `members` paths and globs. Path dependencies never create membership.
- Non-root members and their canonical paths must remain beneath the workspace root. Nested and external workspaces are deferred.
- Shared Incan, Rust, and Rust dev dependencies require `{ workspace = true }` in each member that inherits them. Rust members may add features by set union and choose member-local optionality; version, source, package rename, and default-feature policy remain workspace-owned.
- One root `incan.lock` covers every member's effective dependency graph. Unused shared declarations do not activate packages, and command selection does not narrow dependency resolution or change the lock fingerprint.
- Workspace environments are inherited explicitly through `workspace:<name>` entries in member-local `extends` lists.
- `--workspace` selects every member, repeatable `--member` selects named members or paths, and current-directory/default-member rules define unqualified scope.
- Publication and versions remain per member. A workspace publication command may orchestrate those operations later but does not make them atomic or lockstep.
- Non-root examples, documentation tools, and generated projects are members only when they have their own project manifest and are selected explicitly by `members`.
- Workspace-aware capabilities require graph validation, deterministic selection, machine-readable inspection, scoped mutation planning, and policy evaluation. Until that foundation exists, capabilities remain member-local.
