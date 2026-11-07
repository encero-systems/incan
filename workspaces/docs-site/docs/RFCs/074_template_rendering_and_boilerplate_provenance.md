# RFC 074: template rendering and boilerplate provenance

- **Status:** Planned
- **Created:** 2026-04-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 125 (`loaves.pub` Loaf registry; supersedes RFC 034)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
- **Issue:** <https://github.com/encero-systems/incan/issues/402>
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC defines a deterministic template rendering, generated-file ownership, and provenance model for Oven project tooling. Templates are static input files referenced by lifecycle tooling, starter profiles, capability packs, or package-provided setup data. Rendering is intentionally constrained: templates may substitute declared parameters, but they must not execute arbitrary code, run shell commands, fetch remote data, or infer behavior from filename conventions alone. When rendered files are written into a project, Oven may record provenance so users and tools can later check, diff, status, reset, or update generated boilerplate without treating the original template as hidden build state.

## Core model

Read this RFC as ten foundations:

1. **Template files are tooling inputs, not Incan source syntax.** A file such as `main.incn.template` is a conventional provider-side artifact. The rendered `main.incn` is the Incan source file users edit, compile, and commit.
2. **Descriptors are authoritative.** A template file's source path, target path, target language, parameter declarations, and provenance behavior are declared by a descriptor consumed by lifecycle tooling. The `.template` suffix is a convention, not the semantic contract.
3. **Rendering is deterministic.** Given the same template bytes, declared parameters, parameter values, renderer version, and project metadata, the renderer must produce the same output.
4. **Rendering is intentionally small.** Rendering supports declared placeholder substitution, a small set of pure derived-value transforms, and explicit literal regions. It does not support loops, conditionals, imports, functions, filters, arbitrary expression evaluation, network access, or shell execution.
5. **Parameters are declared and typed.** A template descriptor declares every accepted parameter and its kind. The renderer must reject undeclared placeholders and invalid values before writing files.
6. **Rendered Incan must be validated.** If the target language is `incan`, the toolchain must parse the rendered output and should format it before writing when a formatter is available.
7. **Provenance is explicit tooling state.** A project records which template produced a file, which package or descriptor supplied it, and which parameter values were used as authored intent in `loaf.toml`, and the hashes Oven observed as tool-owned state in `oven.provenance.toml`. Neither is resolver state in `oven.lock`.
8. **Updates are explicit, receiver-owned, conflict-aware, and security-sensitive.** Template provenance enables `check`, `diff`, and `update` workflows, but it must not cause automatic rewrites during build, test, import, editor startup, or package resolution. Updating generated source is a project code change in the receiving repository and must be reviewed as one.
9. **Generated files have ownership policy.** A rendered file carries RFC 075's per-file ownership policy: bootstrap-only, managed by explicit updates, or advisory. The policy is explicit per file rather than inferred from path conventions, and template commands honor it rather than defining a second one.
10. **Template inputs are reviewable.** Tooling must be able to show or write the parameter values that would drive a render so teams can review, automate, and reproduce template application.

## Motivation

RFC 015 gives Incan a minimal lifecycle CLI, and RFC 075 builds richer starter and capability workflows on top of it. Those workflows need a way to create files from reusable boilerplate. Without a template contract, each feature would either invent its own extension convention, embed ad-hoc generator scripts, or copy example files without enough metadata to explain later where they came from.

The problem is not just initial scaffolding. Generated boilerplate often needs maintenance. A project may start with a generated CLI entrypoint, `ctx` configuration module, data-session helper, or test skeleton, then edit it over time. Tooling should be able to answer practical questions: is this file still the generated output, was the upstream template changed, what diff would an update apply, and which capability introduced this file? Those questions require provenance, not just file creation.

This RFC keeps the boundary narrow. It does not create a general template programming language. It gives the lifecycle layer a safe rendering primitive that higher-level RFCs can depend on.

## Goals

- Define a project-tooling template model that can be used by lifecycle commands, starter profiles, capability packs, and package-provided setup data.
- Make descriptor metadata authoritative so behavior does not depend only on `.template` filename conventions.
- Define a constrained placeholder syntax and parameter model.
- Require deterministic rendering and safe path handling.
- Require rendered Incan output to parse before it is written.
- Define provenance records that support check, status, diff, update, and reset workflows.
- Record and honor RFC 075's generated-file ownership policies, which distinguish bootstrap-only files from managed files.
- Support real template suites that need prompts, defaults, constrained choices, checked domain parameter kinds, derived values, and non-rendered binary or fixture assets, rendering the file set RFC 075 selects.
- Update edited files through three-way merge proposals whose conflicts are resolved explicitly.
- Support values-file generation for non-interactive and reviewable template application.
- Support version-aware template status checks when package or registry metadata is available.
- Require template render plans to explain why files are included, skipped, blocked, unchanged, or unsafe.
- Define a safe reset escape hatch for corrupted or heavily conflicted generated files.
- Keep generated projects ordinary: rendered files are normal files, and templates do not participate in compilation unless the user explicitly keeps them as project data.
- Allow templates to be resolved from source-agnostic catalogs such as built-in descriptors, local paths, git sources, public package registries, or private catalogs while keeping rendering and mutation local to the lifecycle CLI.
- Let `loaves.pub` packages distribute public templates, rendered locally under the same rules as built-in ones.
- Provide a tooling surface that IDEs, docs tooling, and agents can inspect without reimplementing template semantics.

## Non-Goals

- Defining starter profiles, capability packs, or their command surface. RFC 075 owns that layer.
- Defining package registry transport, ranking, or remote discovery. RFC 125 owns package registry semantics, superseding RFC 034; RFC 117 owns Loaf project and dependency authority.
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
  session.incn.template
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
source = "templates/session.incn.template"
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

### Checked parameter values

A parameter's kind says what shape of text is valid where it lands. Escaping one generic string is not enough, because a type name, a module path and a package name each have their own grammar. With `config_type` declared as `kind = "identifier"`:

```text
config_type = "JobSettings"      -> model JobSettings:
config_type = "Bad; execute()"   -> rejected before any file is written:
                                    "Bad; execute()" is not an Incan identifier
```

A quoted string would not be an acceptable stand-in for a type name either. The same holds for the domain kinds: `package_name` and `dependency_requirement` are checked by the rules `loaf.toml` applies to dependencies, `env_name` and `script_id` by the rules for environments and scripts. A misspelled dependency is caught before rendering rather than when the project first resolves.

### Preserve placeholders for a later tool

A deployment file may contain placeholders that a later tool fills in. Mark them as a literal region so their bytes pass through unchanged:

```text
service: {{ service_name }}
{% literal %}
token: ${SERVICE_TOKEN}
next_template: "{{ LATER_TOOL_VALUE }}"
{% endliteral %}
```

Rendered with `service_name = "audit_tool"`:

```text
service: audit_tool
token: ${SERVICE_TOKEN}
next_template: "{{ LATER_TOOL_VALUE }}"
```

The renderer does not guess which unknown placeholders belong to another tool. An undeclared `{{ typo }}` outside a literal region is a diagnostic, so a misspelled parameter is never silently preserved.

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

Provenance is tracked by default. The lifecycle tool records two things. In `loaf.toml` it records what the project chose, which is reviewable intent:

```toml
[oven.templates."src/data/session.incn"]
source = "sample_session:templates/session.incn.template"
package = "sample_session"
version = "0.1.0"
origin = "capability:sample_session.session"

[oven.templates."src/data/session.incn".values]
config_module = "data.config"
config_type = "DataConfig"
backend_type = "DataFusion"
```

In `oven.provenance.toml`, a file Oven writes and the project commits next to `oven.lock`, it records the evidence it observed:

```toml
[templates."src/data/session.incn"]
renderer = "incan-template"
language = "incan"
ownership = "managed"
template_hash = "sha256:4b5a..."
values_hash = "sha256:42d9..."
rendered_hash = "sha256:18c2..."
```

Both are tooling provenance. Removing them must not break compilation. Keeping them lets the toolchain answer questions about generated files. Neither belongs in `oven.lock`, which stays resolver state rather than a history of edits.

### Generating a values file

A user may ask the toolchain to write the values a template expects before applying it:

```text
oven template values-file sample_session:templates/session.incn.template --output session.values.toml
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
oven template render sample_session:templates/session.incn.template --values session.values.toml --dry-run
```

The exact command spelling may change, but the lifecycle layer must expose the same capability: inspect required values, write a values file, and render from that file without interactive prompts.

### Checking generated files

A user may ask whether tracked files are stale or edited:

```text
oven template check
```

Example output:

```text
tracked templates:
  src/data/session.incn
    status: edited
    source: sample_session:templates/session.incn.template
    origin: capability:sample_session.session

  tests/test_session.incn
    status: current
    source: sample_session:templates/test_session.incn.template
```

`edited` means the current file hash differs from the recorded `rendered_hash`. It does not mean the file is invalid. Users are expected to edit generated files.

### Checking template status

When a template source comes from a package, starter, capability, or registry-backed catalog, users need more than byte-level drift. They also need to know whether the project is using the latest compatible template source:

```text
oven template status
```

Example output:

```text
target                   source                         current   latest    file
src/data/session.incn    sample_session:templates/session.incn    0.1.0     0.2.0     edited
tests/test_session.incn  sample_session:templates/test_session    0.1.0     0.1.0     current
```

Status works without registry access. Offline, it still compares every tracked file against its recorded hashes, so it can report an edited generated config on a train, and it reports the latest version as `unknown (offline)` or as a cached value with its age rather than guessing. Machine-readable status includes the provenance record path, source package, source version, latest compatible version or the reason it is unknown, and file drift state.

### Diffing an update

If a package version provides a newer template, tooling can show the proposed change without writing it:

```text
oven template diff src/data/session.incn
```

The diff is between the current project file and the output that would be rendered from the current descriptor and parameter values. If the file has user edits, the tool must make that clear before offering an update. `--format json` returns the same facts as a versioned structured patch, so an agent can decide which edits to accept without parsing terminal colors.

### Updating explicitly

Template updates are explicit:

```text
oven template update src/data/session.incn --dry-run
oven template update src/data/session.incn
```

If the current file still matches `rendered_hash`, an explicit receiver-approved update may replace it with the newly rendered output. If the file has user edits, the tool proposes a three-way merge: the old rendering is the base, the user's file is one side, and the new rendering is the other. Edits that do not overlap merge cleanly into the proposal. When the template and the user changed the same lines, the plan shows that conflict and leaves the file unchanged until it is resolved:

```text
src/data/session.incn
  status: edited
  merge: 1 conflict
    line 6, yours:    .with_backend(backends.Memory())
    line 6, template: .with_backend(backends.DataFusion(threads=4))
  action: not written; rerun with --write-conflicts to resolve in the file
```

The tool never overwrites user edits on its own.

### Resetting generated files

If an update cannot be applied cleanly, users may need a stronger operation that recreates managed files from the current template source while preserving recorded values:

```text
oven template reset src/data/session.incn --dry-run
oven template reset src/data/session.incn
```

Reset is not a normal update. It is an explicit recovery operation and never happens implicitly during update or repair. By default, it must preserve bootstrap-owned files, preserve values from provenance or an explicit values file, show a dry-run plan that lists the exact user edits it would replace, require confirmation unless running in an explicit non-interactive mode, and leave the resulting changes for review in source control. Reset is still a receiver-side code mutation and must follow the same rendered-diff review and source-identity checks as update.

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

The `.template` suffix is recommended for provider-side template files because it avoids confusing templates with directly compiled source. For Incan source templates, `.incn.template` is the recommended suffix, so editors can recognize them. Implementations must accept other filenames when the descriptor marks the file as a template, and must not compile a file merely because its name contains `.incn` or infer rendering policy from its suffix.

### Template source kinds

Template source kind affects discovery, provenance, trust, and upgrade metadata. It must not change rendering semantics. A template loaded from a private catalog, public registry, git reference, local path, package, or built-in source is rendered with the same placeholder validation, path safety, parse validation, overwrite rules, and provenance requirements.

Implementations must support built-in templates, explicit local descriptor paths, templates owned by Loaf packages from `loaves.pub`, git sources, and public and private catalog sources, all under the same descriptor semantics.

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

Placeholders use double braces around one parameter name:

```text
{{ parameter_name }}
```

Whitespace immediately inside the braces is ignored. Parameter names must be ASCII identifiers matching this shape:

```text
[A-Za-z_][A-Za-z0-9_]*
```

A literal region starts with `{% literal %}` and ends with the next `{% endliteral %}`. The renderer must copy the bytes between the delimiters unchanged, including any `{{ ... }}`, `{% ... %}` or `${...}` text, and must not treat them as placeholders. A delimiter that stands alone on its line is removed together with that line break; otherwise only the delimiter is removed. Literal regions do not nest.

The renderer must reject:

- placeholders outside a literal region that reference undeclared parameters
- malformed placeholders
- nested placeholders
- expression syntax inside placeholders
- an unmatched `{% literal %}` or `{% endliteral %}`, rather than treating the rest of the file as literal
- escaping or control syntax that is not defined by this RFC

A declared parameter that no placeholder or derivation uses must produce a warning, and an error under strict provider validation. A parameter declared with `allow_unused = true` suppresses that diagnostic, so several templates can share one parameter schema deliberately.

### Parameter kinds

The parameter kinds are:

- `identifier`: an Incan identifier segment
- `module_path`: one or more identifier segments separated by `.`
- `string`: a UTF-8 string value rendered with target-language string escaping when inserted into an Incan template
- `path`: a normalized relative project path that must not escape the project root
- `package_name`: a Loaf package name, valid under the `loaf.toml` rules for dependency names
- `dependency_requirement`: a dependency version requirement, valid under the `loaf.toml` rules for requirements
- `env_name`: an environment name, valid under the `loaf.toml` rules for environments
- `script_id`: a script identifier, valid under the `loaf.toml` rules for scripts
- `literal`: a raw textual value that is inserted without escaping

Every kind must be validated before any file is rendered or written. A kind describes the grammar of the position where the value lands, so escaping one generic string must not stand in for it. The domain kinds must reuse the validation that `loaf.toml` applies to the same field rather than define a parallel rule.

Descriptors should avoid `literal` unless the rendered target language validation can catch invalid output. For `language = "incan"`, use of `literal` is allowed only when the final rendered source parses successfully. For executable, configuration, CI, or agent-facing targets, `literal` parameters should be treated as security-sensitive in dry-run output because they bypass escaping.

Unknown parameter kinds must be rejected unless they belong to an extension namespace the renderer explicitly supports.

### Parameter declarations

A template descriptor may declare metadata for each parameter:

- `kind`: the validation and escaping kind
- `prompt`: a short human-facing prompt for interactive tools
- `default`: the default value, when one exists
- `choices`: a finite set of accepted values
- `required`: whether the caller must provide a value
- `sensitive`: whether the value may contain secret or private material
- `derived`: a deterministic derivation from other parameters or project metadata
- `allow_unused`: whether the parameter may go unused without a diagnostic

Prompt text is tooling metadata. It must not affect rendering semantics.

If `choices` is present, the renderer must reject values outside the declared set before substitution.

If `sensitive = true`, the renderer must not store the raw parameter value in provenance, logs, diagnostics, machine-readable plans, or generated comments. If drift detection requires a value fingerprint, the renderer should store a salted or keyed hash, not an unsalted hash of the raw value. Diagnostics should identify the parameter by name rather than printing the value.

A derived value must be declared as `derived = { from = "<parameter>", transform = "<name>" }`, naming one declared parameter or project metadata field and one built-in transform. The built-in pure transforms are:

- `snake_case`, `pascal_case`, `kebab_case` and `upper_snake_case`, which split words at non-alphanumeric characters and lower-to-upper transitions;
- `identifier`, which produces a valid Incan identifier by replacing invalid characters with `_` and prefixing `_` when the result would start with a digit;
- `module_path`, which applies `identifier` to each `.`-separated segment;
- `package_name`, which normalizes to the `loaf.toml` package-name form;
- `path`, which normalizes a relative path and rejects one that escapes the project root.

Transforms must be deterministic and independent of locale, and the derived value must still pass its own parameter kind. For example, `project_name = "Billing Service"` derives `module_name = "billing_service"` through `snake_case`, without running a script. Descriptors must not embed arbitrary expression evaluation, callbacks or commands to compute derived values.

### Selected file sets

The renderer renders the files it is given. Choosing which optional slices of a suite to include, such as docs, CI, test fixtures, backend-specific files or deployment scaffolding, belongs to RFC 075's starter and mix selection, which hands the renderer an explicit selected file set together with the reason each optional slice was included or skipped. The renderer must not select or drop files itself, and it must not rely on post-generation deletion to express optionality. Its plan must report each file it was given as rendered, unchanged, blocked or unsafe.

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

### Later-phase placeholders

Some generated files intentionally contain placeholder syntax for a later tool, runtime, deployment system, or user edit. The template must mark that text with a literal region, as defined under placeholder syntax, rather than rely on escaping tricks or a list of names to skip. The renderer must not infer from a placeholder's spelling, such as `${NAME}`, or from any semantic analysis, which renderer owns it. A literal region is separate from `render = false`: a rendered file may still preserve selected regions for another phase.

### Rendering algorithm

Rendering one template must follow this order:

1. Load the template descriptor.
2. Normalize and validate the source path relative to the descriptor package or descriptor root.
3. Normalize and validate the target path relative to the project root.
4. Parse the template body for placeholders and literal regions.
5. Validate that every placeholder outside a literal region references a declared parameter, and report unused parameters.
6. Resolve parameter values from descriptor defaults, project metadata, caller-supplied values, derived values, or the calling lifecycle operation.
7. Validate each value against its declared parameter kind, choices, required state, and sensitivity policy.
8. Confirm that the file belongs to the selected file set the calling operation supplied.
9. Substitute placeholders in memory, copying literal regions unchanged.
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

Ownership is RFC 075's per-file mutation policy, with the values `bootstrap` (created once and not rewritten by normal updates), `managed` (eligible for explicit updates) and `advisory` (explained but never owned by update tooling). A template descriptor's `ownership` key is that same field, not a second policy. Template provenance must record the file's ownership, and template commands must honor it: `update` and `reset` must not rewrite a bootstrap file by default and must never take ownership of an advisory file.

The ownership policy must be independent from whether provenance is recorded. A bootstrap file may still record provenance so tooling can explain where it came from. A recorded `rendered_hash` is a precondition fact for an update, never permission to replace a file that has changed.

### Provenance modes

Descriptors may choose one of these provenance modes:

- `tracked`: record template provenance and rendered hashes. This is the default for every file a lifecycle operation writes.
- `none`: do not record template provenance. Choosing it must be explicit, and the plan must state that `check`, `status`, `diff`, `update` and `reset` will not cover the file.

Provenance mode does not encode ownership. Even for a managed file, update tooling may replace the file without a merge only while it still matches the recorded `rendered_hash`, and only as part of an explicit receiver-approved update operation. Once a user edits the file, update tooling must use the merge and conflict rules below.

### Provenance fields

Provenance has two homes:

- **Intent in `loaf.toml`:** the `[oven.templates."<target>"]` table records the source, package and version, origin, and non-sensitive parameter values. It is authored project state that users review and may edit, such as changing a value before an update.
- **Evidence in `oven.provenance.toml`:** a file at the project root that only Oven writes and the project commits. Its `[templates]` table records, per target, the renderer, language, ownership, hashes, and receipts of what was observed; RFC 075 keeps mix evidence in the same file. Its content must remain inspectable through `oven template status --format json`.

`oven.lock` must not hold either; it remains resolver state and must not become a history of edits.

If the evidence file is missing or damaged, tooling must report the affected files as lacking tracking evidence and must treat them as edited, never as current. The merge base can be recovered by rendering the recorded template version with the recorded values, so losing the evidence file costs speed and certainty, not the ability to update. A sensitive value must then be supplied again.

When provenance is recorded, the two records together must include:

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

The records must not store secrets. A sensitive parameter's value must not appear in `loaf.toml` or the evidence file; the evidence file may store only its keyed fingerprint and enough non-secret metadata to repeat or explain the render.

### Template commands

This RFC adds the following lifecycle tooling concepts:

- `oven template check`
- `oven template status`
- `oven template values-file <template-or-origin>`
- `oven template render <template-or-origin>`
- `oven template diff [target]`
- `oven template update [target]`
- `oven template reset [target]`
- `oven template validate <descriptor>`

The exact command spelling may be adjusted if the lifecycle CLI uses a different namespace, but the toolchain must support these operations before tracked provenance can be considered complete.

`check` reports whether tracked files are missing, current, edited, conflicted, stale, lacking tracking evidence, or blocked by missing source metadata.

`status` must work without registry access. It must compare current files against locally recorded provenance and report each fact it could not refresh, such as the latest compatible version, as unknown or cached with its age rather than omit it. When package or registry metadata is available, it also reports whether tracked templates are on the latest compatible source version. Registry access must never be required for drift results.

`values-file` writes a reviewable values file for a template, starter, capability, or provenance origin.

`render` renders one template from supplied values and returns a dry-run or planned file output. It is primarily useful for debugging descriptors and provider validation. The rendered plan must include reason codes for rendered, unchanged, blocked, and unsafe files when those states apply, and must carry the selection reasons the calling operation supplied for included or skipped optional slices.

`diff` renders the current template source with the recorded values and shows what would change for one file or for all tracked files. When the template is part of an applied RFC 075 capability, this file-level diff is a lower-level diagnostic; the normal user-facing upgrade review should happen through the capability-level diff and update plan.

`update` applies an explicit receiver-approved update only when conflicts are absent or resolved. It must respect generated-file ownership: bootstrap files are not rewritten by normal updates, managed files may be updated when unchanged or cleanly merged, and advisory files are explained but not owned.

For an edited managed file, `update` must propose a three-way merge. The base is the recorded rendering, recovered by rendering the recorded template version with the recorded values; the two sides are the current file and the new rendering. Non-overlapping changes must merge into the proposal. Overlapping changes are conflicts: the plan must show the base, current and new text of each one, and `update` must not write that file. With `--write-conflicts`, it must write the file with conflict markers for the user to resolve, and `check` must report it as `conflicted` until the markers are gone. When no base can be recovered, for example because the old template version is unavailable or a sensitive value was not supplied, every difference in an edited file must be treated as a conflict. Update tooling must never overwrite user edits automatically.

`reset` recreates managed files from the selected template source using preserved or supplied values. It must be explicit and dry-runnable, must list the exact user edits it would replace before writing anything, must follow the confirmation policy, and is subject to the same receiver-side rendered-diff review as update. Update and repair operations must never reset a file implicitly.

`validate` performs provider validation, described below.

Every template inspection and mutation operation must support `--format json`, a versioned machine-readable form that carries the same facts as its human-readable output. For `diff`, that form is a structured patch an agent or review tool can apply or reject file by file. Template update and reset plans must expose machine-readable security-sensitive mutation categories, source identity changes, integrity or trust-state changes, and receiver-side rendered file changes.

Semantic tooling, such as Incan's code graph, may enrich a diff or merge proposal by explaining what changed, for example a renamed function. It must not decide whether a placeholder is rendered or preserved, resolve a conflict, or authorize an overwrite.

### Provider validation

Template authors need to know whether a descriptor renders into a realistic project and whether a future update behaves correctly, without publishing it first. `oven template validate` must check:

- the descriptor schema, parameter kinds and derived values;
- path safety of every source and target, so a descriptor that writes outside its declared destination is rejected;
- deterministic rendering, by rendering each template twice from the same values and comparing the bytes;
- fixture renders: each declared fixture value set must render and pass target-language validation.

With `--strict`, unused parameters without `allow_unused` are errors. The result must be reported in the versioned machine-readable form. Publishing a template package to `loaves.pub` must require a passing strict result under RFC 125, and promotion to a higher trust tier may require stronger policy evidence. `validate` may also run a target-project harness that initializes a fixture project, applies the template, applies updates between template versions, runs local checks, and records the rendered diffs.

### Security and trust

Template rendering must not execute arbitrary code. It must not run shell commands, load dynamic plugins, fetch remote URLs, evaluate expressions, or inspect unrelated files in the project.

Templates distributed through packages inherit the package trust story. If package templates are distributed through `loaves.pub`, registry integrity, checksums, signed state changes, and publisher identity rules belong to RFC 125. Rendering remains local and must still validate paths and output.

Procedural extension points are especially risky for upgradeable templates. If a template suite needs environment-derived defaults, optional slices, or target cleanup, those concerns should be represented as typed parameters, descriptor conditions, and mutation-plan rules rather than hidden Python, shell, or plugin hooks. Hooks, sandboxed or not, are not part of this design, because they make provenance and updates less reliable.

The update path is part of the supply chain because templates may render executable source files, scripts, CI configuration, env definitions, or agent-facing guidance. A malicious upstream template update can therefore inject behavior into a project even when the renderer itself is deterministic and non-executable.

The receiver-side codebase is the security boundary. A template provider, package registry, catalog, or automated refresh tool may propose rendered changes, but it must not be treated as having authority to mutate the receiving project. The receiving project owns the decision to accept, edit, reject, or defer the patch.

Reviewing the template source diff is not sufficient. Update tooling must show the exact rendered diff that would land in the receiving project, including generated source, scripts, CI/config files, env definitions, manifest changes, and agent-facing metadata. Template-source diffs are useful supporting evidence, but the rendered project diff is the artifact that must be reviewed.

Template provenance must pin the selected source strongly enough to detect unexpected changes. For git sources, provenance should record an immutable commit, not only a branch or tag. For package or catalog sources, provenance should record the package or descriptor version, source identity, and content hash or verified integrity metadata when available. A later update must surface changes to source identity, publisher identity, integrity state, yanking state, or trust tier before showing file diffs.

`oven template update` must default to a dry-run review path for security-sensitive changes. At minimum, the update plan must call out newly created files, executable source changes, manifest dependency changes, script or task changes, CI/configuration changes, env changes, and agent guidance metadata changes. Non-interactive update modes must still emit this information in machine-readable output and should require an explicit flag for source identity changes.

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

This split keeps the layers clear: RFC 074 answers "how does a static template become a validated project file and how do we track it later?" RFC 075 answers "which project-level recipes and capability mutations should use those rendered files?" RFC 075 therefore owns choosing optional file groups and the per-file ownership policy; this RFC renders the files it is handed and records their ownership in provenance.

For versioned project concerns such as "move the `cli` capability from `1.3.0` to `1.6.0`", RFC 075 owns the user-facing capability update. This RFC owns the file-level mechanics used inside that plan: preserved values, rendered output, generated-file ownership, drift detection, managed-file replacement, edited-file conflicts, and reset behavior. A template update command is therefore an escape hatch or diagnostic tool, not the primary model for upgrading an applied capability.

### Relationship to `ctx`

Templates that need compiler-visible configuration should generate ordinary Incan `ctx` declarations or source files that use `ctx` declarations. They should not invent a sidecar configuration system for values that the compiler should understand.

### Relationship to catalogs and `loaves.pub`

`loaves.pub` is a natural public distribution point for package-owned templates because it already owns Loaf package identity and versioning under RFC 125 and RFC 117. This RFC does not require registry support. It only requires that templates loaded from packages, registries, git sources, local paths, or private catalogs be rendered locally with the same safety rules as built-in templates.

When registry or catalog metadata is available, it can make template status and upgrade previews much more useful: it can expose latest compatible versions, yanked sources, integrity metadata, and precomputed source diffs between template versions. The catalog layer may help with discovery and trust. The local lifecycle CLI still owns project-specific planning, rendering, validation, conflict handling, and file mutation.

### Relationship to RFC 076

RFC 076 owns project and organization policy for receiver-side mutation approval, source allowlists, risk classification, advisory handling, quarantine, and recovery workflows. This RFC defines the template render plan and provenance data that RFC 076 policy evaluates.

### Relationship to tooling and agents

IDE integrations, docs tooling, and agentic workflows may inspect template provenance to explain generated files, offer update code actions, or select relevant project guidance. They must not infer template semantics from filenames or reimplement rendering independently. The lifecycle tooling remains the source of truth for check, diff, update, rendering validation, and conflict diagnostics.

Prompt metadata, choices, derived values, and literal regions are also tooling data. IDEs may use them to build forms or preview plans, but they should call the lifecycle layer to validate the final parameter set and render plan.

### Compatibility and migration

This RFC is additive. Existing projects without template provenance continue to build normally.

Projects that already contain copied example files may choose to adopt provenance later, but tooling must not assume a file is generated merely because it matches a known template by content. Provenance must be explicit.

If an early implementation used only `.template` filename conventions, it can migrate by generating descriptors that record the source, target, language, and parameter declarations explicitly.

When adopting templates into an existing project, tooling should default to preserving existing user-authored files. Initializing an existing directory is not the same operation as creating a new project from an empty directory.

## Alternatives considered

### Use arbitrary generator scripts

Rejected because scripts are powerful but hard to audit, sandbox, reproduce, diff, and explain. Static templates with typed parameters cover the common scaffolding case with a smaller trust boundary.

### Create everything and delete disabled files

Rejected because post-generation cleanup obscures intent. Optional files are excluded by RFC 075's selection before rendering. That gives users an honest dry run and gives provenance enough information to distinguish "not selected" from "selected and later deleted."

### Let the renderer choose optional file groups

Rejected because it would duplicate the starter and mix selection policy RFC 075 already owns. The renderer receives the selected file set instead.

### Keep a renderer-specific ownership flag

Rejected because a file could then be user-owned under a mix's policy and silently replaceable under the template's. One per-file ownership policy, RFC 075's, applies to both.

### Guess which unknown placeholders belong to a later tool

Rejected because guessing is convenient only until a parameter is misspelled or two tools share a delimiter. Explicit literal regions say which renderer owns which text; neither a placeholder's spelling nor semantic analysis can recover that intent.

### Reject every unused parameter

Rejected as a default because it forbids one parameter schema shared across several templates. A warning catches typos, strict validation rejects them, and `allow_unused` marks deliberate sharing.

### Store provenance only in `loaf.toml`, or only in tool state

Rejected because the manifest would mix user intent with per-file hashes, while tool state alone would hide the choices a team needs to review. Intent goes in `loaf.toml` and evidence in `oven.provenance.toml`.

### Replace or refuse edited files on update

Rejected because replacing loses user work and refusing every edited file is cumbersome. A three-way merge proposal applies the changes that do not overlap and stops at each real conflict.

### Make `.template` extension semantics authoritative

Rejected because filename conventions are too weak for tooling. A file extension cannot safely declare parameter kinds, target paths, target language validation, provenance behavior, or update policy.

### Adopt a full template language

Rejected because loops, conditionals, includes, filters, and expression evaluation turn rendering into programming, which makes templates harder to audit, diff, and update safely. Declared parameters, derived values, literal regions, and RFC 075's file selection cover the scaffolding this design targets.

### Do not record provenance

Rejected because initial file creation is only half the problem. Without provenance, tooling cannot explain generated files, check drift, or offer safe updates.

### Treat all generated files as managed

Rejected because generated files are not all the same. Some files are boilerplate that should keep receiving template updates; others are starter content meant to be edited by the user immediately. A single "generated" category causes either missed updates or accidental overwrites.

### Store generated files as hidden build artifacts

Rejected because generated project boilerplate should be reviewable, editable, and source controlled. The generated files are part of the user's project, not hidden compiler inputs.

## Drawbacks

- A constrained template model may feel too small for complex project generation.
- Provenance introduces another piece of tooling metadata that can drift if users edit files manually.
- Hash-based tracking cannot understand user intent; it can only detect byte changes. Semantic tooling can explain changes but cannot decide them.
- Typed parameters require descriptor authors to be more explicit.
- Selecting optional files before rendering is less flexible than arbitrary post-generation hooks.
- Line-based three-way merges can report conflicts that a human would resolve trivially.
- Template check, status, values-file, render, diff, update, reset and validate add lifecycle CLI surface area that must be maintained.
- A committed `oven.provenance.toml` adds one more tool-owned file next to `loaf.toml` and `oven.lock`.
- File ownership policies create another concept users must understand when reviewing generated artifacts.

## Layers affected

- **Parser:** rendered Incan templates must parse before they are written when `language = "incan"`.
- **Formatter:** rendered Incan templates should be formatted before write and before `rendered_hash` is recorded when a formatter is available.
- **Manifest schema / configuration validation:** `loaf.toml` gains the typed `[oven.templates]` intent table, and Oven writes the tool-owned `oven.provenance.toml` evidence file. `oven.lock` remains resolver state and must not become a general mutation-history store. Domain parameter kinds reuse the manifest's own validation rules.
- **CLI / tooling:** lifecycle tooling must implement deterministic template rendering, literal regions, derived-value transforms, path validation, parameter validation, ownership handling, provenance recording, three-way merge proposals, and check/status/values-file/render/diff/update/reset/validate operations with versioned machine-readable output.
- **Registry:** `loaves.pub` publication requires a passing strict `oven template validate` result.
- **LSP / IDE tooling:** editor-facing tools should consume machine-readable template provenance and lifecycle diagnostics rather than reimplementing template rendering.
- **Package integration:** package-provided templates must be loaded as package data and rendered locally under the same safety rules as built-in templates. Package or registry metadata may feed version-aware status and upgrade previews.
- **Documentation:** user docs must explain the difference between provider-side templates, rendered project files, bootstrap files, managed files, and provenance records.

## Design Decisions

- Authored intent (source, version, origin and non-sensitive values) lives in `loaf.toml` under `[oven.templates]`; hashes and receipts live in the Oven-written, committed `oven.provenance.toml`. `oven.lock` holds neither. A missing evidence file makes files count as edited, and the merge base is recovered by re-rendering the recorded version and values.
- Parameter kinds add `package_name`, `dependency_requirement`, `env_name` and `script_id`, which reuse the `loaf.toml` validation for those fields. Every value is validated for the grammar of its position before anything is written; escaping a generic string never stands in for a kind.
- Derived values use a small pure transform set: casing, `identifier`, `module_path`, `package_name` and `path`. No callbacks, commands or expressions.
- RFC 075 selects optional file groups; this RFC renders the selected file set it is handed.
- Ownership is RFC 075's single per-file policy (`bootstrap`, `managed`, `advisory`); the descriptor's `ownership` key is that field, and provenance records it. The `tracked-owned` mode is removed.
- Unused parameters warn by default, are errors under `validate --strict`, and are allowed with `allow_unused = true`.
- Provenance is `tracked` by default for every written file; `none` is explicit and the plan states what it loses.
- Updates propose a three-way merge from the recorded rendering. Overlapping changes are conflicts that are shown and never written, unless `--write-conflicts` writes conflict markers. Edits are never overwritten automatically.
- `diff` and every other template command have a versioned `--format json` form carrying the same facts as the human output.
- `status` works offline from local provenance and reports what it could not refresh as unknown or cached.
- `reset` is an explicit, dry-runnable plan listing the exact edits it replaces, under confirmation policy; it never happens implicitly.
- `oven template validate` checks schema, path safety, deterministic rendering and fixture renders. A passing strict result is required to publish to `loaves.pub`; promotion may require more evidence.
- `.incn.template` is the recommended suffix for Incan source templates. Descriptors stay authoritative and other names are allowed.
- Later-phase placeholders are preserved only inside explicit `{% literal %}` … `{% endliteral %}` regions. Unknown placeholders elsewhere are diagnostics, and semantic tooling may explain changes but never decides rendering, resolves conflicts or authorizes overwrites.
