# RFC 075: starter profiles and capability packs

- **Status:** Draft
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 027 (incan-vocab and library metadata)
    - RFC 031 (Incan library system phase 1)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 034 (`incan.pub` package registry)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 076 (project mutation policy and recovery)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/403
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC extends the project lifecycle model from RFC 015 with starter profiles and capability packs. A starter profile is a declarative recipe for creating or initializing a project with a coherent set of files, manifest entries, and capability packs. A capability pack is a reusable, explicit project mutation that can add package dependencies, scripts, environment configuration, starter files, tooling metadata, and follow-up diagnostics to an existing project. Starter and capability descriptors may depend on other descriptors, but they must resolve into one reviewable mutation plan. The goal is not hidden framework magic; the goal is to keep the Rust-like package workflow from RFC 034 (`incan add <pkg>`) while adding an explicit project-capability workflow (`incan capability add <capability>`) for setup that changes source files, scripts, envs, and tooling metadata.

## Core model

Read this RFC as nine foundations plus five mechanisms:

1. **Foundation:** RFC 015 owns the minimal project lifecycle surface; this RFC layers richer project shapes on top of that surface rather than changing the meaning of `incan.toml`, project roots, or named envs.
2. **Foundation:** Starters and capability packs are tooling descriptors, not new language constructs. Applying one must result in explicit project files and manifest entries.
3. **Foundation:** Generated projects remain ordinary Incan projects. There is no hidden starter mode, no runtime dependency injection container, and no implicit activation that cannot be inspected after creation.
4. **Foundation:** Capability packs are composition units. A starter profile may include zero or more capability packs, and an existing project may add a capability pack later through `incan capability add`.
5. **Foundation:** Descriptor dependencies are explicit. Starters and capability packs may depend on other starters, capabilities, or templates, but the CLI must resolve that graph before applying any mutation.
6. **Foundation:** Starter application must be deterministic and safe by default: no arbitrary script execution, no unprompted overwrites, no silent dependency changes, and no secret material embedded into generated files.
7. **Foundation:** Incan packages may advertise capability packs. Adding a package and applying a capability are separate operations: `incan add <pkg>` changes dependencies; `incan capability add <capability>` applies project setup and may add dependencies as part of an explicit mutation plan.
8. **Foundation:** Applicability and back-off decisions are explicit. A descriptor may explain why it applies, why it is already satisfied, why it skipped a mutation, or why it is blocked, but it must not infer success from undocumented file conventions.
9. **Foundation:** The same descriptor model must be consumable by CLI and editor tooling. IDE support should not infer project intent from ad-hoc filenames when the starter or capability pack can declare that intent directly.
10. **Mechanism A:** `incan new <name> --starter <id>` creates a project by resolving one starter profile, applying its capability packs, rendering its files, and writing a normal `incan.toml`.
11. **Mechanism B:** `incan init --starter <id>` applies starter initialization to an existing directory with adoption-oriented conflict rules. Existing-project initialization must preserve user-authored files more aggressively than greenfield creation.
12. **Mechanism C:** `incan capability add <capability-id>` resolves one capability pack, which may be advertised by a package, then applies the resulting mutation plan to an existing project.
13. **Mechanism D:** `incan starter list`, `incan starter show`, and dry-run output expose what a starter or capability pack would change before files are written.
14. **Mechanism E:** lifecycle tooling exposes machine-readable starter, capability, and project-capability views so LSP servers and IDE plugins can offer completion, code actions, run actions, and project-shape diagnostics from the same contract as the CLI.

## Motivation

RFC 015 deliberately defines a small, explicit project lifecycle CLI: create a project, initialize metadata, define scripts and envs, bump versions, and run named env commands. That is the right base layer, but a minimal `hello world` scaffold is not enough once the ecosystem has reusable libraries for common domains such as CLIs, HTTP clients, web entrypoints, serialization, testing, data access, and documentation. Rust users can often add a crate and immediately use its API; Incan should preserve that simple dependency story through RFC 034's `incan add <pkg>` while also recognizing that application capabilities often need a small amount of project setup. Without a first-class starter model, users will copy examples by hand, drift from recommended dependency combinations, and re-learn the same manifest wiring in every repository.

Library metadata and manifests already point toward a more declarative ecosystem. RFC 027 says feature and dependency metadata should travel through library manifests rather than ad-hoc compiler scans, and RFC 031 establishes `.incnlib` as a semantic library artifact. Those pieces help libraries describe themselves, but they do not answer the project-author question: "start this repo with the standard shape for X" or "add the standard support for Y to this repo."

The missing layer is an explicit project mutation contract. Users need a command that can say what it will do, apply the change deterministically, and leave behind normal files that can be reviewed in source control. That is a different problem from typechecking, package resolution, environment matrix execution, or future product-specific app templates. This RFC defines the generic Incan-side layer those higher-level experiences can build on.

## Goals

- Define starter profiles as declarative recipes for `incan new` and `incan init`.
- Define capability packs as reusable project mutations that can be applied independently through `incan capability add`.
- Define descriptor dependency resolution so starters, capabilities, and templates can compose without hidden ordering.
- Allow package authors to advertise capability packs so `incan add <package>` can add the package dependency and then point users toward optional setup through `incan capability add <capability>`.
- Make starter and capability application deterministic, conflict-aware, dry-runnable, and reviewable.
- Define explainable applicability and back-off behavior for existing projects.
- Distinguish greenfield project creation from existing-project adoption so migration does not overwrite user-authored files under creation-oriented assumptions.
- Allow starters and capability packs to add manifest entries, dependencies, dev-dependencies, scripts, env definitions, file templates, tooling metadata, agent guidance metadata, and user-facing follow-up notes.
- Keep generated projects ordinary: all persistent behavior must be represented in source files, `incan.toml`, `incan.lock`, or documented generated artifacts.
- Provide inspection commands so users can list available starters, preview one starter, and see which capabilities are recorded in a project.
- Define a review-first capability update path so a project can move an applied capability from one recorded descriptor version to another without treating the change as an automatic package upgrade or silent template rewrite.
- Provide machine-readable inspection surfaces so LSP and IDE tooling can list starters, preview capability changes, show enabled capabilities, and surface project-specific actions without reimplementing descriptor resolution.
- Allow starter and capability descriptors to be resolved from source-agnostic catalogs such as built-in descriptors, local paths, git sources, public package registries, or private catalogs.
- Leave room for public and private catalog-backed starter discovery without requiring catalog support in the first implementation.
- Treat `incan.pub` as the eventual default public remote discovery and trust layer for starter and capability descriptors, while allowing non-public catalogs to use the same descriptor semantics and keeping local application semantics in the lifecycle CLI.
- Align with RFC 033 when generated projects need typed configuration: starter-provided config should prefer `ctx` source files over untyped sidecar YAML where compiler visibility matters.

## Non-Goals

- Defining product-specific application templates or app framework semantics.
- Defining a public registry protocol for distributing starters. RFC 034 remains the owner of package registry semantics.
- Defining package ranking, recommendations, telemetry, download analytics, or marketplace UI details for `incan.pub`.
- Defining private catalog hosting, identity, authorization, administration, or commercial policy.
- Defining the low-level `.incnlib` capability manifest schema in full. This RFC may consume library-declared capability metadata when available, but it primarily defines the project lifecycle UX for applying starter and capability descriptors.
- Making every package addition mutate project structure. Plain dependency addition remains valid; capability setup is for packages that advertise it and for users who request it through the capability command surface.
- Adding a general-purpose templating language with arbitrary code execution.
- Defining template rendering or generated-file provenance semantics. RFC 074 owns the template layer this RFC consumes.
- Adding runtime auto-configuration, dependency injection, reflection-based wiring, or hidden framework activation.
- Defining editor-specific plugin APIs for Visual Studio Code, JetBrains IDEs, Vim, or any other editor. This RFC defines the toolchain contract those integrations may consume.
- Defining an agent runtime, agent marketplace, prompt format, or autonomous execution policy. This RFC may describe agent-relevant metadata, but it does not specify how agents run.
- Replacing examples, tutorials, or documentation. Starters complement documentation by making the recommended shape executable.
- Defining capability removal or arbitrary project migration. This RFC defines descriptor-version updates for previously applied capabilities, but it does not promise that every user-authored project change can be merged automatically.

## Guide-level explanation

### Listing starters

A user can ask the toolchain which starter profiles are available:

```text
incan starter list
```

The output should show stable starter ids, short descriptions, source kind, and compatibility with the active Incan version. A built-in `bin` starter may be the default shape already described by RFC 015.

```text
id          source    description
bin         builtin   Minimal executable project
lib         builtin   Minimal reusable library project
cli         builtin   Command-line application with std process and test helpers
http-client builtin   Project preconfigured for typed HTTP calls and JSON handling
```

The exact starter set is not normative. The normative requirement is that available starters are discoverable and inspectable before use.

### Creating a project from a starter

A starter may be selected during project creation:

```text
incan new weather_tool --starter cli
```

The generated project is still an ordinary Incan project. The starter writes `incan.toml`, source files, docs, test files, and any other explicit artifacts described by the starter descriptor. There is no persistent hidden link to the starter that changes compilation semantics later.

If the starter includes capability packs, the CLI applies them as part of creation and records them in the project manifest for inspection:

```toml
[tool.incan.capabilities]
enabled = ["std.process", "testing.basic"]
```

The record is tooling provenance, not a runtime feature switch. The actual behavior must still be represented by dependencies, source files, env definitions, or generated artifacts that the user can review.

### Initializing an existing directory

`incan init` may also accept a starter:

```text
incan init --starter cli
```

This uses the same descriptor as `incan new`, but conflict handling is stricter because files may already exist. By default, generated files must not overwrite existing files. If a starter cannot be applied without overwriting or merging conflicting data, the CLI must stop with a diagnostic and suggest `--dry-run` or explicit overwrite flags.

### Adding a capability pack

An existing project may add one reusable capability without switching to a different starter:

```text
incan capability add std.http-client
```

The CLI resolves the capability pack, previews or applies its manifest changes, writes any declared files that do not conflict, and records the capability as enabled. If the capability depends on other capability packs, the CLI must show those transitive additions before applying them.

Capability packs should be small enough to explain. A pack named `std.http-client` might add a standard HTTP dependency, JSON support, a test fixture file, and a starter `ctx` for endpoint configuration. A pack named `testing.basic` might add test scripts and a sample test file. A pack should not try to become an entire application architecture.

### Dry-run and review

Both starters and capability packs should support dry-run output:

```text
incan new weather_tool --starter cli --dry-run
incan capability add std.http-client --dry-run
```

Dry-run output must show planned file writes, manifest additions, dependency changes, env/script additions, skipped mutations, already-satisfied expectations, and any conflicts. The dry run should be detailed enough that a user can decide whether the command is safe before mutating the project.

### Updating an applied capability

Capability updates are review-first project migrations, not package upgrades and not automatic template rewrites.

When a project records that it applied `cli` from descriptor version `1.3.0`, a later user should be able to ask what it would mean to move that project concern to `1.6.0`:

```text
incan capability status cli
incan capability diff cli --to 1.6.0
incan capability update cli --to 1.6.0 --dry-run
incan capability update cli --to 1.6.0
```

The lifecycle CLI resolves the currently recorded descriptor source and target descriptor version, verifies that the source identity has not silently changed, expands descriptor dependencies, renders affected RFC 074 templates with preserved or supplied values, and produces one receiver-side mutation plan.

For example, a dry run for `cli 1.3.0 -> 1.6.0` might report:

```text
Capability update: cli 1.3.0 -> 1.6.0
Source: incan.pub:app-cli
Publisher: encero
Source identity changed: no
Descriptor integrity: verified

Would update dependencies:
  app-cli 1.3.0 -> 1.6.0

Would update incan.toml:
  add [tool.incan.actions.run-cli]
  rename script "run" -> "cli.run"

Files:
  src/main.incn
    ownership: bootstrap
    status: user-owned
    action: no automatic update

  tests/test_cli.incn
    ownership: managed
    status: unchanged
    action: update

  .github/workflows/ci.yml
    ownership: managed
    status: new high-risk file
    action: requires policy approval
```

The user-facing update unit is the capability because that is how the project concern was applied. RFC 074 still owns the lower-level template behavior for individual files: managed files may be updated when unchanged, bootstrap files are not rewritten by normal capability updates, advisory files are explained but not owned, and edited managed files must stop with a conflict or use a documented merge strategy. RFC 076 evaluates the resulting mutation plan before anything is written.

`incan capability diff` should show the receiver-side patch that would land in the project. Catalog or registry source diffs between descriptor versions are useful supporting evidence, but they are not a substitute for the rendered project diff.

If the descriptor declares ordered migration steps between intermediate versions, the CLI may collapse them into one target update as long as the resulting plan remains explainable. If a required step cannot be represented declaratively, the update must stop and present manual instructions rather than running arbitrary provider code.

### IDE and editor support

The same starter and capability metadata should power editor workflows. An editor or LSP client should be able to ask the toolchain which starters and capabilities are available, which ones are compatible with the current project, and what changes a capability would make before applying it.

Examples of editor-facing behavior this RFC enables:

- completion for starter and capability ids in command palettes or manifest fields
- code actions such as "Add testing.basic capability" when a file imports testing helpers but the project lacks the expected setup
- run/debug actions for scripts contributed by a starter or capability pack
- diagnostics for stale capability provenance, invalid generated file roles, or missing project files that an enabled capability expects
- hover or project-tree annotations that show which capability introduced a source file, env script, or config declaration
- agent-assist affordances that suggest relevant project skills or maintenance workflows for a capability, without running them implicitly

These workflows must be backed by the same mutation plan and descriptor resolution rules as the CLI. An IDE integration should not have to scrape generated files or duplicate starter semantics to understand the project.

### Concrete walkthrough: adding a CLI capability

Suppose a user has an existing project:

```text
weather_core/
  incan.toml
  src/lib.incn
```

They want to add a standard command-line entrypoint without hand-copying example files. The provider has published an `app-cli` package that advertises a `cli` capability.

The user may start with the Rust-like package workflow:

```text
incan add app-cli
```

That only changes dependencies:

```text
Added app-cli = "0.3.1" to [dependencies]

This package advertises project capabilities:
  cli  Adds a CLI entrypoint, run script, and test skeleton

Apply setup with:
  incan capability add cli
```

To preview the project setup, the user asks for a capability dry run:

```text
incan capability add cli --dry-run
```

The CLI resolves `cli` as a capability, sees that its recommended provider is `app-cli`, validates the descriptor against the active project, and prints a mutation plan:

```text
Capability: cli
Package: app-cli 0.3.1
Source: incan.pub:app-cli@0.3.1
Requires Incan: >=0.3,<0.4

Would update dependencies:
  [dependencies]
  app-cli = "0.3.1"

Would create:
  src/main.incn
  tests/test_cli.incn

Would update incan.toml:
  [project.scripts]
  main = "src/main.incn"

  [tool.incan.envs.default.scripts]
  run = ["incan", "run"]
  test = ["incan", "test"]

  [tool.incan.capabilities]
  enabled += ["cli"]

Tooling:
  action run-cli -> script "run"
  file role src/main.incn -> main

Agent guidance:
  cli.write-commands
```

If the plan looks right, they apply it:

```text
incan capability add cli
```

The result is an ordinary project:

```text
weather_core/
  incan.toml
  src/lib.incn
  src/main.incn
  tests/test_cli.incn
```

The updated manifest records the visible project shape and capability provenance:

```toml
[project]
name = "weather_core"
version = "0.1.0"

[dependencies]
app-cli = "0.3.1"

[project.scripts]
main = "src/main.incn"

[tool.incan.envs.default.scripts]
run = ["incan", "run"]
test = ["incan", "test"]

[tool.incan.capabilities]
enabled = ["cli"]
```

After application, the project has both the package dependency and the project setup needed to use it as a CLI application capability. The project does not need the original descriptor to build. The descriptor was the recipe; the resulting source files and manifest are the durable project state. Tooling may still use the recorded capability provenance to explain the project, offer run actions, or suggest relevant agent guidance.

### Starter and capability descriptors

Descriptor syntax is a tooling contract, not Incan source syntax. A descriptor may be represented as TOML, JSON, or another stable encoding, but the semantic fields are:

```toml
[starter]
id = "cli"
title = "Command-line application"
description = "Executable project with standard CLI structure and basic tests."
requires-incan = ">=0.3,<0.4"
capabilities = ["std.process", "testing.basic"]

[[files]]
source = "templates/main.incn"
target = "src/main.incn"
mode = "create"

[[files]]
source = "templates/test_main.incn"
target = "tests/test_main.incn"
mode = "create"

[manifest.project.scripts]
main = "src/main.incn"

[manifest.tool.incan.envs.default.scripts]
test = ["incan", "test"]
```

A capability pack descriptor uses the same project mutation model, but its root identity is a capability rather than a starter:

```toml
[capability]
id = "testing.basic"
title = "Basic testing support"
description = "Adds a standard test script and sample test layout."
requires-incan = ">=0.3,<0.4"

[[files]]
source = "templates/test_main.incn"
target = "tests/test_main.incn"
mode = "create"

[manifest.tool.incan.envs.default.scripts]
test = ["incan", "test"]

[[tooling.actions]]
id = "run-tests"
title = "Run tests"
kind = "run"
script = "test"

[[tooling.file_roles]]
path = "tests/test_main.incn"
role = "test"
origin = "testing.basic"

[[agent_guidance]]
id = "testing.basic.write-tests"
title = "Write tests for this capability"
kind = "skill"
applies_to = ["testing.basic"]
description = "Use when adding or updating tests in projects that enabled this capability."
```

The concrete descriptor file format may evolve, but implementations must preserve the semantic distinction: a starter creates or initializes a project shape; a capability pack adds one reusable project concern.

For the walkthrough above, the provider-side descriptor could be packaged as ordinary catalog data alongside templates:

```text
app-cli/
  incan.toml
  capabilities/
    cli.toml
  templates/
    main.incn.tpl
    test_cli.incn.tpl
```

`capabilities/cli.toml` might contain:

```toml
[capability]
id = "cli"
title = "Command-line application entrypoint"
description = "Adds a standard CLI entrypoint, run script, and CLI test skeleton."
requires-incan = ">=0.3,<0.4"

[package]
name = "app-cli"
version = "0.3.1"
advertises = ["cli"]

[[files]]
source = "templates/main.incn.tpl"
target = "src/main.incn"
mode = "create"

[[files]]
source = "templates/test_cli.incn.tpl"
target = "tests/test_cli.incn"
mode = "create"

[manifest.project.scripts]
main = "src/main.incn"

[manifest.tool.incan.envs.default.scripts]
run = ["incan", "run"]
test = ["incan", "test"]

[[tooling.actions]]
id = "run-cli"
title = "Run CLI"
kind = "run"
script = "run"

[[tooling.file_roles]]
path = "src/main.incn"
role = "main"
origin = "cli"

[[agent_guidance]]
id = "cli.write-commands"
title = "Add or update CLI commands"
kind = "skill"
applies_to = ["cli"]
description = "Use when extending the command-line entrypoint or tests."
```

The provider ships static descriptor data and templates. The provider does not ship an arbitrary setup script that the user's project executes during `incan capability add`.

## Reference-level explanation

### Terminology

A **starter profile** is a named descriptor that can create a new project or initialize an existing directory by applying a deterministic set of project mutations.

A **capability pack** is a named descriptor that applies one reusable project concern to an existing project. A starter profile may include capability packs.

A **project mutation** is a planned change to project files, `incan.toml`, dependency declarations, env definitions, scripts, generated docs, or other explicit artifacts owned by the project.

A **starter catalog** is a source of starter and capability descriptors. V1 implementations must support built-in descriptors. They may also support explicit local descriptor paths. Future source kinds may include git references, package-provided descriptors, public catalog sources, and private catalog sources.

A **catalog-backed source** is a starter catalog backed by a registry or service that can provide discovery, integrity metadata, yanking state, publisher identity, compatibility information, and descriptor payloads or references. Catalog-backed sources do not apply mutations directly to a project.

A **private catalog** is a catalog-backed source for organization-specific or team-specific starters and capabilities that should not be discoverable through the public package ecosystem. Private catalog descriptors may describe whole project blueprints, internal platform conventions, or cross-package capabilities. They do not need to be scoped to one public package.

### Identifier rules

Starter ids and capability ids must be stable ASCII identifiers. They should use lowercase words separated by `-` or `.`. Implementations must reject ids containing path separators, shell metacharacters, or whitespace.

The same id must not resolve to two descriptors in the same catalog source. If two catalog sources provide the same id, the CLI must either reject the ambiguity or apply a documented source precedence rule and show the selected source in diagnostics.

If a capability id is advertised by multiple packages and no default provider is configured, `incan capability add <capability-id>` must not guess silently. It must show the candidate providers and require the user to disambiguate through a package-qualified capability id or an explicit provider flag.

### Command surface

This RFC adds the following lifecycle commands and flags:

- `incan starter list`
- `incan starter show <starter-id>`
- `incan capability list`
- `incan capability show <capability-id>`
- `incan capability status`
- `incan capability diff <capability-id> --to <version-or-ref>`
- `incan capability add <capability-id>`
- `incan capability update <capability-id> --to <version-or-ref>`
- `incan new <name> --starter <starter-id>`
- `incan init --starter <starter-id>`

`incan starter show` and `incan capability show` must report the descriptor source, required Incan constraint, descriptor dependencies, included capabilities, planned manifest effects, declared files, and known conflicts if evaluated in a project context.

`incan capability add` requires a project root. If no `incan.toml` is found, it must fail with a diagnostic suggesting `incan init` or `incan new`.

`incan add <pkg>` remains the RFC 034 package dependency command. If the added package advertises capabilities, the CLI should report those capabilities and show the corresponding `incan capability add <capability-id>` command, but it must not apply project setup as part of plain dependency addition.

`incan capability status` requires a project root. It must report enabled capability provenance and should report whether the project still appears consistent with each enabled capability's declared expectations. When catalog or registry metadata is available, status should also report the latest compatible descriptor version and whether the recorded source is yanked, revoked, superseded, or unknown.

`incan capability diff` requires a project root and an enabled capability. It must resolve the current recorded descriptor and requested target descriptor, then show the receiver-side mutation plan without writing files. Source-level descriptor diffs may be shown as supporting information, but the rendered project diff is the review artifact.

`incan capability update` requires a project root and an enabled capability. It applies an explicit receiver-approved mutation plan for moving from the recorded descriptor version to the requested target version or ref. It must support `--dry-run`, must preserve recorded source identity unless the user explicitly requests a source switch, and must not bypass RFC 076 policy.

`incan new` without `--starter` must keep the RFC 015 default behavior. Implementations may model that default internally as a built-in starter, but users must not be required to know that.

Commands that list, show, or add starters and capabilities may accept a catalog source selector. The exact flag spelling is not normative, but the model must distinguish built-in descriptors, local descriptor paths, git references, package-provided descriptors, public catalog descriptors, and private catalog descriptors in diagnostics and machine-readable output.

### Machine-readable inspection

The CLI must provide a machine-readable format for these read-only operations:

- listing starters
- showing one starter
- listing capabilities
- showing one capability
- reporting project capability status
- producing a dry-run mutation plan

The exact flag spelling is not normative, but `--format json` is the recommended shape because it is familiar and easy for LSP servers and editor plugins to consume.

Machine-readable output must include stable ids, titles, descriptions, descriptor sources, compatibility state, descriptor dependencies, included capabilities, file mutations, generated-file ownership policies, manifest mutations, declared tooling actions, file roles, agent guidance metadata, diagnostics, and unresolved conflicts where applicable.

Human-readable output may be optimized for terminal use, but it must not be the only supported inspection surface.

### Descriptor compatibility

Each starter or capability descriptor may declare `requires-incan`. If present, the active toolchain must satisfy the descriptor constraint before the descriptor is applied.

If a descriptor is applied to an existing project, its `requires-incan` must also be compatible with the project's effective toolchain constraint. The descriptor must not silently loosen `[project].requires-incan`. If the descriptor requires a narrower range, the CLI may propose a manifest update, but it must show the resulting effective constraint before writing.

If applying a descriptor would create an invalid RFC 073 environment matrix or env-level toolchain constraint, the CLI must reject the plan before writing files.

### Descriptor dependencies

Starter and capability descriptors may declare dependencies on other descriptors:

- required capability packs
- optional capability packs
- template sources governed by RFC 074
- package dependencies or dev-dependencies
- required project capabilities that must already be enabled

The CLI must resolve descriptor dependencies into a directed acyclic graph before planning file or manifest mutations. If the graph contains a cycle, incompatible version requirements, ambiguous providers, or a missing dependency, planning must fail before any project files are written.

Dependency resolution must be visible in dry-run output. If applying `inql.project` also applies `inql.session`, `testing.basic`, and three templates, the dry run must show those transitive effects explicitly.

Descriptors should stay small enough that dependency graphs remain explainable. A large starter may compose smaller capability packs, but the user-facing plan must still make clear which concern contributes which files, manifest entries, scripts, and generated-file ownership policies.

### Applicability and back-off

Starter and capability descriptors may declare applicability checks for project state they require or expect. Applicability checks are descriptive planning inputs, not hidden runtime behavior.

Applicability checks may inspect explicit project metadata such as package dependencies, manifest keys, env definitions, scripts, known file roles, capability provenance, template provenance, and declared expected files. They must not depend on executing project code or running arbitrary provider scripts.

The planner must classify each descriptor and mutation with explainable states such as `applicable`, `already-satisfied`, `skipped`, `blocked`, `conflicting`, or `unsafe` when those states apply. For example, a CLI capability may skip creating `src/main.incn` when the project already has a declared `main` script and file role, but it must show that decision in dry-run and machine-readable output.

Back-off behavior must be conservative. If the planner cannot prove that an existing file, manifest entry, or dependency already satisfies a capability expectation, it must report a conflict or warning rather than silently assuming success.

### Project mutation planning

Before applying a starter or capability pack, the CLI must build a mutation plan. The plan must include:

- files to create
- files to merge
- files that would conflict
- generated-file ownership policies from RFC 074
- manifest tables and keys to add or update
- dependency and dev-dependency changes
- applicability states and back-off decisions
- enabled capability provenance records
- env and script changes
- tooling and agent guidance metadata
- follow-up notes or warnings

The CLI must either apply the whole valid plan or apply nothing. Partial starter or capability application is not allowed unless the user explicitly asks for a machine-readable plan export and applies it manually outside this RFC.

The planner must know whether it is creating a new project or adopting an existing directory. Greenfield creation may include bootstrap files intended to teach or seed a project. Existing-project adoption should preserve user-authored files by default and should skip or mark bootstrap-only files as conflicts rather than silently replacing them.

### File mutation rules

File entries must declare one of these modes:

- `create`: write the target only when it does not already exist
- `merge`: merge into a supported structured file using deterministic rules
- `replace`: overwrite an existing file only when an explicit overwrite flag is supplied

The default mode is `create`.

A descriptor must not overwrite an existing file by default. If a file conflict occurs, the CLI must stop before writing and report the conflicting path. Overwrite flags may exist, but they must be explicit at the command line and must not be implied by `--yes`.

File entries may reference templates governed by RFC 074. Starter and capability application must consume rendered files from the template layer; it must not define a second templating language or execute arbitrary generator code.

If a file entry references an RFC 074 template, the mutation plan must carry the template's generated-file ownership policy. Bootstrap-owned files are created during greenfield application but should not be updated by normal capability refreshes. Managed files may participate in explicit template update workflows. Advisory files may record origin metadata without granting future ownership to the toolchain.

### Manifest mutation rules

Manifest mutations must be deterministic and must preserve unrelated user-authored manifest content.

For scalar keys, applying a descriptor must fail if the target key already exists with a different value unless the descriptor declares a supported merge rule or the user supplies an explicit override flag.

For list-like keys, applying a descriptor should append missing values without duplicating existing values. Ordering must be deterministic.

For dependency tables, applying a descriptor must reject incompatible duplicate dependency requirements unless a documented resolver can prove the resulting requirement is compatible. The CLI must not silently replace a user-authored dependency constraint with a starter-provided one.

For env definitions under `[tool.incan.envs.*]`, applying a descriptor must use RFC 015 and RFC 073 validation rules. Inherited envs, scripts, matrices, and toolchain constraints must remain valid after the mutation.

### Capability provenance

When `incan capability add` applies a capability pack, the project manifest should record that the capability is enabled:

```toml
[tool.incan.capabilities]
enabled = ["testing.basic", "std.http-client"]
```

The enabled list is provenance and tooling state. It does not replace explicit dependencies, source files, envs, or config declarations. Removing an item from the list must not be interpreted as uninstalling generated files or dependencies.

Capability provenance must preserve the descriptor source kind, source identity, selected descriptor version or content hash when available, provider package when applicable, applied capability id, expanded transitive capability graph, and the Incan version used to apply the descriptor. When available, it should also preserve publisher identity, integrity/signature state, yanking or revocation state, catalog trust tier, and the top-level user request that caused transitive capabilities to be applied.

The compact `enabled = [...]` form is sufficient for human-readable manifest summaries, but tools that support refresh, status, or security review need access to the richer provenance record. That richer record may live in `incan.toml`, a sidecar state file, or a future lock/state artifact, but it must be explicit project tooling state rather than inferred from generated files.

If a capability pack is already recorded as enabled, `incan capability add <capability-id>` should be idempotent. It may revalidate that the expected project shape is still present, but it must not duplicate manifest entries or files.

Capability provenance should preserve enough information to explain transitive additions. A project may record that a starter enabled `inql.project`, and that `inql.project` pulled in `inql.session` and `testing.basic`. Tooling should be able to show the user both the top-level request and the expanded capability graph.

Capability provenance is also the anchor for future updates. A project that records `cli@1.3.0` can later ask for a reviewable update to `cli@1.6.0` only if tooling can identify the recorded descriptor source, selected version or content hash, parameter values or safe value fingerprints, transitive descriptor graph, generated-file ownership, and applied template provenance. If that information is missing, `incan capability update` must degrade to an explicit adoption or manual-migration flow rather than guessing.

Descriptor-version updates may include declarative migration metadata. V1 migration metadata should stay non-executable: manifest key additions, manifest key renames, parameter renames, template id changes, file ownership changes, dependency version changes, action descriptor changes, and manual instruction blocks are acceptable shapes. Arbitrary shell, Python, plugin, or provider-defined migration hooks are out of scope for this RFC.

### Tooling metadata

Starters and capability packs may declare tooling metadata that describes editor-visible project shape without changing language semantics.

Tooling metadata may include:

- file roles such as `main`, `library`, `test`, `config`, `generated`, `docs`, or `example`
- action descriptors or action references mapped to RFC 015 scripts, env scripts, or project entrypoints
- config declarations that point at generated `ctx` types
- documentation links for a capability or generated file role
- expected files that tooling can validate after application

Tooling metadata must be descriptive. It must not grant special compiler privileges or activate runtime behavior that is not otherwise present in source files or the manifest.

If tooling metadata references files, paths must be relative to the project root and must not escape it. If it references scripts or envs, those scripts and envs must exist after the mutation plan is applied.

The LSP or another editor-facing tool may use this metadata for completions, code actions, diagnostics, project-tree grouping, and run/debug affordances. It must still treat the compiler and lifecycle CLI as the source of truth for validation.

RFC 078 owns typed action semantics, execution modes, risk labels, dry-run expectations for action execution, and `incan action run`. This RFC only lets starters and capabilities contribute or reference action descriptors as part of a project mutation plan. A descriptor that mentions an action must remain descriptive until the user or tooling explicitly invokes the action through RFC 078 behavior.

### Agent guidance metadata

Starters and capability packs may declare agent guidance metadata that describes project-relevant skills, workflows, or maintenance affordances for agentic tooling.

Agent guidance metadata may include:

- skill ids or workflow ids relevant to the capability
- short descriptions of when the guidance applies
- file roles, capabilities, or manifest sections that trigger the guidance
- links to documentation or local instruction files bundled with the package
- safety labels such as `read-only`, `project-mutation`, `networked`, or `requires-human-review`

Agent guidance metadata is descriptive. It must not cause a starter or capability pack to install, enable, or execute an agent automatically. It must not embed secrets, credentials, or hidden prompts that change project behavior. If an agent tool consumes this metadata, it remains responsible for its own permission model and user confirmation rules.

This metadata is useful because capability packs encode project intent. If a project enables a testing capability, an agent can be told which test-writing workflow is relevant. If a project enables an HTTP client capability, an agent can be told which docs, config files, and validation checks matter. The starter/capability layer should expose that intent once instead of forcing every agent integration to rediscover it from filenames.

### Dry-run behavior

`--dry-run` must be supported for `incan new --starter`, `incan init --starter`, and `incan capability add`.

Dry-run output must include the full mutation plan and must not write project files, lockfiles, or generated artifacts. If a dry run detects conflicts, it must report them with the same diagnostics that a real apply would use. The plan must explain why each descriptor or mutation is applicable, already satisfied, skipped, blocked, conflicting, or unsafe when those states apply.

Implementations must provide a machine-readable dry-run format for the full mutation plan. Security-sensitive mutation categories, conflicts, descriptor source identity, provenance changes, and receiver-side rendered file changes must be represented in that format rather than only in human-readable prose.

### Lockfile interaction

Starter and capability application may update `incan.toml`, but it must not silently rewrite `incan.lock` unless the command documents and exposes that behavior. The default behavior should leave lockfile updates to the next `incan lock`, `incan build`, or `incan test` flow governed by RFC 020.

If a descriptor includes dependency changes and the project is in a locked or frozen mode, the CLI must report that lockfile refresh is required rather than pretending the project remains fully locked.

### Security and trust

Applying a starter or capability pack must not execute arbitrary code from the descriptor.

Descriptors must not embed secrets. If generated code needs credentials, it should create typed configuration placeholders, environment-variable references, or documented setup steps.

Descriptors loaded from local paths must be treated as untrusted input for parsing and file path handling. File targets must be normalized relative to the project root and must not escape it.

Registry-backed descriptors, if added later, must inherit registry integrity, checksum, and signature rules from the package distribution layer rather than creating a second supply-chain story.

Applying or refreshing a capability pack is a supply-chain-sensitive project mutation. It may add dependencies, scripts, env definitions, generated source files, CI configuration, tooling metadata, or agent guidance. The lifecycle CLI must therefore present the mutation as a reviewable project diff, not as a cosmetic package setup step.

The receiving project owns acceptance of capability mutations. A starter or capability descriptor is a recipe for a proposed project patch; it is not authority for the provider, registry, catalog, package, or automation system to write into the receiver's codebase. This is especially important for refreshing an already-applied capability, where the command is changing an existing project rather than creating an empty one.

Capability plans must show the rendered receiver-side result, not only descriptor diffs. A descriptor diff can explain why a change is proposed, but reviewers need to inspect the actual project patch: source files, generated tests, scripts, envs, manifests, CI/configuration, tooling metadata, and agent guidance.

Capability provenance must preserve the descriptor source identity and selected descriptor version or content hash when available. If a later refresh would change the descriptor source, publisher identity, catalog trust tier, package provider, checksum/signature state, or yanking state, the CLI must surface that before applying file or manifest changes.

Catalog resolution must avoid dependency-confusion behavior. A public catalog entry must not silently shadow a previously recorded private catalog descriptor with the same id, and a private catalog descriptor must not silently replace a public descriptor without explicit user intent. If multiple catalogs can provide the same starter or capability id, the CLI must either fail with an ambiguity diagnostic or show the selected source according to a documented precedence rule.

Security-sensitive mutation categories must be explicit in dry-run and machine-readable output. At minimum, plans must identify dependency additions or upgrades, script/task changes, env changes, CI/config changes, executable source changes, generated-file ownership changes, and agent guidance metadata changes.

Project and organization policy may restrict which catalog sources, publishers, trust tiers, or descriptor pin forms are allowed for starters and capabilities. Policy may also require approval for high-risk mutation categories before `incan capability add` or a future capability refresh writes changes. This mirrors code review ownership for CI workflows: source files, scripts, CI config, env definitions, and agent guidance may need different reviewers.

Automated capability update should follow a review-first model. It may detect stale descriptors, vulnerable sources, or newer compatible versions, but it should emit a reviewable mutation plan or pull-request-sized patch rather than applying changes unattended. The plan should show before/after descriptor source, version or content hash, provider identity, known advisory state, yanking state, and every security-sensitive mutation category.

Automation that proposes a capability update must not also satisfy the receiver's approval requirement. If policy requires review for a mutation category, the approver must be independent from the tool, service, or agent that produced the proposed patch. Capability update should therefore integrate with normal code review instead of bypassing it.

### Diagnostics

Diagnostics for failed starter or capability application must name:

- the selected starter or capability id
- the descriptor source
- the project root, when one exists
- the conflicting file, manifest key, dependency, env, or toolchain constraint
- the command or flag that would let the user preview or resolve the issue, when applicable

Diagnostics should prefer actionable conflict explanations over generic "template failed" messages.

## Design details

### Relationship to RFC 015

RFC 015 defines the baseline project lifecycle: project metadata, root discovery, `incan new`, `incan init`, versioning, and named envs. This RFC does not change those rules. It adds richer starter selection and capability addition on top of the same project model.

The RFC 015 default scaffold remains valid and should stay small. Starter profiles are the place for opinionated project shapes.

### Relationship to library metadata

Capability packs are project-level activation units. Library manifests may eventually advertise capabilities, dependencies, config needs, or generated helper files, but applying those capabilities to a project remains a lifecycle CLI operation through `incan capability add`. This avoids a hidden model where merely importing a package or adding a dependency rewrites the project or changes source layout.

Where library manifests and starter descriptors overlap, library manifest data should be the source of truth for package-owned dependency and feature requirements. Starter descriptors should compose those capabilities rather than copy stale requirements by hand.

Plain package addition remains owned by RFC 034. A package may advertise capabilities, and the package-add workflow may point users at those capabilities, but capability application is a separate command surface because it may create files and mutate project structure.

### Relationship to `ctx`

If a starter or capability needs application configuration visible to the compiler, it should generate Incan source using `ctx` rather than creating an untyped sidecar configuration format. Sidecar files remain appropriate for operational data that the compiler should not understand.

Generated `ctx` declarations must follow normal source rules. The starter mechanism does not grant special access to configuration values.

### Relationship to env matrices

Starter and capability descriptors may add env definitions. If they add matrix envs, those envs must satisfy RFC 073. This RFC does not redefine matrix expansion or toolchain constraint semantics.

### Relationship to catalogs and package registries

This RFC defines descriptor semantics and local lifecycle behavior. Catalog source kind is a distribution concern, not a different capability system. A starter or capability descriptor should mean the same thing whether it came from a built-in source, local file, git reference, public registry, or private catalog.

`incan.pub` is positioned to become the default public remote catalog because it already owns package identity, versioning, checksums, signatures, yanking, dependency metadata, and package discovery. Starter and capability descriptors published through `incan.pub` should reuse that trust and compatibility model instead of creating a parallel public marketplace.

Private catalogs serve a different need: project and organization blueprints that may be broader than one library package. For example, a private descriptor can describe an end-to-end application shape that includes a CLI, API layer, analytics layer, data quality checks, governance conventions, and agent-relevant guidance. That descriptor can still be represented as ordinary starter and capability metadata. The local lifecycle CLI remains the executor.

The catalog or registry role should be discovery and verification:

- find starters and capabilities by id, package, domain, or required Incan version
- expose descriptor provenance, publisher identity, checksum/signature state, and yanking state
- expose package and capability compatibility metadata that the local CLI can use during planning
- expose descriptor dependency graphs and latest compatible versions for status checks
- expose source diffs or rendered-template diff metadata where that can be computed without local project state
- provide descriptor payloads or descriptor references for the local CLI to resolve

The catalog or registry must not be required to compute or apply project mutation plans. The local lifecycle CLI remains responsible for resolving the current project, checking local files, producing dry-run output, enforcing overwrite rules, and writing changes. This boundary keeps remote behavior cacheable and auditable while preserving local control over project mutation.

Published starters and capability packs should eventually be validated before promotion. At minimum, catalog tooling should be able to check that descriptors parse, dependency graphs resolve, referenced templates exist, and fixture values can render. Full target-project update tests can be a later quality gate.

### Relationship to RFC 076

RFC 076 owns project and organization policy for approving receiver-side mutations, restricting descriptor sources, classifying mutation risk, handling yanked or malicious sources, and recovering from unsafe applied mutations. This RFC defines the starter and capability mutation plans that RFC 076 policy evaluates.

### Relationship to RFC 078

RFC 078 owns the action model. Starter and capability descriptors may contribute action descriptors, but they do not define action execution semantics. During starter or capability application, actions are planned and recorded as metadata; they are not run implicitly. When a user later lists, dry-runs, or runs an action, RFC 078 behavior applies.

### Relationship to LSP and IDE integrations

The lifecycle CLI should expose descriptor and project-capability information in a stable machine-readable form so editor integrations can stay thin. Editor plugins may provide rich UI, and they may use cached `incan.pub` catalog data for discovery, but they should not own the semantics of starter resolution, capability compatibility, conflict detection, or manifest mutation.

This mirrors the broader Incan tooling direction: the language server and editor integrations should surface compiler and lifecycle knowledge rather than reverse-engineer it from conventions. Starter profiles and capability packs should therefore be designed as toolchain data first and editor UI inputs second.

### Relationship to agentic tooling

Agentic tooling should be treated like another consumer of project metadata, not as a privileged project mutation path. Starters and capability packs may advertise relevant skills and workflows, but those advertisements must flow through the same machine-readable inspection surfaces used by IDEs and CLIs.

This keeps the boundary simple: the capability descriptor says what kind of project concern exists and which guidance may be relevant; the agent runtime decides whether and how to use that guidance under its own safety model. The descriptor must not smuggle executable prompts or bypass local project permissions.

### Compatibility and migration

This RFC is additive. Existing projects that use RFC 015 commands without starters continue to behave the same way.

Existing examples can be converted into starter profiles gradually, but examples remain valuable documentation. A starter profile should not replace explanatory docs; it should make a documented recommended shape executable.

Projects may choose never to record capability provenance. That should not prevent ordinary builds, but it may limit future inspection commands that explain which project concerns were added by tooling.

## Alternatives considered

### Fold starter profiles into RFC 015

Rejected because RFC 015 is already a coherent lifecycle foundation. Adding starter catalogs, capability composition, conflict planning, and provenance would broaden it from "project lifecycle CLI" into "project application framework setup." A follow-up RFC keeps the base layer shippable and easier to implement.

### Keep only examples and documentation

Rejected because copy-paste examples drift. Users need a deterministic command that applies the recommended shape, reports conflicts, and records what changed.

### Use arbitrary generator scripts

Rejected because arbitrary scripts make starters powerful but unsafe, hard to inspect, and hard to reproduce. Static descriptors plus file templates are sufficient for v1 and keep the trust boundary narrow.

### Make package import activate capabilities automatically

Rejected because imports should not mutate projects or create hidden configuration. Capability activation should be explicit through `incan capability add` or a selected starter profile.

### Make plain `incan add <pkg>` apply default capability setup

Rejected because it overloads dependency addition with project mutation. RFC 034 defines `incan add <pkg>` as the package workflow, analogous to `cargo add`. It should be safe to add a dependency without creating source files or scripts. Packages may advertise capabilities and the CLI may suggest them after adding the dependency, but applying capability setup belongs to `incan capability add`.

### Store all starter behavior in `incan.toml`

Rejected because starters often need file templates, docs, and sample tests. `incan.toml` should record project metadata and applied capability provenance, not become a large embedded template archive.

### Add capability removal in v1

Rejected because removal is substantially harder than addition. Generated files may have been edited by users, dependencies may be shared by multiple capabilities, and manifest changes may no longer be attributable to one pack. V1 should focus on safe creation, addition, and inspection.

## Drawbacks

- Starter catalogs introduce another project-facing concept that must be documented and supported.
- Capability provenance can drift from the actual project shape if users manually edit generated files or dependencies.
- Descriptor conflict handling will need careful design to avoid both false positives and accidental overwrites.
- Built-in starters may create expectations that the core toolchain supports every domain equally well.
- Without registry-backed catalogs in v1, the first implementation may feel limited to built-in and local descriptors.

## Implementation architecture

The recommended implementation shape is to treat starter and capability application as a planning problem before it is a file-writing problem.

First, resolve the descriptor from a catalog. Then expand included capability packs and template dependencies into one ordered acyclic mutation plan. Then validate the plan against the target project, including manifest conflicts, file conflicts, generated-file ownership policy, toolchain constraints, env validity, and path safety. Finally, apply the plan atomically or report diagnostics without writing anything.

The same mutation plan should power dry-run output, human diagnostics, and machine-readable inspection. This keeps `incan new --starter`, `incan init --starter`, `incan capability add`, and `incan capability update` aligned rather than implementing separate generators and updaters.

## Layers affected

- **Manifest schema / configuration validation:** `incan.toml` should allow capability provenance under `[tool.incan.capabilities]`; starter-applied env, script, dependency, and project metadata changes must validate under existing RFC 015, RFC 020, and RFC 073 rules.
- **CLI / tooling:** `incan starter`, `incan capability`, `incan new --starter`, `incan init --starter`, `incan capability add`, `incan capability diff`, and `incan capability update` are new lifecycle tooling surfaces. They must support inspection, dry-run planning, conflict diagnostics, deterministic application, and review-first descriptor-version updates.
- **LSP / IDE tooling:** editor-facing tools should consume machine-readable starter, capability, mutation-plan, file-role, and action metadata from the lifecycle layer. They may expose completions, code actions, project diagnostics, and run/debug affordances, but they must not fork descriptor semantics.
- **Agentic tooling:** agent-facing tools may consume capability provenance, file roles, and agent guidance metadata to select relevant skills or workflows, but starter/capability descriptors remain descriptive and must not execute agents implicitly.
- **Project scaffolding:** the project generator must support descriptor-backed file creation, manifest mutation, safe path normalization, and non-overwrite defaults.
- **Library/package integration:** future library capability metadata should feed starter and capability descriptors where possible, but project mutation remains explicit. Package and registry metadata may also feed descriptor dependency resolution and status reporting.
- **Documentation:** user docs must explain the difference between minimal scaffolds, starter profiles, capability packs, package dependencies, and runtime configuration.

## Unresolved questions

- Should v1 support only built-in starter catalogs and explicit local descriptor paths, with `incan.pub` as a designed follow-up, or should registry-backed descriptor discovery be part of the first accepted design?
- Should `incan capability add` always record capability provenance under `[tool.incan.capabilities]`, or should provenance be opt-in for projects that want very small manifests?
- How much of descriptor dependency resolution belongs in v1, and should optional dependencies be allowed before required dependency graphs are fully implemented?
- Which files should starters mark as bootstrap-owned versus managed by default?
- Should `incan init --starter` have an explicit adoption mode, or should existing-project adoption be auto-detected?
- What provider validation should be required before a starter or capability can be published or promoted through `incan.pub`?
- Should starter descriptors support interactive prompts in v1, or should all parameterization come from command-line flags and project metadata?
- Which `tooling.file_roles` and `tooling.actions` values should be standardized in v1 versus left as free-form extension strings?
- Which `agent_guidance` fields should be standardized in v1, and which should remain extension metadata for agent runtimes to interpret?
- Which applicability states and reason-code vocabulary should be standardized in v1?
- Which declarative migration operations should be standardized for v1 capability updates, and which should remain manual instructions?
- Should the LSP call the lifecycle CLI as a subprocess for starter/capability inspection, or should the compiler expose the same descriptor-resolution API directly to tooling crates?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
