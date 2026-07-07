# RFC 080: AI assets, models, prompts, evals, and agent metadata

- **Status:** Draft
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 033 (`ctx` typed configuration context)
    - RFC 034 (`incan.pub` package registry)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 079 (`incan.pub` artifact graph)
- **Issue:** https://github.com/encero-systems/incan/issues/408
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines a high-level model for AI-native assets in the Incan ecosystem: model references, prompt templates, adapters, datasets, eval suites, agent guidance, tool permissions, and local/cloud execution constraints. The goal is not to make AI implicit or magical; the goal is to make AI dependencies and workflows explicit, discoverable, reproducible, policy-gated, and connected to packages and capabilities through the same artifact graph used by the rest of the ecosystem.

## Core model

Read this RFC as seven foundations:

1. **AI assets are artifacts:** models, prompts, adapters, evals, datasets, and agent guidance have identity, metadata, provenance, compatibility, and policy state.
2. **Execution is explicit:** adding a model or prompt asset must not cause an agent or model to run automatically.
3. **Local and cloud are distinct:** an AI asset must describe whether it can run locally, remotely, or through either mode.
4. **Prompts are reviewable assets:** prompt templates and system messages are project-affecting behavior and must be inspectable.
5. **Evals are first-class:** AI capabilities should declare how users can test quality, safety, and regression behavior.
6. **Policy gates AI risk:** model downloads, remote inference, network access, data export, tool use, and project mutation are policy-relevant events.
7. **Agent metadata is descriptive:** packages and capabilities may advertise relevant agent skills or workflows, but they must not execute agents implicitly.

## Motivation

AI changes the shape of package ecosystems. A useful capability may include source code, templates, prompts, a local model, a remote model endpoint, embedding configuration, an eval suite, a data fixture, and agent guidance. Treating those as loose documentation leaves too much implicit: users cannot easily tell what model is required, what data may leave the machine, what evals validate behavior, or which prompts changed between versions.

Hugging Face demonstrates the value of metadata-rich model and dataset cards. Ollama demonstrates the value of a small local model blueprint with a base model, parameters, template, system message, adapters, and license. Incan should learn from both, but integrate AI assets with project capabilities, policy, typed actions, and `incan.pub` artifact discovery.

## Goals

- Define AI asset kinds for model references, prompt templates, adapters, datasets, evals, and agent guidance.
- Define metadata needed for discovery, reproducibility, policy, and review.
- Distinguish local execution from remote/cloud execution.
- Require prompt templates, system messages, and agent guidance to be inspectable and provenance-aware.
- Define how AI assets connect to capabilities, actions, policies, and the `incan.pub` artifact graph.
- Define evaluation metadata so AI-backed capabilities can declare quality and regression checks.
- Leave room for multiple model providers and local runtimes without hardcoding one AI platform.

## Non-Goals

- Defining a model runtime or inference server.
- Defining a hosted AI product, billing model, or provider-specific control plane.
- Defining one universal prompt template language.
- Guaranteeing model safety, truthfulness, privacy, or quality.
- Automatically running agents, model downloads, remote inference, or project mutations.
- Replacing model cards, dataset cards, or provider-native metadata where those already exist.

## Guide-level explanation

### Declaring a local model asset

A package or capability may declare a local model asset:

```toml
[[ai.models]]
id = "local-embedder"
kind = "embedding"
runtime = "local"
source = "ollama:nomic-embed-text"
license = "Apache-2.0"

[ai.models.constraints]
min-memory = "8GiB"
```

This declaration does not download or run the model by itself. It gives lifecycle tooling, policy, and IDEs enough information to explain what the capability expects.

### Declaring a prompt template

Prompt templates are project behavior and should be reviewable:

```toml
[[ai.prompts]]
id = "summarize-errors"
model = "local-embedder"
template = "prompts/summarize-errors.prompt"
purpose = "Summarize compiler diagnostics for developer assistance."
inputs = ["diagnostics"]
safety = ["no-network", "no-project-mutation"]
```

If a prompt changes, status and policy tooling should be able to show the diff like any other project-affecting artifact.

### Declaring evals

An AI-backed capability should declare evals:

```toml
[[ai.evals]]
id = "diagnostic-summary-regression"
kind = "golden"
dataset = "datasets/diagnostic-summaries.jsonl"
action = "eval-diagnostic-summary"
```

The eval action can be exposed through RFC 078 typed workflow actions.

## Reference-level explanation

### AI asset kinds

V1 should recognize these AI asset kinds:

- `model`
- `adapter`
- `prompt-template`
- `system-message`
- `dataset`
- `eval`
- `embedding-index`
- `tool-permission`
- `agent-guidance`

Implementations may add extension kinds, but unknown AI asset kinds must be visible in machine-readable output and should default to conservative policy handling.

### Model metadata

Model assets should include:

- stable id
- task kind such as text generation, embedding, classification, extraction, ranking, or reranking
- source identity and version or digest when available
- runtime kind such as local, remote, or either
- provider or runtime name when available
- license
- base model, adapter, quantization, or derivation metadata when available
- hardware or memory requirements when known
- context length or token limits when known
- privacy and data-retention labels when remote execution is involved
- intended use and limitations

### Prompt and system metadata

Prompt templates and system messages must be inspectable artifacts. They should include:

- stable id
- target model or model family
- purpose
- input and output shape
- template source path or artifact id
- safety labels
- mutation permissions
- data access requirements
- provenance and content hash when available

Prompt assets must not be hidden inside opaque agent guidance when they affect project behavior.

### Eval metadata

Eval assets should include:

- stable id
- eval kind such as golden, benchmark, safety, regression, human-review, or policy-check
- dataset reference
- target model, prompt, capability, or action
- metric names and expected thresholds when applicable
- local or remote execution requirement
- privacy labels for data used during evaluation

Eval results may become artifact graph metadata in RFC 079, but this RFC does not require registry-hosted eval execution.

### Agent guidance

Agent guidance metadata may reference skills, workflows, tools, docs, file roles, capabilities, and safety labels. It must remain descriptive and must not execute an agent automatically.

If agent guidance can lead to project mutation, network access, model invocation, or tool execution, it must declare those risk categories for RFC 076 policy and RFC 078 action planning.

### Local and remote execution

AI assets must distinguish local execution from remote execution. Remote execution is policy-relevant because it may send project data, prompts, diagnostics, source snippets, or user content to an external service.

Lifecycle tooling must surface local/cloud execution mode before running AI-backed actions. Non-interactive AI execution must not silently switch from local to remote or from one provider to another.

### Provenance and locking

AI assets should record enough provenance to reproduce or explain behavior:

- model source and version or digest
- adapter source and version or digest
- prompt template hash
- dataset hash or version
- eval suite version
- runtime version when available
- parameter values such as temperature, context length, seed, or embedding dimensions when relevant

If an AI action affects project files, generated artifacts, or diagnostics, the action output should reference the AI asset provenance used.

### Policy integration

RFC 076 policy may restrict:

- model sources
- local versus remote execution
- providers
- licenses
- data export
- model downloads
- prompt changes
- agent tool permissions
- project mutation by AI-backed actions
- use of unpinned or unknown model versions

Policy outcomes must be visible in machine-readable action and asset inspection.

## Design details

### Relationship to RFC 075

Capabilities may declare AI assets when the capability needs a model, prompt, eval, dataset, or agent guidance. Applying the capability records metadata; it does not run inference or agents implicitly.

### Relationship to RFC 078

AI-backed operations should be exposed as typed actions. Evaluation, embedding generation, prompt validation, model conversion, and agent workflows are action kinds or action providers subject to policy.

### Relationship to RFC 079

AI assets are artifact graph nodes. `incan.pub` and private catalogs may use cards to expose model lineage, datasets, evals, license, intended use, limitations, and compatibility.

### Relationship to RFC 033

AI configuration that should be compiler-visible should use typed `ctx` declarations or generated source files rather than untyped sidecar data. Operational metadata that the compiler should not understand can remain in descriptors or manifests.

## Alternatives considered

### Leave AI metadata as documentation only

Rejected because prompts, models, evals, and agent guidance affect behavior and safety. They need structured metadata for tooling, policy, and reproducibility.

### Hardcode one AI provider

Rejected because the ecosystem should support local runtimes, remote providers, private catalogs, and future model formats.

### Let agents infer everything from files

Rejected because implicit discovery is brittle and unsafe. Capabilities should expose intent explicitly.

### Automatically run AI setup during capability application

Rejected because model downloads, remote inference, and agent execution are policy-relevant operations and should be explicit.

## Drawbacks

- AI metadata introduces new concepts before Incan has a mature package ecosystem.
- Providers may over-declare or under-declare model limitations and eval quality.
- Local and remote execution constraints can be hard to model portably.
- Prompt and eval provenance adds more metadata to maintain.
- Policy may block convenient AI workflows until users configure trust settings.

## Implementation architecture

The recommended implementation shape is to start with descriptive AI asset metadata and typed action integration rather than runtime execution. Packages and capabilities declare assets; `incan pub` indexes them; CLI and IDE tooling inspect them; RFC 078 actions run them only when explicitly invoked and RFC 076 policy allows it.

This RFC should not block the lifecycle core. AI asset metadata becomes useful after the project can already record provenance, apply capabilities, evaluate policy, and expose typed actions. Early implementations should treat AI assets as descriptive metadata and action references first; model downloads, remote inference, agent workflow execution, and registry-hosted AI graph behavior should come only after RFC 074, RFC 075, RFC 076, and RFC 078 have a working local slice.

## Layers affected

- **Manifest schema / configuration validation:** packages and projects need AI asset metadata for models, prompts, evals, datasets, and agent guidance.
- **CLI / tooling:** lifecycle tooling needs AI asset inspection, status, provenance, and policy evaluation.
- **Action execution:** RFC 078 actions need AI-backed execution modes and risk categories.
- **Package and catalog integration:** `incan.pub` should index AI assets as artifact graph nodes with cards and relationships.
- **LSP / IDE tooling:** editor tooling may surface AI capabilities, prompts, evals, and policy warnings.
- **Agentic tooling:** agents may consume asset metadata but must respect policy and explicit user approval.
- **Documentation:** docs must explain local versus remote AI execution, model provenance, prompt review, and evals.

## Unresolved questions

- Which AI asset kinds should be included in v1?
- Should Incan define its own prompt template syntax or only reference prompt files with declared input/output metadata?
- What metadata is required for remote providers without hardcoding provider-specific fields?
- How should model digests and local model cache identities be represented?
- Should eval results be stored in project state, registry cards, both, or neither in v1?
- What privacy labels are required before remote AI actions can be considered safe enough to standardize?
- Should model downloads be handled by `incan tool`, `incan pub`, provider-specific CLIs, or a future AI runtime command?
- How should agent guidance reference skills without becoming an agent marketplace spec?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
