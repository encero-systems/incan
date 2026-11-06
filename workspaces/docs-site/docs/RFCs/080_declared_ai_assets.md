# RFC 080: Declared AI assets

- **Status:** Planned
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 075 (starter profiles and mixes)
    - RFC 076 (project mutation policy and `oven audit`)
    - RFC 078 (typed workflow actions)
    - RFC 079 (`loaves.pub` artifact graph)
    - RFC 104 (capabilities and receipts)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 124 (Oven store unit identity)
    - RFC 125 (`loaves.pub` registry)
- **Issue:** https://github.com/encero-systems/incan/issues/408
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

A project can depend on a model or a prompt, and those affect behavior as much as code does. `[assets.<id>]` in `loaf.toml` declares them, so each has an identity, a change to one is reviewable, policy can restrict it, and `oven audit` reports it. Declaring an asset never downloads or runs anything. Incan owns only this general part. Providers, runtimes, measurements, model selection and eval metrics change far more often than a compiler release, so they belong to a higher-level framework, which builds on these declarations.

## Core model

1. **An asset is a declared artifact.** It has a kind, a source or a path, and a digest.
2. **Declaring is not running.** Nothing downloads or executes until an RFC 078 action explicitly does it.
3. **Local and remote are distinct.** Sending data to a remote model is a policy question, so a model says which it is.
4. **Unknown means riskiest.** A missing digest or an unknown retention label is visible, and policy can refuse it.
5. **Incan stays general.** It knows no provider, runtime or metric, and a higher layer can attach its own metadata without changing Incan.

## Motivation

A project may need a local embedding model, a prompt, a dataset and an eval that checks them. Treated as loose files and documentation, these leave too much implicit: nobody can easily tell which model is required, whether data leaves the machine, or that a prompt changed between versions. A prompt change can matter as much as a code change, and it should be just as visible.

Hugging Face shows the value of metadata-rich model and dataset cards, and Ollama shows the value of a small local model blueprint. Those details also change quickly. A compiler release is the wrong place to track them, while what every project needs, identity, review and policy, is stable.

## Goals

- Let a project declare models, prompts, datasets, evals and agent guidance.
- Make a changed asset reviewable, and an unpinned one visible.
- Let RFC 076 policy restrict sources, licenses, and local versus remote execution.
- Report assets through `oven audit`, alongside packages.
- Let a higher layer extend the declarations without changing Incan.

## Non-Goals

- Defining a model runtime, inference server, or provider integration.
- Provider-specific fields, execution profiles and measurements, model selection and fallbacks, eval metrics and thresholds, or agent guidance formats. A higher-level framework owns these.
- A prompt template language. Prompts are files.
- Guaranteeing model safety, truthfulness, privacy, or quality.
- An agent marketplace.

## Guide-level explanation

### Declaring assets

```toml
[assets.embedder]
kind = "model"
source = "ollama:nomic-embed-text"
digest = "sha256:..."
execution = "local"
license = "Apache-2.0"

[assets.summarizer]
kind = "model"
source = "provider:summary-large"
execution = "remote"
retains-data = "no"

[assets.summarize-errors]
kind = "prompt"
path = "prompts/summarize-errors.prompt"
for = "summarizer"
purpose = "Summarize compiler diagnostics for developers."

[assets.summary-goldens]
kind = "dataset"
path = "datasets/diagnostic-summaries.jsonl"

[assets.summary-regression]
kind = "eval"
action = "eval-summaries"
dataset = "summary-goldens"
```

None of this downloads or runs anything. A prompt or dataset in the repository is reviewed like any other file, and `summarizer` above has no digest, so it shows up as unpinned.

### Using an asset

An action says which assets it uses:

```incan
@action(kind=ActionKind.Eval, uses=["summarizer", "summarize-errors"], caps=[host.model.invoke])
def eval_summaries() -> Result[EvalReport, EvalError]:
    ...
```

`oven action run eval-summaries --dry-run` shows the assets and whether data would leave the machine, and policy can refuse before anything runs. The receipt records which assets ran, by digest. Downloading a model is an ordinary action using `host.http.request`, not a new command.

### Restricting assets

```toml
[[policy.assets]]
execution = "remote"
outcome = "require-approval"

[[policy.assets]]
pinned = false
outcome = "block"
```

### Checking a project

`oven audit` reports assets beside packages: an unpinned model, a remote model with unknown retention, or a file that no longer matches a pinned digest.

### Building on this

A higher-level framework adds what it needs under an `x-` table, which Incan keeps and shows but does not interpret:

```toml
[assets.embedder.x-runtime]
quantization = "q4"
min-memory = "8GiB"
```

## Reference-level explanation

### Declarations

1. `[assets.<id>]` in `loaf.toml` must declare an asset. `kind` must be `model`, `prompt`, `dataset`, `eval` or `agent-guidance`, or an extension prefixed `x-`. An unknown kind must stay visible in machine-readable output and must default to `require-approval`.
2. An asset must name exactly one of `source` or `path`, except an `eval`, which must name an `action` from RFC 078 and may name a `dataset`.
3. `digest` must use the same content digest form as Oven's store under RFC 124. A `source` without a `digest` is unpinned, and must be reported as such. A `path` with a `digest` must be verified by `oven audit`.
4. `license`, `purpose` and `for` are optional. `for` must name another asset.
5. Tables prefixed `x-` must be preserved and shown in machine-readable output, and must not affect Incan's behavior.

### Execution and data

6. A `model` must declare `execution` as `local` or `remote`.
7. A `remote` model may declare `retains-data` as `no` or `yes`. If absent, it must be `unknown`, and must be treated as the riskiest value.
8. An asset declared `local` must not silently be replaced by a remote one. Any tool that does so without an explicit declaration is misreporting what ran.

### Activation and running

9. Declaring, resolving or adding an asset, including through an RFC 075 mix, must not download or run anything.
10. `@action` and `[actions.<id>]` must accept `uses`, naming assets. A dry run must show those assets, their execution and retention. Policy must be evaluated against them before an action runs, and the action's RFC 104 receipt must record their identities and digests.
11. Eval results must live in the action's receipt. A registry card may show them under RFC 079, and the project must not store them.
12. A changed `prompt`, `dataset` or `agent-guidance` file must be an ordinary file change, classified under RFC 076's `ai-asset` risk category when a mutation plan proposes it. An `agent-guidance` asset must declare any mutation, network, model or tool risk it can lead to.

### Policy and auditing

13. `[[policy.assets]]` must support matching on `kind`, `source`, `license`, `execution`, `retains-data` and `pinned`, with an RFC 076 outcome.
14. `oven audit` must report unpinned assets, `unknown` retention, and a digest mismatch. A mismatch must fail, and the others must be warnings unless `--deny` is used.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Declaring | Each kind parses, an unknown kind stays visible, and an `x-` table is preserved without effect. |
| No side effects | Adding an asset, including through a mix, downloads and runs nothing. |
| Pinning | A missing digest is reported as unpinned, and a changed file fails against its digest. |
| Policy | Remote, unpinned and unknown-retention rules block or require approval before an action runs. |
| Running | A dry run lists the assets, and the receipt records what ran by digest. |

## Design details

### Why so little

Everything left out is real and useful, but changes quickly. A framework can release when a provider changes, and a compiler should not have to. What stays here rarely changes: a declared artifact has an identity, a reviewer can see it change, and policy can see where data goes. A plain project gets all of that with no framework.

### Why prompts have no syntax

A universal template language would be wrong for most tools and would date quickly. A prompt is a file, and its reviewability is what matters here.

### Prior art

Hugging Face model cards and Ollama's Modelfile inform what metadata exists, and a framework can carry the richer forms. Oven's lockfile and store already handle identity for packages, so assets use the same digest form.

## Alternatives considered

- **Keep everything in Incan:** providers, execution profiles and metrics would be tied to compiler releases.
- **Remove this from Incan entirely:** a plain project could not declare a prompt or a model, and policy could not see them.
- **Leave AI metadata as documentation:** prompts and models affect behavior and safety, so tooling has to be able to see them.
- **Hardcode one provider:** the ecosystem has local runtimes, remote providers and private catalogs.
- **A dedicated model download command:** a download is an action like any other, with a capability and a receipt.
- **Run setup when a mix is applied:** downloads and inference are policy-relevant and must be explicit.

## Drawbacks

- Two places to look: Incan for declarations, and a framework for AI specifics.
- A provider can under-declare, and `retains-data` is a claim, not something Incan can verify.
- Without a framework, a project has to write its own actions to download and run models.

## Layers affected

- **Manifest schema / configuration validation:** `[assets.<id>]`, and `[[policy.assets]]` under RFC 076.
- **Stdlib:** `uses` on RFC 078's `@action`.
- **CLI / tooling:** dry runs and `oven audit` report assets, and receipts record them.
- **Package and catalog integration:** published assets can be artifact-graph nodes under RFC 079.
- **LSP / IDE tooling:** editors may surface asset warnings from machine-readable output.
- **Documentation:** local versus remote execution, pinning, and what a framework adds.

## Design Decisions

- **Incan owns the general part.** Identity, review, policy and auditing live here. Providers, runtimes, execution profiles, measurements, model selection, fallbacks, eval metrics and agent guidance formats belong to a higher-level framework, which extends declarations through `x-` tables.
- **Kinds:** `model`, `prompt`, `dataset`, `eval` and `agent-guidance`, plus `x-` extensions.
- **Prompt syntax:** none. Prompts are files with a declared purpose.
- **Remote metadata and privacy:** `execution` is `local` or `remote`, and `retains-data` is `no`, `yes` or `unknown`, with unknown treated as the riskiest.
- **Digests:** the same content digest Oven already uses.
- **Eval results:** in the action's receipt, never in the project.
- **Downloads:** an ordinary action using `host.http.request`, with no new command.
- **Agent guidance:** files that declare their risk categories.
