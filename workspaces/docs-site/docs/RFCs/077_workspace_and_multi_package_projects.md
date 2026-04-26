# RFC 077: workspace and multi-package projects

- **Status:** Draft
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
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a first-class workspace model for Incan projects that contain multiple related packages, applications, libraries, tools, examples, or generated artifacts in one repository. RFC 015 envs answer how commands run; workspaces answer what project members exist, how they share dependency resolution and policy, and how lifecycle commands operate across member boundaries.

## Core model

Read this RFC as seven foundations:

1. **Topology is explicit:** a workspace declares its root and members instead of relying on ad-hoc directory conventions.
2. **Members remain ordinary projects:** each member can have its own package metadata, source tree, dependencies, envs, scripts, capabilities, and publish settings.
3. **The root coordinates shared concerns:** a workspace may provide shared lock state, shared policy, shared dependency overrides, shared metadata, shared tooling, and shared catalog configuration.
4. **Commands select a scope:** lifecycle commands must know whether they apply to the current member, selected members, default members, or the whole workspace.
5. **Envs compose with topology:** RFC 015 envs still describe execution context, but workspace commands decide which members receive that context.
6. **Capabilities are scoped:** starters and capability packs may target one member, multiple members, or workspace-level metadata, and must show that scope in mutation plans.
7. **The workspace is publish-aware:** publication, documentation, artifact discovery, and policy may need workspace-level views without forcing every member to publish together.

## Motivation

RFC 015 gives Incan a project lifecycle and RFC 073 gives env matrices a way to describe execution variation. Those are necessary but not enough for repositories that naturally contain multiple packages: an application plus internal libraries, a CLI plus a core library, examples plus integration tests, or a product surface plus shared schemas. Treating those as unrelated projects loses shared locking, shared policy, shared scripts, and cross-member dependency intent.

Cargo and uv both make workspaces a central scaling primitive. Incan should have the same topology concept, but it should integrate with Incan-specific concerns: capability application, template provenance, env matrices, policy evaluation, and future artifact graph publication.

## Goals

- Define workspace root discovery and workspace member declaration.
- Allow members to be selected by name, path, glob, default set, or all-members mode.
- Define shared workspace lock and dependency resolution expectations.
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
- Defining registry hosting behavior for workspace artifacts.
- Defining exact CLI flag spelling for every member-selection operation.

## Guide-level explanation

### Declaring a workspace

A repository may declare a workspace at the root:

```toml
[workspace]
members = ["packages/core", "packages/cli", "examples/*"]
default-members = ["packages/core", "packages/cli"]

[workspace.dependencies]
json = "1.2.0"
http = "0.8.0"

[workspace.policy]
source-policy = "strict"
```

Each member remains an ordinary Incan project:

```text
repo/
  incan.toml
  incan.lock
  packages/
    core/
      incan.toml
      src/lib.incn
    cli/
      incan.toml
      src/main.incn
```

The root is not magic source code. It coordinates member discovery, shared dependency resolution, shared lock state, shared policy, and shared tooling.

### Running across members

A user can run commands against the current member, the default members, or the whole workspace:

```text
incan test
incan test --workspace
incan test --member cli
incan run --member cli --env ci
```

The exact flags are not normative. The requirement is that command scope must be explicit in diagnostics and machine-readable output.

### Applying capabilities in a workspace

A capability can target one member:

```text
incan capability add cli --member packages/cli --dry-run
```

Or a workspace-level capability can add coordinated metadata:

```text
incan capability add workspace.ci --workspace --dry-run
```

The dry-run plan must show which member receives each file, manifest entry, dependency, script, env, policy, or agent-guidance change.

## Reference-level explanation

### Workspace root and members

A workspace root is a directory containing a manifest with a `[workspace]` table. Implementations should search parent directories for a workspace root when executing member-aware commands.

Workspace members are project roots selected by explicit paths, globs, or other stable member selectors. A member must contain a project manifest unless a later RFC defines generated or virtual members.

Member names must be stable within one workspace. If two members declare the same package name, workspace inspection must report the ambiguity and member selection must require path or another disambiguator.

### Command scope

Workspace-aware commands must decide their scope before reading or mutating member manifests. Scope may be:

- current member
- workspace root
- default members
- selected members
- all members

Diagnostics and machine-readable output must include the selected scope. A command that mutates files or manifests must not silently apply to more members than the user requested.

### Shared lock state

A workspace may use a shared lockfile at the root. If a shared lockfile exists, dependency resolution should consider all selected members and shared workspace dependency declarations.

Commands that require locked or frozen operation must fail if the shared lockfile would need to change. If only one member is selected, the toolchain must still respect shared workspace constraints.

### Shared dependencies and overrides

The workspace root may declare shared dependency requirements, dev-dependencies, overrides, patches, or source configuration. Member manifests may opt into shared dependencies using a stable encoding defined by the implementation.

The toolchain must make it clear whether a dependency requirement came from a member manifest or from workspace-level shared configuration.

### Envs and matrices

RFC 015 envs and RFC 073 matrices remain execution-context features. Workspaces add member selection on top of those contexts.

When an env is declared at the workspace root, members may inherit it if inheritance is explicitly enabled. Member envs may override or extend workspace envs only through deterministic merge rules.

### Workspace mutation plans

Any workspace-scoped mutation plan must include member scope. For each planned change, the plan must state whether it affects the workspace root, one member, selected members, or all members.

Workspace mutation plans must be compatible with RFC 076 policy. Policy may require additional approval for cross-member changes, shared dependency changes, shared env changes, or workspace-level source policy changes.

### Machine-readable inspection

The CLI must expose a machine-readable workspace view containing:

- workspace root
- members and member paths
- default members
- selected scope for a command
- shared dependency declarations
- shared envs and inherited envs
- shared policy and source configuration
- member capabilities and provenance summaries
- lock state and lockfile location

## Design details

### Relationship to RFC 015

RFC 015 owns single-project lifecycle commands, manifest metadata, envs, scripts, and root discovery. This RFC extends root discovery to workspace topology and defines how lifecycle commands select member scope.

### Relationship to RFC 073

RFC 073 owns matrix expansion and toolchain constraints. Workspace member selection happens before env or matrix execution. Matrix expansion should be reported per selected member.

### Relationship to RFC 075

Starter and capability descriptors may be workspace-aware. A descriptor that mutates multiple members must report member scope in its dry-run and machine-readable plan.

### Relationship to RFC 076

Workspace-level policy may be stricter than member-level policy. If multiple policies apply, RFC 076's conservative precedence model should be used unless this RFC later defines a more specific workspace precedence rule.

### Relationship to RFC 079

The artifact graph may represent workspace relationships: root project, member packages, examples, docs, generated artifacts, AI assets, and publishable units. This RFC defines the local topology that a future registry can mirror.

## Alternatives considered

### Model workspaces as envs

Rejected because envs describe execution context, while workspaces describe project topology. Collapsing them would make it hard to express shared locks, member selection, cross-member dependencies, and publish topology.

### Require one package per repository

Rejected because real projects often need multiple related packages, examples, tools, and applications in one repository.

### Make every project a workspace

Rejected because it would add unnecessary conceptual overhead to small projects. Single-project behavior should remain simple.

## Drawbacks

- Workspaces add another layer of command scope that users must understand.
- Shared dependency and env inheritance can become confusing without strong diagnostics.
- Cross-member mutation plans increase the importance of machine-readable output and policy review.
- Workspace support may force lifecycle commands to reason about more project topology than v1 strictly needs.

## Implementation architecture

The recommended implementation shape is to build a workspace graph before command planning. The graph contains the root, members, shared configuration, lock state, and selected scope. Existing lifecycle commands can then operate on one or more member project contexts rather than special-casing workspace behavior throughout the toolchain.

## Layers affected

- **Manifest schema / configuration validation:** manifests need workspace root, member, default-member, shared dependency, shared env, and shared policy fields.
- **CLI / tooling:** lifecycle commands need member selection, workspace discovery, workspace inspection, and workspace-scoped mutation plans.
- **Locking / dependency resolution:** shared lockfiles and shared dependency constraints must be understood by project resolution.
- **LSP / IDE tooling:** editor tooling should surface workspace members, default members, selected command scope, and member-specific diagnostics.
- **Agentic tooling:** agents may use workspace topology to select relevant project skills, but must respect member scope and policy.
- **Documentation:** docs must explain the difference between envs, members, workspace roots, and project packages.

## Unresolved questions

- Should workspace members be discovered only from explicit `members`, or should path dependencies under the root become members automatically?
- Should shared dependencies be inherited explicitly by each member or applied implicitly across all members?
- Should a workspace always have one shared lockfile, or should per-member lockfiles be allowed?
- How should workspace env inheritance be encoded?
- Should package publication happen per member, per selected group, or through a workspace publish command?
- Should generated examples and docs be first-class workspace members or ordinary files?
- What is the minimum workspace support needed before RFC 075 capabilities can target multiple members?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
