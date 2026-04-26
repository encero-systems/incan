# RFC 076: project mutation policy and recovery

- **Status:** Draft
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (Cargo offline and locked policy)
    - RFC 034 (`incan.pub` package registry)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 075 (starter profiles and capability packs)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/404
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a high-level policy, approval, advisory, and recovery model for receiver-side project mutations proposed by template updates, starter profiles, capability packs, catalogs, registries, automation, IDE tooling, or agents. RFC 074 and RFC 075 define how mutation plans are produced; this RFC defines how projects and organizations can decide which plans are allowed, which require approval, which are blocked, and how users recover when a previously trusted source is yanked, revoked, or discovered to be unsafe.

## Core model

Read this RFC as six foundations:

1. **Receiver authority:** the receiving project owns acceptance of source-code, manifest, script, CI, env, tooling, and agent-guidance mutations.
2. **Policy evaluates plans:** policy decisions are made against explicit mutation plans from RFC 074 and RFC 075, not against opaque provider intent.
3. **Risk is classified:** mutation plans identify security-sensitive categories so policy can require different review levels for source files, scripts, CI configuration, dependencies, env definitions, and agent guidance.
4. **Automation proposes:** automated maintenance may produce patch proposals, but it must not grant its own approval or bypass receiver policy.
5. **Advisories are actionable:** yanked, revoked, vulnerable, suspicious, or superseded sources must produce status and recovery information rather than only generic warnings.
6. **Recovery is reviewable:** rollback, quarantine, source replacement, or rendered-file repair must be represented as receiver-side mutation plans with the same review boundary as normal updates.

## Motivation

RFC 074 and RFC 075 make Incan project generation and evolution more powerful by turning boilerplate, starters, and capabilities into explicit tooling data. That power creates a new responsibility: these features do not merely resolve dependencies; they may write or rewrite code in a receiving repository. Versioning, checksums, signatures, and provenance help establish where a proposal came from, but they do not by themselves decide whether a receiver should accept the proposal.

The missing layer is receiver-side governance. A project may want to allow built-in templates, require immutable pins for git-backed templates, block public catalog sources for internal projects, require review for CI or script changes, permit low-risk docs scaffolding with local confirmation, or quarantine a capability when its source is later yanked. Those choices should be represented as policy instead of scattered across ad-hoc command flags and editor integrations.

This RFC intentionally stays above hosting and registry transport. It captures the policy and recovery contract that local CLI tooling, IDE integrations, registries, private catalogs, and future automation can share.

## Goals

- Define a receiver-side policy model for project mutations proposed by templates, starters, capabilities, catalogs, registries, automation, IDEs, and agents.
- Define policy outcomes such as allow, require approval, block, warn, quarantine, and propose recovery.
- Define a risk-category vocabulary that can distinguish source changes, dependency changes, scripts, CI/configuration, env definitions, generated-file ownership changes, and agent guidance.
- Require policy decisions to evaluate rendered receiver-side mutation plans, not only descriptor or template-source diffs.
- Define approval semantics at a high level so local confirmation, explicit CLI flags, code-review approval, and automation-produced proposals are not conflated.
- Define status and recovery behavior for yanked, revoked, vulnerable, suspicious, unknown, or source-switched templates and capabilities.
- Ensure policy and recovery surfaces are machine-readable so CLI, LSP, IDE, docs tooling, and agents can consume the same decision data.
- Leave room for `incan.pub`, private catalogs, and organization tooling to provide advisory and trust metadata without letting remote services mutate projects directly.

## Non-Goals

- Defining public registry transport, catalog hosting, identity-provider integration, billing, tenancy, or administration.
- Defining a full code-review system or replacing GitHub, GitLab, Gerrit, local review, or future project-specific review tools.
- Defining exact CLI flag spelling for every policy operation.
- Automatically rolling back source code after a bad template or capability is discovered.
- Providing malware scanning, formal verification, or proof that rendered code is safe.
- Reopening RFC 074 template syntax or RFC 075 starter/capability descriptor semantics.
- Making policy mandatory for simple local projects that only use built-in templates and no automated refresh.

## Guide-level explanation

### Policy as a receiver-side contract

A project can define a policy that describes which mutation sources are allowed and which categories require review:

```toml
[tool.incan.policy.sources]
builtin = "allow"
local = "warn"
git = "require-immutable-pin"
public-catalog = "require-review"
private-catalog = "allow"

[tool.incan.policy.risk]
source = "require-review"
dependency = "require-review"
script = "require-review"
ci = "require-review"
env = "require-review"
agent-guidance = "require-review"
docs = "allow"
```

The exact policy encoding is not normative in this Draft. The contract is that policy is evaluated by the receiver's lifecycle tooling against the mutation plan that would actually change the project.

### Policy check before mutation

When a user previews a capability, policy evaluation appears alongside the mutation plan:

```text
incan capability add cli --dry-run
```

Example output:

```text
Capability: cli
Source: public-catalog:app-cli@0.3.1
Policy: requires review

Risk categories:
  source            src/main.incn
  source            tests/test_cli.incn
  script            [tool.incan.envs.default.scripts].run
  dependency        app-cli = "0.3.1"
  agent-guidance    cli.write-commands

Blocked:
  source pin is mutable: git branch "main"

Next:
  rerun with an immutable source pin or select an approved catalog source
```

The important behavior is not the exact wording. The important behavior is that the receiver sees why a plan is allowed, blocked, or review-gated before project files change.

### Automated proposal flow

Automation may detect that a template or capability source has a newer compatible version or a known advisory:

```text
incan mutation propose --stale --security
```

A valid implementation may choose a different command name, but the behavior should be review-first. Automation creates a patch-sized proposal containing source identity changes, integrity or advisory changes, rendered receiver-side diffs, and policy outcomes. It does not merge, approve, or apply the proposal by itself when policy requires review.

### Recovery after an unsafe source

If a previously applied template or capability is later yanked, revoked, or marked unsafe, status should show a receiver-actionable state:

```text
incan mutation status
```

Example output:

```text
origin                         state       reason
capability:cli                 revoked     publisher revoked descriptor app-cli@0.3.1
template:src/main.incn         affected    rendered from revoked descriptor

Recovery options:
  propose source replacement: app-cli@0.3.2
  propose revert of unchanged managed files
  mark origin quarantined in project policy
```

Recovery remains a mutation proposal. The tool may help produce a patch, but the receiver still reviews and accepts the resulting project changes.

## Reference-level explanation

### Policy inputs

Policy evaluation must operate on a mutation plan that includes at least:

- selected descriptor or template source kind
- selected source identity and immutable version or content hash when available
- publisher, provider, catalog, and trust-tier metadata when available
- integrity, signature, yanking, revocation, vulnerability, or advisory state when available
- receiver-side rendered file changes
- manifest dependency and dev-dependency changes
- script, env, CI/configuration, tooling metadata, and agent-guidance changes
- generated-file ownership changes
- applicability, back-off, skipped, blocked, unsafe, and conflict reason codes from RFC 074 and RFC 075

Policy must not rely only on provider-side descriptor diffs or template-source diffs. Those diffs may explain why a proposal exists, but the receiver-side rendered mutation is the object being approved.

### Risk categories

Mutation plans should classify changes into risk categories. V1 should include at least:

- `source`: executable or imported source files
- `test`: generated test files
- `dependency`: dependency and dev-dependency requirements
- `script`: project scripts and env scripts
- `ci`: CI, workflow, release, or automation configuration
- `env`: env definitions, matrices, and toolchain constraints
- `config`: formatter, linter, editor, build, package, or tool configuration
- `ownership`: generated-file ownership or provenance policy changes
- `agent-guidance`: agent skills, workflows, instruction references, or safety labels
- `docs`: documentation-only files
- `asset`: non-rendered copied files

Implementations may add extension risk categories, but unknown categories must be visible in machine-readable output and should default to requiring review rather than allow.

### Policy outcomes

Policy evaluation may produce these outcomes:

- `allow`: the plan may be applied by the local lifecycle command under normal confirmation rules
- `warn`: the plan may be applied, but diagnostics should call attention to a non-blocking concern
- `require-approval`: the plan must be approved under the receiver's configured approval mechanism before writing changes
- `block`: the plan must not be applied until the cause is resolved
- `quarantine`: the source or origin is treated as unsafe for future mutation, and existing provenance should be reported by status commands
- `propose-recovery`: the tool should present recovery options as mutation plans

Policy outcomes must be represented in machine-readable output. Human-readable output may summarize the same information, but it must not be the only policy surface.

### Approval semantics

Policy must distinguish local confirmation from review approval. A local interactive prompt or `--yes` flag may be sufficient for low-risk changes when policy allows it, but it must not satisfy review requirements for high-risk categories if project or organization policy requires independent approval.

Automation that produces a mutation proposal must not also satisfy the receiver's approval requirement. The approving identity must be independent from the tool, service, or agent that produced the patch when policy requires review.

Non-interactive modes must not silently downgrade approval requirements. If policy requires approval and none is available, the operation must fail or emit a proposal artifact without applying changes.

### Source policy

Policy may restrict source kinds, source identities, publishers, catalogs, trust tiers, and pin forms. Useful controls include:

- allow built-in sources
- allow or warn for local paths
- require immutable git commits for git sources
- reject mutable git refs for managed files
- allow only selected public catalogs
- allow only selected private catalogs
- require verified integrity or signature metadata
- block yanked, revoked, vulnerable, or unknown sources
- require explicit approval when source identity changes

If multiple sources can satisfy the same id, policy must not silently select a different source from the one recorded in project provenance. A source switch is a policy-relevant event.

### Advisory and recovery states

Status operations should be able to report source and provenance states such as:

- `current`: the recorded source is still valid and compatible
- `stale`: a newer compatible source is available
- `yanked`: the source has been withdrawn from normal selection
- `revoked`: the source or publisher identity has been revoked
- `vulnerable`: an advisory affects the source or generated artifact
- `superseded`: a replacement source is recommended
- `unknown`: source metadata cannot be resolved
- `source-mismatch`: the available source does not match recorded identity or integrity metadata
- `quarantined`: project or organization policy has blocked further use of the source

Recovery actions must be proposed as mutation plans. Recovery may include pinning a known-good source, switching to a newer compatible source, reverting unchanged managed files, marking a provenance origin as quarantined, removing generated agent guidance, or producing manual remediation instructions.

Recovery tooling must be conservative when files have user edits. If the current receiver-side file differs from recorded rendered output, recovery must avoid silent overwrite and should present a conflict or merge plan.

### Policy storage and precedence

This Draft does not mandate one storage location. Policy may come from project configuration, organization configuration, a local developer profile, CI environment, or catalog-provided defaults.

Implementations must make the effective policy explainable. When policy blocks or gates a mutation, diagnostics should show which policy source caused the decision when that information is available.

If multiple policies apply, the more restrictive decision should win unless a later RFC defines an explicit precedence model. Policy must not become weaker merely because a less restrictive local setting exists.

### Audit and provenance

When policy affects a mutation, tooling should record enough audit metadata to explain the decision later:

- policy outcome
- policy source or policy id when available
- approved risk categories
- approving identity or review reference when available
- source identity and integrity state at the time of approval
- rendered mutation plan hash when available

Audit metadata is tooling state. It must not affect compilation semantics.

## Design details

### Relationship to RFC 074

RFC 074 owns template rendering, generated-file ownership, template provenance, template update, and template reset. This RFC evaluates the update and reset plans that RFC 074 produces, including source identity changes, rendered receiver-side diffs, and risk categories.

### Relationship to RFC 075

RFC 075 owns starter and capability descriptors, applicability, back-off, mutation planning, and capability provenance. This RFC evaluates those mutation plans and defines how policy gates application, refresh, automation, and recovery.

### Relationship to RFC 034

RFC 034 owns package registry semantics. Registries and catalogs may provide publisher identity, integrity metadata, yanking state, advisories, and compatibility metadata. This RFC does not change registry transport. It defines how local lifecycle tooling uses that metadata when deciding whether a receiver-side project mutation is allowed.

### Relationship to RFC 020

RFC 020 owns locked and reproducible build behavior. Mutation policy must respect locked or frozen project modes. If a mutation requires lockfile refresh or dependency changes, policy must see that as a dependency or lock-state effect rather than treating the project as still fully locked.

### Prior art

This RFC borrows broad lessons from ecosystem tooling that treats updates as reviewable proposals rather than silent rewrites. Dependency update bots, CI workflow policy, and framework condition reports are useful prior art, but Incan's receiver-side mutation model is stricter because templates and capabilities may write directly into the user's source tree.

## Alternatives considered

### Put all policy rules in RFC 074 and RFC 075

Rejected because template rendering and capability planning are already substantial. Keeping policy in a separate RFC lets 074 and 075 define the mutation facts while 076 defines how those facts are accepted, blocked, or recovered from.

### Trust source versioning and checksums alone

Rejected because integrity only says what source was selected. It does not answer whether the receiver should accept the rendered code change, whether the mutation category needs review, or what to do when a previously trusted source is later revoked.

### Let automation apply safe updates automatically

Rejected for v1 because "safe" depends on receiver policy, project context, and rendered diffs. Automation may propose updates, but applying them unattended should be an explicit policy decision outside the default model.

### Treat recovery as automatic rollback

Rejected because generated files may have been edited by users and because recovery itself can change source code, scripts, dependencies, or CI configuration. Recovery must be reviewable.

## Drawbacks

- Policy adds another concept to project lifecycle tooling.
- Strict receiver-side approval may make template and capability refresh slower than dependency updates.
- Machine-readable policy and risk output increases CLI and tooling surface area.
- Conservative defaults may block legitimate updates until users understand source pins, trust tiers, and approval settings.
- Recovery workflows can only be as precise as the recorded provenance and rendered hashes allow.

## Implementation architecture

The recommended implementation shape is to evaluate policy after RFC 074 or RFC 075 has produced a complete mutation plan and before any project files are written. The policy evaluator consumes source metadata, provenance, risk categories, rendered file diffs, manifest effects, and advisory state, then returns allow, warn, require-approval, block, quarantine, or recovery proposal outcomes.

The same policy result should power terminal diagnostics, machine-readable JSON, IDE code actions, CI checks, and automation-generated proposals. Tooling should avoid duplicating policy logic in editor plugins or agents.

## Layers affected

- **Manifest schema / configuration validation:** project tooling needs a place to describe project-local policy and record policy-related audit metadata, whether in `incan.toml`, a sidecar state file, or a future lock/state artifact.
- **CLI / tooling:** lifecycle commands must evaluate policy before applying template, starter, capability, update, reset, refresh, or recovery mutations.
- **LSP / IDE tooling:** editor-facing tools should surface policy outcomes, blocked mutation reasons, review requirements, and recovery actions from machine-readable lifecycle output.
- **Package and catalog integration:** registries and catalogs may provide source identity, integrity, yanking, revocation, advisory, compatibility, and trust-tier metadata that policy can consume.
- **Agentic tooling:** agents may consume policy output and propose patches, but policy must not allow them to approve their own receiver-side mutations.
- **Documentation:** user docs must explain the difference between provenance, policy, approval, quarantine, recovery, and receiver-owned mutation review.

## Unresolved questions

- Where should project-local mutation policy live: `incan.toml`, a sidecar policy file, or a future lock/state artifact?
- What is the minimum useful policy syntax for v1?
- Which risk categories should be standardized in v1, and which should remain extension labels?
- Should policy support explicit reviewer or owner requirements, or should it only emit categories for external review systems to interpret?
- How should organization-level policy be discovered without defining hosted product semantics in this RFC?
- Should quarantined sources prevent only future mutation, or should they also make ordinary build/test commands warn?
- What recovery actions should be required in v1: status-only, source replacement proposal, revert proposal, or quarantine marking?
- Should automated proposal creation be a lifecycle CLI command in v1, or should this RFC only define the output contract for future automation?
- What audit metadata is safe and useful enough to record without leaking private source or reviewer information?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
