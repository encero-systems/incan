---
title: "Incan ecosystem north star"
status: "Snapshot"
snapshot_date: "2026-04-26"
authors:
  - "Danny Meijer"
audience:
  - "Incan language and tooling contributors"
  - "Package, registry, IDE, and agent-tooling designers"
scope: "Directional architecture for project lifecycle, package use, tooling, policy, registry discovery, and AI assets."
normative: false
related_rfc:
  - "RFC 074"
  - "RFC 075"
  - "RFC 076"
  - "RFC 077"
  - "RFC 078"
  - "RFC 079"
  - "RFC 080"
research_context:
  - "Spring Boot starters and auto-configuration"
  - "npm init, npm scripts, package lockfiles, and trusted publishing"
  - "PyPI metadata, trusted publishing, and attestations"
  - "Hatch project-management workflows"
  - "Cargo and uv workspace/tool execution models"
  - "Hugging Face model cards and Ollama Modelfiles"
review_after: "2026-06-30"
---

# Incan ecosystem north star

--8<-- "_snippets/callouts/whitepaper_status.md"

## Abstract

Incan needs more than a package manager if it is going to support real project evolution, registry discovery, IDE workflows, generated files, and AI-aware tooling. The proposed solution shape is an ecosystem lifecycle where packages, templates, starters, capabilities, policy, workspaces, typed actions, registry artifacts, and AI assets are all typed and inspectable, while local projects retain authority over every mutation. This snapshot synthesizes lessons from Spring Boot, npm, PyPI, Hatch, Cargo, uv, GitHub Actions, Hugging Face, and Ollama, then applies them to the Incan toolchain. The main pressure point is scope control: the north star is broad, but the implementation must branch into focused RFCs rather than becoming one oversized feature.

## North star

Incan should become an ecosystem where project shape, package use, tool execution, policy, registry discovery, and AI assets are one coherent lifecycle:

> Every ecosystem object that can change a project, guide a tool, or influence AI behavior should be a typed, inspectable artifact; every project mutation should be proposed with provenance, policy, and a receiver-owned review path.

That is the line that holds this direction together. The goal is not to copy Spring Boot, npm, PyPI, Hatch, Cargo, uv, Hugging Face, or Ollama literally. The goal is to absorb the ecosystem lessons that made those tools useful, then fit them to Incan's constraints: explicit source, reviewable project evolution, deterministic tooling contracts, and strong local authority.

This direction gives Incan a bigger identity than "a language plus a package manager." It makes Incan a language ecosystem where the project lifecycle is understandable to humans, CLIs, IDEs, registries, CI, and agents through the same metadata.

## Worked examples

The north star is easiest to judge through concrete project moves. These examples are illustrative; the exact command spelling can change.

### Example 1: making a library usable by other people

A user has a small Incan library that started as internal code. It works for them because they know which function to call, which sample file to run, and which local command proves it still works. Now the library is becoming shared. A teammate wants to try it from the terminal. CI needs a stable smoke test. The docs need one command readers can copy. An IDE should know which file is the runnable entrypoint instead of guessing from filenames.

The user is not thinking, "I need a CLI package." They are thinking, "I need this library to become a usable tool without hand-assembling the project glue."

In many ecosystems this turns into a messy choice:

- install a package and then follow README instructions
- copy a sample `main` file from docs
- hand-add scripts to the manifest
- teach CI a slightly different command
- hope the IDE figures out what to run
- rerun a generator later and manually inspect what it overwrote

The ideal Incan flow should let the user describe the project outcome they want. In this case the outcome is "make this project runnable as a CLI-shaped tool."

They discover that outcome by intent:

```text
incan pub search capability:cli
incan capability show cli
```

The result is not just a package name. The registry can show the package that provides the capability, the files it may create, the actions it advertises, the required Incan version, the source identity, and known advisory state. The user discovers a project outcome, not a README scavenger hunt.

The user can still add only the dependency when that is all they need:

```text
incan add app-cli
```

That operation should not create source files or scripts. It is dependency addition. This keeps experimentation cheap and avoids the npm-style trap where adding something can also activate hidden project behavior.

When the user asks for the project to become runnable, the lifecycle CLI plans the change before writing:

```text
incan capability add cli --dry-run
```

The dry-run should show a receiver-side plan:

```text
Capability: cli
Source: incan.pub package app-cli 0.3.1

Files:
  create   src/main.incn              ownership=bootstrap
  create   tests/test_cli.incn        ownership=bootstrap

Manifest:
  add dependency app-cli = "0.3.1"
  add action run-cli kind=run
  add action test-cli kind=test

Risk:
  source, test, manifest, action

Policy:
  require-approval for executable source creation
```

Now the user sees the whole bargain before accepting it: an entrypoint, a test skeleton, manifest metadata, two typed actions, and a policy callout that executable source is being created. The framework is doing the project-glue work, but it is not hiding the patch.

The CLI can write only after policy is satisfied:

```text
incan capability add cli
```

Afterward the project has ordinary files, not hidden framework state. The immediate DX improvement is that every tool sees the same project shape:

- the terminal can run the tool without knowing package-specific commands
- the IDE can show a run button for the entrypoint
- CI can discover the relevant test action
- docs can point to the same action instead of a bespoke command
- an agent can select the relevant edit workflow without scanning filenames
- future update tooling can explain which files came from the capability

IDE tooling can see file roles, provenance, and capability status. It can also ask for the project's typed actions:

```text
incan action list
incan capability status --format json
incan template status --format json
```

`incan action list` is not meant to be a generic shell-script directory. It would list the project workflows that the toolchain understands because they were declared by the project, a starter, a capability, a package, or a workspace. In this example it might report:

```text
run-cli    kind=run   source=capability:cli   mutates=none
test-cli   kind=test  source=capability:cli   mutates=none
```

That gives CLIs, IDEs, CI, and agents a shared answer to "what can this project do, where did that command come from, and what risk does running it carry?" The user benefits because run/debug/test affordances no longer depend on every integration reverse-engineering project conventions.

If the upstream capability later changes, automation may propose an update, but it must show the rendered diff that would land in this project. It cannot approve its own mutation. The user gets an upgrade path for boilerplate without surrendering ownership of their source tree.

This is the gap the example is solving. Incan can already compile and run code, but it does not yet have a first-class way to say: "turn this project into this kind of usable thing, show me the patch, record where it came from, expose the workflows to every tool, and keep future updates reviewable." Without that lifecycle layer, each integration has to invent its own conventions: package READMEs, starter copies, shell scripts, CI snippets, IDE heuristics, and agent instructions. The north-star proposal turns that scattered setup knowledge into typed project metadata and receiver-owned project mutation.

The same example exercises the core rule: packages are dependencies, capabilities are explicit project concerns, templates are rendered files with provenance, actions are typed workflows, and policy decides whether the project accepts the patch.

### Example 2: creating a sample query project

A user wants a typed data project with sample query files, tests, and one local validation action. The DX problem is not only "create some files." The user wants a project where query files, model files, validation actions, examples, and tests are already connected so the IDE can catch query-shape mistakes before anything reaches a warehouse.

They start with a starter:

```text
incan new revenue_model --starter sample_query.project --dry-run
```

The starter composes several smaller concerns: a sample query project layout, a basic testing capability, query examples, typed actions, and optional data-quality scaffolding. That saves the user from assembling a data-project shape by hand and gives tools a stable contract for "this is a query project."

The plan should explain the composition rather than hiding it:

```text
Starter: sample_query.project

Applies:
  capability sample_query.query
  capability testing.basic
  template sample_query.model-example
  template sample_query.query-example

Actions:
  test              kind=test
  validate-query    kind=validate
  explain-query     kind=run

Files:
  create src/models.incn
  create src/queries/revenue.incn
  create tests/test_revenue.incn
```

Once applied, the project should be self-describing. The IDE can understand that `src/queries/revenue.incn` is query code, that `validate-query` is the relevant action, and that generated starter content is bootstrap-owned rather than managed by future updates. The practical payoff is faster feedback: query validation, explain output, and tests become discoverable project actions instead of tribal knowledge.

The user can then add a package or adapter separately:

```text
incan add sample-query-duckdb
incan capability add sample_query.adapter.duckdb --dry-run
```

The adapter capability may add an action, manifest metadata, and configuration placeholders. Policy can treat this differently from ordinary source creation because it may introduce external execution, local files, credentials, or network access.

This example shows why the registry must be more than package storage. A user is not searching only for "which tarball contains the code?" They are searching for a usable project concern: query authoring, validation, explain, adapter binding, examples, and IDE-visible file roles.

This is the gap the example is solving. Incan can express models and run code, but it does not yet have a first-class way to say: "this is a typed query project; connect the model files, query files, validation actions, explain actions, tests, adapter setup, and IDE affordances." Without that lifecycle layer, each query project has to assemble its own conventions for where queries live, how they are validated, how adapters are added, and which command proves the project is healthy. The north-star proposal turns that query-project shape into discoverable, repeatable, tool-visible metadata.

### Example 3: creating a synthetic application with governed analytics

A user wants an application starter with a page, one server action, and one analytics read backed by a typed data slice. The DX problem is that a "full-stack app" usually starts with several disconnected setup chores: UI files, server action conventions, local run commands, test wiring, config, and data access. The user wants a runnable application shape by the end of the first session, not a blank framework shell.

They start from a public starter:

```text
incan new customer_console --starter sample_app.console --dry-run
```

The dry-run should reveal the full lifecycle shape before it creates files:

```text
Starter: sample_app.console

Files:
  create src/app.incn
  create src/actions/update_customer.incn
  create src/pages/customer_overview.incn
  create tests/test_actions.incn

Capabilities:
  sample_app.app
  sample_app.server-actions
  sample_query.analytics-read

Actions:
  run-local       kind=serve
  test            kind=test
  validate-app    kind=validate

Policy-sensitive categories:
  source, test, action, config, analytics-binding
```

The benefit is not just faster scaffolding. The user gets a project where local preview, tests, server actions, and analytics validation are all declared as typed actions with source and policy metadata. If the analytics slice is provided by a package or private catalog, the descriptor source must be visible. If it references a template, template provenance records the origin. If it exposes an action that calls a remote service, policy sees that before execution.

An agent can consume the same metadata without becoming privileged:

```text
incan capability status --format json
incan action list --format json
incan policy check --format json
```

The agent can learn which files are server actions, which tests validate them, which analytics binding matters, and which operations require review. It still cannot approve its own generated patch.

This is the gap the example is solving. Incan can compile application code, but it does not yet have a first-class way to say: "create a runnable application shape with pages, server actions, analytics bindings, preview workflows, tests, policy-sensitive config, and starter provenance." Without that lifecycle layer, the first app experience depends on copied folders, handwritten setup docs, framework-specific scripts, and IDE guesses. The north-star proposal makes the application shape explicit enough for local tooling, CI, IDEs, agents, and future update proposals to agree on what was created and how it should evolve.

This example shows the end-state promise: project creation, application structure, data access, actions, provenance, policy, IDE support, and agent guidance can all flow through one lifecycle without turning the registry into a remote mutator.

## Conclusions from the examples

The examples point to a few conclusions.

First, `incan add <pkg>` must stay boring. It should add a dependency and perhaps report advertised capabilities, but it should not rewrite the project.

Second, project concerns need their own explicit unit. A CLI, query adapter, data-quality pack, server-action setup, or AI eval workflow is more than a package and less than a framework. That unit is a capability.

Third, templates need provenance because generated files do not stop mattering after creation. Some are bootstrap examples the user owns immediately. Some are managed configuration files that can be updated. Some are advisory origin records. Treating all generated files the same would either block useful updates or risk overwriting user work.

Fourth, actions need types and risk labels. `test`, `serve`, `generate`, `validate`, `publish`, and `eval` are not the same operation. Tools, IDEs, CI, and agents need to know what an action does before running it.

Fifth, the registry is most valuable when it models relationships. The useful question is often "how do I add a CLI?", "how do I validate a query?", or "which starter gives me an application with analytics?", not only "which package name should I install?"

Sixth, policy is not a late enterprise feature. It is required as soon as templates, capabilities, actions, and AI assets can affect source, scripts, credentials, network access, or agent behavior.

## Concept breakdown

The command surface should keep the ecosystem nouns crisp:

- **Package:** dependency code that can be built, imported, or used by the project.
- **Template:** static provider-side file rendered into an ordinary project file.
- **Starter:** project creation or initialization recipe.
- **Capability:** explicit project concern that can add files, dependencies, actions, metadata, and guidance.
- **Policy:** receiver-side decision layer for whether a mutation or action is allowed.
- **Workspace:** project topology for multiple related members.
- **Action:** typed workflow operation such as test, run, validate, generate, serve, publish, audit, or eval.
- **Artifact graph:** registry model that relates packages, templates, capabilities, actions, examples, docs, policies, advisories, and AI assets.
- **AI asset:** model, prompt, adapter, dataset, eval, embedding index, or agent guidance artifact with provenance and policy metadata.

## Design principles

### 1. Receiver-owned mutation

Anything that writes into a user's project is a receiver-side mutation. A package, registry, catalog, template provider, agent, or automation service may propose a change, but the receiving project owns acceptance.

This is stricter than ordinary package installation because templates and capabilities can write source files, scripts, CI configuration, env definitions, manifests, and agent guidance. Integrity metadata can prove what was selected, but it does not prove that the receiver should accept the rendered project diff.

### 2. Ordinary files remain ordinary

Generated project files should be source-controlled, reviewable, editable, and understandable. Incan should avoid hidden generated source trees for project boilerplate. If a starter creates `src/main.incn`, that file is part of the project.

The ecosystem can still track origin through provenance, but provenance must not turn files into untouchable tool-owned blobs. RFC 074's `bootstrap`, `managed`, and `advisory` ownership categories are important because not all generated files have the same lifecycle.

### 3. Descriptors are contracts, not programs

Templates, starters, capabilities, actions, and AI assets should be described as structured metadata. V1 should avoid arbitrary provider scripts, install hooks, expression languages, and hidden plugin execution for project mutation.

That constraint is not a lack of ambition. It is the thing that makes dry runs, IDE support, policy, provenance, update review, and agent consumption possible. If a descriptor is data, tooling can explain it before anything runs.

### 4. One machine-readable truth

The CLI, LSP, IDE plugins, docs tooling, CI, and agents should consume the same machine-readable inspection surfaces. Editor integrations should be thin UI over lifecycle and compiler knowledge; they should not reverse-engineer semantics from filenames, conventions, or copied snippets.

This is where Incan can outgrow traditional scaffolding. The project lifecycle should expose enough metadata for an IDE to explain:

- which capabilities are enabled
- which files came from templates
- which generated files are managed versus bootstrap-owned
- which actions are available
- which policies block a proposed update
- which agent guidance or AI assets are relevant
- which workspace member a command or mutation targets

### 5. Registry discovery, local execution

`incan.pub` should become the public artifact graph for discovery, provenance, compatibility, advisories, and trust metadata. It should not be the authority that mutates local projects.

The registry can answer "what exists?", "who published it?", "what does it provide?", "what is it compatible with?", "is it yanked or affected by an advisory?", and "what relationships does it have?" The local lifecycle CLI answers "what would this do to my project?", "does policy allow it?", and "what files would change?"

### 6. Policy is not optional plumbing

The security model is part of the product. A template update can inject code. A capability refresh can add a script. A typed action can mutate source. An AI-backed action can send project data to a remote provider. Agent guidance can influence project-changing workflows.

Those are not all the same risk. RFC 076's job is to classify them, make decisions explainable, and keep approval separate from proposal generation.

### 7. AI is explicit ecosystem metadata

AI assets should be first-class, but not magical. Models, prompts, evals, datasets, adapters, and agent guidance should have identity, provenance, policy state, and typed relationships to packages and capabilities.

AI should enter the ecosystem through the same rule as everything else: if it can influence behavior, mutation, data flow, or developer decisions, it should be inspectable and policy-relevant.

## Operating model

The ecosystem should be deliberately layered:

```text
incan.pub artifact graph
  discovers packages, templates, capabilities, actions, policies, AI assets

local lifecycle CLI
  resolves sources, builds plans, renders templates, evaluates policy, writes files

project/workspace manifests and provenance
  record project shape, capabilities, envs, actions, sources, generated-file origin

compiler and runtime
  compile and run ordinary Incan source; no hidden privileges for generated files

LSP, IDE, CI, docs, agents
  consume the same machine-readable inspection and policy output
```

## Prior art and the specific lessons

Spring Boot shows the value of convention plus conditional activation. Its auto-configuration model uses conditions to decide "when the auto-configuration should apply" and starters bundle typical dependencies with that setup. Incan should borrow the user-facing clarity of "add this concern and the project becomes ready for it," but not the hidden runtime activation model. Capabilities should produce explicit project mutations and metadata.

Source: [Spring Boot auto-configuration documentation](https://docs.spring.io/spring-boot/reference/features/developing-auto-configuration.html).

npm shows the power of a common project command surface and easy project initialization. npm documents that `npm init <initializer>` can set up a package and that the initializer package has its "main bin executed." npm scripts also support "arbitrary scripts." Incan should borrow the ergonomic center of gravity, but should make project mutation typed, dry-runnable, and policy-gated instead of defaulting to arbitrary executable setup.

Sources: [npm init](https://docs.npmjs.com/cli/v11/commands/npm-init/) and [npm scripts](https://docs.npmjs.com/cli/v11/using-npm/scripts/).

npm lockfiles also demonstrate why readable, committed state matters. npm says `package-lock.json` describes the exact generated tree so future installs can generate "identical trees" and provide "readable source control diffs." Incan should apply that lesson beyond dependencies: template provenance, capability provenance, action sources, and AI asset provenance should make ecosystem state reviewable.

Source: [npm package-lock.json documentation](https://docs.npmjs.com/cli/v11/configuring-npm/package-lock-json/).

PyPI shows the value and the limits of a huge public package index. It does well at central package distribution, project pages, release files, metadata, project links, hashes, trusted publishing, and attestations. PyPI's own metadata docs say project URLs are rendered and split into "verified" and "unverified" groups, and its attestation docs say signatures bind a distribution to "a strong cryptographic digest of its contents." Those are real strengths for discovery and provenance. The limitation is that PyPI remains package-centered: metadata helps users judge packages, but it does not describe project capabilities, templates, typed actions, generated-file ownership, or receiver-side mutation plans. Its verified-link docs also warn that verification "does not imply any additional safety." Incan should borrow PyPI's metadata and publishing discipline without stopping at a package index.

Sources: [PyPI project metadata](https://docs.pypi.org/project_metadata/) and [PyPI attestations](https://docs.pypi.org/attestations/).

Hatch shows the value of one project CLI that combines scaffolding, environments, builds, testing, publishing, versioning, script running, and sane defaults. Hatch describes itself as a "modern, extensible Python project manager" and highlights environment management, project generation, and publishing. The useful lesson is workflow coherence: a project should have one obvious command surface for common development tasks. The limitation is that Hatch is primarily a Python project-management CLI; it does not make package-provided capabilities, registry artifact graphs, receiver-side template provenance, policy-gated mutation, or AI asset metadata part of the language ecosystem contract. Incan should borrow the coherent CLI posture, but push more semantics into typed project metadata.

Source: [Hatch documentation](https://hatch.pypa.io/1.16/).

Cargo shows why workspaces are a core scaling primitive. Cargo defines a workspace as packages "managed together" and emphasizes common commands plus a shared lockfile. Incan should borrow that topology model, while integrating it with envs, policy, capabilities, template provenance, and artifact graph publication.

Source: [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html).

uv shows that modern tooling wins when project execution, workspaces, and isolated tools are fast and obvious. uv workspaces require member globs and support applications and libraries in one context; uv also says `uvx` invokes a tool "without installing it." Incan should borrow the fast workflow shape and isolated tool idea, while preserving typed actions and policy metadata.

Sources: [uv workspaces](https://docs.astral.sh/uv/concepts/projects/workspaces/) and [uv tools](https://docs.astral.sh/uv/guides/tools/).

GitHub Actions and Dependabot show two relevant security lessons. GitHub recommends: "Pin actions to a full-length commit SHA." Dependabot updates are review-first: when it finds an outdated dependency, it "raises a pull request." Incan should treat template and capability updates similarly: detect staleness or risk, propose a reviewable change, and keep approval independent from the producer.

Sources: [GitHub Actions secure use](https://docs.github.com/en/actions/reference/security/secure-use) and [Dependabot version updates](https://docs.github.com/en/code-security/concepts/supply-chain-security/about-dependabot-version-updates).

Hugging Face shows that AI artifacts need metadata-rich cards, not just binary blobs. Its model-card metadata includes fields such as language, tags, license, datasets, and base model, and it discourages relying on automatic detection when explicit metadata is available. Incan should borrow that explicit metadata posture and attach AI assets to the same artifact graph as packages and capabilities.

Source: [Hugging Face model cards](https://huggingface.co/docs/hub/model-cards).

Ollama shows that a compact model blueprint can describe base model, parameters, prompt template, system message, adapters, license, and version requirements. Incan should borrow the idea that model behavior is describable as reviewable metadata, while keeping execution explicit and policy-gated.

Source: [Ollama Modelfile reference](https://docs.ollama.com/modelfile).

## The `incan.pub` role

`incan.pub` should become the ecosystem's context engine.

As a package registry, it stores and serves packages. As an artifact graph, it also understands the things around packages:

- templates a package provides
- capabilities a package advertises
- starters that compose capabilities
- typed actions and tools
- workspace-aware project shapes
- docs and examples
- policy fragments and advisories
- AI models, adapters, prompt templates, datasets, evals, and agent guidance

The public registry should be useful even for local-only workflows because it can answer discovery and trust questions. It can tell the CLI and IDE which capability provides a CLI entrypoint, which package advertises a data-quality workflow, which template is superseded, which action implements an eval, which prompt changed, and which model asset has a stricter privacy label.

Private catalogs should share the same descriptor semantics. A private catalog may describe a broad end-to-end application blueprint that is not scoped to one public library: CLI, API, analytics, governance, data-quality checks, deployment conventions, and relevant agent guidance. That should not require a different local mutation system. The descriptor source is different; the receiver-owned planning, policy, and provenance model is the same.

This division matters:

- `incan.pub` and catalogs discover, verify, relate, and distribute artifacts.
- The local lifecycle CLI resolves project context, renders templates, builds mutation plans, evaluates policy, and writes files.
- IDEs, CI, and agents consume the same inspection output rather than inventing their own interpretation.

## IDEs, tooling, and agents

This north star only works if descriptors are useful outside the CLI.

An IDE should be able to show project capabilities, file roles, action buttons, blocked policy reasons, template provenance, stale generated files, AI asset privacy labels, and workspace member scope without parsing human terminal output.

A CI job should be able to run:

```text
incan capability status --format json
incan template status --format json
incan action list --format json
incan policy check --format json
```

An agent should be able to ask the project what matters before editing:

- Which capability owns this concern?
- Which files are bootstrap content versus managed template output?
- Which tests or evals are relevant?
- Which action should validate the change?
- Does policy allow this mutation?
- Is this workspace member the right scope?

Agent guidance metadata should help select relevant skills and workflows, but it must not be a hidden execution path. Agent runtimes remain responsible for user confirmation, permissions, and tool safety. The Incan ecosystem provides structured project intent; it does not grant agents special authority.

## What this is not

This is not a plan to make imports mutate projects. `incan add <pkg>` should remain dependency addition. Capability activation is explicit.

This is not a plan to make the registry apply patches to local repositories. The registry discovers and verifies. The local lifecycle CLI plans and applies.

This is not a plan to turn every project into a framework project. Small projects should remain small. Workspaces, capabilities, template provenance, AI assets, and policy should scale down.

This is not a plan to make arbitrary scripts the extension model. Shell escape hatches may exist, but they are not the foundation for project mutation.

This is not a plan to make AI implicit. AI-backed actions, remote inference, model downloads, prompt changes, data export, and agent workflows are explicit and policy-relevant.

## Success criteria

This direction is working when these things become true:

- A user can understand the difference between a package, a capability, a starter, a template, an action, and an AI asset from CLI and IDE output.
- A package can advertise a capability without plain dependency addition mutating the project.
- A capability can be applied to an existing project with a dry-run plan that explains conflicts, skipped files, manifest changes, actions, policy outcomes, and provenance.
- A generated file can be explained later: source, template hash, rendered hash, ownership, descriptor source, and update status.
- A template or capability update shows the rendered project diff, not only the provider-side descriptor diff.
- A workspace command never silently targets more members than the user selected.
- An IDE can discover actions, capabilities, file roles, policy warnings, and AI assets from machine-readable lifecycle output.
- `incan.pub` can answer intent-driven questions, not only package-name queries.
- AI prompts, models, evals, and agent guidance are visible artifacts with provenance and policy state.
- Automation can propose updates, but cannot approve its own receiver-side project mutation.

## Hard problems to keep visible

The biggest risk is conceptual bloat. The answer is not to collapse the RFCs back into one feature. The answer is to keep the nouns crisp and the command surface honest:

- packages are dependencies
- templates render files
- starters create project shapes
- capabilities add project concerns
- policy decides whether mutations are allowed
- workspaces define topology
- actions run typed workflows
- `incan.pub` discovers and verifies artifacts
- AI assets describe model/prompt/eval/agent metadata

Another risk is under-specifying provenance. If provenance is too weak, update and recovery workflows become theater. The selected source identity, immutable version or content hash, rendered hash, publisher or trust metadata, and ownership policy need to survive beyond the first command.

A third risk is pretending dry-run solves everything. Dry-run is necessary, but it is not sufficient. Reviewers need rendered diffs, risk categories, source identity changes, advisory state, policy outcomes, and enough context to understand why a patch exists.

A fourth risk is letting AI metadata become a prompt marketplace in disguise. The Incan-specific value is not generic prompt sharing. The value is connecting AI assets to concrete project capabilities, typed actions, evals, data access constraints, and local policy.

## Sequencing discipline

This whitepaper describes the north star, not a claim that every RFC seam is already perfectly sliced. The current draft set is promising, but it still contains overlapping edges. Implementation should therefore protect sequencing more aggressively than ambition.

The core lifecycle slice is:

1. RFC 074: render static templates into ordinary project files with provenance and ownership.
2. RFC 075: apply starters and capabilities as explicit, dry-runnable project mutation plans.
3. RFC 076: evaluate those mutation plans through receiver-side policy.
4. RFC 078: expose typed actions contributed by the project, starters, capabilities, packages, and workspaces.

That slice should be proven before broader surfaces are allowed to block it. RFC 077 extends the lifecycle across workspace topology, but a single-project lifecycle can land first. RFC 079 describes the registry graph that can index and distribute lifecycle artifacts, but rich graph semantics must not block local lifecycle behavior. RFC 080 describes AI-native artifacts that should attach to the same action, provenance, and policy machinery once it exists; it must not block lifecycle implementation.

The sequencing rule is: local, receiver-owned lifecycle first; workspace, registry graph, and AI breadth after the lifecycle contract is real.

## Open north-star decisions

These are the decisions that most affect the final shape:

1. Where does rich provenance live: `incan.toml`, `incan.lock`, a sidecar state file, or a future lifecycle state artifact?
2. Which command names should be stable enough to teach early: `incan capability add`, `incan action run`, `incan template update`, `incan pub search`, and `incan ai asset list` are descriptive, but may still be too wide.
3. How much registry-backed discovery belongs in the first implementation versus built-in and local descriptors?
4. What is the minimum policy language that can block dangerous template, capability, action, and AI mutations without creating a heavyweight governance system?
5. Which metadata vocabulary must be standardized in v1: file roles, action kinds, risk categories, AI asset kinds, applicability states, and relationship kinds?
6. How should private catalogs integrate without making hosted product assumptions in the language RFCs?
7. What is the first end-to-end demo that proves the model: a CLI app capability, a sample query project starter, a workspace starter, or an AI eval-backed capability?

## Branch-out RFCs

The whitepaper stands on its own as the ecosystem direction. The concrete branch-out work lives in seven focused RFCs:

- [RFC 074: template rendering and boilerplate provenance](../RFCs/074_template_rendering_and_boilerplate_provenance.md) defines how static provider-side templates become validated project files and how provenance, ownership, update, and reset behavior work.
- [RFC 075: starter profiles and capability packs](../RFCs/075_starter_profiles_and_capability_packs.md) defines explicit project recipes for starters and capabilities, including mutation plans, applicability, file roles, actions, and agent guidance metadata.
- [RFC 076: project mutation policy and recovery](../RFCs/076_project_mutation_policy_and_recovery.md) defines receiver-side policy, risk classification, approval, quarantine, advisory handling, and recovery for project mutations.
- [RFC 077: workspace and multi-package projects](../RFCs/closed/implemented/077_workspace_and_multi_package_projects.md) defines workspace topology, member selection, shared lock and policy state, and workspace-scoped mutations.
- [RFC 078: tool execution and typed workflow actions](../RFCs/078_tool_execution_and_typed_workflow_actions.md) defines typed actions, tool sources, execution modes, dry-run behavior, and policy-gated workflow execution.
- [RFC 079: incan.pub artifact graph](../RFCs/079_incan_pub_artifact_graph.md) defines the registry as an artifact graph across packages, templates, capabilities, actions, policies, examples, advisories, and AI assets.
- [RFC 080: AI assets, models, prompts, evals, and agent metadata](../RFCs/080_ai_assets_models_prompts_evals_and_agent_metadata.md) defines AI-native artifacts, local versus remote execution metadata, prompts, evals, datasets, model provenance, and agent guidance.

## Proposed incremental path

The north star is large, but the first implementation should prove one full vertical slice rather than many disconnected stubs.

The most useful first slice is:

1. RFC 074 minimal static templates with typed parameters, path safety, Incan parse validation, generated-file ownership, and provenance.
2. RFC 075 one built-in starter and one built-in capability that use those templates and produce a dry-run mutation plan.
3. RFC 076 minimal local policy evaluation over that plan, including risk categories and approval behavior.
4. RFC 078 typed actions emitted by the starter or capability, visible through machine-readable action listing.
5. IDE/LSP consumption of the same machine-readable outputs for capability status, file roles, and actions.

That slice proves the contract: a descriptor creates a reviewable project mutation, provenance explains it later, policy gates it, and tooling sees it. After that, workspaces, registry graph discovery, private catalogs, richer actions, and AI assets can attach without changing the core trust model.

The long-term win is not any single command. The long-term win is that Incan projects become self-describing enough for humans and tools to evolve them safely.
