# RFC 105: `incan architect` rule engine for design, safety, idiom, maintainability, and risk findings

- **Status:** Draft
- **Created:** 2026-05-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (generators)
    - RFC 048 (contract-backed models emit and tooling)
    - RFC 070 (Result combinators)
    - RFC 088 (iterator adapter surface)
    - RFC 096 (declaration metadata blocks)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 120 (canonical source symbol identity)
- **Issue:** [#663](https://github.com/encero-systems/incan/issues/663)
- **RFC PR:** —
- **Written against:** ~~v0.3~~ ~~v0.5~~ v0.7
- **Shipped in:** —

## Summary

This RFC proposes `incan architect` as a deterministic code-advice command for direct Incan source and for Oven-managed projects authored in Incan, Rust, or both. The command reports evidence-backed findings across architecture, safety, idiom usage, maintainability, and deterministic risk signals by running maintainable rules over the shared compiler-backed codegraph defined by RFC 106. The central goal is not to create a broad subjective linter, but to create a durable rule authoring surface where new advice can be added cheaply, tested precisely, calibrated against real projects, and consumed by humans, agents, editors, and CI without relying on model inference for core detection.

## Core model

1. **Oven selects project scope:** Inside a Loaf, `incan architect` obtains the selected workspace members, source facets, dependencies, targets, generated inputs, and source roles from Oven. It must not recreate project discovery or infer a project graph from nearby files.
2. **Language frontends own semantic facts:** Incan's compiler owns checked Incan facts. Rust inspection and compiler infrastructure own checked or resolved Rust facts. Both enter the shared RFC 106 graph as ordinary declarations, references, calls, modules, and relationships carrying a `language` attribute.
3. **Provenance is preserved:** Every source-backed fact and finding retains whether the source is authored, generated, external, or projected, plus its source facet and selected project authority. Generated Rust is not an independently authored implementation merely because it is visible to the graph.
4. **Rules interpret facts:** Each rule consumes typed fact views and emits findings with stable codes, priorities, categories, confidence, evidence, suggestions, and risks. A rule declares the fact capabilities and languages it supports rather than assuming every source owner is Incan.
5. **Findings are advisory:** Architect findings are not compiler errors. They describe design pressure or code-shape opportunities with enough evidence for a human or agent to decide whether to act.
6. **Categories are explicit:** Architecture findings, safety findings, idiom findings, maintainability findings, and risk findings remain separate in rule codes and profiles even when they share one command.
7. **Broad collection, precise classification:** The command should scan the requested scope broadly and emit evidence-backed findings across categories. Confidence, category, profile, baseline, and priority decide presentation and action pressure; they should not erase valid findings up front merely because the finding is local, optional, or not architectural.
8. **Rule authoring is a product surface:** The feature is only maintainable if adding a rule means using stable typed facts and reusable queries, not hand-parsing raw graph nodes or reimplementing per-language syntax walks.
9. **Peer context is explicit:** Projects may declare locally meaningful peer groups such as command handlers, lowering passes, adapters, registries, or generators. A broad category may contain several valid peer groups; names and filesystem position are useful signals, not sufficient proof of membership or intent.
10. **Exploration stays subordinate to rules:** Future baseline or similarity analysis may identify review candidates from comparable peers, but it must remain explainable, provenance-aware, and non-authoritative. It cannot silently create an architectural contract or replace deterministic rule detection.

## Motivation

Incan and Rust already have syntax checks, semantic checks, formatters, tests, and language-specific diagnostics. Oven adds one language-neutral project, dependency, target, artifact, and receipt model over authored Incan and Rust facets. Those tools answer whether a selected project parses, typechecks, compiles, formats, and runs. They do not answer whether the project is accumulating design pressure: repeated dispatch over the same domain, public boundaries that can panic on recoverable input, competing source authorities, duplicated contracts across language facets, cache identities that omit consumed inputs, old-shaped Incan control flow that should now use language features, or small helper functions that add indirection without carrying domain meaning.

The first experiments with an architecture-advice command showed that deterministic rules can surface useful pressure when they report concrete source evidence and classify action pressure separately from proof strength. Repeated match dispatch can reveal a growing operation boundary. Fail-fast calls inside public APIs can reveal recoverability problems. Body-shape facts can also support maintainability findings such as compound-assignment candidates, single-use trivial helpers, append-only list builders that could become comprehensions, or `Result` matches that could use RFC 070 combinators.

Without a formal rule engine, each new check risks becoming a one-off command-private or language-private syntax walk with custom parsing, inconsistent output, and ad hoc severity. That path does not scale, especially when the same project can contain authored Incan and Rust. The value is in a shared substrate: one Oven-selected project graph, one bilingual codegraph, one typed query layer, one finding model, one de-duplication path, and many small rules that are easy to review and calibrate.

This feature also matters for agent workflows. Agents can already make broad refactoring suggestions, but those suggestions are often expensive to verify and easy to overfit. `incan architect` should provide deterministic evidence that an agent can use as grounding: exact files, lines, matched domains, shared patterns, call sites, usage counts, and counterexample risks. A model may later summarize or prioritize findings, but the core detection should remain inspectable and reproducible.

## Goals

- Define `incan architect` as the umbrella command for deterministic design, safety, idiom, maintainability, and risk-signal advice.
- Provide a stable finding model with rule code, category, priority, confidence, evidence, pressure, suggestions, risks, and machine-readable output.
- Provide direct Incan file and directory scanning outside a Loaf, and Oven-selected package or workspace scanning inside a Loaf, with deterministic source, module, and finding de-duplication across Incan and Rust facets. The same finding model should later support PR-diff and graph-snapshot scopes.
- Treat Rust-only, Incan-only, and mixed Loaves as first-class project analysis scopes.
- Preserve language, source role, source facet, generation provenance, and selected Oven project authority in graph facts and finding evidence.
- Establish rule categories and profiles so users can run architecture-only, safety-only, idiom-only, maintainability-only, risk-only, or all-rule scans.
- Establish a maintainable rule authoring surface based on typed facts and reusable queries over codegraph data.
- Extend codegraph body facts as needed for rule families such as match dispatch, call sites, references, assignment/update shapes, helper usage, loop-builder shapes, and result-match shapes.
- Include maintainability findings in scope when they have clear evidence, useful counterexamples, and calibrated confidence.
- Include deterministic risk findings when local inputs such as coverage, churn, ownership, co-change, or decision records are available, while keeping risk evidence separate from architecture recommendations.
- Keep detection deterministic for the first version; no language model is required for core finding generation.
- Support text output for humans and stable JSON output for tools, agents, editors, and CI.
- Make suppression and baselining part of the product model so mature codebases can adopt the command incrementally.
- Define a metadata and fact-model path for project-specific peer context, including peer-group hierarchy, declared boundary constraints, exceptions, and membership provenance.
- Reserve an explicit experimental profile for explainable peer-baseline outliers once the required facts, calibration fixtures, and versioned baseline contract exist.

## Non-Goals

- This RFC does not make architect findings compiler errors.
- This RFC does not replace formatter rules, language diagnostics, Clippy, generated-source validation, or project tests. In particular, Architect should not reproduce local Rust lint rules that Clippy already owns without adding project-wide evidence.
- This RFC does not treat generated Rust as authored Rust or report duplicate authored implementations merely because an Incan declaration has a generated Rust projection.
- This RFC does not make Oven the owner of Incan or Rust semantic facts. Oven selects project scope; language frontends remain semantic authorities.
- This RFC does not add arbitrary ambient source languages to a Loaf. Incan and Rust are the built-in authored facets defined by RFCs 117 and 119; later language integrations require their own explicit contracts.
- This RFC does not require a small language model or remote AI service for rule detection.
- This RFC does not attempt to infer developer intent from names alone.
- This RFC does not make a similarity score, inferred peer membership, or outlier classification an architectural finding by itself.
- This RFC does not allow an exploratory baseline to fail CI by default, silently update itself from a scan, or promote an observed pattern into a deterministic rule automatically.
- This RFC does not require every possible maintainability rule or risk signal to ship in the first version.
- This RFC does not define automatic rewrites or apply fixes.
- This RFC does not decide whether a reported finding should be fixed in the current change. That is a separate user, CI, or fix-loop policy decision.
- This RFC does not define a public plugin ABI for third-party binary rule packages.
- This RFC does not require every codegraph fact to be part of a permanently stable external schema in the first release; only the JSON findings format and documented command behavior need v0.7 stability.

## Guide-level explanation

Users run `incan architect` on direct Incan source or on an Oven-managed project. In a Loaf, the default scope is the selected project graph, including every authored Incan and Rust facet in that selection.

```bash
incan architect .
incan architect src/lib.incn --format json
incan architect . --profile architecture
incan architect . --profile maintainability
```

The command remains on the `incan` semantic-tooling surface defined by RFC 118. Inside a Loaf it asks Oven for project selection and source authority, then analyzes the resulting graph; it does not become a second workspace resolver. A Rust-only Loaf is still a valid Architect scope. A mixed Loaf is analyzed as one project rather than as unrelated per-language scans.

Rules may be language-neutral, language-specific, or cross-language:

- language-neutral rules operate over normalized facts such as declarations, calls, references, visibility, reachability, identities, caches, registries, and boundaries;
- language-specific rules use a fact capability owned by one frontend, such as an Incan `Result` combinator candidate; and
- cross-language rules reason about relationships between authored Incan and Rust, such as duplicated contracts, divergent registries, bypassed interop authority, or an authored implementation that is disconnected from the public facet that claims to expose it.

Generated projections remain visible as evidence but do not become authored peers. For example, Architect must not report an Incan function and its generated Rust body as duplicate implementations. Their `source_role` and generation relationship make them one authored declaration and one derived projection.

The command prints findings grouped by priority and grounded in source evidence.

```text
[P3] Repeated match dispatch over `source_kind`
Pressure: 2 match expressions dispatch over `source_kind` and share 3/3 explicit arms: SourceKind.Arrow(...), SourceKind.Csv(...), SourceKind.Parquet(...)
Suggestions:
  - Decide whether this is intentionally exhaustive local logic or a growing operation boundary.
  - If it is a growing operation boundary, prefer an adapter or registry outside the domain type when the operation belongs to another subsystem.
Risks:
  - Keep local exhaustive matches when they are clearer than an abstraction and the case set changes rarely.
Evidence:
  - src/backend.incn:160:5 in register_one (explicit arms: 3/3; fallback: no)
  - src/schema.incn:322:5 in schema_columns_for_source (explicit arms: 3/3; fallback: no)
```

The architecture value is not merely that two matches are textually similar. The useful signal is that separate subsystems are making parallel decisions over the same closed domain. For example, an ingestion package might register execution backends in one module and infer schemas in another module, with both operations matching every `SourceKind` variant.

```incan
def register_backend(kind: SourceKind, registry: BackendRegistry) -> None:
    match kind:
        SourceKind.Csv(_) => registry.add("csv", csv_backend())
        SourceKind.Json(_) => registry.add("json", json_backend())
        SourceKind.Parquet(_) => registry.add("parquet", parquet_backend())


def infer_columns(source: Source) -> Result[list[Column], SchemaError]:
    match source.kind:
        SourceKind.Csv(_) => return infer_csv_columns(source)
        SourceKind.Json(_) => return infer_json_columns(source)
        SourceKind.Parquet(_) => return infer_parquet_columns(source)
```

The recommendation should not be "put backend registration and schema inference methods on `SourceKind`." That would move subsystem responsibilities onto the enum. The more architectural advice is to ask whether this is a growing operation boundary. If every new source format requires coordinated edits to backend registration, schema inference, validation, documentation, and test fixtures, the code may want a format-handler registry or adapter table where each format owns its related operations.

```text
[P3] Repeated match dispatch over `source.kind`
Pressure: backend registration and schema inference both dispatch over all source formats.
Suggestion: Consider a format-handler registry if adding one format requires shotgun edits across subsystems.
Risk: Keep exhaustive local matches if the format set is closed, the operations are genuinely local, and cross-format registration would obscure control flow.
```

Architect findings use categories. Architecture findings describe design pressure. Safety findings describe failure or recoverability risk. Idiom findings describe opportunities to use Incan features more directly. Maintainability findings describe local readability, cleanup, or code-shape pressure. Risk findings describe deterministic prioritization evidence such as churn, weak coverage, co-change, or ownership spread.

```text
safety.fail_fast_boundary_call
idiom.result_combinator_candidate
maintainability.single_use_trivial_helper
risk.untested_hotspot
arch.repeated_match_dispatch
```

Maintainability findings are allowed when they are precise and humble. A trivial helper rule can identify a private helper that is used once and only returns a pure expression.

```incan
def add(left: int, right: int) -> int:
    return left + right
```

The finding should not say that the helper is definitely wrong. It should say that the helper may be unnecessary unless its name carries useful domain meaning.

```text
[P3] Private helper only wraps one expression
Pressure: `add` is private, used once, and only returns `left + right`.
Suggestion: Inline the expression if the helper does not name a useful domain concept.
Risk: Keep the helper if it documents intent, preserves API shape, acts as a callback, or is expected to grow.
```

A comprehension candidate should likewise report a specific body shape, not a broad preference.

```incan
def positive_scores(scores: list[int]) -> list[int]:
    out = []
    for score in scores:
        if score > 0:
            out.append(score)
    return out
```

The corresponding advice is useful only because the shape is append-only, the accumulator is returned, and no other mutation or side effect participates in the loop.

```text
[P3] Append-only list builder can be a comprehension
Pressure: `positive_scores` builds and returns a list with one append-only loop.
Suggestion: Use `[score for score in scores if score > 0]` if the eager list is the intended result.
Risk: Keep the loop if additional statements, logging, early exits, or mutation are part of the real workflow.
```

For RFC 070 `Result` combinators, architect can identify obvious match shapes and suggest the equivalent method only when the transformation is mechanically recognizable.

```incan
match parsed:
    Ok(value) => Ok(clean(value))
    Err(err) => Err(err)
```

The finding can suggest `parsed.map(clean)` because one branch transforms the `Ok` payload and the `Err` branch passes through unchanged.

## Reference-level explanation

### Command behavior

`incan architect [PATH] [OPTIONS]` must accept a source file or directory. When `PATH` is omitted, the command should analyze the current directory.

When `PATH` is an Incan file outside a Loaf, the command must scan the file and the modules needed to resolve its imports according to ordinary Incan module rules.

When `PATH` is within a discovered Loaf, the command must delegate project and workspace selection to Oven and analyze the selected source graph. That graph may contain authored Incan, authored Rust, or both. The command must use Oven's selected members, facets, dependencies, generated inputs, and source roles rather than recursively treating every nearby source file as part of the project.

When `PATH` is a directory outside a Loaf, the command may retain the direct-source behavior of recursively scanning `.incn` files. Direct standalone Rust-file and non-Loaf Rust-directory behavior is unresolved by this RFC; Rust-only project analysis is required for Rust facets selected by a Loaf.

Every scan must be deterministic. A source owner imported through multiple roots or visible through both an authored declaration and a generated projection must contribute according to its graph identity and provenance, not as duplicated text.

The requested scope is the scan boundary. Maintainability findings are not limited to code that happens to be near another edit. Profiles, baselines, suppressions, and output filters decide which findings are shown or acted on.

The command must provide `--format text` and `--format json`. Text output is for humans. JSON output is the integration surface for agents, editors, CI, dashboards, and future baselining tools.

The command should provide `--profile` with at least `architecture`, `safety`, `idioms`, `maintainability`, `risk`, `experimental`, and `all`. The default profile is unresolved by this draft. `experimental` is opt-in and may contain peer-baseline review candidates; `all` excludes it unless the user requests it explicitly.

### Finding model

Every finding must have a stable rule code. Rule codes must be namespaced by category.

```text
arch.repeated_match_dispatch
safety.fail_fast_boundary_call
idiom.result_combinator_candidate
maintainability.single_use_trivial_helper
risk.untested_hotspot
```

Every finding must include a category, priority, confidence, title, pressure, evidence, suggestions, and risks. Source-backed evidence must also identify its language and source role. Findings emitted for a Loaf scope must identify the selected project authority or graph snapshot from which their evidence was derived.

Priority must describe expected action pressure, not proof certainty.

```text
P1: likely correctness, reliability, or public-boundary risk that should be reviewed before release
P2: meaningful design or maintainability pressure that should be tracked or scheduled
P3: low-risk local maintainability, idiom, or watchlist finding that is valid within the requested scan scope but optional to act on
Info: low-pressure educational or style-level advice
```

Confidence must describe how mechanically strong the rule match is.

```text
High: the rule found a narrow, mechanically recognizable shape
Medium: the rule found a useful pattern with plausible counterexamples
Low: the rule is exploratory and should normally be hidden outside explicit profiles
```

Evidence must identify source file, line, column, owner declaration, language, source role, and source facet when available, plus rule-specific context. Rule-specific context may include matched arms, overlap counts, fallback/default-arm presence, callee labels, usage counts, body-shape summaries, generation relationships, selected project authority, or suggested replacement text.

Suggestions must be phrased as advice, not certainty. Risks must name the common counterexamples that would make the suggestion wrong.

Category, confidence, priority, profile, suppression, and baseline state shape presentation and policy. They must not be used as hidden reasons to drop evidence-backed findings before output unless the user selected a profile, suppression, or baseline that explicitly hides them.

Findings must be de-duplicated before output. Identical findings produced through multiple import roots must appear once.

### Investigation consumers

RFC 126 proposes evidence-backed investigations that may reference Architect findings when forming and testing hypotheses. A finding remains advisory evidence about a rule match; its priority and confidence do not establish the cause of a runtime failure. An investigation must retain the finding's source snapshot, rule identity, evidence, and counterexample risks separately from its execution observations. Architect does not acquire process-control, experiment-execution, or automatic-repair responsibilities through this integration.

### Rule categories

Architecture rules describe design pressure across declarations, modules, domains, language facets, or boundaries. Repeated match dispatch, growing literal domains, registry source-of-truth drift, competing source authorities, cache-key omissions, wrong-layer behavior, cross-language contract drift, disconnected implementations, and operation-boundary pressure belong here.

Safety rules describe recoverability, fail-fast behavior, partial handling, unchecked assumptions, or public-boundary hazards. A public function that can panic on caller-provided data belongs here.

Idiom rules describe opportunities to use a language or standard-library feature more directly. The first stable idiom rules are Incan-specific: Result combinator candidates, iterator adapter candidates, generator/comprehension candidates, and compound assignment candidates. Rust-local style and expression advice remains with Rust tooling unless Architect can add project-wide context that the language linter does not have.

Maintainability rules describe local readability, cleanup, or code-shape pressure. Single-use trivial helpers, repeated literals, unnecessary wrappers, long branch-heavy functions, broad type plumbing, parallel fixture lists, and append-only builders belong here when detected with concrete evidence.

Risk rules describe deterministic prioritization evidence rather than recommendations by themselves. Untested hotspots, churn, ownership spread, co-change without structural edges, decision staleness, and code-age volatility belong here when the required local inputs are available. Risk findings must expose raw contributing measures and caveats rather than only a composite score.

Rules must not be categorized as architecture findings merely because they are emitted by `incan architect`.

Every rule must declare its applicability independently of category:

- `language-neutral`: requires normalized facts and may match any authored facet;
- `language-specific`: requires one or more named languages or frontend capabilities; or
- `cross-language`: requires relationships whose endpoints have different authored languages.

Applicability is a precondition, not a confidence adjustment. A rule that requires checked Incan body facts must not guess from Rust syntax, and a cross-language rule must not compare authored source with its generated projection as though they were independent implementations.

### Peer context and experimental baselines

Project architecture often has distinctions that compiler facts cannot establish alone. A repository may contain several kinds of command handler, adapter, generator, registry, or lowering pass that share a broad category but have different responsibilities and dependency directions. Architect must therefore support project-specific peer context without treating a filename, directory, declaration name, or one aggregate metric as architectural truth.

A peer group is a named set of comparable source owners, such as files, modules, declarations, or other codegraph-backed scopes. Peer context may describe a hierarchy from broad category to local archetype, declared dependency boundaries, known exceptions, and the provenance of every membership decision. The first useful provenance states are:

- `declared`: project metadata explicitly assigns the member or boundary;
- `derived`: a deterministic rule or compiler-backed query establishes the membership; and
- `experimental`: an exploratory process proposes a comparable peer set but has not established a project contract.

Project metadata must be able to represent multiple valid peer groups below one broad role. It must not force a source owner into a single global category merely because its path or name resembles another group.

Peer signatures may combine compiler-backed declaration kind, parameter and type shape, imports, resolved calls, references, ownership position, body summaries, and source topology. Names and paths may contribute weak context, but the signature must retain the provenance and strength of every fact so a consumer can distinguish checked identity from syntax-only or project-declared context.

An experimental baseline may compare a changed owner with its peer group and report a review candidate only when it explains the comparison: the selected peers, observed differences, relevant source locations, baseline version, and known counterexamples. For example:

```text
[Info] Experimental peer-context outlier in `sync_user`
Pressure: `sync_user` imports a web boundary and calls two HTTP-facing APIs; none of its 31 declared worker peers do.
Evidence:
  - peer group: `background-workers` (membership: declared)
  - src/workers/sync_user.incn:4:1 imports `std.web`
  - baseline: codegraph schema 1, compiler 0.5.0, project revision abc123
Risk: the worker may intentionally own a web ingress boundary; record that exception or move the boundary if the dependency is accidental.
```

The candidate is not a deterministic `arch.*` violation unless a declared boundary or independently testable Architect rule establishes that the dependency is forbidden. Experimental baselines must be opt-in, advisory, and unable to alter their own committed baseline from the scan being evaluated. A baseline snapshot must record the Codegraph schema version, compiler version, project identity, and source revision or equivalent content identity.

### Rule authoring contract

Rules must declare metadata: code, category, default priority, default confidence, profile membership, applicability (`language-neutral`, `language-specific`, or `cross-language`), supported languages or frontend capabilities, required fact kinds, and a short explanation.

Rules must consume typed fact views rather than raw serialized facts. A rule that needs match dispatch sites, call sites, assignment shapes, helper usage counts, or loop-builder shapes should ask for those views directly.

Rules should be small and independently testable. Each rule should have positive and negative fixtures. Negative fixtures are required for common counterexamples named in the rule's risk text.

Rules must not require typechecked metadata when a syntactic fact is sufficient. Rules may use type facts when precision depends on type information, such as recognizing `Result[T, E]` match shapes.

Rules should prefer narrow body-shape facts over broad textual heuristics. For example, a comprehension candidate should be based on an append-only list-builder shape, not the mere presence of a `for` loop and `append`.

Rules must not emit authored-source findings for generated, projected, or known external code unless the rule explicitly owns that source role or the user explicitly scans it. Generated facts may support a finding about their authored owner or a broken generation relationship, but they must not silently become peer authored declarations.

### Codegraph fact requirements

The codegraph exporter must provide enough source facts for rules to avoid command-private or language-private syntax walks. The first useful fact families are declarations, imports, public API metadata, match dispatches, call sites, references, assignment/update shapes, function body summaries, usage counts, loop-builder shapes, result-match shapes, declaration signatures, source topology, and stable identity/provenance fields.

Every source-backed fact must carry the RFC 106 language attribute and enough provenance to distinguish authored, generated, projected, and external source. For a Loaf-selected scan, facts must also retain the source facet and selected Oven project or graph authority. Generation and projection relationships must be explicit edges rather than inferred from filenames or output directories.

Normalized declaration, reference, call, containment, visibility, dependency, and reachability views must work across Incan and Rust. A query asking what a declaration depends on must not require its caller to choose a language-specific edge kind first.

Frontend-specific body facts may remain capability-gated. Incan idiom rules can require checked Incan body summaries; Rust-aware architecture rules can require Rust declaration and reference facts without claiming that every Incan body-shape query has an equivalent Rust implementation. Missing capability must disable the rule for that source owner rather than produce a low-confidence guess.

Cross-language facts must preserve the checked interop identity that connects an Incan declaration to the Rust declaration it names, plus generated-from, projected-from, implements, exports, and public-contract relationships where available. This is what lets Architect distinguish a legitimate generated projection from two authored implementations that can drift.

Match dispatch facts must include the matched domain, explicit pattern labels, explicit pattern count, source arm count, and wildcard/default-arm context.

Call-site facts must include callee key, callee label, receiver shape when available, source location, and owner declaration.

Reference facts must support usage counting for private declarations and helper functions.

Assignment/update facts must make compound-assignment candidates expressible without string matching.

Function body summary facts should identify simple shapes such as single-return expression, pure expression wrapper, append-only list builder, and short result-match transform. These summaries must be conservative.

Result-match facts should identify branch-preserving transformations only when the matched expression is known to be a `Result[T, E]` or the syntactic shape is unambiguous enough for an idiom finding with appropriate confidence.

Risk rules may consume optional process facts such as git churn, ownership, co-change, coverage reports, and decision records when available. Missing optional inputs must be represented as absent evidence, not as zero risk.

### Suppression and baselining

The command should support local suppression of a specific rule at a specific source location. Suppression syntax is unresolved by this draft.

The command should support project baselines so existing findings can be recorded and new findings can fail CI or be highlighted separately. Baseline storage is unresolved by this draft.

Suppressions and baselines must preserve rule code and evidence identity. A future change that moves or changes the evidence should not silently suppress an unrelated finding.

## Design details

### Profiles

Profiles let users choose the kind of advice they want. `architecture` should include cross-cutting design pressure. `safety` should include fail-fast and recoverability risk. `idioms` should include feature-usage opportunities. `maintainability` should include local readability, cleanup, and code-shape findings. `risk` should include deterministic prioritization evidence. `all` should include every non-experimental rule.

Rules may belong to more than one profile only when that does not blur the category. For example, a public fail-fast boundary call is a safety finding even if it also has architecture implications.

Exploratory rules may exist behind an explicit experimental profile, but they must not be enabled by default.

### Severity calibration

Severity should be calibrated against evidence strength, public surface impact, and likely cost of ignoring the finding. Public API failures are generally higher priority than private helper maintainability findings. Repeated design pressure across files is generally higher priority than a local expression-level cleanup. Idiom suggestions are generally P3 or Info unless the shape creates repeated complexity or risk.

Rules should downrank, lower confidence, or route known low-action cases to explicit profiles instead of mislabeling them as urgent. Rules should suppress a match only when counterexample evidence makes it invalid, or when user-selected profiles, suppressions, or baselines hide it. For example, fail-fast calls around trusted constants may be lower priority than fail-fast calls around caller-provided input. Exhaustive matches over a closed domain may be preferable to abstraction when the matched operation is local and the domain changes rarely.

### Examples of initial rules

`arch.repeated_match_dispatch` reports repeated match expressions that dispatch over the same domain and share multiple explicit arms. The rule should report overlap counts and wildcard/default context.

`safety.fail_fast_boundary_call` reports `unwrap`, `expect`, `panic`, `todo`, and `unreachable` inside public or internal boundaries. Public API boundaries should generally be P1. Internal boundaries should generally be P2 unless evidence shows trusted constants or invariant setup.

`idiom.result_combinator_candidate` reports obvious RFC 070 match shapes that can be expressed with `map`, `map_err`, `and_then`, `or_else`, `inspect`, or `inspect_err`.

`idiom.compound_assignment_candidate` reports assignments such as `i = i + 1` when the target and left operand are the same simple storage place and `i += 1` is equivalent.

`idiom.comprehension_candidate` reports append-only list builders that can be represented as eager list comprehensions.

`maintainability.single_use_trivial_helper` reports private, undocumented, undecorated helpers that are used once and only return a simple pure expression. The rule must mention that domain vocabulary can justify keeping the helper.

`maintainability.repeated_literal_domain` reports repeated raw string or scalar literal domains used as branch keys or dispatch keys across multiple sites.

`arch.competing_source_authority` reports two reachable paths that independently select project source, dependency, registry, or implementation authority where the project declares one canonical owner. This rule is language-neutral and may cite Incan, Rust, Oven, or mixed evidence.

`arch.cross_facet_contract_drift` reports an authored Incan/Rust boundary whose independently maintained declarations or registry entries disagree with the checked interop or public-contract relationship. Generated projections are explicitly excluded from peer comparison.

`maintainability.unreachable_public_surface` reports a public declaration with no production reachability when the selected project graph is complete enough to support that claim. Unresolved Rust dynamic dispatch must remain affected or unknown according to RFC 106's lower-bound reachability rule; absence of a statically resolved caller is not sufficient by itself.

`risk.untested_hotspot` reports files, modules, declarations, or architecture findings with high change frequency and weak test or coverage evidence when those process facts are available. The rule must expose the contributing churn and coverage facts rather than only a score.

## Alternatives considered

### Keep architect as architecture-only

This would preserve a narrow name, but it would force closely related idiom, maintainability, and risk findings into separate commands even though they need the same project-wide codegraph, evidence model, de-duplication, profiles, suppressions, and JSON output. The better boundary is category namespace, not separate infrastructure.

### Build a general linter instead

A general linter would fit small syntax-level advice, but it would understate the project-wide design-pressure use case. The command should remain broader than a linter while still identifying local maintainability findings as one category.

### Use a language model for rule detection

Model-based detection may be useful later for summarization, clustering, or explaining findings in pull requests. It is not the right foundation for v0.7 rule detection because findings need to be reproducible, testable, source-grounded, and suitable for CI.

### Let every rule walk the AST directly

This is the fastest way to add a first rule and the worst way to maintain many rules. It duplicates traversal logic, fragments fact extraction, and makes rule behavior harder to share with agents, editors, and other code-intelligence tools.

### Make findings auto-fixable from the start

Some findings will eventually support safe rewrites, such as compound assignment candidates. Making fixes part of the first version would expand the scope into formatter, semantic preservation, and edit application. The first version should focus on reliable findings and stable output.

## Drawbacks

This feature adds a new advisory surface that can become noisy if rule quality is poor. The command must earn trust by showing evidence, classifying findings precisely, calibrating confidence and priority, and naming counterexamples.

The codegraph fact model will grow. If facts are added without a typed query layer, rules will become stringly and brittle. If facts are over-designed too early, implementation will slow down before the rule set proves itself.

Some maintainability findings are subjective. A helper that looks unnecessary may carry important domain meaning. A loop that could be a comprehension may be clearer as a loop when side effects are about to be added. The finding model must make room for this uncertainty through confidence and risk text.

Project-wide scanning may be slower than entry-point scanning, especially for mixed Loaves whose Rust facts require compiler-grade inspection. The implementation should reuse Oven and codegraph identities where possible and leave room for caching, but v0.7 should prioritize correctness and evidence over premature optimization.

## Implementation architecture

This section is non-normative.

The recommended internal shape is a layered pipeline: direct-source or Oven project selection, Incan and Rust semantic fact extraction into the shared RFC 106 graph, provenance-preserving typed fact views, query indexes, project peer context and boundary metadata, independent rule modules, finding normalization, de-duplication, profile filtering, and text/JSON rendering.

Oven should supply project selection and operational authority, not semantic interpretation. The Incan and Rust frontend infrastructure should produce source facts through the codegraph layer. Architect should own neither parsing, typechecking, Rust project reconstruction, nor Loaf resolution. Architect rules should operate over typed views such as declarations, cross-language identities, reachability, match dispatch sites, call sites, references, assignment/update candidates, usage counts, loop-builder shapes, and result-match shapes.

The rule engine should provide a small metadata contract for rule authors. A rule should declare its code, category, default priority, confidence, profiles, applicability, supported languages or capabilities, required facts, accepted source roles, and explanation. A rule should receive a query context and emit findings. Project peer context is separate from rule metadata: it describes locally meaningful comparable scopes and declared boundary intent, and must preserve whether each claim is declared, derived, or experimental.

The report layer should be shared by all rules. Sorting, de-duplication, JSON serialization, text formatting, suppression matching, and baseline matching should not be implemented per rule.

The first version should ship with a small calibrated rule set rather than a large catalogue. New rules should be added only when they have clear positive fixtures, negative fixtures, and calibration evidence from real source.

Experimental peer-baseline analysis is a later consumer of the same fact and query layers. It may construct explainable signatures and compare declared or derived peer groups, but it must remain outside the deterministic rule path until its calibration, storage, and user-facing contract are independently settled.

## Layers affected

- **Oven project model**: Inside a Loaf, Architect must consume Oven's selected workspace members, authored source facets, dependencies, generated inputs, source roles, and graph authority without becoming a second resolver.
- **Incan parser / AST**: No new user syntax is required, but Incan source traversal must expose enough body shapes for codegraph facts.
- **Incan typechecker / Symbol resolution**: Rules may need checked public API metadata, resolved imports, type facts for `Result` shapes, and symbol usage information.
- **Rust inspection / compiler facts**: Rust-only and mixed Loaves require declaration, reference, call, visibility, reachability, public-contract, and provenance facts in the same RFC 106 identities and relationships used for Incan. Dynamic Rust reachability must retain RFC 106's lower-bound semantics.
- **IR Lowering**: No required impact.
- **Emission**: No required impact.
- **Stdlib / Runtime (`incan_stdlib`)**: No required runtime impact, though stdlib feature surfaces such as Result combinators and iterator adapters inform idiom rules.
- **Formatter**: No required impact unless future auto-fix support is added.
- **LSP / Tooling**: The JSON findings format should be usable by editors, agents, CI, and future diagnostics-style surfaces. Editor consumers must see the same selected source authority and language provenance as the CLI.
- **CLI / Project tooling**: `incan architect` needs direct-source and Oven-selected scope handling, profiles, stable text/JSON output, suppression support, and baseline support. Future peer-baseline tooling needs opt-in invocation and schema-, compiler-, Oven-plan-, and graph-versioned snapshots.
- **Documentation**: The CLI reference must document direct-source versus Loaf-selected behavior, Incan/Rust/mixed coverage, source-role provenance, profiles, categories, priorities, confidence, suppressions, peer-context provenance, and examples.

## Unresolved questions

- What is the default profile for `incan architect .`: architecture-only, architecture plus safety, or all stable rules?
- What suppression syntax should Incan use for architect findings, and should it share vocabulary with compiler diagnostic suppressions?
- Should baselines live in a typed `loaf.toml` table, a separate versioned baseline artifact, or generated project-tooling state? They must not be folded into `oven.lock` merely because a project uses Oven.
- Where should peer-group metadata live, and which membership or boundary declarations belong in source, package metadata, or local tooling state?
- Which peer-signature facts are stable and useful enough to expose without turning names, paths, or aggregate similarity into semantic authority?
- How should a project review, accept, version, and retire an experimental baseline or intentional outlier without allowing a scan to rewrite its own comparison set?
- Which finding fields are stable enough to commit as v0.7 JSON output, and which should remain experimental?
- Which maintainability rules belong in the first stable profile, and which should remain experimental until enough corpus evidence exists?
- Should a Loaf-selected project scan include every test, example, benchmark, and tool role by default, or should Oven environments and target roles determine the default analysis closure?
- Should `incan architect path/to/file.rs` support standalone Rust outside a Loaf, or should first-class Rust analysis require an Oven-selected project graph?
- Should explicit Cargo-compatibility mode expose Architect analysis, and if so, which project-authority and provenance guarantees can it honestly provide without a Loaf?
- How should rules declare partial frontend capability when a normalized fact exists for one language but not yet for the other?
- How should architect distinguish trusted-constant fail-fast calls from caller-input fail-fast calls in a deterministic, maintainable way?
- Which risk findings should ship first, and which local inputs are required before a risk profile can produce useful results?
- Should third-party rule packages be considered after v0.7, or should v0.7 explicitly restrict rule authoring to the Incan repository?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
