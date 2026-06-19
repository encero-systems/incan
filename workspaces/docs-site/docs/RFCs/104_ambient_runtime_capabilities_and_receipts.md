# RFC 104: Ambient Runtime Capabilities and Receipts

- **Status:** Draft
- **Created:** 2026-05-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 033 (`ctx` typed configuration context)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 089 (`std.environ` runtime environment access)
    - RFC 090 (typed CLI framework)
    - RFC 092 (interactive runtime stdlib contracts)
    - RFC 093 (`std.telemetry`)
    - RFC 094 (context managers)
    - RFC 095 (`span` vocabulary blocks)
    - RFC 102 (semantic layer inspection surface)
    - RFC 103 (secret values and redaction-safe values)
- **Issue:** https://github.com/encero-systems/incan/issues/662
- **RFC PR:** -
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines ambient runtime capabilities and receipts for Incan. Importing a module remains Python-readable and low ceremony, but using authority-bearing operations such as filesystem, environment, process, HTTP, clock, random, model, tool, or package-defined domain operations produces structured receipts and may be denied by a governed runtime. The stdlib is the first capability publisher, not the only one: library authors can define domain capabilities, attach receipt schemas, and participate in the same audit and policy system without reimplementing tracing or reaching for stdlib internals. The goal is ambient observation with explicit authority.

## Core model

Read this RFC as ten foundations:

1. **Import is not authority:** source code may import `std.fs`, `std.process`, `std.environ`, `std.http`, or a capability-aware package without automatically receiving permission to perform those operations.
2. **Observation is ambient:** ordinary stdlib and library calls can emit structured receipts without requiring users to annotate every function with effect types.
3. **Authority is granted at boundaries:** runs, actions, tests, packages, and hosts grant capabilities; library code may request or declare capabilities, but cannot grant itself authority.
4. **Stdlib capabilities are built in:** host authority such as filesystem, environment, process, network, clock, random, model invocation, and tool invocation has reserved capability identities.
5. **Library capabilities are first-class:** packages may publish domain capabilities such as `example.policy.evaluate` or `example.index.query` that describe domain authority and receipt semantics.
6. **Receipts are not logs:** receipts are structured runtime facts with stable kinds, source spans where available, redaction state, status, and replay information; terminal logs are only one possible view.
7. **Strict enforcement is optional:** ordinary runs should remain simple, while governed runs can deny operations not covered by granted capabilities.
8. **Redaction is mandatory:** receipts must preserve sensitivity metadata and must not expose raw secret or policy-sensitive values by default.
9. **Replay claims must be honest:** the runtime should describe what can be replayed exactly, what requires fixtures, and what cannot be replayed.
10. **Policy consumes receipts:** policy systems, CI, editors, docs tooling, and agents consume the same capability declarations and receipt facts; they do not infer authority from prose or hidden conventions.

## Motivation

Python-shaped source is a major Incan strength, but Python's module model also hides authority. If Python code can import `os`, it can generally attempt to read environment variables, inspect and mutate files, spawn processes, or discover host state. External sandboxing can restrict that, but the source/module surface does not make authority visible or explainable.

Incan should preserve the ergonomic part and reject the hidden-authority part. A user should be able to write ordinary readable code, import the modules they need, and run the program normally. When the same code is run in a governed context, the runtime should be able to say that a filesystem read, environment read, process spawn, HTTP request, model invocation, or package-defined domain operation was allowed, denied, redacted, or replay-limited.

This matters most for real tools, automation, generated artifacts, policy-bound workflows, and agent-assisted maintenance. A failed or suspicious run should produce receipts that answer what authority was requested, what authority was granted, what actually happened, which values were redacted, which artifacts were touched, and what can be replayed. Without a shared capability and receipt model, every stdlib module and library will invent its own logs, policy hooks, and audit JSON.

The key design constraint is usability. This RFC must not turn ordinary Incan into an algebraic-effect language where every helper function has capability algebra in its type signature. The default user experience should be: write normal Incan; capability-aware boundaries produce structured receipts; governed entrypoints can restrict and audit those receipts.

## Goals

- Split module availability from runtime authority.
- Define reserved host capability identities for common authority-bearing operations.
- Allow library authors to define domain capabilities and receipt schemas.
- Define ambient receipt emission for stdlib and library boundaries.
- Define governed runtime behavior when an operation requires a capability that was not granted.
- Define machine-readable run reports that include requested capabilities, granted capabilities, denied operations, emitted receipts, redaction state, and replay limits.
- Define how domain capabilities may imply or request host capabilities without granting themselves authority.
- Make receipts consumable by RFC 102 semantic inspection, RFC 078 typed actions, RFC 093 telemetry, RFC 076 policy, CI, LSP, docs tooling, and agents.
- Align typed action dry-runs and runtime reports so declared capability requirements can be compared with actual receipt emission.
- Keep ordinary source readable and low ceremony.

## Non-Goals

- This RFC does not introduce a full algebraic effect system.
- This RFC does not require every function type to include a capability parameter or effect row.
- This RFC does not make imports fail merely because the current run has not granted a capability.
- This RFC does not define a complete operating-system sandbox.
- This RFC does not define no-std/freestanding target profiles, kernel support, unsafe/layout controls, panic strategy, or allocator strategy. Capability and receipt metadata may inform those later RFCs, but this RFC is not the freestanding/kernel RFC.
- This RFC does not guarantee perfect deterministic replay for external systems.
- This RFC does not replace `std.telemetry`, `std.logging`, diagnostics, or semantic inspection.
- This RFC does not require every package to publish capability metadata.
- This RFC does not allow libraries to grant themselves host authority.
- This RFC does not define the final CLI flag spelling for governed runs or reports.
- This RFC does not define a secret-value type; it only requires receipts to preserve sensitivity and redaction metadata from the owning subsystem.

## Guide-level explanation

Ordinary code should stay ordinary:

```incan
from std.environ import env
from std.http import get

def fetch_status() -> int:
    url = env.get("STATUS_URL")?
    response = get(url)?
    return response.status.code
```

A normal run may behave just like a normal program:

```text
incan run status.incn
```

An observed run asks the runtime to emit a machine-readable report:

```text
incan run status.incn --report json
```

The report can show the authority-bearing operations that happened:

```json
{
  "entrypoint": "status.fetch_status",
  "granted_capabilities": [],
  "mode": "observe",
  "receipts": [
    {
      "capability": "host.env.read",
      "operation": "env.get",
      "status": "observed",
      "attributes": {"key": "STATUS_URL"},
      "redacted": false
    },
    {
      "capability": "host.http.request",
      "operation": "http.request",
      "status": "observed",
      "attributes": {"method": "GET", "url_policy": "external", "status_code": 200},
      "redacted": false
    }
  ]
}
```

A governed run grants only selected authority:

```text
incan run status.incn --allow host.env.read,host.http.request --report json
```

If the program later tries to spawn a process, the runtime should fail with a useful diagnostic:

```text
status.incn:8 used std.process.Command.run(...)
This requires capability: host.process.spawn

Granted capabilities:
  host.env.read
  host.http.request
```

Library authors should be able to participate without depending on stdlib-private hooks. A package can define a domain capability:

```incan
capability example.policy.evaluate:
    description = "Evaluate an input against a policy"
    emits = "policy.evaluation"
```

The exact declaration syntax is unresolved. The important contract is that packages can publish stable capability identities, descriptions, receipt schemas, and relationships to host capabilities.

Library code can then emit a receipt through a low-ceremony boundary:

```incan
from std.runtime import receipts

def evaluate(policy: Policy, input: Input) -> Decision:
    with receipts.event("example.policy.evaluate", subject=policy.id):
        return policy.evaluate(input)
```

For common entrypoints, typed actions can declare the capabilities they require:

```incan
@action(caps=["example.policy.evaluate", "host.model.invoke"])
def review(input: ReviewInput) -> ReviewReport:
    ...
```

Granting a domain capability does not automatically let a package bypass host policy. If `example.policy.evaluate` needs `host.fs.read` to load a policy file, that relationship must be visible in metadata and accepted by the runtime or host policy. Libraries can name and explain authority; the runtime grants authority.

## Reference-level explanation

### Capability identities

A capability identity must be a stable string. The exact naming grammar is unresolved, but this RFC reserves the `host.*` namespace for host authority capabilities owned by the Incan toolchain and runtime.

Initial host capability families should include:

- `host.env.read`
- `host.fs.read`
- `host.fs.write`
- `host.process.spawn`
- `host.http.request`
- `host.clock.read`
- `host.random`
- `host.model.invoke`
- `host.tool.invoke`

Implementations may define narrower capabilities such as scoped filesystem paths, hostnames, methods, or model families, but the broad families must remain understandable in diagnostics and reports.

Package-defined capabilities must be namespaced so two packages cannot accidentally define the same authority name. Package-defined capabilities may describe domain operations, typed actions, generated artifacts, policy checks, workflow steps, or library-specific effects.

### Import, request, grant, and use

Importing a module must not grant authority. Importing `std.process` is allowed even in a run that has not granted `host.process.spawn`. Authority is checked when an authority-bearing operation is invoked.

A package, action, function, descriptor, or runtime operation may request capabilities. A run, host, action invoker, test harness, package manager, CI environment, or policy system may grant capabilities. Only the runtime or host authority boundary may decide whether a request is granted.

When an operation requiring a capability is invoked in governed mode and the capability is not granted, the operation must fail before performing the authority-bearing behavior. The diagnostic must identify the required capability and should include the source span, import/module/function path, and a suggested grant spelling when available.

### Runtime modes

The runtime should support at least these conceptual modes:

- `permissive`: operations run normally and receipts may be disabled.
- `observe`: operations run normally and receipts are emitted.
- `governed`: operations require granted capabilities and receipts are emitted.

The exact CLI spelling is not normative. A natural user-facing shape is:

```text
incan run app.incn --report json
incan run app.incn --allow host.env.read,host.http.request --report json
```

The default mode for ordinary local development is unresolved. The default must not surprise users by silently exporting data or sending reports to remote services.

### Capability declarations

A capability declaration should include:

- stable identity;
- human-readable description;
- owning package or toolchain component;
- capability kind, such as host, library, action, artifact, or policy;
- optional implied or requested capabilities;
- optional scope schema, such as path, hostname, method, model, artifact kind, or action id;
- receipt event kinds emitted by the capability;
- redaction and sensitivity rules for receipt attributes;
- docs and diagnostic labels.

Capability declarations may live in source, package metadata, manifest metadata, generated descriptors, or capability packs. Wherever they live, RFC 102 semantic inspection must be able to expose them as project facts.

Package-defined capabilities must not grant host authority by implication alone. If a domain capability requests or implies `host.fs.read`, the runtime must resolve that relationship through host policy before allowing filesystem reads.

### Receipts

A receipt is a structured runtime fact emitted by a capability-aware operation. A receipt must include:

- event id or sequence id;
- capability identity;
- operation kind;
- status, such as observed, allowed, denied, failed, redacted, or skipped;
- source location or semantic identity when available;
- package/module/function identity when available;
- parent span or context id when available;
- redacted attributes;
- sensitivity metadata;
- replay classification.

A receipt should include operation-specific attributes such as environment variable key, filesystem path policy, HTTP method, URL policy, process command policy, model id policy, artifact id, action id, or policy id. Sensitive values must be redacted by default.

Receipts must be machine-readable. Human output may summarize receipts, but human output must not be the integration contract.

### Run reports

A run report is a machine-readable summary of a run, action, test, or governed entrypoint. A report must include:

- toolchain version;
- run mode;
- entrypoint identity;
- action identity when the run was invoked through a typed action;
- requested capabilities when available;
- granted capabilities;
- denied capability requests;
- emitted receipts;
- diagnostics;
- redaction summary;
- replay manifest or replay limitations.

Reports may include artifact references, span trees, telemetry correlation ids, package versions, lockfile identity, source snapshot identity, and semantic package references.

Reports must not include raw secret values or sensitive payloads unless a separate, explicit reveal policy approves that exposure.

### Typed action alignment

Typed actions from RFC 078 provide the expected authority contract before execution. A typed action may declare required capabilities, optional capabilities, receipt schemas, mutation categories, network or model access, input and output artifacts, replay expectations, and non-plannable behavior. Those declarations are static metadata; they do not grant authority.

When a typed action runs under this RFC, the run report must preserve the action identity and should include enough metadata to compare declared capability requirements with runtime behavior. If the action emits a receipt for a capability that was not declared by the action, package, or selected capability pack, the report should mark the mismatch. If the action declares a required capability that is never requested during a successful run, the report may mark the declaration as unused rather than treating it as an error by default. Policy may choose to reject undeclared capability use, require review for unused broad grants, or allow either case in permissive workflows.

Dry-run output from RFC 078 and run reports from this RFC should use compatible capability identities, action identities, risk categories, redaction markers, artifact identities, and replay classifications. A user, CI job, LSP client, or agent should be able to read the dry-run plan, run the action, and compare the actual receipts without interpreting separate schemas.

### Replay classification

Each receipt and run report should classify replayability. Initial replay classifications should include:

- `deterministic`: the operation can be replayed from recorded local inputs.
- `fixture-required`: replay requires recorded fixtures or test doubles.
- `external`: replay depends on external systems and cannot be exact without a recording.
- `unavailable`: replay is not supported for this operation.
- `redacted`: replay data exists but is intentionally hidden or incomplete.

This RFC does not require the runtime to implement full replay. It requires the runtime to avoid dishonest replay claims.

### Budgets

Capability grants may include budgets. Budgets are optional constraints over granted authority, such as maximum request count, maximum bytes written, allowed path roots, allowed hosts, allowed process names, timeout limits, model-token limits, or artifact count.

If a budget is exhausted in governed mode, the runtime must deny the operation before performing it where practical and must emit a denial receipt. If the operation cannot be prevented before partial work occurs, the receipt must describe the partial state honestly.

### Library participation

Library authors may define capabilities and receipt schemas. Libraries should not need to import stdlib-private modules or manually construct the full run report.

The stdlib should provide a small public runtime receipt surface for library authors. The exact spelling is unresolved, but it should support scoped events, one-shot events, status updates, redacted attributes, and parent span/context attachment.

Library-defined receipts must flow into the same run report as stdlib receipts. A package manager, LSP, CI job, or agent must not need separate integration logic for each library's audit output.

### Relationship to telemetry

Receipts and telemetry are related but distinct. Receipts are capability and authority facts. Telemetry is observability data. A receipt may be exported as a telemetry event or span attribute when telemetry is configured, but receipt generation must not require telemetry export.

Receipts must remain available to local reports and policy systems even when `std.telemetry` is not configured.

### Relationship to semantic inspection

RFC 102 semantic inspection should expose declared capabilities, receipt schemas, action capability requirements, policy relationships, and report artifacts. Semantic inspection should not need to execute a program to discover static capability declarations.

Runtime receipts may reference semantic identities from RFC 102 so tools can connect a run event back to source declarations, actions, generated artifacts, package metadata, and policy decisions.

### Relationship to stdlib modules

Stdlib modules that cross host authority boundaries must emit receipts when reporting is enabled and must enforce grants in governed mode.

At minimum:

- `std.environ` reads require `host.env.read`.
- `std.fs` reads require `host.fs.read`.
- `std.fs` writes require `host.fs.write`.
- `std.process` spawning requires `host.process.spawn`.
- `std.http` requests require `host.http.request`.
- clock APIs that read current time require `host.clock.read`.
- random APIs require `host.random`.
- model or tool invocation APIs require `host.model.invoke` or `host.tool.invoke`.

Pure computation, parsing, formatting, local model construction, and in-memory transformations should not require host capabilities.

## Design details

### Syntax

This RFC intentionally does not require new syntax. Capability declarations may eventually use source syntax, declaration metadata, package metadata, or manifest descriptors. The required contract is capability identity, declaration, grant, enforcement, receipt emission, and inspection.

Illustrative source syntax such as `capability example.policy.evaluate:` is non-normative.

### Semantics

Capability checks occur at authority-bearing operation boundaries. In ordinary source, a helper function that calls `std.http.get` does not need to declare an effect type merely because it may perform HTTP. If the program runs in governed mode without `host.http.request`, the operation fails at the boundary with a capability diagnostic.

Static capability declarations are still useful for actions, packages, generated artifacts, docs, and policy review. They should describe expected authority before a run happens. Runtime receipts describe actual authority use during a run.

Static declarations and runtime receipts should be compared where possible. If a run uses a capability not declared by its action or package metadata, the report should mark that mismatch.

### Interaction with existing features

- **RFC 033 (`ctx`)**: configuration fields may require environment or secret-provider capabilities when resolved at runtime.
- **RFC 055 (`std.fs`)**: file APIs become standard publishers of filesystem receipts and governed checks.
- **RFC 063 (`std.process`)**: process spawning becomes a governed host capability with structured command-policy receipts.
- **RFC 066 (`std.http`)**: HTTP requests become governed host capabilities with redacted request/response receipts and replay classifications.
- **RFC 075 (capability packs)**: project capability packs may declare expected package and action capabilities, but they must not grant host authority without runtime policy.
- **RFC 076 (policy)**: policy consumes capability declarations and receipts, and may approve, deny, or require review for grants and mutations.
- **RFC 078 (typed actions)**: actions may declare required capabilities, optional capabilities, receipt schemas, artifact effects, and dry-run plans; this RFC defines how runtime receipts and reports confirm, deny, or differ from those declarations.
- **RFC 089 (`std.environ`)**: environment access becomes a governed and receipted host boundary.
- **RFC 090 (typed CLI framework)**: CLI commands may declare capability requirements and expose helpful denial diagnostics.
- **RFC 092 (interactive runtime contracts)**: target manifests may describe host capabilities supported by a runtime target.
- **RFC 093 (`std.telemetry`)**: telemetry may export receipts, but receipts remain local authority facts when telemetry is disabled.
- **RFC 094 and RFC 095**: context managers and span vocabulary blocks provide convenient scopes for receipt correlation, but receipts do not require span syntax.
- **RFC 102 (semantic inspection)**: capability declarations, receipt schemas, run reports, and replay manifests become inspectable semantic artifacts.
- **RFC 103 (secret values)**: receipt redaction should preserve secret-value sensitivity metadata without requiring receipts to expose raw secret payloads.

### Compatibility

This RFC is additive. Existing programs can continue to run in permissive mode. Governed mode may reveal hidden authority assumptions in existing programs, but those failures are the point of governed execution and must be diagnosable.

Stdlib APIs that already perform authority-bearing operations should be updated to emit receipts and enforce grants in governed mode. Libraries may opt in incrementally by publishing capability descriptors and using the public receipt surface.

## Alternatives considered

### Full algebraic effects

Rejected for now. Algebraic effects or effect rows may become useful later, but they would fight Incan's Python-shaped ergonomics if introduced as the first user-facing authority model.

### Stdlib-only auditing

Rejected because it would prevent library authors from defining domain capabilities and would force every serious package to invent its own audit layer.

### External sandbox only

Rejected because external sandboxing can restrict behavior but does not provide source-level capability identities, semantic inspection, domain receipts, or useful diagnostics.

### Logging-only receipts

Rejected because logs are human-oriented and often unstructured. Receipts must be machine-readable authority facts with stable semantics, redaction, and replay information.

### Import-time capability checks

Rejected because it makes code harder to reuse and breaks ordinary Python-shaped authoring. Authority should be checked when authority-bearing operations are invoked, not when modules are imported.

## Drawbacks

This RFC adds a cross-cutting runtime contract. Stdlib modules, package metadata, typed actions, policy, LSP, reports, and agents must agree on capability identities and receipt shapes.

Capability names can sprawl if packages publish overly fine-grained or inconsistent capability vocabularies. Tooling will need naming guidance, validation, and docs support.

Receipts can create overhead and sensitive metadata risk. Implementations must make reporting configurable, preserve redaction, and avoid accidental remote export.

Governed mode can frustrate users if diagnostics are vague or if common operations require too many grants. The initial capability set should stay coarse and understandable until real usage proves finer scope is needed.

## Implementation architecture

This section is non-normative.

A practical architecture is to route capability-aware operations through a runtime authority context. That context can hold run mode, grants, budgets, redaction policy, receipt sink, telemetry bridge, and source/semantic identity mapping.

Stdlib modules should call a small shared runtime authority API before crossing host boundaries and emit receipts through the same API after success, failure, denial, or partial completion. Library authors should get a public receipt API that creates domain receipts without exposing private stdlib internals.

Generated build artifacts and run reports should be ordinary artifacts that RFC 102 can inspect. LSP, CI, docs tooling, and agents should consume the report schema rather than parsing logs.

## Layers affected

- **Stdlib / Runtime (`incan_stdlib`)**: host-boundary modules need capability checks, receipt emission, redaction handling, and report integration.
- **Tooling / CLI**: run, test, action, and build commands need report output, governed-mode grants, denial diagnostics, and machine-readable schemas.
- **Package metadata**: packages need a way to publish capability declarations and receipt schemas.
- **Typechecker / Semantic metadata**: static capability declarations and action requirements should be exposed as checked metadata where available.
- **IR Lowering / Backend**: source spans and semantic identities should be preserved well enough for receipts to point back to source and semantic objects.
- **LSP / Docs tooling**: editors and docs can surface capability declarations, required grants, denial diagnostics, and report artifacts.
- **Policy / CI / Agents**: policy and automation can consume capability declarations, action dry-runs, receipt schemas, and actual receipts to decide whether runs, actions, generated artifacts, or proposed changes are acceptable.

## Unresolved questions

- What is the exact grammar for capability identities?
- Should capability declarations live in source syntax, declaration metadata, package manifests, or all of them?
- What should the default run mode be for `incan run`, `incan test`, and typed actions?
- What is the minimum stable host capability set?
- How should scoped grants be represented for paths, hosts, methods, models, tools, and artifacts?
- Should package-defined capabilities be allowed to imply host capabilities automatically when a user grants the package capability, or should host grants always be listed separately?
- What is the first stable receipt schema version?
- How should receipt sinks be configured, and where should reports be written by default?
- Which replay classifications are required for the first implementation?
- How should telemetry export represent receipts without making telemetry a dependency of receipt generation?
- How should capability budgets be expressed in CLI, package metadata, and typed action declarations?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
