# RFC 075: starter profiles and mixes

- **Status:** Planned
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 027 (incan-vocab and library metadata)
    - RFC 031 (Incan library system phase 1)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 118 (Oven command families and dependency addition)
    - RFC 125 (`loaves.pub` Loaf registry; supersedes RFC 034)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 076 (project mutation policy and recovery)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
- **Issue:** <https://github.com/encero-systems/incan/issues/403>
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC extends the Loaf/Oven project model with starter profiles and mixes. A starter profile is a declarative recipe for creating or initializing a project with a coherent set of files, manifest entries, and mixes. A mix is a reusable, explicit project mutation that can add package dependencies, scripts, environment configuration, starter files, tooling metadata, and follow-up diagnostics to an existing project. Starter and mix descriptors may depend on other descriptors, but they must resolve into one reviewable mutation plan. The goal is not hidden framework magic; the goal is to keep the package workflow from RFC 118 (`oven add <pkg>`), with registry authority under RFC 125, while adding an explicit project-mix workflow (`oven mix add <mix>`) for setup that changes source files, scripts, envs, and tooling metadata.

## Core model

Read this RFC as nine foundations plus five mechanisms:

1. **Foundation:** RFC 117 owns the `loaf.toml` project model and root authority; this RFC layers richer project shapes on top without redefining manifest discovery, workspace inheritance, dependency resolution, or named-environment semantics.
2. **Foundation:** Starters and mixes are tooling descriptors, not new language constructs. Applying one must result in explicit project files and manifest entries.
3. **Foundation:** Generated projects remain ordinary Incan projects. There is no hidden starter mode, no runtime dependency injection container, and no implicit activation that cannot be inspected after creation.
4. **Foundation:** Mixes are composition units. A starter profile may include zero or more mixes, and an existing project may add a mix later through `oven mix add`.
5. **Foundation:** Descriptor dependencies are explicit. Starters and mixes may depend on other starters, mixes, or templates, but the CLI must resolve that graph before applying any mutation.
6. **Foundation:** Starter application must be deterministic and safe by default: no arbitrary script execution, no unprompted overwrites, no silent dependency changes, and no secret material embedded into generated files.
7. **Foundation:** Incan packages may advertise mixes. Adding a package and applying a mix are separate operations: `oven add <pkg>` changes dependencies; `oven mix add <mix>` applies project setup and may add dependencies as part of an explicit mutation plan.
8. **Foundation:** Applicability and back-off decisions are explicit. A descriptor may explain why it applies, why it is already satisfied, why it skipped a mutation, or why it is blocked, but it must not infer success from undocumented file conventions.
9. **Foundation:** The same descriptor model must be consumable by CLI and editor tooling. IDE support should not infer project intent from ad-hoc filenames when the starter or mix can declare that intent directly.
10. **Mechanism A:** `oven new <name> --starter <id>` creates a project by resolving one starter profile, applying its mixes, rendering its files, and writing a normal `loaf.toml`.
11. **Mechanism B:** `oven init --starter <id>` applies starter initialization to an existing directory with adoption-oriented conflict rules. Existing-project initialization must preserve user-authored files more aggressively than greenfield creation.
12. **Mechanism C:** `oven mix add <mix-id>` resolves one mix, which may be advertised by a package, then applies the resulting mutation plan to an existing project.
13. **Mechanism D:** `oven starter list`, `oven starter show`, and dry-run output expose what a starter or mix would change before files are written.
14. **Mechanism E:** lifecycle tooling exposes machine-readable starter, mix, and project-mix views so LSP servers and IDE plugins can offer completion, code actions, run actions, and project-shape diagnostics from the same contract as the CLI.

## Motivation

RFC 015 deliberately defines a small, explicit project lifecycle CLI: create a project, initialize metadata, define scripts and envs, bump versions, and run named env commands. That is the right base layer, but a minimal `hello world` scaffold is not enough once the ecosystem has reusable libraries for common domains such as CLIs, HTTP clients, web entrypoints, serialization, testing, data access, and documentation. Rust users can often add a crate and immediately use its API; Incan should preserve that simple dependency story through RFC 118's `oven add <pkg>` while also recognizing that application mixes often need a small amount of project setup. Without a first-class starter model, users will copy examples by hand, drift from recommended dependency combinations, and re-learn the same manifest wiring in every repository.

Library metadata and manifests already point toward a more declarative ecosystem. RFC 027 says feature and dependency metadata should travel through library manifests rather than ad-hoc compiler scans, and RFC 031 establishes `.incnlib` as a semantic library artifact. Those pieces help libraries describe themselves, but they do not answer the project-author question: "start this repo with the standard shape for X" or "add the standard support for Y to this repo."

The missing layer is an explicit project mutation contract. Users need a command that can say what it will do, apply the change deterministically, and leave behind normal files that can be reviewed in source control. That is a different problem from typechecking, package resolution, environment matrix execution, or future product-specific app templates. This RFC defines the generic Oven-side layer those higher-level experiences can build on. RFC 118 may expose downward-delegating Incan conveniences, but it must not duplicate Oven mutation planning, resolver, policy, or receipt behavior.

## Goals

- Define starter profiles as declarative recipes for `oven new` and `oven init`.
- Define mixes as reusable project mutations that can be applied independently through `oven mix add`.
- Define descriptor dependency resolution so starters, mixes, and templates can compose without hidden ordering.
- Allow package authors to advertise mixes so `oven add <package>` can add the package dependency and then point users toward optional setup through `oven mix add <mix>`.
- Make starter and mix application deterministic, conflict-aware, dry-runnable, and reviewable.
- Define explainable applicability and back-off behavior for existing projects.
- Distinguish greenfield project creation from existing-project adoption so migration does not overwrite user-authored files under creation-oriented assumptions.
- Allow starters and mixes to add manifest entries, dependencies, dev-dependencies, scripts, env definitions, file templates, tooling metadata, agent guidance metadata, and user-facing follow-up notes.
- Keep generated projects ordinary: all persistent behavior must be represented in source files, `loaf.toml`, `oven.lock`, or documented generated artifacts.
- Provide inspection commands so users can list available starters, preview one starter, and see which mixes are recorded in a project.
- Define a review-first mix update path so a project can move an applied mix from one recorded descriptor version to another without treating the change as an automatic package upgrade or silent template rewrite.
- Provide machine-readable inspection surfaces so LSP and IDE tooling can list starters, preview mix changes, show enabled mixes, and surface project-specific actions without reimplementing descriptor resolution.
- Allow starter and mix descriptors to be resolved from source-agnostic catalogs such as built-in descriptors, local paths, git sources, public package registries, or private catalogs.
- Support public and private catalog-backed starter and mix discovery.
- Use `loaves.pub` as the default public discovery and trust layer for starter and mix descriptors, while allowing non-public catalogs to use the same descriptor semantics and keeping local application semantics in the lifecycle CLI.
- Align with RFC 033 when generated projects need typed configuration: starter-provided config should prefer `ctx` source files over untyped sidecar YAML where compiler visibility matters.

## Non-Goals

- Defining product-specific application templates or app framework semantics.
- Defining a public registry protocol for distributing starters. RFC 125 is the owner of package registry semantics, superseding RFC 034; RFC 117 owns the Loaf project and dependency model.
- Defining package ranking, recommendations, telemetry, download analytics, or marketplace UI details for `loaves.pub`.
- Defining private catalog hosting, identity, authorization, administration, or commercial policy.
- Defining the low-level `.incnlib` mix manifest schema in full. This RFC may consume library-declared mix metadata when available, but it primarily defines the project lifecycle UX for applying starter and mix descriptors.
- Making every package addition mutate project structure. Plain dependency addition remains valid; mix setup is for packages that advertise it and for users who request it through the mix command surface.
- Adding a general-purpose templating language with arbitrary code execution.
- Defining template rendering or generated-file provenance semantics. RFC 074 owns the template layer this RFC consumes.
- Adding runtime auto-configuration, dependency injection, reflection-based wiring, or hidden framework activation.
- Defining editor-specific plugin APIs for Visual Studio Code, JetBrains IDEs, Vim, or any other editor. This RFC defines the toolchain contract those integrations may consume.
- Defining an agent runtime, agent marketplace, prompt format, or autonomous execution policy. This RFC may describe agent-relevant metadata, but it does not specify how agents run.
- Replacing examples, tutorials, or documentation. Starters complement documentation by making the recommended shape executable.
- Defining mix removal or arbitrary project migration. This RFC defines descriptor-version updates for previously applied mixes, but it does not promise that every user-authored project change can be merged automatically.

## Guide-level explanation

### Listing starters

A user can ask the toolchain which starter profiles are available:

```text
oven starter list
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
oven new weather_tool --starter cli
```

The generated project is still an ordinary Incan project. The starter writes `loaf.toml`, source files, docs, test files, and any other explicit artifacts described by the starter descriptor. There is no persistent hidden link to the starter that changes compilation semantics later.

If the starter includes mixes, the CLI applies them as part of creation and records them in the project manifest for inspection:

```toml
[oven.mixes]
enabled = ["std.process", "testing.basic"]
```

The record is tooling provenance, not a runtime feature switch. The actual behavior must still be represented by dependencies, source files, env definitions, or generated artifacts that the user can review.

### Initializing an existing directory

`oven init` may also accept a starter:

```text
oven init --starter cli
```

This uses the same descriptor as `oven new`. In an empty directory it creates the project. In a directory that already has a `loaf.toml` or source files, it is adoption, and the user must say so:

```text
oven init --starter cli --adopt --dry-run
oven init --starter cli --adopt
```

Without `--adopt`, the CLI stops and points at the adoption preview rather than inferring permission to restructure an existing project. The adoption plan keeps existing files and manifest entries intact, lists every conflict, and applies only after each conflict is resolved explicitly, by skipping the change, keeping the existing content, or an explicit overwrite flag.

### Answering prompts or passing values

A starter or mix can declare typed parameters, using RFC 074's parameter declarations. A person at a terminal is prompted for each value that is missing:

```text
$ oven new weather_tool --starter http-client
Base URL for the API [https://api.example.com]: https://weather.example.com
Request timeout in seconds [10]:
```

An agent or CI job passes the same values as flags or a values file and gets exactly the same result:

```text
oven new weather_tool --starter http-client --set base_url=https://weather.example.com --set timeout=10
oven new weather_tool --starter http-client --values http-client.values.toml
```

Prompts are only a way of collecting the declared parameters. Every prompted value can also be supplied non-interactively, it is validated by the same parameter kind either way, and a non-interactive run with a missing required value fails with a diagnostic listing what is missing.

### Adding a mix

An existing project may add one reusable mix without switching to a different starter:

```text
oven mix add std.http-client
```

The CLI resolves the mix, previews or applies its manifest changes, writes any declared files that do not conflict, and records the mix as enabled. If the mix depends on other mixes, the CLI must show those transitive additions before applying them.

Mixes should be small enough to explain. A mix named `std.http-client` might add a standard HTTP dependency, JSON support, a test fixture file, and a starter `ctx` for endpoint configuration. A mix named `testing.basic` might add test scripts and a sample test file. A mix should not try to become an entire application architecture.

### Dry-run and review

Both starters and mixes should support dry-run output:

```text
oven new weather_tool --starter cli --dry-run
oven mix add std.http-client --dry-run
```

Dry-run output must show planned file writes, manifest additions, dependency changes, env/script additions, skipped mutations, already-satisfied expectations, and any conflicts. The dry run should be detailed enough that a user can decide whether the command is safe before mutating the project.

### Updating an applied mix

Mix updates are review-first project migrations, not package upgrades and not automatic template rewrites.

When a project records that it applied `cli` from descriptor version `1.3.0`, a later user should be able to ask what it would mean to move that project concern to `1.6.0`:

```text
oven mix status cli
oven mix diff cli --to 1.6.0
oven mix update cli --to 1.6.0 --dry-run
oven mix update cli --to 1.6.0
```

The lifecycle CLI resolves the currently recorded descriptor source and target descriptor version, verifies that the source identity has not silently changed, expands descriptor dependencies, renders affected RFC 074 templates with preserved or supplied values, and produces one receiver-side mutation plan.

For example, a dry run for `cli 1.3.0 -> 1.6.0` might report:

```text
Mix update: cli 1.3.0 -> 1.6.0
Source: loaves.pub:app-cli
Publisher: encero
Source identity changed: no
Descriptor integrity: verified

Would update dependencies:
  app-cli 1.3.0 -> 1.6.0

Would update loaf.toml:
  add [oven.actions.run-cli]
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

The user-facing update unit is the mix because that is how the project concern was applied. Each file's ownership policy, defined under File ownership below, decides what the update may do: managed files may be updated, bootstrap files are not rewritten by normal mix updates, and advisory files are explained but not owned. RFC 074 owns the lower-level template mechanics, including the three-way merge that keeps user edits in a managed file and stops at each real conflict. RFC 076 evaluates the resulting mutation plan before anything is written.

`oven mix diff` should show the receiver-side patch that would land in the project. Catalog or registry source diffs between descriptor versions are useful supporting evidence, but they are not a substitute for the rendered project diff.

The migration steps travel with the mix, not with the project. The provider writes them into the newer descriptor version, inside the Loaf it publishes:

```text
app-cli 1.6.0 (a Loaf on loaves.pub)
  loaf.toml
  mixes/
    cli.toml              # the [mix] descriptor, with its [[migrations]]
  templates/
    main.incn.template
```

```toml
# app-cli 1.6.0: mixes/cli.toml
[[migrations]]
from = "1.3"
to = "1.6"

[[migrations.steps]]
op = "rename_script"
from = "run"
to = "cli.run"

[[migrations.steps]]
op = "manual"
text = "Replace `App.start()` with `App.run()` in your entrypoint."
```

The project only records that it applied `cli` at `1.3.0`. `oven mix update cli --to 1.6.0` reads the 1.6.0 descriptor, selects the migrations that lead from the recorded version to the target, checks each step against the project, and shows one plan:

```text
rename script "run" -> "cli.run"
  precondition: script "run" is ["oven", "run"]   ok
manual: Replace `App.start()` with `App.run()` in your entrypoint.
  shown, not applied
```

If the user had changed the `run` script, that step would be a conflict to resolve, not a guess. The provider writes the steps once, and every project updating from 1.3 gets the same plan for the same project state. A change the step vocabulary cannot express stays a manual instruction rather than provider code.

### IDE and editor support

The same starter and mix metadata should power editor workflows. An editor or LSP client should be able to ask the toolchain which starters and mixes are available, which ones are compatible with the current project, and what changes a mix would make before applying it.

Examples of editor-facing behavior this RFC enables:

- completion for starter and mix ids in command palettes or manifest fields
- code actions such as "Add testing.basic mix" when a file imports testing helpers but the project lacks the expected setup
- run/debug actions for scripts contributed by a starter or mix
- diagnostics for stale mix provenance, invalid generated file roles, or missing project files that an enabled mix expects
- hover or project-tree annotations that show which mix introduced a source file, env script, or config declaration
- agent-assist affordances that suggest relevant project skills or maintenance workflows for a mix, without running them implicitly

These workflows must be backed by the same mutation plan and descriptor resolution rules as the CLI. An IDE integration should not have to scrape generated files or duplicate starter semantics to understand the project.

For example, a code action "Add testing.basic mix" previews exactly the plan that `oven mix add testing.basic --dry-run` prints, because one planning library produces both. If the user edits `tests/test_main.incn` after the preview and then applies it, the apply step finds that the file changed since the preview, refuses the stale plan, and asks for a new preview.

### Concrete walkthrough: adding a CLI mix

Suppose a user has an existing project:

```text
weather_core/
  loaf.toml
  src/lib.incn
```

They want to add a standard command-line entrypoint without hand-copying example files. The provider has published an `app-cli` package that advertises a `cli` mix.

The user may start with the Rust-like package workflow:

```text
oven add app-cli
```

That only changes dependencies:

```text
Added app-cli = "0.3.1" to [dependencies]

This package advertises project mixes:
  cli  Adds a CLI entrypoint, run script, and test skeleton

Apply setup with:
  oven mix add cli
```

To preview the project setup, the user asks for a mix dry run:

```text
oven mix add cli --dry-run
```

The CLI resolves `cli` as a mix, sees that its recommended provider is `app-cli`, validates the descriptor against the active project, and prints a mutation plan:

```text
Mix: cli
Package: app-cli 0.3.1
Source: loaves.pub:app-cli@0.3.1
Requires Incan: >=0.3,<0.4

Would update dependencies:
  [dependencies]
  app-cli = "0.3.1"

Would create:
  src/main.incn
  tests/test_cli.incn

Would update loaf.toml:
  [project.scripts]
  main = "src/main.incn"

  [oven.envs.default.scripts]
  run = ["oven", "run"]
  test = ["oven", "test"]

  [oven.mixes]
  enabled += ["cli"]

Tooling:
  action run-cli -> script "run"
  file role src/main.incn -> main

Agent guidance:
  cli.write-commands
```

If the plan looks right, they apply it:

```text
oven mix add cli
```

The result is an ordinary project:

```text
weather_core/
  loaf.toml
  src/lib.incn
  src/main.incn
  tests/test_cli.incn
```

The updated manifest records the visible project shape and mix provenance:

```toml
[project]
name = "weather_core"
version = "0.1.0"

[dependencies]
app-cli = "0.3.1"

[project.scripts]
main = "src/main.incn"

[oven.envs.default.scripts]
run = ["oven", "run"]
test = ["oven", "test"]

[oven.mixes]
enabled = ["cli"]

[oven.mixes.details.cli]
source = "loaves.pub:app-cli"
version = "0.3.1"
requested_by = "user"
```

Oven also writes the mix's descriptor digest and expanded graph to `oven.provenance.toml`, so the project can later explain which mix added the `run` script.

After application, the project has both the package dependency and the project setup needed to use it as a CLI application mix. The project does not need the original descriptor to build. The descriptor was the recipe; the resulting source files and manifest are the durable project state. Tooling may still use the recorded mix provenance to explain the project, offer run actions, or suggest relevant agent guidance.

### Starter and mix descriptors

Descriptor syntax is a tooling contract, not Incan source syntax. A descriptor may be represented as TOML, JSON, or another stable encoding, but the semantic fields are:

```toml
[starter]
id = "cli"
title = "Command-line application"
description = "Executable project with standard CLI structure and basic tests."
requires-incan = ">=0.3,<0.4"
mixes = ["std.process", "testing.basic"]

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

[manifest.oven.envs.default.scripts]
test = ["oven", "test"]
```

A mix descriptor uses the same project mutation model, but its root identity is a mix rather than a starter:

```toml
[mix]
id = "testing.basic"
title = "Basic testing support"
description = "Adds a standard test script and sample test layout."
requires-incan = ">=0.3,<0.4"

[[files]]
source = "templates/test_main.incn"
target = "tests/test_main.incn"
mode = "create"

[manifest.oven.envs.default.scripts]
test = ["oven", "test"]

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
title = "Write tests for this mix"
kind = "skill"
applies_to = ["testing.basic"]
description = "Use when adding or updating tests in projects that enabled this mix."
```

The concrete descriptor file format may evolve, but implementations must preserve the semantic distinction: a starter creates or initializes a project shape; a mix adds one reusable project concern.

For the walkthrough above, the provider-side descriptor could be packaged as ordinary catalog data alongside templates:

```text
app-cli/
  loaf.toml
  mixes/
    cli.toml
  templates/
    main.incn.template
    test_cli.incn.template
```

`mixes/cli.toml` might contain:

```toml
[mix]
id = "cli"
title = "Command-line application entrypoint"
description = "Adds a standard CLI entrypoint, run script, and CLI test skeleton."
requires-incan = ">=0.3,<0.4"

[package]
name = "app-cli"
version = "0.3.1"
advertises = ["cli"]

[[files]]
source = "templates/main.incn.template"
target = "src/main.incn"
mode = "create"

[[files]]
source = "templates/test_cli.incn.template"
target = "tests/test_cli.incn"
mode = "create"

[manifest.project.scripts]
main = "src/main.incn"

[manifest.oven.envs.default.scripts]
run = ["oven", "run"]
test = ["oven", "test"]

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

The provider ships static descriptor data and templates. The provider does not ship an arbitrary setup script that the user's project executes during `oven mix add`.

## Reference-level explanation

### Terminology

A **starter profile** is a named descriptor that can create a new project or initialize an existing directory by applying a deterministic set of project mutations.

A **mix** is a named descriptor that applies one reusable project concern to an existing project. A starter profile may include mixes.

A **project mutation** is a planned change to project files, `loaf.toml`, dependency declarations, env definitions, scripts, generated docs, or other explicit artifacts owned by the project.

A **starter catalog** is a source of starter and mix descriptors. The source kinds are built-in descriptors, explicit local descriptor paths, descriptors provided by Loaf packages from `loaves.pub`, git references, and public and private catalog sources. Implementations must support all of them under one identity, version, and validation model: a mix written as a local file and later published to `loaves.pub` remains the same kind of mix.

Every source must resolve a descriptor to its exact content, identified by source, id, version, and digest, before any plan is built. Offline operation must use the verified descriptors already available locally, such as built-ins, local paths, and `loaves.pub` descriptors Oven already holds. A fact that could not be fetched or verified must be reported as unknown, never assumed compatible.

A **catalog-backed source** is a starter catalog backed by a registry or service that can provide discovery, integrity metadata, yanking state, publisher identity, compatibility information, and descriptor payloads or references. Catalog-backed sources do not apply mutations directly to a project.

A **private catalog** is a catalog-backed source for organization-specific or team-specific starters and mixes that should not be discoverable through the public package ecosystem. Private catalog descriptors may describe whole project blueprints, internal platform conventions, or cross-package mixes. They do not need to be scoped to one public package.

### Identifier rules

Starter ids and mix ids must be stable ASCII identifiers. They should use lowercase words separated by `-` or `.`. Implementations must reject ids containing path separators, shell metacharacters, or whitespace.

The same id must not resolve to two descriptors in the same catalog source. If two catalog sources provide the same id, the CLI must either reject the ambiguity or apply a documented source precedence rule and show the selected source in diagnostics.

If a mix id is advertised by multiple packages and no default provider is configured, `oven mix add <mix-id>` must not guess silently. It must show the candidate providers and require the user to disambiguate through a package-qualified mix id or an explicit provider flag.

### Command surface

This RFC adds the following lifecycle commands and flags:

- `oven starter list`
- `oven starter show <starter-id>`
- `oven mix list`
- `oven mix show <mix-id>`
- `oven mix status`
- `oven mix diff <mix-id> --to <version-or-ref>`
- `oven mix add <mix-id>`
- `oven mix update <mix-id> --to <version-or-ref>`
- `oven new <name> --starter <starter-id>`
- `oven init --starter <starter-id> [--adopt]`
- `oven starter validate <descriptor>` and `oven mix validate <descriptor>`

Every command that takes parameters must accept `--set <name>=<value>` and `--values <file>` and must prompt only for missing values when attached to an interactive terminal. Prompted, flagged, and file-supplied values must normalize to the same typed parameter values under RFC 074's parameter kinds; a non-interactive run with a missing required value must fail with a diagnostic listing every missing parameter.

`oven init --starter` must require `--adopt` when the directory already contains a `loaf.toml` or source files, and must otherwise refuse with a diagnostic pointing at `--adopt --dry-run`. An adoption plan must preserve existing files and manifest entries, list every conflict, and apply only after each conflict is resolved explicitly.

`oven starter show` and `oven mix show` must report the descriptor source, required Incan constraint, descriptor dependencies, included mixes, planned manifest effects, declared files, and known conflicts if evaluated in a project context.

`oven mix add` requires a project root. If no `loaf.toml` is found, it must fail with a diagnostic suggesting `oven init` or `oven new`.

`oven add <pkg>` remains the RFC 118 package dependency command. If the added package advertises mixes, the CLI should report those mixes and show the corresponding `oven mix add <mix-id>` command, but it must not apply project setup as part of plain dependency addition.

`oven mix status` requires a project root. It must report enabled mix provenance and should report whether the project still appears consistent with each enabled mix's declared expectations. When catalog or registry metadata is available, status should also report the latest compatible descriptor version and whether the recorded source is yanked, revoked, superseded, or unknown.

`oven mix diff` requires a project root and an enabled mix. It must resolve the current recorded descriptor and requested target descriptor, then show the receiver-side mutation plan without writing files. Source-level descriptor diffs may be shown as supporting information, but the rendered project diff is the review artifact.

`oven mix update` requires a project root and an enabled mix. It applies an explicit receiver-approved mutation plan for moving from the recorded descriptor version to the requested target version or ref. It must support `--dry-run`, must preserve recorded source identity unless the user explicitly requests a source switch, and must not bypass RFC 076 policy.

`oven new` without `--starter` must keep the RFC 015 default behavior. Implementations may model that default internally as a built-in starter, but users must not be required to know that.

Commands that list, show, or add starters and mixes may accept a catalog source selector. The exact flag spelling is not normative, but the model must distinguish built-in descriptors, local descriptor paths, git references, package-provided descriptors, public catalog descriptors, and private catalog descriptors in diagnostics and machine-readable output.

### Machine-readable inspection

The CLI must provide a machine-readable format for these read-only operations:

- listing starters
- showing one starter
- listing mixes
- showing one mix
- reporting project mix status
- producing a dry-run mutation plan

The exact flag spelling is not normative, but `--format json` is the recommended shape because it is familiar and easy for LSP servers and editor plugins to consume.

Machine-readable output must include stable ids, titles, descriptions, descriptor sources, compatibility state, descriptor dependencies, included mixes, file mutations, generated-file ownership policies, manifest mutations, declared tooling actions, file roles, agent guidance metadata, diagnostics, and unresolved conflicts where applicable.

Human-readable output may be optimized for terminal use, but it must not be the only supported inspection surface.

### Descriptor compatibility

Each starter or mix descriptor may declare `requires-incan`. If present, the toolchain selected under RFC 073 must satisfy the descriptor constraint before the descriptor is applied.

If a descriptor is applied to an existing project, its `requires-incan` must also be compatible with the project's effective toolchain constraint. The descriptor must not silently loosen `[project].requires-incan`. If the descriptor requires a narrower range, the CLI may propose a manifest update, but it must show the resulting effective constraint before writing.

If applying a descriptor would create an invalid RFC 073 environment matrix or env-level toolchain constraint, the CLI must reject the plan before writing files.

### Descriptor dependencies

Starter and mix descriptors may declare dependencies on other descriptors:

- required mixes
- optional mixes
- template sources governed by RFC 074
- package dependencies or dev-dependencies
- required project mixes that must already be enabled

The CLI must resolve descriptor dependencies into a directed acyclic graph before planning file or manifest mutations. If the graph contains a cycle, incompatible version requirements, ambiguous providers, or a missing dependency, planning must fail before any project files are written, with a diagnostic that names the descriptors involved. Two requested mixes must never silently pick incompatible versions of a shared descriptor.

An optional dependency must be an explicit edge, enabled by a declared parameter or an explicit user choice and shown in the plan as included or skipped with its reason. Once enabled, it must be resolved exactly like a required dependency, with the same cycle, version, and provider checks; optional edges must not weaken that model.

Dependency resolution must be visible in dry-run output. If applying `sample_query.project` also applies `sample_query.session`, `testing.basic`, and three templates, the dry run must show those transitive effects explicitly.

Descriptors should stay small enough that dependency graphs remain explainable. A large starter may compose smaller mixes, but the user-facing plan must still make clear which concern contributes which files, manifest entries, scripts, and generated-file ownership policies.

### Applicability and back-off

Starter and mix descriptors may declare applicability checks for project state they require or expect. Applicability checks are descriptive planning inputs, not hidden runtime behavior.

Applicability checks may inspect explicit project metadata such as package dependencies, manifest keys, env definitions, scripts, known file roles, mix provenance, template provenance, and declared expected files. They must not depend on executing project code or running arbitrary provider scripts.

The planner must classify each descriptor with exactly one of these states:

- `applicable`: the descriptor can be applied to this project.
- `already-applied`: the same descriptor version is already recorded and its expected shape is present, so a repeated request changes nothing.
- `conflicting`: applying it would conflict with existing files, manifest entries, dependencies, or another mix.
- `unsupported`: the project or toolchain cannot take it, for example because of `requires-incan` or a missing required project mix.

It must classify each mutation as `planned`, `already-satisfied`, `skipped`, `blocked`, `conflicting`, or `unsafe`. Every state other than `applicable` and `planned` must carry a reason code and structured data, including the missing prerequisites for `unsupported` and `blocked`. Reason codes are stable kebab-case identifiers documented with the CLI, such as `missing-prerequisite`, `toolchain-incompatible`, `file-exists`, `user-edited`, `ownership-bootstrap`, and `policy-denied`; a provider-specific code must be namespaced by its package. For example, a CLI mix may skip creating `src/main.incn` when the project already has a declared `main` script and file role, but it must show that decision, with its reason code, in dry-run and machine-readable output. An agent can then tell a harmless repeated request from a real incompatibility.

Back-off behavior must be conservative. If the planner cannot prove that an existing file, manifest entry, or dependency already satisfies a mix expectation, it must report a conflict or warning rather than silently assuming success.

### Project mutation planning

Before applying a starter or mix, the CLI must build a mutation plan. The plan must include:

- files to create
- files to merge
- files that would conflict
- the ownership policy of every file
- optional file groups included or skipped, with the reason for each
- manifest tables and keys to add or update
- dependency and dev-dependency changes
- applicability states and back-off decisions
- enabled mix provenance records
- env and script changes
- tooling and agent guidance metadata
- follow-up notes or warnings

The CLI must either apply the whole valid plan or apply nothing. Partial starter or mix application is not allowed unless the user explicitly asks for a machine-readable plan export and applies it manually outside this RFC.

The planner must know whether it is creating a new project or adopting an existing directory. Greenfield creation may include bootstrap files intended to teach or seed a project. Existing-project adoption should preserve user-authored files by default and should skip or mark bootstrap-only files as conflicts rather than silently replacing them.

### File mutation rules

File entries must declare one of these modes:

- `create`: write the target only when it does not already exist
- `merge`: merge into a supported structured file using deterministic rules
- `replace`: overwrite an existing file only when an explicit overwrite flag is supplied

The default mode is `create`.

A descriptor must not overwrite an existing file by default. If a file conflict occurs, the CLI must stop before writing and report the conflicting path. Overwrite flags may exist, but they must be explicit at the command line and must not be implied by `--yes`.

File entries may reference templates governed by RFC 074. Starter and mix application must consume rendered files from the template layer; it must not define a second templating language or execute arbitrary generator code.

### File ownership

Every file entry carries one ownership policy, and this RFC is its single definition. RFC 074's template descriptors use the same `ownership` field and template provenance records it; neither layer may define a second ownership flag.

- `bootstrap`: created during initial application and not rewritten by normal mix refreshes or template updates. It suits files users customize immediately, such as application source, domain models, local configuration examples and initial docs.
- `managed`: eligible for explicit updates, through mix refreshes or RFC 074 template updates, while preserving user edits under RFC 074's merge and conflict rules. It suits files that should follow upstream evolution, such as tool configuration, shared helpers, CI boilerplate and formatter or linter configuration.
- `advisory`: origin metadata only. Tooling may explain where the file came from but must never treat it as owned.

A file entry that declares no ownership is `bootstrap`. `managed` must be declared explicitly, and only for files that are genuinely regenerated from upstream, so editing application source such as `src/main.incn` is ordinary ownership rather than a generated-file violation.

A recorded rendering digest is a precondition fact for an update, never permission to erase a file that has changed.

### Optional file groups

A starter or mix may declare optional groups of files, such as docs, CI, test fixtures, backend-specific files or deployment scaffolding, and the condition that enables each:

```toml
[[file_groups]]
id = "docs"
enabled_by = "enable_docs"

[[file_groups.files]]
source = "templates/docs/index.md.template"
target = "docs/index.md"
language = "markdown"
```

The planner must decide every group before any file is rendered, and must hand RFC 074's renderer the explicit selected file set. It must not create a full tree and delete disabled parts afterward: that makes dry runs less honest and leaves provenance unable to tell an excluded file from one the user later removed. The plan must report each group as included, skipped, blocked or unavailable, with the parameter, project metadata, source compatibility or policy condition that decided it.

### Manifest mutation rules

Manifest mutations must be deterministic and must preserve unrelated user-authored manifest content.

For scalar keys, applying a descriptor must fail if the target key already exists with a different value unless the descriptor declares a supported merge rule or the user supplies an explicit override flag.

For list-like keys, applying a descriptor should append missing values without duplicating existing values. Ordering must be deterministic.

For dependency tables, applying a descriptor must reject incompatible duplicate dependency requirements unless a documented resolver can prove the resulting requirement is compatible. The CLI must not silently replace a user-authored dependency constraint with a starter-provided one.

For env definitions under `[oven.envs.*]`, applying a descriptor must use RFC 073 validation rules. Inherited envs, scripts, matrices, and toolchain constraints must remain valid after the mutation.

### Mix provenance

When `oven mix add` applies a mix, it must record what was applied. The intent goes in `loaf.toml`:

```toml
[oven.mixes]
enabled = ["testing.basic", "std.http-client"]

[oven.mixes.details."std.http-client"]
source = "loaves.pub:std-http"
version = "0.4.2"
requested_by = "user"

[oven.mixes.details."std.http-client".values]
base_url = "https://weather.example.com"
```

The evidence goes in the `[mixes]` table of RFC 074's tool-owned `oven.provenance.toml`, next to template evidence: the descriptor digest, the expanded transitive mix graph, integrity and signature state, and the Incan version used. `oven.lock` holds neither. Small manifests are not a reason to hide a project mutation, so recording is not optional.

The enabled list is provenance and tooling state. It does not replace explicit dependencies, source files, envs, or config declarations. Removing an item from the list must not be interpreted as uninstalling generated files or dependencies.

Mix provenance must preserve the descriptor source kind, source identity, selected descriptor version or content hash when available, provider package when applicable, applied mix id, expanded transitive mix graph, and the Incan version used to apply the descriptor. When available, it should also preserve publisher identity, integrity/signature state, yanking or revocation state, catalog trust tier, and the top-level user request that caused transitive mixes to be applied.

The compact `enabled = [...]` form is sufficient for human-readable manifest summaries; refresh, status, and security review read the details and the evidence file. Both are explicit project tooling state rather than inferred from generated files. Sensitive parameter values follow RFC 074's rule: they never appear in either file.

If a mix is already recorded as enabled, `oven mix add <mix-id>` should be idempotent. It may revalidate that the expected project shape is still present, but it must not duplicate manifest entries or files.

Mix provenance should preserve enough information to explain transitive additions. A project may record that a starter enabled `sample_query.project`, and that `sample_query.project` pulled in `sample_query.session` and `testing.basic`. Tooling should be able to show the user both the top-level request and the expanded mix graph.

Mix provenance is also the anchor for updates. A project that records `cli@1.3.0` can later ask for a reviewable update to `cli@1.6.0` only if tooling can identify the recorded descriptor source, selected version or content hash, parameter values or safe value fingerprints, transitive descriptor graph, generated-file ownership, and applied template provenance. If that information is missing, `oven mix update` must degrade to an explicit adoption or manual-migration flow rather than guessing.

Descriptor-version updates may include declarative migrations, written in the newer descriptor version as `[[migrations]]` entries with `from` and `to` versions and ordered `steps`. Each step must use one of these operations: `add_manifest_key`, `rename_manifest_key`, `rename_script`, `rename_parameter`, `change_template_id`, `change_file_ownership`, `change_dependency_version`, `change_action_descriptor`, or `manual`. Arbitrary shell, Python, plugin, or provider-defined migration hooks are not part of this design.

Migration planning must be deterministic. Its inputs are the recorded and target descriptor versions, pinned by digest, and the current manifest and file digests; the same inputs must produce the same ordered plan. Every operation except `manual` must state its precondition, such as the current value of the script it renames, and the planner must check it against the project. A failed precondition must become a conflict in the plan, never a guess. A `manual` step must be shown to the user and never applied. When several migrations lie between the recorded and target versions, the planner may collapse them into one plan only while each step remains attributable to its migration. Applying must recheck every precondition and file digest, and must refuse a plan whose inputs changed after it was previewed.

### Tooling metadata

Starters and mixes may declare tooling metadata that describes editor-visible project shape without changing language semantics.

Tooling metadata may include:

- file roles, from the standard set `main`, `library`, `test`, `config`, `generated`, `docs`, and `example`
- action descriptors or action references mapped to RFC 015 scripts, env scripts, or project entrypoints, whose `kind` is one of RFC 078's typed action kinds
- config declarations that point at generated `ctx` types
- documentation links for a mix or generated file role
- expected files that tooling can validate after application

Standard roles and action kinds are checked identities with typed schemas, so an editor knows that a declared action is a test action without interpreting its display name. Any other role or kind must be namespaced as `<namespace>.<name>`, with the namespace declared by the providing package. Validation must reject an unknown unnamespaced value, and tooling must never treat a display name or free-form string as semantic authority.

Tooling metadata must be descriptive. It must not grant special compiler privileges or activate runtime behavior that is not otherwise present in source files or the manifest.

If tooling metadata references files, paths must be relative to the project root and must not escape it. If it references scripts or envs, those scripts and envs must exist after the mutation plan is applied.

The LSP or another editor-facing tool may use this metadata for completions, code actions, diagnostics, project-tree grouping, and run/debug affordances. It must still treat the compiler and lifecycle CLI as the source of truth for validation.

RFC 078 owns typed action semantics, execution modes, risk labels, dry-run expectations for action execution, and `oven action run`. This RFC only lets starters and mixes contribute or reference action descriptors as part of a project mutation plan. A descriptor that mentions an action must remain descriptive until the user or tooling explicitly invokes the action through RFC 078 behavior.

### Agent guidance metadata

Starters and mixes may declare agent guidance metadata that describes project-relevant skills, workflows, or maintenance affordances for agentic tooling.

An `[[agent_guidance]]` entry has these standard structured fields:

- `id`, `title`, `kind` (`skill` or `workflow`), and `description` of when the guidance applies;
- `applies_to`: the file roles, mixes, or manifest sections that trigger it;
- `entrypoints`: the files or declarations an agent should start from;
- `contracts`: the documentation, local instruction files bundled with the package, or specifications the work must follow;
- `effects`: safety labels from the set `read-only`, `project-mutation`, `networked`, and `requires-human-review`;
- `verification`: the actions or scripts that check the work, referenced by id and never run by the guidance itself.

For example, guidance for a testing mix can name `tests/` as its entrypoint and the `run-tests` action as its verification, without gaining any authority to execute that action. Runtime-specific extras must sit under a namespaced `extensions.<namespace>` table and are inert: tooling must not give them meaning beyond the namespace owner's documentation.

Agent guidance metadata is descriptive. It must not cause a starter or mix to install, enable, or execute an agent automatically. It must not embed secrets, credentials, or hidden prompts that change project behavior. If an agent tool consumes this metadata, it remains responsible for its own permission model and user confirmation rules.

This metadata is useful because mixes encode project intent. If a project enables a testing mix, an agent can be told which test-writing workflow is relevant. If a project enables an HTTP client mix, an agent can be told which docs, config files, and validation checks matter. The starter/mix layer should expose that intent once instead of forcing every agent integration to rediscover it from filenames.

### Dry-run behavior

`--dry-run` must be supported for `oven new --starter`, `oven init --starter`, `oven mix add`, and `oven mix update`.

Dry-run output must include the full mutation plan and must not write project files, lockfiles, or generated artifacts. If a dry run detects conflicts, it must report them with the same diagnostics that a real apply would use. The plan must explain each descriptor and mutation state with its reason code.

### One planning contract

One library-level contract must resolve descriptors and build mutation plans for every consumer: the CLI, its dry runs, the LSP, and editor integrations. A consumer may reach it in-process or by running the CLI with `--format json` as a subprocess, but neither path may reimplement resolution, compatibility, conflict detection, or mutation semantics, so the CLI and an editor report the same conflicts for the same selected graph.

Every plan must carry its preconditions: the resolved descriptor identities and digests, and the digests of the manifest and every file it reads or changes. Applying a plan, whether just built or previewed earlier in an editor, must recheck those preconditions and must refuse a plan whose inputs changed since it was built, asking for a new preview rather than replaying an outdated approval over new edits.

Implementations must provide a machine-readable dry-run format for the full mutation plan. Security-sensitive mutation categories, conflicts, descriptor source identity, provenance changes, and receiver-side rendered file changes must be represented in that format rather than only in human-readable prose.

### Lockfile interaction

Starter and mix application may update `loaf.toml`, but it must not silently rewrite `oven.lock` unless the command documents and exposes that behavior. The default behavior should leave lockfile updates to the next `oven lock`, `oven bake`, or `oven test` flow governed by RFC 020.

If a descriptor includes dependency changes and the project is in a locked or frozen mode, the CLI must report that lockfile refresh is required rather than pretending the project remains fully locked.

### Security and trust

Applying a starter or mix must not execute arbitrary code from the descriptor.

Descriptors must not embed secrets. If generated code needs credentials, it should create typed configuration placeholders, environment-variable references, or documented setup steps.

Descriptors loaded from local paths must be treated as untrusted input for parsing and file path handling. File targets must be normalized relative to the project root and must not escape it.

Registry-backed descriptors must inherit registry integrity, checksum, and signature rules from the package distribution layer rather than creating a second supply-chain story.

Applying or refreshing a mix is a supply-chain-sensitive project mutation. It may add dependencies, scripts, env definitions, generated source files, CI configuration, tooling metadata, or agent guidance. The lifecycle CLI must therefore present the mutation as a reviewable project diff, not as a cosmetic package setup step.

The receiving project owns acceptance of mix mutations. A starter or mix descriptor is a recipe for a proposed project patch; it is not authority for the provider, registry, catalog, package, or automation system to write into the receiver's codebase. This is especially important for refreshing an already-applied mix, where the command is changing an existing project rather than creating an empty one.

Mix plans must show the rendered receiver-side result, not only descriptor diffs. A descriptor diff can explain why a change is proposed, but reviewers need to inspect the actual project patch: source files, generated tests, scripts, envs, manifests, CI/configuration, tooling metadata, and agent guidance.

Mix provenance must preserve the descriptor source identity and selected descriptor version or content hash when available. If a later refresh would change the descriptor source, publisher identity, catalog trust tier, package provider, checksum/signature state, or yanking state, the CLI must surface that before applying file or manifest changes.

Catalog resolution must avoid dependency-confusion behavior. A public catalog entry must not silently shadow a previously recorded private catalog descriptor with the same id, and a private catalog descriptor must not silently replace a public descriptor without explicit user intent. If multiple catalogs can provide the same starter or mix id, the CLI must either fail with an ambiguity diagnostic or show the selected source according to a documented precedence rule.

Security-sensitive mutation categories must be explicit in dry-run and machine-readable output. At minimum, plans must identify dependency additions or upgrades, script/task changes, env changes, CI/config changes, executable source changes, generated-file ownership changes, and agent guidance metadata changes.

Project and organization policy may restrict which catalog sources, publishers, trust tiers, or descriptor pin forms are allowed for starters and mixes. Policy may also require approval for high-risk mutation categories before `oven mix add` or a future mix refresh writes changes. This mirrors code review ownership for CI workflows: source files, scripts, CI config, env definitions, and agent guidance may need different reviewers.

Automated mix update should follow a review-first model. It may detect stale descriptors, vulnerable sources, or newer compatible versions, but it should emit a reviewable mutation plan or pull-request-sized patch rather than applying changes unattended. The plan should show before/after descriptor source, version or content hash, provider identity, known advisory state, yanking state, and every security-sensitive mutation category.

Automation that proposes a mix update must not also satisfy the receiver's approval requirement. If policy requires review for a mutation category, the approver must be independent from the tool, service, or agent that produced the proposed patch. Mix update should therefore integrate with normal code review instead of bypassing it.

### Provider validation

`oven starter validate` and `oven mix validate` must run the same checks as RFC 074's `oven template validate` for every template the descriptor references, and must also check:

- the descriptor schema, identifiers, standard or namespaced tooling values, and agent guidance fields;
- the dependency graph, including cycles, version conflicts, and ambiguous providers;
- applicability fixtures: declared fixture projects must classify as the descriptor expects, including at least one project on which it is `unsupported` or `conflicting`;
- mutation-plan fixtures: applying the descriptor to each fixture project must produce the declared plan, and each declared migration must plan cleanly from its `from` version.

A mix must therefore demonstrate what it edits and how it behaves on an incompatible project. With `--strict`, warnings are errors. The result must be reported in a versioned machine-readable form. Publishing to `loaves.pub` must require a passing strict result; promotion to a higher trust tier is a separate policy step under RFCs 076 and 125.

### Diagnostics

Diagnostics for failed starter or mix application must name:

- the selected starter or mix id
- the descriptor source
- the project root, when one exists
- the conflicting file, manifest key, dependency, env, or toolchain constraint
- the command or flag that would let the user preview or resolve the issue, when applicable

Diagnostics should prefer actionable conflict explanations over generic "template failed" messages.

## Design details

### Relationship to RFCs 015 and 117

RFC 015 remains the historical source of the baseline lifecycle ideas. RFC 117 prospectively supersedes its manifest filename, discovery, lock, and environment-configuration placement. This RFC adds richer starter selection and mix application on top of the `loaf.toml` project model without reviving the old manifest contract.

The RFC 015 default scaffold remains valid and should stay small. Starter profiles are the place for opinionated project shapes.

### Relationship to library metadata

Mixes are project-level activation units. Library manifests may advertise mixes, dependencies, config needs, or generated helper files, but applying those mixes to a project remains a lifecycle CLI operation through `oven mix add`. This avoids a hidden model where merely importing a package or adding a dependency rewrites the project or changes source layout.

Where library manifests and starter descriptors overlap, library manifest data should be the source of truth for package-owned dependency and feature requirements. Starter descriptors should compose those mixes rather than copy stale requirements by hand.

Plain package addition remains owned by RFC 118, with registry semantics under RFC 125. A package may advertise mixes, and the package-add workflow may point users at those mixes, but mix application is a separate command surface because it may create files and mutate project structure.

### Relationship to `ctx`

If a starter or mix needs application configuration visible to the compiler, it should generate Incan source using `ctx` rather than creating an untyped sidecar configuration format. Sidecar files remain appropriate for operational data that the compiler should not understand.

Generated `ctx` declarations must follow normal source rules. The starter mechanism does not grant special access to configuration values.

### Relationship to env matrices

Starter and mix descriptors may add env definitions. If they add matrix envs, those envs must satisfy RFC 073. This RFC does not redefine matrix expansion or toolchain constraint semantics.

### Relationship to catalogs and package registries

This RFC defines descriptor semantics and local lifecycle behavior. Catalog source kind is a distribution concern, not a different mix system. A starter or mix descriptor should mean the same thing whether it came from a built-in source, local file, git reference, public registry, or private catalog.

`loaves.pub` is positioned to become the default public remote catalog because it already owns package identity, versioning, checksums, signatures, yanking, dependency metadata, and package discovery. Starter and mix descriptors published through `loaves.pub` should reuse that trust and compatibility model instead of creating a parallel public marketplace.

Private catalogs serve a different need: project and organization blueprints that may be broader than one library package. For example, a private descriptor can describe an end-to-end application shape that includes a CLI, API layer, analytics layer, data quality checks, governance conventions, and agent-relevant guidance. That descriptor can still be represented as ordinary starter and mix metadata. The local lifecycle CLI remains the executor.

The catalog or registry role should be discovery and verification:

- find starters and mixes by id, package, domain, or required Incan version
- expose descriptor provenance, publisher identity, checksum/signature state, and yanking state
- expose package and mix compatibility metadata that the local CLI can use during planning
- expose descriptor dependency graphs and latest compatible versions for status checks
- expose source diffs or rendered-template diff metadata where that can be computed without local project state
- provide descriptor payloads or descriptor references for the local CLI to resolve

The catalog or registry must not be required to compute or apply project mutation plans. The local lifecycle CLI remains responsible for resolving the current project, checking local files, producing dry-run output, enforcing overwrite rules, and writing changes. This boundary keeps remote behavior cacheable and auditable while preserving local control over project mutation.

Publishing a starter or mix to `loaves.pub` must require a passing strict `oven mix validate` result, described under provider validation. Promotion to a higher trust tier is a separate policy step under RFCs 076 and 125.

### Relationship to RFC 076

RFC 076 owns project and organization policy for approving receiver-side mutations, restricting descriptor sources, classifying mutation risk, handling yanked or malicious sources, and recovering from unsafe applied mutations. This RFC defines the starter and mix mutation plans that RFC 076 policy evaluates.

### Relationship to RFC 078

RFC 078 owns the action model. Starter and mix descriptors may contribute action descriptors, but they do not define action execution semantics. During starter or mix application, actions are planned and recorded as metadata; they are not run implicitly. When a user later lists, dry-runs, or runs an action, RFC 078 behavior applies.

### Relationship to LSP and IDE integrations

The language server uses the one planning contract above, in-process or through the CLI's `--format json` output, so editor integrations stay thin. Editor plugins may provide rich UI, and they may use cached `loaves.pub` catalog data for discovery, but they must not own the semantics of starter resolution, mix compatibility, conflict detection, or manifest mutation. Whether the language server links the planning library or runs the CLI is an integration choice, not a second semantic engine.

This mirrors the broader Incan tooling direction: the language server and editor integrations should surface compiler and lifecycle knowledge rather than reverse-engineer it from conventions. Starter profiles and mixes should therefore be designed as toolchain data first and editor UI inputs second.

### Relationship to agentic tooling

Agentic tooling should be treated like another consumer of project metadata, not as a privileged project mutation path. Starters and mixes may advertise relevant skills and workflows, but those advertisements must flow through the same machine-readable inspection surfaces used by IDEs and CLIs.

This keeps the boundary simple: the mix descriptor says what kind of project concern exists and which guidance may be relevant; the agent runtime decides whether and how to use that guidance under its own safety model. The descriptor must not smuggle executable prompts or bypass local project permissions.

### Compatibility and migration

This RFC is additive. Existing projects that use RFC 015 commands without starters continue to behave the same way.

Existing examples can be converted into starter profiles gradually, but examples remain valuable documentation. A starter profile should not replace explanatory docs; it should make a documented recommended shape executable.

Mix provenance is always recorded when a mix is applied. Removing it must not break ordinary builds, but status, update, and explanation commands then treat the affected mixes as unknown rather than guessing.

## Alternatives considered

### Fold starter profiles into RFC 015

Rejected because RFC 015 is already a coherent lifecycle foundation. Adding starter catalogs, mix composition, conflict planning, and provenance would broaden it from "project lifecycle CLI" into "project application framework setup." A separate RFC keeps the base layer small and coherent.

### Keep only examples and documentation

Rejected because copy-paste examples drift. Users need a deterministic command that applies the recommended shape, reports conflicts, and records what changed.

### Use arbitrary generator scripts

Rejected because arbitrary scripts make starters powerful but unsafe, hard to inspect, and hard to reproduce. Static descriptors plus file templates are sufficient and keep the trust boundary narrow.

### Make package import activate mixes automatically

Rejected because imports should not mutate projects or create hidden configuration. Mix activation should be explicit through `oven mix add` or a selected starter profile.

### Make plain `oven add <pkg>` apply default mix setup

Rejected because it overloads dependency addition with project mutation. RFC 118 defines `oven add <pkg>` as the package workflow, analogous to `cargo add`. It should be safe to add a dependency without creating source files or scripts. Packages may advertise mixes and the CLI may suggest them after adding the dependency, but applying mix setup belongs to `oven mix add`.

### Store all starter behavior in `loaf.toml`

Rejected because starters often need file templates, docs, and sample tests. `loaf.toml` should record project metadata and applied mix provenance, not become a large embedded template archive.

### Add mix removal

Rejected because removal is substantially harder than addition. Generated files may have been edited by users, dependencies may be shared by multiple mixes, and manifest changes may no longer be attributable to one mix. This design covers safe creation, addition, update, and inspection.

## Drawbacks

- Starter catalogs introduce another project-facing concept that must be documented and supported.
- Mix provenance can drift from the actual project shape if users manually edit generated files or dependencies.
- Descriptor conflict handling will need careful design to avoid both false positives and accidental overwrites.
- Built-in starters may create expectations that the core toolchain supports every domain equally well.
- Supporting every catalog source kind under one validation model makes each source kind carry the same identity, digest, and integrity obligations.
- The fixed migration step vocabulary cannot automate every migration; source rewrites stay manual instructions.

## Implementation architecture

The recommended implementation shape is to treat starter and mix application as a planning problem before it is a file-writing problem.

First, resolve the descriptor from a catalog. Then expand included mixes and template dependencies into one ordered acyclic mutation plan. Then validate the plan against the target project, including manifest conflicts, file conflicts, generated-file ownership policy, toolchain constraints, env validity, and path safety. Finally, apply the plan atomically or report diagnostics without writing anything.

The same mutation plan should power dry-run output, human diagnostics, and machine-readable inspection. This keeps `oven new --starter`, `oven init --starter`, `oven mix add`, and `oven mix update` aligned rather than implementing separate generators and updaters.

## Layers affected

- **Manifest schema / configuration validation:** `loaf.toml` records mix intent under `[oven.mixes]` and its `details` tables, and mix evidence goes in the `[mixes]` table of `oven.provenance.toml`; starter-applied env, script, dependency, and project metadata changes must validate under RFCs 020, 073, 076, and 117.
- **CLI / tooling:** `oven starter`, `oven mix`, `oven new --starter`, `oven init --starter [--adopt]`, `oven mix add`, `oven mix diff`, `oven mix update`, and the `validate` commands are new lifecycle tooling surfaces, all built on one planning library. They must support prompts and equivalent non-interactive values, inspection, dry-run planning with checked preconditions, conflict diagnostics, deterministic application, and review-first descriptor-version updates through declared migrations.
- **LSP / IDE tooling:** editor-facing tools should consume machine-readable starter, mix, mutation-plan, file-role, and action metadata from the lifecycle layer. They may expose completions, code actions, project diagnostics, and run/debug affordances, but they must not fork descriptor semantics.
- **Agentic tooling:** agent-facing tools may consume mix provenance, file roles, and agent guidance metadata to select relevant skills or workflows, but starter/mix descriptors remain descriptive and must not execute agents implicitly.
- **Project scaffolding:** the project generator must support descriptor-backed file creation, manifest mutation, safe path normalization, and non-overwrite defaults.
- **Library/package integration:** library mix metadata feeds starter and mix descriptors where possible, but project mutation remains explicit. Package and registry metadata may also feed descriptor dependency resolution and status reporting.
- **Registry:** `loaves.pub` serves starter and mix descriptors from Loaf packages and requires a passing strict `validate` result to publish them.
- **Documentation:** user docs must explain the difference between minimal scaffolds, starter profiles, mixes, package dependencies, and runtime configuration.

## Design Decisions

- Descriptors come from built-ins, local paths, Loaf packages on `loaves.pub`, git references, and public or private catalogs, all under one identity, version, digest, and validation model. Catalogs find and verify descriptors; the local planner builds and applies plans. Offline work uses verified local evidence and reports anything unverified as unknown.
- Mix provenance is always recorded: intent (source, version, parameters, top-level request) in `loaf.toml`, evidence (digest, expanded graph, integrity state) in the `[mixes]` table of `oven.provenance.toml`, nothing in `oven.lock`.
- Required descriptor graphs are resolved completely, with cycle, version, and provider diagnostics. Optional dependencies are explicit edges that, once enabled, resolve exactly like required ones.
- A file without declared ownership is `bootstrap`; `managed` is declared explicitly for genuinely regenerated files.
- `oven init --starter` on a non-empty directory requires `--adopt`, previews an adoption plan, and needs every conflict resolved explicitly.
- `oven starter validate` and `oven mix validate` run RFC 074's template checks plus applicability, dependency, mutation-plan, and migration fixtures, including an incompatible project. Publishing to `loaves.pub` requires a passing strict result; promotion is a separate policy step.
- Prompts collect the same typed parameters that `--set` and `--values` supply, with identical validation; a non-interactive run lists every missing required value.
- Standard file roles and RFC 078 action kinds are checked identities; any other value is namespaced by its package, and free-form strings never carry meaning.
- Agent guidance has standard `entrypoints`, `contracts`, `effects`, and `verification` fields; runtime-specific extras are namespaced and inert, and guidance never grants authority to run anything.
- Descriptor states are `applicable`, `already-applied`, `conflicting`, and `unsupported`; mutation states are `planned`, `already-satisfied`, `skipped`, `blocked`, `conflicting`, and `unsafe`. Each carries a stable reason code and structured data, such as missing prerequisites.
- Migrations live in the newer descriptor version as ordered steps from a fixed operation set plus `manual`. Each step checks its precondition; failed preconditions become conflicts; the same inputs give the same plan; and apply refuses a plan whose inputs changed.
- One library-level planning contract serves the CLI, dry runs, and the language server. A subprocess is only a transport, and every plan carries the preconditions that apply rechecks.
