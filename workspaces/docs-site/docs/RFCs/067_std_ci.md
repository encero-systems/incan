# RFC 067: `std.ci` — deterministic CI and automation scripting primitives

- **Status:** Draft
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 051 (`JsonValue` for `std.json`)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/85
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.ci` as Incan's standard library surface for deterministic CI and automation scripts. The module family standardizes environment access, workflow input and output files, small automation-oriented filesystem helpers, argument and exit handling, and integration with `std.http` and `JsonValue` so repository automation does not need to fall back to fragile shell glue or JavaScript snippets.

## Core model

Read this RFC as one foundation plus three mechanisms:

1. **Foundation:** CI scripting is ordinary Incan automation work, not a special-purpose embedded DSL or provider-specific builtin.
2. **Mechanism A:** `std.ci` provides a narrow set of primitives for the execution environment: env vars, argument access, output files, and explicit exit behavior.
3. **Mechanism B:** network access remains a separate general-purpose concern through `std.http`, not a CI-only HTTP surface.
4. **Mechanism C:** provider-specific behavior such as GitHub payload interpretation should live in libraries built on these primitives rather than as a special builtin contract.

## Motivation

CI automation is usually held together by shell scripts, YAML snippets, and tiny bits of JavaScript. That works, but it is a poor foundation for a language that wants to be credible in tooling and automation. Scripts become hard to test locally, hard to refactor, hard to type-check, and hard to reuse.

This is especially visible for workflows such as issue triage, label synchronization, release automation, or event-driven repository checks. Those tasks typically need the same small set of capabilities:

- read environment variables
- read an event payload file
- write outputs or step summaries
- make HTTP requests
- exit clearly and deterministically

Today, each of those usually gets reinvented with ad hoc shell behavior or backend-specific libraries. `std.ci` should provide one explicit and testable base layer instead.

## Goals

- Provide a small stdlib surface for deterministic CI and automation scripts.
- Standardize environment-variable access, argument access, exit behavior, and workflow-output file handling.
- Keep HTTP out of the CI module itself by composing through RFC 066 `std.http`.
- Make the surface portable across hosted CI systems while still being usable for GitHub Actions-style workflows.
- Support local fixture-driven testing of automation scripts.
- Keep secrets handling explicit and conservative.

## Non-Goals

- Shipping a GitHub-specific, GitLab-specific, or provider-specific SDK as part of `std.ci`.
- Replacing `std.http`, `std.fs`, or `std.process`; `std.ci` should compose with them, not duplicate them.
- Defining a full JWT or cryptography story here beyond whatever minimal hooks later prove necessary.
- Defining a workflow YAML format or a hosted-runner orchestration system.
- Making CI automation a language feature rather than a stdlib surface.

## Guide-level explanation

### Reading the execution environment

An automation script should be able to read environment variables explicitly:

```incan
from std.ci.env import get, get_optional

event_path = get("GITHUB_EVENT_PATH")?
token = get_optional("GITHUB_TOKEN")
```

Missing required values should be explicit failures, not silent `None` unless the user asked for optional access.

### Reading workflow payloads

Scripts should be able to combine `std.ci` primitives with `JsonValue`:

```incan
from std.ci.env import get
from std.ci.fs import read_text
from std.json import JsonValue

payload = JsonValue.parse(read_text(get("GITHUB_EVENT_PATH")?)?)?
issue_number = payload["issue"]["number"].as_int()
```

This is still ordinary Incan. The CI-specific part is only how the script learns where the payload file lives.

### Writing outputs

Workflow systems often expose file-based output channels. The stdlib should make that explicit:

```incan
from std.ci.output import set_output

set_output("triaged", "true")?
```

or, for a more direct file-oriented model:

```incan
from std.ci.env import get
from std.ci.fs import append_text

append_text(get("GITHUB_OUTPUT")?, "triaged=true\n")?
```

The important point is that the file-write behavior is explicit and testable.

### Exiting clearly

Automation scripts should be able to fail or succeed explicitly:

```incan
from std.ci.process import exit

if not ok:
    exit(1)
```

This does not replace structured `Result`-returning APIs. It only gives the script one explicit terminal action when needed.

## Reference-level explanation

### Module family

`std.ci` must provide a narrow but sufficient set of modules or equivalent surfaces covering:

- environment access
- workflow input and output file handling
- process arguments and exit behavior
- deterministic helper behavior suitable for local testing

The module family may be split as:

- `std.ci.env`
- `std.ci.fs`
- `std.ci.process`
- `std.ci.output`

or another equivalent subdivision, but the public contract must remain small and explicit.

### Environment access

The environment-access surface must provide:

- a required getter that fails clearly when a variable is absent
- an optional getter that returns `None` when absent

It must not silently coerce missing values into empty strings.

### Workflow file handling

The CI surface must support the common workflow pattern of reading payload files and writing output files. It may do this either through:

- explicit helpers such as `set_output(...)`, or
- a smaller primitive model built on environment variables and append-safe file helpers

This RFC does not require one exact spelling yet, but it does require the capability.

### Exit behavior

`std.ci` must provide explicit process-exit behavior or a functionally equivalent way to terminate with a chosen exit code.

### Determinism and testability

The module family should prefer deterministic behavior:

- no hidden network
- no hidden retries
- no hidden provider-specific fallbacks

CI scripts must remain locally testable with fixture payloads and explicit environment setup.

### Secrets and diagnostics

The CI surface should avoid exposing secrets carelessly in debug-facing or error-facing helpers. It does not need to define a complete secret type system, but it should follow conservative diagnostic practices and defer richer secret handling to dedicated follow-up work where needed.

## Design details

### Syntax

This RFC introduces no new language syntax.

### Semantics

The semantic center is narrowness:

- `std.ci` is not a general automation framework
- `std.ci` is not an HTTP stack
- `std.ci` is not a provider-specific SDK

It is a small bridge between hosted automation environments and ordinary Incan code.

### Interaction with existing features

- **RFC 066 (`std.http`)**: HTTP access for automation scripts should go through `std.http`, not a CI-only network API.
- **RFC 051 (`JsonValue`)**: event payload parsing should compose naturally with `JsonValue`.
- **RFC 055 (`std.fs`)**: path and file handling should stay coherent with the broader filesystem surface where possible.
- **RFC 063 (`std.process`)**: argument access and exit behavior should align with the broader process model rather than diverge.
- **RFC 015**: a future CLI wrapper such as `incan ci` may exist, but it is not required for the base stdlib contract.

### Compatibility / migration

This feature is additive. Existing bash, shell, or JavaScript-based CI steps remain valid. The design claim is that Incan should offer a better standard path for new automation scripts once the module exists.

## Alternatives considered

- **Keep shell and JavaScript glue only**
  - Rejected because it leaves automation brittle, weakly typed, and difficult to test.
- **Make GitHub a builtin**
  - Rejected because it introduces too much provider policy too early.
- **Push everything into `std.http` plus `std.fs`**
  - Rejected because CI environments do have a small amount of distinct execution-context behavior worth standardizing.

## Drawbacks

- There is a real risk of overfitting the module to one provider if the design is not kept disciplined.
- Output-file and env-var conventions vary across systems, so portability rules need careful wording.
- Secret-handling expectations need to be conservative even if the module stays intentionally small.

## Implementation architecture

*(Non-normative.)* A sensible rollout starts with `env`, `fs`, and `process` primitives plus fixture-driven examples that demonstrate local testing against payload files. Once `std.http` is available, real CI automation flows can compose on top without needing any provider-specific builtin APIs.

## Layers affected

- **Stdlib / runtime**: must provide the CI module family and its deterministic environment-facing helpers.
- **Language surface**: the module family must be available as specified and must compose cleanly with ordinary Incan code.
- **Execution handoff**: implementations must preserve explicit environment, file, and exit semantics without hidden provider-specific behavior.
- **Docs / tooling**: examples should show local fixture-driven testing and explicit composition with `std.http` and `JsonValue`.

## Unresolved questions

- Should output-file helpers such as `set_output(...)` be first-class, or should the contract stay one layer lower and rely on environment variables plus file append helpers?
- Should `std.ci` standardize step-summary and annotation helpers in the base module family, or leave those to follow-up provider libraries?
- Does a minimal JWT-signing helper belong here for GitHub App workflows, or should all cryptographic behavior remain outside this RFC?
- Should a future `incan ci` CLI wrapper be part of the same design family, or remain separate from the stdlib contract?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
