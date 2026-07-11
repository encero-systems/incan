# RFC 077: workspace and multi-package projects

- **Status:** Planned
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
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a first-class workspace model for Incan projects that contain multiple related packages, applications, libraries, tools, examples, or generated artifacts in one repository. RFC 015 envs answer how commands run; workspaces answer what project members exist, how they share dependency resolution and policy, and how lifecycle commands operate across member boundaries.

## Core model

Read this RFC as nine foundations:

1. **Topology is explicit:** a workspace declares its root and members instead of relying on ad-hoc directory conventions.
2. **Members remain ordinary projects:** each member can have its own package metadata, source tree, dependencies, envs, scripts, capabilities, and publish settings.
3. **A root may also be a member:** `[project]` and `[workspace]` may coexist in the root manifest, but the root project participates only when `"."` is declared explicitly in `members`.
4. **Resolution is workspace-wide:** one canonical root lockfile represents the dependency graph of every member, independent of which members a command executes.
5. **Commands select a scope:** lifecycle commands must know whether they apply to the current member, selected members, default members, or the whole workspace.
6. **Inheritance is explicit:** members opt into shared dependency and environment declarations; workspace declarations do not leak into every member automatically.
7. **Envs compose with topology:** RFC 015 envs still describe execution context, but workspace commands decide which members receive that context.
8. **Capabilities are scoped:** starters and capability packs may target one member, multiple members, or workspace-level metadata, and must show that scope in mutation plans.
9. **The workspace is publish-aware:** publication, documentation, artifact discovery, and policy may need workspace-level views without forcing every member to publish or version together.

## Motivation

RFC 015 gives Incan a project lifecycle and RFC 073 gives env matrices a way to describe execution variation. Those are necessary but not enough for repositories that naturally contain multiple packages: an application plus internal libraries, a CLI plus a core library, examples plus integration tests, or a product surface plus shared schemas. Treating those as unrelated projects loses shared locking, shared policy, shared scripts, and cross-member dependency intent.

Cargo and uv both make workspaces a central scaling primitive. Incan should have the same topology concept, but it should integrate with Incan-specific concerns: capability application, template provenance, env matrices, policy evaluation, and future artifact graph publication.

## Goals

- Define workspace root discovery and workspace member declaration.
- Allow members to be selected by name, path, glob, default set, or all-members mode.
- Allow an existing root project to become an explicit workspace member without relocating its source tree.
- Define one shared workspace lock and deterministic whole-workspace dependency resolution.
- Define explicit inheritance for shared Incan dependencies, Rust dependencies, and environments.
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

## Guide-level explanation

### Declaring a workspace

A repository may add a workspace to an existing root project without moving that project:

```toml
[project]
name = "query_core"
version = "0.4.0"

[workspace]
members = [".", "packages/storage_backend", "examples/*"]
default-members = ["."]
exclude = ["examples/experimental"]

[workspace.dependencies]
query_core = { path = "." }

[workspace.rust-dependencies]
serde = { version = "1", features = ["derive"] }
```

Each non-root member remains an ordinary Incan project and opts into shared declarations explicitly:

```toml
[project]
name = "storage_backend"
version = "0.1.0"

[dependencies]
query_core = { workspace = true }

[rust-dependencies]
serde = { workspace = true }
```

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

The root coordinates member discovery, dependency resolution, shared lock state, policy, and tooling. When `"."` is a member, the same manifest also describes an ordinary project at the root.

### Running across members

A user can run commands against the current member, the default members, or the whole workspace:

```bash
incan test
incan test --workspace
incan test --member storage_backend
incan run --member demo
incan workspace inspect --format json
```

With no explicit selector, a command run inside a member targets that member. A command run at a virtual workspace root targets `default-members`, or every member when no defaults are declared. `--workspace` selects every member, while one or more `--member` flags select named members or member paths.

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

A workspace root is a directory whose `incan.toml` contains a top-level `[workspace]` table. The same manifest may also contain `[project]`; the two sections have independent meanings.

`[workspace]` has these fields:

- `members: list[str]` is required, must be non-empty, and contains explicit root-relative member paths or glob patterns.
- `default-members: list[str]` is optional and names the members selected when a command starts at a virtual workspace root without an explicit selector.
- `exclude: list[str]` is optional and removes root-relative paths from the expanded member set.

The root project is a member only when `members` contains `"."`. A path dependency on a project does not make that project a workspace member, and declaring a shared dependency does not make its target a member.

Every expanded member path must resolve to a directory containing an `incan.toml` with a non-empty `[project].name`. Member paths must remain beneath the canonical workspace root after resolving `.` components and symbolic links. Nested workspaces and external members are not supported by this RFC.

Member globs must be expanded in deterministic lexical order. Exclusions are applied after expansion, duplicate canonical paths are removed, and a literal member path that does not contain a manifest is an error. A glob that matches no members should produce a warning so temporary repository states remain inspectable without silently hiding a likely typo.

The final expanded member set must be non-empty.

Member names must be unique within one workspace. Duplicate names make the workspace invalid; commands must not silently choose one by path order.

Every `default-members` entry must resolve to one expanded member by name or root-relative path. Missing, excluded, or ambiguous defaults make the workspace invalid.

### Workspace discovery and member identity

Project discovery continues to locate the nearest `incan.toml` as defined by RFC 015. Workspace discovery then searches that manifest and its ancestors for the nearest `[workspace]` whose expanded member set contains the discovered project. An ancestor workspace that does not list the project has no authority over it.

When a command starts at a virtual workspace root that has `[workspace]` but no `[project]`, the workspace is still a valid command context. When the root has both sections and lists `"."`, the root has both workspace identity and member identity.

Each workspace graph must record the canonical root, root manifest, ordered members, member names and manifests, root-member status, defaults, exclusions, and the relationship between each member and its path dependencies. A member must belong to at most one active workspace graph.

### Command scope

Workspace-aware commands must resolve scope before compiling, executing, locking, or mutating member state. The standard selectors are:

- `--workspace` selects every member and conflicts with `--member`.
- `--member <name-or-path>` selects one member and may be repeated by commands that support multiple members.
- no selector uses the member containing the current directory; at a virtual workspace root it uses `default-members`, or all members when no defaults are declared.

Explicit selectors take precedence over current-directory inference. Unknown names, paths outside the workspace, ambiguous selections, and an empty selected set are errors.

Commands such as `check`, `build`, `test`, and `fmt` may support multiple members and must process the selected set in deterministic workspace order. Commands whose semantics require one project, including `run` and `version`, must reject a multi-member scope with a diagnostic that requests `--member`. `incan lock` must resolve and write the complete workspace graph regardless of the current member or selector.

Diagnostics, progress output, build reports, and machine-readable results must identify the workspace root, selected scope, and member associated with each result. A command that mutates files or manifests must not silently apply to more members than the resolved scope.

### Shared dependencies and explicit inheritance

The workspace root may declare shared dependency specifications under `[workspace.dependencies]`, `[workspace.rust-dependencies]`, and `[workspace.rust-dev-dependencies]`. Paths in these tables are resolved relative to the workspace root.

A member inherits a shared specification only by declaring the same key with `{ workspace = true }` in the corresponding member table. A workspace-inherited entry must not also declare a version, source, path, package alias, optionality, feature list, or default-feature policy in the member manifest. The root specification is the complete inherited contract.

Missing workspace keys, dependency-kind mismatches, and inheritance cycles are manifest errors. Inspection and diagnostics must show whether each effective dependency is member-owned or inherited from the workspace.

### Shared lock state

A declared workspace has one canonical `incan.lock` at the workspace root. Lock generation must resolve the union of all declared member dependency graphs plus shared declarations, regardless of the execution scope of the command that triggered locking. Member selection must never change the root lock fingerprint.

Project-aware commands run from any member must consult the root lock. A member-local `incan.lock` inside a declared workspace is not authoritative and must not be read as a fallback. Workspace inspection must report stale member-local lockfiles so migration is visible, but tooling must not delete them without an explicit mutation operation.

Commands that require locked or frozen operation must fail if the root lock is absent, stale for any member, or inconsistent with shared declarations. A command targeting one member must still respect the whole-workspace lock contract.

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
- shared dependency declarations and member inheritance provenance;
- shared environments and member inheritance provenance;
- shared policy and source configuration;
- member capabilities and provenance summaries;
- root lock state, fingerprint, and lockfile location; and
- warnings such as unmatched globs and stale member-local lockfiles.

Inspection must remain available when the active toolchain does not satisfy a member's `requires-incan` constraint, although the result must report that incompatibility.

### Publication and versions

Workspace members keep independent `[project].version` values and are published independently. A future convenience command may orchestrate publication of several selected members, but each publication remains a separately validated operation with its own artifact identity and result. This RFC does not define an atomic workspace release.

### Compatibility and migration

Manifests without `[workspace]` retain RFC 015 single-project behavior. Existing projects may become root members by adding `[workspace]` and listing `"."`; they do not need to move their source tree or change package identity.

When separate projects adopt one workspace, they must generate a canonical root lockfile. Existing member-local lockfiles cease to be authoritative and should be removed through an explicit reviewed change after workspace inspection confirms the new root lock.

## Design details

### Relationship to RFC 015

RFC 015 owns single-project lifecycle commands, manifest metadata, envs, scripts, and nearest-project discovery. This RFC preserves that project identity and adds an ancestor workspace graph only when the project is an explicit member.

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

### Why the root project is explicit

Allowing `[project]` and `[workspace]` in one manifest lets an established root package adopt a monorepo without a disruptive source move. Requiring `"."` in `members` preserves the stronger rule that topology is declared rather than inferred from table coexistence.

## Alternatives considered

### Model workspaces as envs

Rejected because envs describe execution context, while workspaces describe project topology. Collapsing them would make it hard to express shared locks, member selection, cross-member dependencies, and publish topology.

### Require one package per repository

Rejected because real projects often need multiple related packages, examples, tools, and applications in one repository.

### Infer members from path dependencies

Rejected because dependency edges and workspace ownership are different relationships. Automatic membership would make an unrelated nested project join workspace commands, policy, and lock resolution merely because one package references it.

### Keep one lockfile per member

Rejected because independently resolved member locks cannot represent shared dependency constraints or guarantee that a workspace-wide command sees one coherent dependency graph. Per-member locks would also make results depend on command entrypoint and selection.

### Require moving the root package under `packages/`

Rejected because directory layout should not determine whether a package can participate in a workspace. Explicit `"."` membership provides the same boundary without forcing repository-wide path churn.

### Make every project a workspace

Rejected because it would add unnecessary conceptual overhead to small projects. Single-project behavior should remain simple.

## Drawbacks

- Workspaces add another layer of command scope that users must understand.
- Shared dependency and env inheritance can become confusing without strong diagnostics.
- Cross-member mutation plans increase the importance of machine-readable output and policy review.
- Whole-workspace locking means a stale dependency declaration in an unselected member can block a locked command in another member.
- First-class workspace support requires lifecycle commands, the test runner, build reports, and editor tooling to carry member identity consistently.

## Implementation architecture

The implementation should build one validated workspace graph before command planning. The graph contains the root, ordered members, shared declarations, root lock state, and selected scope. Existing project commands can then operate on one or more member project contexts while preserving RFC 015 behavior for projects outside a workspace.

## Layers affected

- **Manifest schema / configuration validation:** manifests need workspace root, member, default-member, shared dependency, shared env, and shared policy fields.
- **CLI / tooling:** lifecycle commands need member selection, workspace discovery, workspace inspection, and workspace-scoped mutation plans.
- **Locking / dependency resolution:** shared lockfiles and shared dependency constraints must be understood by project resolution.
- **Build / test / format orchestration:** multi-member commands must fan out deterministically while preserving member-local source roots, diagnostics, reports, and generated artifacts.
- **LSP / IDE tooling:** editor tooling should surface workspace members, default members, selected command scope, and member-specific diagnostics.
- **Agentic tooling:** agents may use workspace topology to select relevant project skills, but must respect member scope and policy.
- **Documentation:** docs must explain the difference between envs, members, workspace roots, and project packages.

## Design Decisions

- Workspace membership comes only from explicit `members` paths and globs. Path dependencies never create membership.
- `[project]` and `[workspace]` may coexist. The root project joins the workspace only through explicit `"."` membership.
- Members and their canonical paths must remain under the workspace root. Nested and external workspaces are deferred.
- Shared Incan, Rust, and Rust dev dependencies require `{ workspace = true }` in each member that inherits them. Inheritance is never implicit.
- One root `incan.lock` covers all members and shared declarations. Command selection does not narrow dependency resolution or change the lock fingerprint.
- Workspace environments are inherited explicitly through `workspace:<name>` entries in member-local `extends` lists.
- `--workspace` selects every member, repeatable `--member` selects named members or paths, and current-directory/default-member rules define unqualified scope.
- Publication and versions remain per member. A workspace publication command may orchestrate those operations later but does not make them atomic or lockstep.
- Examples, documentation tools, and generated projects are members only when they have their own project manifest and are selected explicitly by `members`.
- Workspace-aware capabilities require graph validation, deterministic selection, machine-readable inspection, scoped mutation planning, and policy evaluation. Until that foundation exists, capabilities remain member-local.
