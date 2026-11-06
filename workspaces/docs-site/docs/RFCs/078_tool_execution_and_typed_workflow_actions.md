# RFC 078: tool execution and typed workflow actions

- **Status:** Planned
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 036 (user-defined decorators)
    - RFC 075 (starter profiles and mixes)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 079 (`loaves.pub` artifact graph)
    - RFC 080 (declared AI assets)
    - RFC 090 (typed CLI framework)
    - RFC 104 (capabilities, limits and receipts)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 124 (Oven store unit identity)
    - RFC 125 (`loaves.pub` registry)
- **Issue:** https://github.com/encero-systems/incan/issues/406
- **RFC PR:** —
- **Written against:** ~~v0.3~~ v0.5
- **Shipped in:** —

## Summary

This RFC defines a typed workflow-action and tool-execution model for Loaf projects. It gives Oven the inspectable lifecycle center of gravity that Hatch scripts, npm scripts and uv tool execution provide, while avoiding arbitrary install-time mutation as the default: packages, starters, mixes, and AI assets may advertise actions, but Oven exposes them as typed, inspectable, policy-gated commands. An action written in Incan is a function with `@action`, and `loaf.toml` lists what a project offers, including tools that are not written in Incan.

## Core model

Read this RFC as seven foundations:

1. **Actions are project workflow contracts:** an action describes a known project operation such as run, test, lint, format, generate, validate, publish, evaluate, or serve.
2. **Tools are executable providers:** a tool may come from a package, built-in command, local path, registry artifact, or a runtime from a higher-level framework, but its source and permissions are explicit.
3. **Execution mode matters:** actions distinguish project-context execution from isolated one-off tool execution.
4. **No hidden lifecycle hooks:** installing a package or resolving a dependency must not run arbitrary project mutation hooks.
5. **Policy gates risk:** actions that mutate files, access the network, use models, or run external tools are classified for RFC 076 policy.
6. **Workspaces are scoped:** actions must declare whether they run for one member, selected members, default members, or the whole workspace.
7. **Machine-readable action metadata is first-class:** CLI, LSP, IDEs, docs tooling, and agents consume the same action model.

## Motivation

Hatch gives a Python project named scripts inside its environments, and that is where this design started. npm became central to JavaScript partly because `npm run` gave every project a common task surface. uv improved Python workflows by making project commands and ephemeral tools fast and unified. Incan should have a similarly ergonomic workflow layer, but should avoid making arbitrary shell hooks the primary integration mechanism.

RFC 015 already has envs and scripts. RFC 075 lets mixes advertise tooling actions. This RFC turns those pieces into a coherent model: actions have kinds, inputs, outputs, scope, risk labels, source identity, and execution mode. That gives users a predictable command loop while giving policy, IDEs, and agents enough structure to reason about what a command will do.

## Goals

- Define typed workflow actions for common project operations.
- Define tool execution modes for project-context tools and isolated one-off tools.
- Let an action be written in Incan, with typed parameters and checked capabilities.
- Allow packages, mixes, starters, workspaces, and AI assets to advertise actions without executing them implicitly.
- Classify action risk for RFC 076 policy.
- Define action inputs, outputs, required envs, workspace scope, and mutation behavior.
- Define action capability requirements, optional capability requests, receipt-schema expectations, and dry-run metadata that can be compared with RFC 104 runtime receipts.
- Provide machine-readable action discovery for CLI, LSP, IDEs, docs tooling, and agents.
- Leave room for `loaves.pub` to index tools and actions as ecosystem artifacts.

## Non-Goals

- Replacing general-purpose shells.
- Defining a remote build or distributed task execution service.
- Allowing package install or import to run arbitrary setup hooks.
- Defining exact syntax for every action kind.
- Defining AI model execution semantics. RFC 080 owns declared AI assets, and a higher-level framework owns runtimes.
- Defining no-std/freestanding target profiles, kernel support, unsafe/layout controls, panic strategy, or allocator strategy. Action capability and receipt metadata may inform those RFCs, but this RFC is not the freestanding/kernel RFC.
- Requiring every project script to be converted to a typed action.

## Guide-level explanation

### Declaring actions

An action written in Incan is a function:

```incan
# tools/client.incn
from std.actions import ActionKind, action
from std.fs import Path

@action(kind=ActionKind.Generate, caps=[host.fs.write], outputs=["src/generated/client.incn"])
def generate_client(spec: Path = Path("openapi.yaml")) -> Result[None, GenerateError]:
    ...
```

`loaf.toml` lists what the project offers, including tools that are not written in Incan. A mix or package may advertise the same entries:

```toml
[actions.generate-client]
entry = "tools.client.generate_client"

[actions.format-sql]
kind = "format"
tool = "pub:sqlfmt"
mutates = ["source"]

[actions.lint-docs]
kind = "lint"
command = ["vale", "docs/"]
scope = "member"
```

An action is inspectable before it runs, whichever form it takes.

### Running project actions

A user can run a project action:

```text
oven action run lint-docs
oven action run generate-client --spec other.yaml
oven action run generate-client --dry-run
```

A function's parameters become flags through RFC 090's `std.cli`, so `--spec` is parsed and type-checked. The dry run shows the tool source, inputs, outputs, workspace scope, env, mutation risk categories, and policy outcome.

For actions with declared capabilities, the dry run should also show required capabilities, optional capabilities, expected receipt kinds, redaction-sensitive fields, replay expectations, and whether any capability requirement comes from the action itself, a package descriptor, a mix, a tool source, or policy expansion.

### Running one-off tools

Oven supports isolated tool execution:

```text
oven tool run formatter@1.2.0 -- src/
oven tool run pub:docs-preview --port 8000
```

An isolated tool should not become a project dependency merely because it was executed once. If it mutates the project, the mutation must still be visible and policy-gated.

## Reference-level explanation

### Actions written in Incan

`std.actions` must provide `action` and `ActionKind`. `action` must be an ordinary RFC 036 decorator factory that returns an action value, with no compiler knowledge of the name beyond what RFC 104 checks.

A function with `@action` must take parameters that `std.cli` can derive flags from, either individually or as one model, and must return `None`, a typed report model, or a `Result` of either. An `Err`, or a report that marks failure, must be a non-zero exit. `kind`, `caps`, `uses`, `inputs`, `outputs`, `mutates` and `scope` must have the same meaning as in `loaf.toml`, where `uses` names RFC 080 assets.

`caps=` must follow RFC 104: a compile error when it omits something the body uses, and a ceiling while the action runs. It is optional, and without it the action has neither.

`--dry-run` on an Incan action must report its declarations without running its body.

### Listing actions

`[actions.<id>]` in `loaf.toml` must name exactly one of `entry`, `tool` or `command`. `entry` must name a function with `@action`, and anything else must be a diagnostic. No decorator may register an action as a side effect, and an `@action` function no `loaf.toml` lists must not be runnable through `oven action run`. A project must be able to list its actions by id even when its code does not currently compile, with details it cannot determine reported as unavailable.

### Action kinds

A small action-kind vocabulary is standardized:

- `run`: run an application entrypoint
- `test`: run tests
- `lint`: report code or manifest problems
- `format`: format source or config files
- `generate`: create or update project files from declared inputs
- `validate`: check generated artifacts, descriptors, policies, or model metadata
- `serve`: run a local service or docs preview
- `publish`: prepare or publish a package or artifact
- `audit`: inspect dependencies, sources, policies, or advisories
- `eval`: run an AI or data evaluation workload

Implementations may add extension kinds, but unknown action kinds must remain visible in machine-readable output and should default to conservative policy handling.

### Execution modes

An action must declare or resolve to one execution mode:

- `project`: runs inside the selected project or workspace member context
- `isolated`: runs in a temporary environment independent of project dependencies
- `toolchain`: runs a built-in Incan toolchain operation
- `external`: runs an external executable with explicit source and policy metadata

Project-context actions may access project dependencies and envs. Isolated tools should not mutate project dependencies unless the user explicitly asks to add them.

### Tool sources

Tool sources may include:

- built-in toolchain commands
- package-provided binaries
- local paths
- git sources
- public catalog tools
- private catalog tools
- runtimes a higher-level framework provides

The selected tool source, version, content hash, publisher, and trust metadata must be represented in machine-readable output when available.

### Inputs, outputs, and mutation

Actions should declare inputs and outputs when they can. Actions that mutate files must declare mutation categories compatible with RFC 076 risk categories.

If an action mutates source files, scripts, CI configuration, env definitions, manifests, generated-file ownership, or agent guidance, it must support dry-run or plan output unless the action kind is explicitly marked as non-plannable. Non-plannable mutating actions must default to the `require-approval` outcome under RFC 076.

### Workspace scope

Workspace-aware actions must declare supported scopes:

- current member
- selected members
- default members
- all members
- workspace root

The selected scope must appear in diagnostics and machine-readable output.

### Policy and approval

Before running an action, lifecycle tooling must evaluate RFC 076 policy when the action is mutating, networked, external, model-backed, or security-sensitive.

`--yes` may satisfy local confirmation only when policy allows it. It must not satisfy independent approval requirements.

### Capabilities and receipts

Actions must be able to declare static capability expectations compatible with RFC 104. The action model should distinguish:

- required capabilities that must be granted before the action can run in governed mode;
- optional capabilities that the action may request only on particular paths;
- package- or tool-provided capability requirements inherited from the selected provider;
- domain capabilities that explain project-specific authority;
- host capabilities such as filesystem, environment, process, HTTP, model, or tool authority;
- expected receipt event kinds and redaction-sensitive attributes where the provider can declare them.

Action metadata does not grant authority. It describes expected authority so lifecycle tooling, policy, CI, LSP, docs, and agents can review the action before it runs. Runtime authority remains governed by RFC 104.

A dry-run or plan for an action with declared capabilities must include the declared capability requirements, source of each requirement, policy outcome when evaluated, and whether the action is expected to emit a runtime report. After execution, the action's RFC 104 run report should be comparable with the dry-run plan. Undeclared runtime capability use, denied operations, unused broad grants, and redacted receipt attributes should be visible as machine-readable facts rather than only terminal text.

### Machine-readable discovery

The CLI must expose a machine-readable action list containing:

- action id
- action kind
- provider and source identity
- execution mode
- required envs or toolchain constraints
- required and optional capabilities when known
- expected receipt event kinds and report behavior when known
- supported workspace scope
- inputs and outputs when known
- mutation categories
- policy outcome when evaluated in a project
- agent guidance metadata when relevant

## Design details

### Relationship to RFC 015

RFC 015 scripts and envs remain valid. This RFC layers typed actions above scripts so tooling can understand project workflows without parsing arbitrary shell commands.

### Relationship to RFC 075

Starter and mix descriptors may advertise actions. Those actions must remain descriptive until a user or tool explicitly runs them.

RFC 075 owns how starter and mix mutation plans add, merge, or record action descriptors in project metadata. This RFC owns what those action descriptors mean once discovered: action kinds, execution modes, source resolution, risk classification, dry-run behavior, policy checks, and invocation.

### Relationship to RFC 076

RFC 076 policy evaluates action risk, source identity, mutating behavior, network behavior, model use, and approval requirements.

### Relationship to RFC 104

RFC 104 owns runtime capability enforcement, receipt emission, and run reports. This RFC owns the static action metadata that predicts and explains what authority an action expects before it runs. The two schemas should use compatible action identities, capability identities, risk categories, artifact identities, redaction markers, and replay classifications so dry-runs and actual reports can be compared.

### Relationship to RFC 077

Workspace-aware actions must report member scope and avoid accidentally applying to more members than the user selected.

### Relationship to RFC 080

AI-backed actions such as prompt evaluation, model conversion, embedding generation, data-quality classification, or agent workflow execution should reference AI assets from RFC 080 rather than inventing ad-hoc model metadata.

## Alternatives considered

### Use arbitrary scripts only

Rejected because arbitrary scripts are ergonomic but opaque. Typed actions preserve script convenience while giving tools, policy, and agents structured metadata.

### Describe actions in TOML only

Rejected because actions would stay untyped, with strings for inputs, and RFC 104's compile check would have nothing to check.

### Discover actions from source, with no listing

Rejected because Oven would have to compile a project just to say what actions it has, and a broken project could not list them.

### Make every action a package dependency

Rejected because many tools are one-off or developer-only utilities. Isolated tool execution keeps projects cleaner.

### Run package hooks during install

Rejected because install-time hooks are difficult to review and policy-gate. Project mutation should be explicit.

## Drawbacks

- Typed actions add metadata authors must maintain.
- Some workflows will still need shell escape hatches.
- Dry-run support for mutating tools can be hard if the underlying tool does not support planning.
- Policy-gated action execution may feel heavier than simple scripts.

## Implementation architecture

The recommended implementation shape is to build an action registry from built-ins, `loaf.toml` project and workspace data, package metadata, starter and mix descriptors, and AI assets. `oven action list` and IDE tooling consume this registry. `oven action run` resolves source, inherited workspace scope, env, policy, target context, and execution mode before invoking anything. RFC 118 has no Incan alias for actions.

## Layers affected

- **Stdlib:** `std.actions`, with `action` and `ActionKind`, using `std.cli` for parameters.
- **Manifest schema / configuration validation:** `[actions.<id>]` needs `entry`, `tool` or `command`, execution modes, inputs, outputs, and mutation categories.
- **CLI / tooling:** commands need action listing, dry-run planning, isolated tool execution, source resolution, capability preview, receipt expectation display, and policy gating.
- **Workspace tooling:** actions must support hierarchical sub-Loaf selection within one inherited root-workspace authority and must show the selected closure in plans and receipts.
- **LSP / IDE tooling:** editor integrations should surface actions, run/debug affordances, policy outcomes, and mutation previews.
- **Package and catalog integration:** packages and catalogs may advertise tool binaries and action descriptors.
- **Runtime / reporting:** actions should produce RFC 104-compatible report identities so declared authority and actual receipts can be compared.
- **Agentic tooling:** agents may consume action metadata, capability requirements, dry-run plans, and receipt expectations, but action execution remains subject to policy and user approval.
- **Documentation:** docs must distinguish scripts, actions, tools, project-context execution, and isolated execution.

## Design Decisions

- **Actions are written two ways, with one model.** A function with `@action` from `std.actions` gets typed parameters through `std.cli`, and RFC 104's `caps=` check and ceiling. `tool` and `command` describe what is not written in Incan. `[actions.<id>]` lists both, so nothing registers as a side effect and listing needs no compile.
- **Standard action kinds.** The ten kinds in the reference section are standardized. Extension kinds use an `x-` prefix, appear in machine-readable output, and receive conservative policy handling.
- **`oven run` and `oven test` are not actions.** `run`, `test`, `build`, `bake`, and `check` remain first-class Oven command families with their own plan identities (RFC 118). Actions are the extension mechanism for everything else and are invoked through `oven action run <id>`. A project action may declare kind `run` or `test`, but it never redefines or shadows the built-in operation, so there is no npm-scripts-style override of `oven test`.
- **Minimum dry-run contract for mutating external tools.** The plan must show the tool's source identity, version, and content hash; execution mode; argv, working directory, and environment additions; declared mutation categories and target paths; capability requirements (RFC 104); and the policy outcome. A tool that cannot predict its outputs declares `outputs = "unpredictable"`, is non-plannable, requires approval by default, and runs in a staging directory whose post-run diff is presented as a mutation plan before anything is adopted into the project (RFC 112 for the atomic adoption).
- **Isolated tool execution uses the Oven store.** Tools are units under RFC 124, cached globally by unit identity and shared across projects; there is no per-project tool cache. `--fresh` requests a temporary environment for one run.
- **Inputs and outputs for unpredictable tools.** Declared as `outputs = "unpredictable"`; observed outputs are recorded in the receipt and surfaced as the post-run diff above. Tools that can predict outputs declare them and are checked against the receipt.
- **Secrets and credentials.** Action metadata may declare named credential requirements (kind and purpose), never values. Policy mediates each request; values are resolved from Oven-controlled secure storage (the credential references RFC 117 already keeps outside the project) as RFC 103 secret values; every request is receipted under RFC 104 with redaction.
- **AI-backed actions.** Metadata carries three fields, execution locality (`local` or `remote`), a cost class, and a privacy label, referencing RFC 080 assets where they exist; policy may block remote execution. The field shapes are fixed here; their detailed semantics remain RFC 080's.
