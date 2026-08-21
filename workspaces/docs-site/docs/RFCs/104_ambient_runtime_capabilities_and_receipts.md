# RFC 104: Ambient Runtime Capabilities and Receipts

- **Status:** Planned
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
- Make capability identities checked symbols, resolved from where they are declared, rather than string literals a caller can misspell or a package can spoof.
- Let the compiler statically verify that a typed action's declared capabilities match what its body actually calls, so a mismatch is a compile-time diagnostic rather than a runtime capability denial discovered later.
- Allow a host to supply a capability ceiling that bounds an invocation's effective grants regardless of what that invocation requests, so an untrusted or agent-driven caller cannot widen its own authority.

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
- This RFC does not define global CLI flags unrelated to capability grants and reports, such as verbosity, color, or profile selection.
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

Library authors should be able to participate without depending on stdlib-private hooks. A package can define a domain capability, declared inside its own module tree so its identity comes from where it lives rather than from a string the author types:

```incan
# declared inside the package's own `policy` submodule
capability evaluate:
    description = "Evaluate an input against a policy"
    requires = [host.fs.read]
```

This resolves to a fully-qualified identity such as `example_lib.policy.evaluate`, namespaced by the declaring package's own registered identity so two packages can never collide on the same name. `requires` documents that evaluating a policy needs `host.fs.read` to load the policy file; it does not grant that authority by implication -- see "Import, request, grant, and use."

Library code can then emit a receipt through a low-ceremony boundary, referencing the capability as a checked symbol rather than a string:

```incan
from example_lib.capabilities import policy_evaluate
from std.runtime import receipts

def evaluate(policy: Policy, input: Input) -> Decision:
    with receipts.event(policy_evaluate, subject=policy.id):
        return policy.evaluate(input)
```

For common entrypoints, typed actions declare the capabilities they require the same way:

```incan
from example_lib.capabilities import policy_evaluate
from std.runtime import host

@action(caps=[policy_evaluate, host.model.invoke])
def review(input: ReviewInput) -> ReviewReport:
    ...
```

Granting a domain capability does not automatically let a package bypass host policy. `policy_evaluate` declares that it `requires` `host.fs.read`, but that relationship is inspectable documentation the runtime and host policy can check and warn against when it is missing -- it is never an implicit grant. Libraries can name and explain authority; the runtime grants authority.

## Reference-level explanation

### Capability identities

A capability identity must be a checked symbol, not a string literal that source code writes out by hand. A capability declaration's fully-qualified identity must be derived from where it is declared, the same way any other Incan symbol's fully-qualified path is derived from its declaration site rather than from a string the author types. A package capability declared inside a package's own module tree resolves to `<package_identity>.<module_path>.<name>`, where `<package_identity>` is the package's own registered identity (the same identity a registry already enforces as unique), never a prefix the author writes literally. Two packages must never collide on the same capability name, because the namespace segment is derived from an identity the registry already guarantees is unique, not from an author-chosen string.

`host.*` is reserved for capabilities owned by the Incan toolchain and runtime. It must use the same declaration mechanism as package capabilities -- a `capability` declaration with the same fully-qualified-identity-from-location rule -- except that only the compiler's own bundled `std.runtime` source may declare under it. A package that attempts to declare a capability under `host.*` must be rejected with a reserved-namespace diagnostic, the same enforcement class as a package trying to claim another package's registered identity. There is exactly one capability-declaration mechanism in this RFC; `host.*` capabilities look fixed because `std.runtime`'s source is fixed between releases, not because host and package capabilities are two different kinds of thing.

The minimum stable host capability set for this RFC is exactly:

- `host.env.read`
- `host.fs.read`
- `host.fs.write`
- `host.process.spawn`
- `host.http.request`
- `host.clock.read`
- `host.random`
- `host.model.invoke`
- `host.tool.invoke`

This set must not be extensible by packages. A package that needs narrower or additional authority defines its own capability and, where applicable, declares which of these host capabilities it `requires` -- see "Capability declarations."

A capability reference at a use site (an `@action(caps=[...])` list, a `receipts.event(...)` call, an `--allow` grant) must resolve against the declaration the same way an import resolves against a definition. A misspelled or nonexistent capability reference must be an unresolved-symbol compile error, not a runtime capability-denial diagnostic discovered later.

### Import, request, grant, and use

Importing a module must not grant authority. Importing `std.process` is allowed even in a run that has not granted `host.process.spawn`. Authority is checked when an authority-bearing operation is invoked.

A package, action, function, descriptor, or runtime operation may request capabilities. A run, host, action invoker, test harness, package manager, CI environment, or policy system may grant capabilities. Only the runtime or host authority boundary may decide whether a request is granted.

When an operation requiring a capability is invoked in governed mode and the capability is not granted, the operation must fail before performing the authority-bearing behavior. The diagnostic must identify the required capability and should include the source span, import/module/function path, and a suggested grant spelling when available.

A host may also supply a capability ceiling: a maximum grant set sourced from outside the invocation itself, such as a policy file a harness writes before starting an agent or CI job. The effective grant for any invocation must be the intersection of its ceiling, if one is supplied, and whatever it requests via `--allow` or a typed action's declared caps -- never their union. An invocation cannot widen its own authority by requesting more than its ceiling allows; it can only receive less. This applies uniformly to host and package capabilities alike. The exact mechanism for supplying a ceiling is illustrative, not normative; what is normative is that a ceiling source exists, is distinct from per-invocation requests, and is enforced as an intersection. Whether that source is itself protected from tampering by the process it bounds is an operating-system/sandbox concern this RFC does not define -- see Non-Goals.

### Runtime modes

The runtime should support at least these conceptual modes:

- `permissive`: operations run normally and receipts may be disabled.
- `observe`: operations run normally and receipts are emitted.
- `governed`: operations require granted capabilities and receipts are emitted.

The user-facing shape is:

```text
incan run app.incn --report json
incan run app.incn --allow host.env.read,host.http.request --report json
```

`incan run` defaults to `permissive`: ordinary local development stays ordinary, and nothing is denied or exported without an explicit flag. `incan test` defaults to `governed` with nothing pre-granted, so a test that unexpectedly reaches the real filesystem or network fails with a capability diagnostic instead of silently succeeding -- the same test-isolation property most test frameworks already enforce by convention, made structural here instead. A typed action runs under whatever mode and grants its invoking context provides (a CI job's explicit `--allow`, a host's policy-selected grant set); this RFC does not define a third default distinct from `incan run` and `incan test`, since actions are never invoked in a vacuum.

### Capability declarations

Capability declarations live in source, as a first-class `capability` declaration form -- not in package manifest metadata, generated descriptors, or capability packs. This is required, not a style preference: deriving a capability's fully-qualified identity from its declaration location (see "Capability identities") only works if the declaration site is something the compiler resolves, the same way it resolves a function or type declaration.

A capability declaration must include:

- a human-readable description;
- an optional `scope` schema: a typed set of named scope dimensions the capability accepts, such as `tenant: str` or `path: str`. A grant may constrain zero or more of a capability's declared scope dimensions; a grant referencing a scope key the capability did not declare is a checked error, not a silently ignored key;
- an optional `requires` list of other capabilities (checked symbol references, typically host capabilities) whose authority this capability's implementation needs.

```incan
capability refund:
    description = "Issue a refund for a captured charge"
    scope:
        tenant: str
    requires = [host.http.request]
```

RFC 102 semantic inspection must be able to expose capability declarations, their scope schemas, and their `requires` relationships as project facts.

Package-defined capabilities must not grant host authority by implication alone. `requires` documents that a domain capability's implementation needs a host capability; it is metadata the runtime, host policy, and the static check in "Typed action alignment" can inspect and warn against when missing, but granting the domain capability never automatically grants the capabilities it `requires`. Those must always be granted separately.

Scope values are never written into a static declaration such as `@action(caps=[...])` -- a static declaration only names which capabilities, generically, an action needs. Scope values bind at grant time (`--allow example_lib.policy.refund:tenant=acct_a`) and are checked against the actual attributes of the operation being performed at the moment it happens, independent of whatever the calling code decided. This holds for both host and package capabilities: a scoped `host.http.request` grant is checked against the real outbound host at the point `std.http` makes the request, not against anything decided when the action was defined.

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

Report output reuses the existing `--report`/`--report-output` contract `incan build` already established, rather than defining a new one: `--report json` writes to stdout by default, and `--report-output <PATH>` redirects it to a file. Receipts and run reports use a numeric `schema_version`, matching the same convention already used by `incan check --format json`, `incan build --report json`, and other existing JSON report surfaces; this RFC does not define a new versioning scheme.

### Typed action alignment

Typed actions from RFC 078 provide the expected authority contract before execution. A typed action may declare required capabilities, optional capabilities, receipt schemas, mutation categories, network or model access, input and output artifacts, replay expectations, and non-plannable behavior. Those declarations are static metadata; they do not grant authority.

When a typed action runs under this RFC, the run report must preserve the action identity and should include enough metadata to compare declared capability requirements with runtime behavior. If the action emits a receipt for a capability that was not declared by the action, package, or selected capability pack, the report should mark the mismatch. If the action declares a required capability that is never requested during a successful run, the report may mark the declaration as unused rather than treating it as an error by default. Policy may choose to reject undeclared capability use, require review for unused broad grants, or allow either case in permissive workflows.

Dry-run output from RFC 078 and run reports from this RFC should use compatible capability identities, action identities, risk categories, redaction markers, artifact identities, and replay classifications. A user, CI job, LSP client, or agent should be able to read the dry-run plan, run the action, and compare the actual receipts without interpreting separate schemas.

The declared-versus-actual comparison this section already requires for runtime reports must also happen statically, before a typed action ever runs. The typechecker must resolve every call inside an `@action`-decorated function body, union the host and package capabilities those calls require (via the existing stdlib-to-host-capability mapping in "Relationship to stdlib modules" and via any called capability's own `requires` metadata), and compare that computed set against the function's declared `caps=[...]` list:

- if the body requires a capability the declaration omits, this is a compile error ("incomplete action caps"), not a warning -- an action whose declared caps are incomplete is guaranteed to fail at runtime the first time a governed run reaches the undeclared operation, so catching it statically is strictly better than discovering it as a runtime denial;
- if the declaration lists a capability the body never actually uses, this is a warning, matching the non-error default this section already establishes for runtime-observed unused declarations, not a new stricter policy for the static case.

This diagnostic must be available through the same channel as any other typechecker diagnostic, including LSP, so it is visible while authoring the decorator, not only at build time. An editor may offer a quick fix that inserts a missing capability into the declared list, the same shape as a quick fix for a missing import.

### Replay classification

Each receipt and run report must classify replayability. This RFC requires four classifications for the first implementation, each motivated by an operation this RFC already describes:

- `deterministic`: the operation can be replayed from recorded local inputs, such as a filesystem write whose output is fully determined by its recorded arguments.
- `external`: replay depends on an external system and cannot be exact without a recording, such as an HTTP call to a partner API whose response can change between calls.
- `fixture-required`: replay requires a recorded fixture or test double, such as the same HTTP call made under `incan test`'s default governed, ungranted mode.
- `redacted`: replay data existed but was intentionally not persisted, such as a receipt whose attributes were redacted before reaching a sink.

`unavailable` (replay is not supported for this operation) remains a trivial always-available fallback classification and needs no further design work in this RFC.

This RFC does not require the runtime to implement full replay. It requires the runtime to avoid dishonest replay claims.

### Budgets

Capability grants may include budgets. Budgets are optional constraints over granted authority, such as maximum request count, maximum bytes written, allowed path roots, allowed hosts, allowed process names, timeout limits, model-token limits, or artifact count.

Budgets are expressed through the same grant syntax as scope, not a separate mechanism, using a reserved `budget.` key prefix alongside a capability's own scope keys:

```text
--allow "host.http.request:host=partner.example.com,budget.max_requests=100"
```

CLI grants, package metadata, and typed action declarations all use this same `budget.<key>=<value>` shape wherever a budget needs to be expressed; there is no separate budget-specific declaration form.

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

This RFC requires new syntax: `capability` is a first-class declaration form, with an optional `scope` schema and an optional `requires` list, as shown in "Capability declarations." This is necessary, not merely convenient -- a capability's fully-qualified identity being derived from its declaration location, rather than typed by the author, only works if the declaration is real source syntax the compiler resolves.

`host.*` capabilities use the same declaration form; only `std.runtime`'s own bundled source may declare under that reserved namespace.

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
- **Tooling / CLI**: run, test, action, and build commands need report output, governed-mode grants, denial diagnostics, machine-readable schemas, and host-ceiling resolution.
- **Package metadata**: packages need a way to publish capability declarations and receipt schemas.
- **Typechecker / Semantic metadata**: static capability declarations and action requirements should be exposed as checked metadata where available.
- **IR Lowering / Backend**: source spans and semantic identities should be preserved well enough for receipts to point back to source and semantic objects.
- **LSP / Docs tooling**: editors and docs can surface capability declarations, required grants, denial diagnostics, and report artifacts.
- **Policy / CI / Agents**: policy and automation can consume capability declarations, action dry-runs, receipt schemas, and actual receipts to decide whether runs, actions, generated artifacts, or proposed changes are acceptable.

## Design Decisions

- **Capability identities are checked symbols, not strings:** a capability's fully-qualified identity is derived from where it is declared (package or toolchain identity plus module path), never typed by the author. Two packages cannot collide on a capability name because the namespace segment comes from an identity a registry already enforces as unique. A capability reference at a use site resolves like an import; a misspelled or nonexistent reference is a compile error, not a runtime surprise.
- **Capability declarations live in source**, as a first-class `capability` declaration form, because deriving identity from declaration location requires the declaration to be real, compiler-resolved syntax rather than manifest or metadata.
- **Default run modes:** `incan run` defaults to `permissive`; `incan test` defaults to `governed` with nothing pre-granted, enforcing the same test-isolation property most test frameworks already assume by convention. Typed actions run under whatever mode and grants their invoking context provides; there is no third default.
- **Minimum stable host capability set is exactly the nine already proposed** in Reference-level explanation (`host.env.read`, `host.fs.read`, `host.fs.write`, `host.process.spawn`, `host.http.request`, `host.clock.read`, `host.random`, `host.model.invoke`, `host.tool.invoke`), fixed and not package-extensible.
- **Scoped grants:** a capability declares its own typed `scope` schema (host capabilities via `std.runtime`'s own declarations, package capabilities via their own). Scope values bind at grant time, never in a static declaration such as `@action(caps=[...])`, and are checked against the real operation's attributes at the moment it happens, independent of the declaring code.
- **No implicit host-capability grants:** a package capability's `requires` list documents which host capabilities its implementation needs, for tooling and policy to inspect, but granting the package capability never automatically grants what it requires. Those must always be listed and granted separately.
- **Receipt/report schema versioning is not this RFC's decision to make** -- it is already established product convention. Receipts and run reports use a numeric `schema_version`, matching `incan check`, `incan build`, and other existing JSON report surfaces exactly. There is no starting version number for this RFC to bless.
- **Report output reuses `incan build`'s existing `--report`/`--report-output` contract** rather than defining a new one.
- **Four required replay classifications** for the first implementation: `deterministic`, `external`, `fixture-required`, `redacted`. `unavailable` remains a trivial fallback needing no further design.
- **Telemetry export is confirmed orthogonal** to receipt generation, per this RFC's existing text: receipts remain available to local reports and policy regardless of whether `std.telemetry` is configured; telemetry is a second consumer of the same stream, not a dependency of producing it.
- **Capability budgets use the same grant syntax as scope**, via a reserved `budget.<key>=<value>` prefix, in CLI grants, package metadata, and typed action declarations alike -- no separate budget declaration form.
- **Typed actions get a static caps-completeness check**, extending "Typed action alignment" from a runtime-only comparison to a compile-time one: the typechecker resolves every call in an `@action`-decorated body, unions the capabilities it actually requires, and diffs that against the declared `caps=[...]` list. A missing required capability is a compile error; an unused declared capability is a warning, matching the non-error default this RFC already gives runtime-observed unused declarations. The diagnostic is available through LSP the same way any other typechecker diagnostic is, including a quick fix to insert a missing capability.
- **Host-supplied capability ceilings bound effective grants via intersection, never union:** a host may supply a maximum grant set sourced from outside the invocation itself -- for example, a harness-written policy file for an agent or CI job. The effective grant for any invocation is always the intersection of its ceiling and what it requests; an invocation can only receive less than its ceiling, never more, regardless of what it asks for. This RFC defines the ceiling as a distinct grant source and the intersection rule; it does not define how the ceiling's own source is protected from tampering, which is an operating-system/sandbox concern already excluded by this RFC's Non-Goals.
