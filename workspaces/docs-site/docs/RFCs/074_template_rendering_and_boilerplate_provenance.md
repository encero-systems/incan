# RFC 074: template rendering and boilerplate provenance

- **Status:** Draft
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 034 (`incan.pub` package registry)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/402
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a deterministic template rendering, generated-file ownership, and provenance model for Incan project tooling. Templates are static input files referenced by lifecycle tooling, starter profiles, capability packs, or package-provided setup data. Rendering is intentionally constrained: v1 templates may substitute declared parameters, but they must not execute arbitrary code, run shell commands, fetch remote data, or infer behavior from filename conventions alone. When rendered files are written into a project, the toolchain may record provenance so users and tools can later check, diff, status, reset, or update generated boilerplate without treating the original template as hidden build state.

## Core model

Read this RFC as ten foundations:

1. **Template files are tooling inputs, not Incan source syntax.** A file such as `main.incn.tpl` is a conventional provider-side artifact. The rendered `main.incn` is the Incan source file users edit, compile, and commit.
2. **Descriptors are authoritative.** A template file's source path, target path, target language, parameter declarations, and provenance behavior are declared by a descriptor consumed by lifecycle tooling. The `.tpl` suffix is a convention, not the semantic contract.
3. **Rendering is deterministic.** Given the same template bytes, declared parameters, parameter values, renderer version, and project metadata, the renderer must produce the same output.
4. **Rendering is intentionally small.** V1 supports declared placeholder substitution. It does not support loops, conditionals, imports, functions, filters, arbitrary expression evaluation, network access, or shell execution.
5. **Parameters are declared and typed.** A template descriptor declares every accepted parameter and its kind. The renderer must reject undeclared placeholders and invalid values before writing files.
6. **Rendered Incan must be validated.** If the target language is `incan`, the toolchain must parse the rendered output and should format it before writing when a formatter is available.
7. **Provenance is explicit tooling state.** A project may record which template produced a file, which package or descriptor supplied it, which parameter values were used, and which hashes were observed.
8. **Updates are explicit, receiver-owned, conflict-aware, and security-sensitive.** Template provenance enables `check`, `diff`, and `update` workflows, but it must not cause automatic rewrites during build, test, import, editor startup, or package resolution. Updating generated source is a project code change in the receiving repository and must be reviewed as one.
9. **Generated files have ownership policy.** A rendered file may be bootstrap-only, managed by template updates, or advisory provenance only. The update policy must be explicit per file rather than inferred from path conventions.
10. **Template inputs are reviewable.** Tooling must be able to show or write the parameter values that would drive a render so teams can review, automate, and reproduce template application.

## Motivation

RFC 015 gives Incan a minimal lifecycle CLI, and RFC 075 builds richer starter and capability workflows on top of it. Those workflows need a way to create files from reusable boilerplate. Without a template contract, each feature would either invent its own extension convention, embed ad-hoc generator scripts, or copy example files without enough metadata to explain later where they came from.

The problem is not just initial scaffolding. Generated boilerplate often needs maintenance. A project may start with a generated CLI entrypoint, `ctx` configuration module, data-session helper, or test skeleton, then edit it over time. Tooling should be able to answer practical questions: is this file still the generated output, was the upstream template changed, what diff would an update apply, and which capability introduced this file? Those questions require provenance, not just file creation.

This RFC keeps the boundary narrow. It does not create a general template programming language. It gives the lifecycle layer a safe rendering primitive that higher-level RFCs can depend on.

## Goals

- Define a project-tooling template model that can be used by lifecycle commands, starter profiles, capability packs, and package-provided setup data.
- Make descriptor metadata authoritative so behavior does not depend only on `.tpl` filename conventions.
- Define a constrained v1 placeholder syntax and parameter model.
- Require deterministic rendering and safe path handling.
- Require rendered Incan output to parse before it is written.
- Define provenance records that support check, status, diff, update, and reset workflows.
- Define generated-file ownership policies that distinguish bootstrap-only files from managed files.
- Support real template suites that need prompts, defaults, constrained choices, derived values, optional file groups, and non-rendered binary or fixture assets.
- Support values-file generation for non-interactive and reviewable template application.
- Support version-aware template status checks when package or registry metadata is available.
- Require template render plans to explain why files are included, skipped, blocked, unchanged, or unsafe.
- Define a safe reset escape hatch for corrupted or heavily conflicted generated files.
- Keep generated projects ordinary: rendered files are normal files, and templates do not participate in compilation unless the user explicitly keeps them as project data.
- Allow templates to be resolved from source-agnostic catalogs such as built-in descriptors, local paths, git sources, public package registries, or private catalogs while keeping rendering and mutation local to the lifecycle CLI.
- Leave room for `incan.pub` packages to distribute public templates without making public registry support mandatory for v1.
- Provide a tooling surface that IDEs, docs tooling, and agents can inspect without reimplementing template semantics.

## Non-Goals

- Defining starter profiles, capability packs, or their command surface. RFC 075 owns that layer.
- Defining package registry transport, ranking, or remote discovery. RFC 034 owns package registry semantics.
- Defining private catalog hosting, identity, authorization, administration, or commercial policy.
- Defining a general-purpose template language comparable to Jinja, Handlebars, Tera, or Liquid.
- Supporting arbitrary generator scripts, shell commands, network calls, plugin execution, or procedural macros during template rendering.
- Supporting post-generation cleanup hooks as the primary way to model optional slices. Optional files should be represented in descriptors before rendering rather than created and deleted afterward.
- Automatically updating generated files during build, test, package add, import, or editor startup.
- Perfectly merging arbitrary user edits with updated templates.
- Treating generated files as compiler-special or read-only.
- Replacing examples and docs. Templates make recommended boilerplate executable; they do not explain why the boilerplate exists.

## Guide-level explanation

### Provider-side template

A package or built-in tool may ship a static template:

```text
templates/
  session.incn.tpl
```

The template is not compiled directly. It is provider-side data:

```incan
from pub::sample_session import Session, backends
from {{ config_module }} import {{ config_type }}

def default_session() -> Session:
    return Session.builder()
        .with_backend(backends.{{ backend_type }}())
        .with_data_root({{ config_type }}.data_root)
        .build()
```

The descriptor declares how that file may be rendered:

```toml
[[templates]]
source = "templates/session.incn.tpl"
target = "src/data/session.incn"
language = "incan"
provenance = "tracked"
ownership = "managed"

[templates.parameters.config_module]
kind = "module_path"
prompt = "Configuration module path"
default = "data.config"

[templates.parameters.config_type]
kind = "identifier"
prompt = "Configuration type name"
default = "DataConfig"

[templates.parameters.backend_type]
kind = "identifier"
prompt = "Session backend"
default = "DataFusion"
choices = ["DataFusion", "Memory"]
```

The renderer validates every value, substitutes the placeholders, parses the rendered Incan source, formats it when possible, and writes the target file only if the calling lifecycle operation allows that file mutation.

### Consumer-side result

After a starter or capability applies the template, the project contains a normal file:

```text
src/data/session.incn
```

The rendered file is ordinary source:

```incan
from pub::sample_session import Session, backends
from data.config import DataConfig

def default_session() -> Session:
    return Session.builder()
        .with_backend(backends.DataFusion())
        .with_data_root(DataConfig.data_root)
        .build()
```

The project does not need the template to compile. The template was used to create a file; it is not a hidden source dependency.

### Provenance

If the descriptor requests tracked provenance, the lifecycle tool records enough metadata to explain the file later:

```toml
[tool.incan.templates."src/data/session.incn"]
source = "sample_session:templates/session.incn.tpl"
package = "sample_session"
version = "0.1.0"
origin = "capability:sample_session.session"
language = "incan"
renderer = "incan-template-v1"
template_hash = "sha256:4b5a..."
values_hash = "sha256:42d9..."
rendered_hash = "sha256:18c2..."
```

This record is tooling provenance. Removing it must not break compilation. Keeping it allows the toolchain to answer questions about generated files.

### Generating a values file

A user may ask the toolchain to write the values a template expects before applying it:

```text
incan template values-file sample_session:templates/session.incn.tpl --output session.values.toml
```

The generated file contains declared parameters, defaults, choices, and prompt text:

```toml
[values]
config_module = "data.config"
config_type = "DataConfig"
backend_type = "DataFusion"
```

This gives teams something concrete to review in automation and allows non-interactive application:

```text
incan template render sample_session:templates/session.incn.tpl --values session.values.toml --dry-run
```

The exact command spelling may change, but the lifecycle layer must expose the same capability: inspect required values, write a values file, and render from that file without interactive prompts.

### Checking generated files

A user may ask whether tracked files are stale or edited:

```text
incan template check
```

Example output:

```text
tracked templates:
  src/data/session.incn
    status: edited
    source: sample_session:templates/session.incn.tpl
    origin: capability:sample_session.session

  tests/test_session.incn
    status: current
    source: sample_session:templates/test_session.incn.tpl
```

`edited` means the current file hash differs from the recorded `rendered_hash`. It does not mean the file is invalid. Users are expected to edit generated files.

### Checking template status

When a template source comes from a package, starter, capability, or registry-backed catalog, users need more than byte-level drift. They also need to know whether the project is using the latest compatible template source:

```text
incan template status
```

Example output:

```text
target                   source                         current   latest    file
src/data/session.incn    sample_session:templates/session.incn    0.1.0     0.2.0     edited
tests/test_session.incn  sample_session:templates/test_session    0.1.0     0.1.0     current
```

If the source catalog is unavailable, status may fall back to local provenance and report the version as unknown or cached. Machine-readable status should include the provenance record path, source package, source version, latest compatible version, and file drift state.

### Diffing an update

If a package version provides a newer template, tooling can show the proposed change without writing it:

```text
incan template diff src/data/session.incn
```

The diff is between the current project file and the output that would be rendered from the current descriptor and parameter values. If the file has user edits, the tool must make that clear before offering an update.

### Updating explicitly

Template updates are explicit:

```text
incan template update src/data/session.incn --dry-run
incan template update src/data/session.incn
```

If the current file still matches `rendered_hash`, an explicit receiver-approved update may replace it with the newly rendered output. If the file has user edits, the tool must either stop with a conflict diagnostic or use a documented merge strategy. It must not silently overwrite user edits.

### Resetting generated files

If an update cannot be applied cleanly, users may need a stronger operation that recreates managed files from the current template source while preserving recorded values:

```text
incan template reset src/data/session.incn --dry-run
incan template reset src/data/session.incn
```

Reset is not a normal update. It should be treated as an explicit recovery operation. By default, it must preserve bootstrap-owned files, preserve values from provenance or an explicit values file, show a dry-run plan, require confirmation unless running in an explicit non-interactive mode, and leave the resulting changes for review in source control. Reset is still a receiver-side code mutation and must follow the same rendered-diff review and source-identity checks as update.

## Reference-level explanation

### Terminology

A **template file** is a static provider-side file that contains placeholders and is rendered into a project file.

A **template descriptor** is structured metadata that declares the template source, target, target language, parameters, parameter kinds, provenance mode, and any renderer options.

A **placeholder** is a marked location in the template body that references one declared parameter.

A **parameter** is a named value accepted by a template descriptor. Parameters have a kind that controls validation and escaping.

A **rendered file** is the output produced by applying parameter values to a template file.

A **template provenance record** is project tooling state that records where a rendered file came from and which hashes were used.

A **generated-file ownership policy** describes how the lifecycle tool may treat a rendered file after initial creation.

A **template source** is where a template descriptor and its files are resolved from. Source kinds may include built-in templates, local paths, git references, package-provided templates, public catalogs, and private catalogs.

### Descriptor authority

The descriptor is authoritative for template behavior. Implementations must not infer target language, target path, provenance behavior, or parameter kinds solely from the template filename.

The `.tpl` suffix is recommended for provider-side template files because it is familiar and avoids confusing templates with directly compiled source. For Incan templates, `.incn.tpl` is the recommended provider-side suffix. Implementations may accept other filenames when the descriptor marks the file as a template.

### Template source kinds

Template source kind affects discovery, provenance, trust, and upgrade metadata. It must not change rendering semantics. A template loaded from a private catalog, public registry, git reference, local path, package, or built-in source is rendered with the same placeholder validation, path safety, parse validation, overwrite rules, and provenance requirements.

V1 implementations must support built-in templates. They may also support explicit local descriptor paths. Later implementations may add git sources, package-owned templates, public catalog sources, and private catalog sources without changing the descriptor semantics.

Source metadata should answer these questions:

- where the descriptor came from
- which immutable version, commit, package version, or content hash was selected
- whether integrity metadata was verified
- whether the source has been yanked, revoked, or superseded
- whether the source can provide latest-compatible version information
- whether the source can provide source diffs between template versions

Private catalog sources are for organization-specific or team-specific templates that should not be discoverable through the public package ecosystem. They are not special renderers. They must still provide descriptor bytes, referenced template files, and provenance metadata to the local lifecycle CLI.

Diagnostics and machine-readable output must include the selected source kind and source identity when that information is available. If multiple sources provide the same template id, the lifecycle CLI must either reject the ambiguity or apply a documented source precedence rule and show the selected source.

### Placeholder syntax

V1 placeholders use double braces around one parameter name:

```text
{{ parameter_name }}
```

Whitespace immediately inside the braces is ignored. Parameter names must be ASCII identifiers matching this shape:

```text
[A-Za-z_][A-Za-z0-9_]*
```

The renderer must reject:

- placeholders that reference undeclared parameters
- parameters that are declared but unused, unless the descriptor explicitly allows unused parameters
- malformed placeholders
- nested placeholders
- expression syntax inside placeholders
- escaping or control syntax that is not defined by this RFC

### Parameter kinds

V1 defines these parameter kinds:

- `identifier`: an Incan identifier segment
- `module_path`: one or more identifier segments separated by `.`
- `string`: a UTF-8 string value rendered with target-language string escaping when inserted into an Incan template
- `path`: a normalized relative project path that must not escape the project root
- `literal`: a raw textual value that is inserted without escaping

Descriptors should avoid `literal` unless the rendered target language validation can catch invalid output. For `language = "incan"`, use of `literal` is allowed only when the final rendered source parses successfully. For executable, configuration, CI, or agent-facing targets, `literal` parameters should be treated as security-sensitive in dry-run output because they bypass escaping.

Future RFCs may add more kinds. Unknown parameter kinds must be rejected unless the renderer has explicitly opted into an extension namespace.

### Parameter declarations

A template descriptor may declare metadata for each parameter:

- `kind`: the validation and escaping kind
- `prompt`: a short human-facing prompt for interactive tools
- `default`: the default value, when one exists
- `choices`: a finite set of accepted values
- `required`: whether the caller must provide a value
- `sensitive`: whether the value may contain secret or private material
- `derived`: a deterministic derivation from other parameters or project metadata

Prompt text is tooling metadata. It must not affect rendering semantics.

If `choices` is present, the renderer must reject values outside the declared set before substitution.

If `sensitive = true`, the renderer must not store the raw parameter value in provenance, logs, diagnostics, machine-readable plans, or generated comments. If drift detection requires a value fingerprint, the renderer should store a salted or keyed hash, not an unsalted hash of the raw value. Diagnostics should identify the parameter by name rather than printing the value.

Derived values must be deterministic. V1 descriptors should prefer renderer-known transforms such as casing, identifier normalization, slug normalization, path joining, and package-name normalization. Descriptors must not embed arbitrary expression evaluation to compute derived values in v1.

### Optional file groups

Real template suites often contain optional slices: docs support, CI support, test fixtures, backend-specific files, or deployment scaffolding. Descriptors should model those slices before rendering:

```toml
[[file_groups]]
id = "docs"
enabled_by = "enable_docs"

[[file_groups.files]]
source = "templates/docs/index.md.tpl"
target = "docs/index.md"
language = "markdown"
```

The renderer must not rely on post-generation deletion hooks to express optionality. Creating a large tree and deleting disabled parts makes dry-run output less honest, weakens provenance, and makes updates harder because the tool cannot tell whether a missing file was intentionally excluded or later removed by the user.

Optional file-group decisions must be explainable. A render plan should report whether each group was included, skipped, blocked, or unavailable, and it should include the parameter, project metadata, source compatibility, or policy condition that caused that decision. This gives lifecycle tooling enough information to show a condition-style report without reimplementing template logic.

### Non-rendered assets

Some template suites include binary fixtures, fonts, icons, archives, datasets, or other files that must be copied without placeholder scanning. The descriptor must mark those files explicitly:

```toml
[[templates]]
source = "templates/assets/logo.png"
target = "docs/assets/logo.png"
render = false
provenance = "tracked"
```

For `render = false`, the renderer copies bytes exactly after path validation. It must not scan the file for placeholders. Provenance should record the source hash and copied output hash.

### Multi-pass placeholders

Some generated files intentionally contain placeholder syntax for a later tool, runtime, deployment system, or user edit. The descriptor must make this explicit rather than relying on renderer-specific escaping tricks:

```toml
[[templates.preserve_placeholders]]
syntax = "double-brace"
names = ["runtime_schema", "deployment_env"]
```

Preserved placeholders are copied through as literal text and are not treated as missing Incan template parameters. This is separate from `_copy_without_render`: a rendered file may still preserve selected placeholder regions for another phase.

### Rendering algorithm

Rendering one template must follow this order:

1. Load the template descriptor.
2. Normalize and validate the source path relative to the descriptor package or descriptor root.
3. Normalize and validate the target path relative to the project root.
4. Parse the template body for placeholders.
5. Validate that every placeholder references a declared parameter.
6. Resolve parameter values from descriptor defaults, project metadata, caller-supplied values, derived values, or the calling lifecycle operation.
7. Validate each value against its declared parameter kind, choices, required state, and sensitivity policy.
8. Resolve optional file-group inclusion before rendering or copying.
9. Substitute placeholders in memory, preserving explicitly declared later-phase placeholders.
10. Validate the rendered output according to the declared target language.
11. Format the rendered output when a formatter is available and formatting is enabled.
12. Return a planned rendered file to the calling lifecycle operation.

The renderer does not decide whether the file may be written. File creation, merge, replace, and overwrite policy belong to the calling lifecycle operation, such as RFC 015 project creation or RFC 075 capability application.

### Incan validation

If `language = "incan"`, the rendered output must parse as Incan source before it is written. If the parser rejects the rendered output, the lifecycle command must fail without writing the target file.

If an Incan formatter is available, the lifecycle command should format the rendered output before computing `rendered_hash` and before writing. Hashes must be computed over the exact bytes that would be written.

This RFC does not require full type checking before writing a rendered file because a template may depend on files that are created by the same mutation plan. The calling lifecycle operation may typecheck the whole project after applying a plan when appropriate.

### Non-Incan targets

Templates may target non-Incan files such as Markdown, TOML, JSON, GitHub Actions YAML, or editor settings. Non-Incan targets must still use safe path handling and deterministic rendering.

Where the toolchain has a structured parser for a target format, it should validate rendered output before writing. If no parser exists, the descriptor must declare `language = "text"` or another known unchecked language mode so users and tools can see that only textual rendering was performed.

### Path safety

Source paths are resolved relative to the descriptor root or package root. Target paths are resolved relative to the project root.

The renderer must reject absolute target paths, parent-directory escapes, symlink escapes when detectable, control characters, and platform-specific path forms that would escape the project root.

Templates must not write outside the target project.

### Generated-file ownership

Each rendered file may declare one ownership policy:

- `bootstrap`: create the file during initial application, record provenance if requested, but do not update it through normal template update commands
- `managed`: create the file and consider it eligible for explicit template updates
- `advisory`: record origin metadata for explanation, but never treat the file as owned by update tooling

`bootstrap` is appropriate for files that users are expected to customize immediately, such as application-specific source files, domain models, local configuration examples, and initial docs content.

`managed` is appropriate for files that should follow template evolution, such as generated tool configuration, shared helper files, CI boilerplate, or formatter/linter configuration.

`advisory` is appropriate when a tool wants to explain origin without assuming future ownership.

The ownership policy must be independent from whether provenance is recorded. A bootstrap file may still record provenance so tooling can explain where it came from, but `incan template update` must not rewrite it by default.

### Provenance modes

Descriptors may choose one of these provenance modes:

- `none`: do not record template provenance
- `tracked`: record template provenance and rendered hashes
- `tracked-owned`: record provenance and treat unchanged generated output as tool-owned for update purposes

`tracked-owned` is retained as a shorthand for `provenance = "tracked"` with `ownership = "managed"` while the exact descriptor encoding is finalized. It does not make files read-only. It means update tooling may replace the file without a content merge only while the current file still matches the recorded `rendered_hash`, and only as part of an explicit receiver-approved update operation. Once a user edits the file, update tooling must treat it as user-owned and require conflict handling.

The default provenance mode is `tracked` for files produced by starter or capability operations and `none` for one-off explicit rendering commands, unless the command documents a different default.

### Provenance fields

When provenance is recorded, the project record must include:

- target path
- source kind and source identity
- source identifier or source path
- renderer id
- target language
- generated-file ownership policy
- template hash
- rendered hash

When available, the record should also include:

- package name and version
- registry or descriptor source
- immutable source version, git commit, descriptor version, or content hash
- publisher or provider identity
- integrity, signature, yanking, revocation, or trust-tier metadata
- starter or capability origin
- values hash
- Incan version used by the renderer

The record must not store secrets. If parameter values may include secrets, the provenance record must store only a values hash and enough non-secret metadata to repeat or explain the render.

### Template commands

This RFC adds the following lifecycle tooling concepts:

- `incan template check`
- `incan template status`
- `incan template values-file <template-or-origin>`
- `incan template render <template-or-origin>`
- `incan template diff [target]`
- `incan template update [target]`
- `incan template reset [target]`

The exact command spelling may be adjusted if the lifecycle CLI uses a different namespace, but the toolchain must support these operations before tracked provenance can be considered complete.

`check` reports whether tracked files are missing, current, edited, stale, or blocked by missing source metadata.

`status` reports version-aware source status when package or registry metadata is available, including whether tracked templates are on the latest compatible source version.

`values-file` writes a reviewable values file for a template, starter, capability, or provenance origin.

`render` renders one template from supplied values and returns a dry-run or planned file output. It is primarily useful for debugging descriptors and provider validation. The rendered plan must include reason codes for included, skipped, blocked, unchanged, and unsafe files when those states apply.

`diff` renders the current template source with the recorded values and shows what would change for one file or for all tracked files. When the template is part of an applied RFC 075 capability, this file-level diff is a lower-level diagnostic; the normal user-facing upgrade review should happen through the capability-level diff and update plan.

`update` applies an explicit receiver-approved update only when conflicts are absent or resolved through documented flags. It must respect generated-file ownership: bootstrap files are not rewritten by normal updates, managed files may be updated when unchanged or cleanly merged, and advisory files are explained but not owned.

`reset` recreates managed files from the selected template source using preserved or supplied values. It must be explicit, dry-runnable, conflict-aware, and subject to the same receiver-side rendered-diff review as update.

All template inspection and mutation operations should support a machine-readable output mode. Template update and reset plans must expose machine-readable security-sensitive mutation categories, source identity changes, integrity or trust-state changes, and receiver-side rendered file changes.

### Provider validation

Template authors need to know whether a descriptor renders into a realistic project and whether a future update behaves correctly. The lifecycle layer should therefore make template validation possible without publishing the template first.

V1 should support at least descriptor validation and dry-run rendering against fixture values. A later implementation may add a full target-project harness that initializes a project, applies updates, runs local checks, and records archive diffs between template versions. This RFC does not require that full harness in v1, but descriptor and provenance design must not prevent it.

### Security and trust

Template rendering must not execute arbitrary code. It must not run shell commands, load dynamic plugins, fetch remote URLs, evaluate expressions, or inspect unrelated files in the project.

Templates distributed through packages inherit the package trust story. If package templates are distributed through `incan.pub`, registry integrity, checksum, yanking, and publisher identity rules belong to RFC 034 or later registry RFCs. Rendering remains local and must still validate paths and output.

Procedural extension points are especially risky for upgradeable templates. If a template suite needs environment-derived defaults, optional slices, or target cleanup, those concerns should be represented as typed parameters, descriptor conditions, and mutation-plan rules rather than hidden Python, shell, or plugin hooks. A later RFC may define sandboxed hooks, but v1 should assume hooks make provenance and updates less reliable.

The update path is part of the supply chain because templates may render executable source files, scripts, CI configuration, env definitions, or agent-facing guidance. A malicious upstream template update can therefore inject behavior into a project even when the renderer itself is deterministic and non-executable.

The receiver-side codebase is the security boundary. A template provider, package registry, catalog, or automated refresh tool may propose rendered changes, but it must not be treated as having authority to mutate the receiving project. The receiving project owns the decision to accept, edit, reject, or defer the patch.

Reviewing the template source diff is not sufficient. Update tooling must show the exact rendered diff that would land in the receiving project, including generated source, scripts, CI/config files, env definitions, manifest changes, and agent-facing metadata. Template-source diffs are useful supporting evidence, but the rendered project diff is the artifact that must be reviewed.

Template provenance must pin the selected source strongly enough to detect unexpected changes. For git sources, provenance should record an immutable commit, not only a branch or tag. For package or catalog sources, provenance should record the package or descriptor version, source identity, and content hash or verified integrity metadata when available. A later update must surface changes to source identity, publisher identity, integrity state, yanking state, or trust tier before showing file diffs.

`incan template update` must default to a dry-run review path for security-sensitive changes. At minimum, the update plan must call out newly created files, executable source changes, manifest dependency changes, script or task changes, CI/configuration changes, env changes, and agent guidance metadata changes. Non-interactive update modes must still emit this information in machine-readable output and should require an explicit flag for source identity changes.

If two template sources can satisfy the same id, update tooling must not silently switch sources. This is especially important when private and public catalogs both contain a matching id. The lifecycle CLI must either preserve the recorded source identity or require explicit user intent to change it.

Implementations should support project or organization policy controls for template updates. Useful controls include requiring immutable source pins for non-built-in templates, restricting allowed catalog sources, rejecting mutable git refs for managed files, requiring explicit approval for source identity changes, and classifying high-risk file categories in machine-readable output so code review tooling can request the right owners.

Automation that keeps templates current should propose reviewable changes rather than applying them silently. A Dependabot-like flow is appropriate: detect stale or vulnerable template sources, produce a dry-run plan or pull-request-sized patch, include source identity and integrity changes, and leave final approval to normal project review.

Automation that proposes a template update must not also satisfy the receiver's review gate. If a project requires approval for source files, scripts, CI configuration, env definitions, or agent guidance, the approving identity must be independent from the update producer. This prevents a compromised update process from both injecting and approving its own rendered code changes.

### Diagnostics

Template diagnostics must name:

- the template source
- the target path
- the placeholder or parameter involved, when applicable
- the parameter kind and invalid value shape, when applicable
- the output parser error, when target-language validation fails
- the provenance record involved, when check, diff, or update fails

Diagnostics should distinguish template rendering failures from file mutation conflicts. A syntactically invalid rendered Incan file is a rendering failure. An existing target file is a mutation conflict owned by the lifecycle operation that requested the render.

## Design details

### Relationship to RFC 015

RFC 015 owns basic project creation and initialization. This RFC provides the rendering primitive those commands may use when they need more than a fixed built-in source file.

The RFC 015 default scaffold may continue to be implemented without this RFC. Once template support exists, the default scaffold may be represented internally as templates, but that is an implementation choice.

### Relationship to RFC 075

RFC 075 depends on this RFC for file templates used by starters and capability packs. Starter and capability descriptors may reference templates, parameter values, and provenance modes, but the rendering semantics come from this RFC.

This split keeps the layers clear: RFC 074 answers "how does a static template become a validated project file and how do we track it later?" RFC 075 answers "which project-level recipes and capability mutations should use those rendered files?"

For versioned project concerns such as "move the `cli` capability from `1.3.0` to `1.6.0`", RFC 075 owns the user-facing capability update. This RFC owns the file-level mechanics used inside that plan: preserved values, rendered output, generated-file ownership, drift detection, managed-file replacement, edited-file conflicts, and reset behavior. A template update command is therefore an escape hatch or diagnostic tool, not the primary model for upgrading an applied capability.

### Relationship to `ctx`

Templates that need compiler-visible configuration should generate ordinary Incan `ctx` declarations or source files that use `ctx` declarations. They should not invent a sidecar configuration system for values that the compiler should understand.

### Relationship to catalogs and `incan.pub`

`incan.pub` is a natural public distribution point for package-owned templates because it already owns package identity and versioning in RFC 034. This RFC does not require registry support. It only requires that templates loaded from packages, registries, git sources, local paths, or private catalogs be rendered locally with the same safety rules as built-in templates.

When registry or catalog metadata is available, it can make template status and upgrade previews much more useful: it can expose latest compatible versions, yanked sources, integrity metadata, and precomputed source diffs between template versions. The catalog layer may help with discovery and trust. The local lifecycle CLI still owns project-specific planning, rendering, validation, conflict handling, and file mutation.

### Relationship to RFC 076

RFC 076 owns project and organization policy for receiver-side mutation approval, source allowlists, risk classification, advisory handling, quarantine, and recovery workflows. This RFC defines the template render plan and provenance data that RFC 076 policy evaluates.

### Relationship to tooling and agents

IDE integrations, docs tooling, and agentic workflows may inspect template provenance to explain generated files, offer update code actions, or select relevant project guidance. They must not infer template semantics from filenames or reimplement rendering independently. The lifecycle tooling remains the source of truth for check, diff, update, rendering validation, and conflict diagnostics.

Prompt metadata, choices, optional file groups, and preserved placeholders are also tooling data. IDEs may use them to build forms or preview plans, but they should call the lifecycle layer to validate the final parameter set and render plan.

### Compatibility and migration

This RFC is additive. Existing projects without template provenance continue to build normally.

Projects that already contain copied example files may choose to adopt provenance later, but tooling must not assume a file is generated merely because it matches a known template by content. Provenance must be explicit.

If an early implementation used only `.tpl` filename conventions, it can migrate by generating descriptors that record the source, target, language, and parameter declarations explicitly.

When adopting templates into an existing project, tooling should default to preserving existing user-authored files. Initializing an existing directory is not the same operation as creating a new project from an empty directory.

## Alternatives considered

### Use arbitrary generator scripts

Rejected because scripts are powerful but hard to audit, sandbox, reproduce, diff, and explain. Static templates with typed parameters cover the common scaffolding case with a smaller trust boundary.

### Create everything and delete disabled files

Rejected because post-generation cleanup obscures intent. Optional files should be excluded from the mutation plan before rendering. That gives users an honest dry run and gives provenance enough information to distinguish "not selected" from "selected and later deleted."

### Make `.tpl` extension semantics authoritative

Rejected because filename conventions are too weak for tooling. A file extension cannot safely declare parameter kinds, target paths, target language validation, provenance behavior, or update policy.

### Adopt a full template language in v1

Rejected for scope. Loops, conditionals, includes, filters, and expression evaluation may become useful later, but they should be added only after the minimal rendering and provenance model proves insufficient.

### Do not record provenance

Rejected because initial file creation is only half the problem. Without provenance, tooling cannot explain generated files, check drift, or offer safe updates.

### Treat all generated files as managed

Rejected because generated files are not all the same. Some files are boilerplate that should keep receiving template updates; others are starter content meant to be edited by the user immediately. A single "generated" category causes either missed updates or accidental overwrites.

### Store generated files as hidden build artifacts

Rejected because generated project boilerplate should be reviewable, editable, and source controlled. The generated files are part of the user's project, not hidden compiler inputs.

## Drawbacks

- A constrained template model may feel too small for complex project generation.
- Provenance introduces another piece of tooling metadata that can drift if users edit files manually.
- Hash-based tracking cannot understand user intent; it can only detect byte changes.
- Typed parameters require descriptor authors to be more explicit.
- Descriptor-level optional file groups are less flexible than arbitrary post-generation hooks.
- Template check, status, values-file, diff, update, and reset add lifecycle CLI surface area that must be maintained.
- File ownership policies create another concept users must understand when reviewing generated artifacts.

## Layers affected

- **Parser:** rendered Incan templates must parse before they are written when `language = "incan"`.
- **Formatter:** rendered Incan templates should be formatted before write and before `rendered_hash` is recorded when a formatter is available.
- **Manifest schema / configuration validation:** project tooling must support an explicit provenance location for tracked templates, whether in `incan.toml`, a sidecar state file, or a future lock/state artifact.
- **CLI / tooling:** lifecycle tooling must implement deterministic template rendering, path validation, parameter validation, ownership handling, provenance recording, and check/status/values-file/diff/update/reset operations.
- **LSP / IDE tooling:** editor-facing tools should consume machine-readable template provenance and lifecycle diagnostics rather than reimplementing template rendering.
- **Package integration:** package-provided templates must be loaded as package data and rendered locally under the same safety rules as built-in templates. Package or registry metadata may feed version-aware status and upgrade previews.
- **Documentation:** user docs must explain the difference between provider-side templates, rendered project files, bootstrap files, managed files, and provenance records.

## Unresolved questions

- Should template provenance live in `incan.toml`, `incan.lock`, or a separate tool-owned state file?
- Is the v1 parameter kind set sufficient, or should it include specific kinds for package names, dependency requirements, env names, and script ids?
- Which derived-value transforms should be standardized in v1?
- Should optional file groups live in RFC 074 as template renderer metadata, or should RFC 075 own them as starter/capability mutation metadata?
- Should generated-file ownership be encoded as a separate `ownership` field, as part of provenance mode, or as part of RFC 075 file mutation metadata?
- Should descriptors reject declared-but-unused parameters by default, or should unused parameters be permitted for descriptor reuse?
- Should `tracked` or `none` be the default provenance mode for file templates applied by starter and capability workflows?
- What merge strategy, if any, should `incan template update` support for edited tracked files in v1?
- Should `incan template diff` produce a stable machine-readable patch format in v1, or is human-readable diff output enough initially?
- What is the minimum useful `incan template status` behavior without registry access?
- Should `incan template reset` be part of v1, or should it wait until normal update behavior has shipped?
- What provider validation command, if any, should be required before templates can be published or promoted in `incan.pub`?
- Should `.incn.tpl` be the recommended suffix for all Incan source templates, or should package authors prefer neutral names plus descriptor metadata?
- How should preserved later-phase placeholders be represented so they are explicit but not awkward for common deployment/configuration tools?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
