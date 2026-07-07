# RFC 078: tool execution and typed workflow actions

- **Status:** Draft
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 034 (`incan.pub` package registry)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 080 (AI assets and agent metadata)
- **Issue:** https://github.com/encero-systems/incan/issues/406
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a typed workflow-action and tool-execution model for Incan projects. It gives Incan the ergonomic center of gravity that npm scripts and uv tool execution provide, while avoiding arbitrary install-time mutation as the default: packages, starters, capabilities, and AI assets may advertise actions, but lifecycle tooling exposes them as typed, inspectable, policy-gated commands.

## Core model

Read this RFC as seven foundations:

1. **Actions are project workflow contracts:** an action describes a known project operation such as run, test, lint, format, generate, validate, publish, evaluate, or serve.
2. **Tools are executable providers:** a tool may come from a package, built-in command, local path, registry artifact, or AI asset runtime, but its source and permissions are explicit.
3. **Execution mode matters:** actions distinguish project-context execution from isolated one-off tool execution.
4. **No hidden lifecycle hooks:** installing a package or resolving a dependency must not run arbitrary project mutation hooks.
5. **Policy gates risk:** actions that mutate files, access the network, use models, or run external tools are classified for RFC 076 policy.
6. **Workspaces are scoped:** actions must declare whether they run for one member, selected members, default members, or the whole workspace.
7. **Machine-readable action metadata is first-class:** CLI, LSP, IDEs, docs tooling, and agents consume the same action model.

## Motivation

npm became central to JavaScript partly because `npm run` gave every project a common task surface. uv improved Python workflows by making project commands and ephemeral tools fast and unified. Incan should have a similarly ergonomic workflow layer, but should avoid making arbitrary shell hooks the primary integration mechanism.

RFC 015 already has envs and scripts. RFC 075 lets capabilities advertise tooling actions. This RFC turns those pieces into a coherent model: actions have kinds, inputs, outputs, scope, risk labels, source identity, and execution mode. That gives users a predictable command loop while giving policy, IDEs, and agents enough structure to reason about what a command will do.

## Goals

- Define typed workflow actions for common project operations.
- Define tool execution modes for project-context tools and isolated one-off tools.
- Allow packages, capabilities, starters, workspaces, and AI assets to advertise actions without executing them implicitly.
- Classify action risk for RFC 076 policy.
- Define action inputs, outputs, required envs, workspace scope, and mutation behavior.
- Define action capability requirements, optional capability requests, receipt-schema expectations, and dry-run metadata that can be compared with RFC 104 runtime receipts.
- Provide machine-readable action discovery for CLI, LSP, IDEs, docs tooling, and agents.
- Leave room for `incan.pub` to index tools and actions as ecosystem artifacts.

## Non-Goals

- Replacing general-purpose shells.
- Defining a remote build or distributed task execution service.
- Allowing package install or import to run arbitrary setup hooks.
- Defining exact syntax for every action kind in v1.
- Defining AI model execution semantics in full. RFC 080 owns AI assets and model metadata.
- Requiring every project script to be converted to a typed action immediately.

## Guide-level explanation

### Declaring actions

A project or capability may declare typed actions:

```toml
[[tool.incan.actions]]
id = "test"
kind = "test"
command = ["incan", "test"]
scope = "member"
mutates = []

[[tool.incan.actions]]
id = "generate-client"
kind = "generate"
tool = "pub:openapi-client"
inputs = ["openapi.yaml"]
outputs = ["src/generated/client.incn"]
mutates = ["source"]
requires-review = true
```

The exact encoding is not normative in this Draft. The key requirement is that actions are inspectable before they run.

### Running project actions

A user can run a project action:

```text
incan action run test
incan action run generate-client --dry-run
```

The dry run shows the tool source, inputs, outputs, workspace scope, env, mutation risk categories, and policy outcome.

For capability-aware actions, the dry run should also show required capabilities, optional capabilities, expected receipt kinds, redaction-sensitive fields, replay expectations, and whether any capability requirement comes from the action itself, a package descriptor, a capability pack, a tool source, or policy expansion.

### Running one-off tools

Incan may support isolated tool execution:

```text
incan tool run formatter@1.2.0 -- src/
incan tool run pub:docs-preview --port 8000
```

An isolated tool should not become a project dependency merely because it was executed once. If it mutates the project, the mutation must still be visible and policy-gated.

## Reference-level explanation

### Action kinds

V1 should standardize a small action-kind vocabulary:

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
- AI asset runtimes from RFC 080

The selected tool source, version, content hash, publisher, and trust metadata must be represented in machine-readable output when available.

### Inputs, outputs, and mutation

Actions should declare inputs and outputs when they can. Actions that mutate files must declare mutation categories compatible with RFC 076 risk categories.

If an action mutates source files, scripts, CI configuration, env definitions, manifests, generated-file ownership, or agent guidance, it must support dry-run or plan output unless the action kind is explicitly marked as non-plannable. Non-plannable mutating actions should require review by default.

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

A dry-run or plan for a capability-aware action must include the declared capability requirements, source of each requirement, policy outcome when evaluated, and whether the action is expected to emit a runtime report. After execution, the action's RFC 104 run report should be comparable with the dry-run plan. Undeclared runtime capability use, denied operations, unused broad grants, and redacted receipt attributes should be visible as machine-readable facts rather than only terminal text.

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

Starter and capability descriptors may advertise actions. Those actions must remain descriptive until a user or tool explicitly runs them.

RFC 075 owns how starter and capability mutation plans add, merge, or record action descriptors in project metadata. This RFC owns what those action descriptors mean once discovered: action kinds, execution modes, source resolution, risk classification, dry-run behavior, policy checks, and invocation.

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

The recommended implementation shape is to build an action registry from built-ins, project manifests, workspace manifests, package metadata, starter/capability descriptors, and AI assets. `incan action list` and IDE tooling consume this registry. `incan action run` resolves source, scope, env, policy, and execution mode before invoking anything.

## Layers affected

- **Manifest schema / configuration validation:** projects need typed action metadata, tool source references, execution modes, inputs, outputs, and mutation categories.
- **CLI / tooling:** commands need action listing, dry-run planning, isolated tool execution, source resolution, capability preview, receipt expectation display, and policy gating.
- **Workspace tooling:** actions must support member scope and workspace-level execution.
- **LSP / IDE tooling:** editor integrations should surface actions, run/debug affordances, policy outcomes, and mutation previews.
- **Package and catalog integration:** packages and catalogs may advertise tool binaries and action descriptors.
- **Runtime / reporting:** actions that execute through capability-aware runtimes should produce RFC 104-compatible report identities so declared authority and actual receipts can be compared.
- **Agentic tooling:** agents may consume action metadata, capability requirements, dry-run plans, and receipt expectations, but action execution remains subject to policy and user approval.
- **Documentation:** docs must distinguish scripts, actions, tools, project-context execution, and isolated execution.

## Unresolved questions

- Which action kinds should be standardized in v1?
- Should `incan run` and `incan test` become aliases for typed actions, or remain separate command families?
- What is the minimum useful dry-run contract for mutating external tools?
- Should isolated tool execution use a global cache, per-project cache, or always temporary environment?
- How should action inputs and outputs be represented for tools that cannot predict outputs?
- Should action metadata support secrets or credential requests, and if so how should policy mediate them?
- How should AI-backed actions expose model cost, privacy, and local/cloud execution constraints?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
