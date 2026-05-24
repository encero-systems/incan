# RFC 105: `incan architect` rule engine for design, safety, idiom, and smell findings

- **Status:** Draft
- **Created:** 2026-05-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (generators)
    - RFC 048 (contract-backed models emit and tooling)
    - RFC 070 (Result combinators)
    - RFC 088 (iterator adapter surface)
    - RFC 096 (declaration metadata blocks)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/663
- **RFC PR:** https://github.com/dannys-code-corner/incan/pull/664
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes `incan architect` as a deterministic code-advice command for Incan projects. The command reports evidence-backed findings across architecture, safety, idiom usage, and maintainability smells by running maintainable rules over compiler-backed codegraph facts. The central goal is not to create a broad subjective linter, but to create a durable rule authoring surface where new advice can be added cheaply, tested precisely, calibrated against real projects, and consumed by humans, agents, editors, and CI without relying on model inference for core detection.

## Core model

1. **Compiler-backed facts first:** `incan architect` consumes source facts produced by Incan's parser, module/import resolver, typechecker, metadata pipeline, and codegraph exporter rather than independently scraping text.
2. **Rules interpret facts:** Each rule consumes typed fact views and emits findings with stable codes, priorities, categories, confidence, evidence, suggestions, and risks.
3. **Findings are advisory:** Architect findings are not compiler errors. They describe design pressure or code-shape opportunities with enough evidence for a human or agent to decide whether to act.
4. **Categories are explicit:** Architecture findings, safety findings, idiom findings, and code-smell findings remain separate in rule codes and profiles even when they share one command.
5. **Conservative detection is preferred:** The command should under-report ambiguous style opportunities rather than produce noisy, low-trust advice.
6. **Rule authoring is a product surface:** The feature is only maintainable if adding a rule means using stable typed facts and reusable queries, not hand-parsing raw graph nodes or reimplementing AST walks.

## Motivation

Incan already has syntax checks, semantic checks, formatter behavior, tests, and generated-Rust validation. Those tools answer whether a program parses, typechecks, formats, and runs. They do not answer whether a project is accumulating design pressure: repeated dispatch over the same domain, public boundaries that can panic on recoverable input, old-shaped control flow that should now use language features, or small helper functions that add indirection without carrying domain meaning.

The first experiments with an architecture-advice command showed that deterministic rules can surface useful pressure when they report concrete source evidence and stay cautious about severity. Repeated match dispatch can reveal a growing operation boundary. Fail-fast calls inside public APIs can reveal recoverability problems. Body-shape facts can also support smaller maintainability smells such as compound-assignment candidates, single-use trivial helpers, append-only list builders that could become comprehensions, or `Result` matches that could use RFC 070 combinators.

Without a formal rule engine, each new check risks becoming a one-off command-private AST walk with custom parsing, inconsistent output, and ad hoc severity. That path does not scale. The value is in a shared substrate: one project-wide codegraph, one typed query layer, one finding model, one de-duplication path, and many small rules that are easy to review and calibrate.

This feature also matters for agent workflows. Agents can already make broad refactoring suggestions, but those suggestions are often expensive to verify and easy to overfit. `incan architect` should provide deterministic evidence that an agent can use as grounding: exact files, lines, matched domains, shared patterns, call sites, usage counts, and counterexample risks. A model may later summarize or prioritize findings, but the core detection should remain inspectable and reproducible.

## Goals

- Define `incan architect` as the umbrella command for deterministic design, safety, idiom, and maintainability-smell advice.
- Provide a stable finding model with rule code, category, priority, confidence, evidence, pressure, suggestions, risks, and machine-readable output.
- Provide project-wide directory scanning over `.incn` source trees with deterministic module de-duplication and finding de-duplication.
- Establish rule categories and profiles so users can run architecture-only, safety-only, idiom-only, smell-only, or all-rule scans.
- Establish a maintainable rule authoring surface based on typed facts and reusable queries over codegraph data.
- Extend codegraph body facts as needed for rule families such as match dispatch, call sites, references, assignment/update shapes, helper usage, loop-builder shapes, and result-match shapes.
- Include code smells in scope when they can be detected conservatively with clear evidence and useful counterexamples.
- Keep detection deterministic for the first version; no language model is required for core finding generation.
- Support text output for humans and stable JSON output for tools, agents, editors, and CI.
- Make suppression and baselining part of the product model so mature codebases can adopt the command incrementally.

## Non-Goals

- This RFC does not make architect findings compiler errors.
- This RFC does not replace formatter rules, typechecker diagnostics, Clippy-style generated-Rust checks, or project tests.
- This RFC does not require a small language model or remote AI service for rule detection.
- This RFC does not attempt to infer developer intent from names alone.
- This RFC does not require every possible code smell to ship in the first version.
- This RFC does not define automatic rewrites or apply fixes.
- This RFC does not define a public plugin ABI for third-party binary rule packages.
- This RFC does not require every codegraph fact to be part of a permanently stable external schema in the first release; only the JSON findings format and documented command behavior need v0.5 stability.

## Guide-level explanation

Users run `incan architect` on a file or project directory.

```bash
incan architect .
incan architect src/lib.incn --format json
incan architect . --profile architecture
incan architect . --profile smells
```

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

Architect findings use categories. Architecture findings describe design pressure. Safety findings describe failure or recoverability risk. Idiom findings describe opportunities to use Incan features more directly. Smell findings describe local maintainability pressure.

```text
safety.fail_fast_boundary_call
idiom.result_combinator_candidate
smell.single_use_trivial_helper
arch.repeated_match_dispatch
```

Small smells are allowed when they are precise and humble. A trivial helper rule can identify a private helper that is used once and only returns a pure expression.

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

`incan architect [PATH] [OPTIONS]` must accept a source file or directory. When `PATH` is omitted, the command should scan the current directory.

When `PATH` is a file, the command must scan the file and the modules needed to resolve its imports according to ordinary Incan module rules.

When `PATH` is a directory, the command must scan `.incn` files under that directory recursively. The scan must be deterministic. The scan must de-duplicate modules by source path so a file imported by multiple roots contributes facts once.

The command must provide `--format text` and `--format json`. Text output is for humans. JSON output is the integration surface for agents, editors, CI, dashboards, and future baselining tools.

The command should provide `--profile` with at least `architecture`, `safety`, `idioms`, `smells`, and `all`. The default profile is unresolved by this draft.

### Finding model

Every finding must have a stable rule code. Rule codes must be namespaced by category.

```text
arch.repeated_match_dispatch
safety.fail_fast_boundary_call
idiom.result_combinator_candidate
smell.single_use_trivial_helper
```

Every finding must include a category, priority, confidence, title, pressure, evidence, suggestions, and risks.

Priority must describe expected action pressure, not proof certainty.

```text
P1: likely correctness, reliability, or public-boundary risk that should be reviewed before release
P2: meaningful design or maintainability pressure that should be tracked or scheduled
P3: watchlist, idiom, or local smell that may be worth cleanup when nearby work touches the code
Info: low-pressure educational or style-level advice
```

Confidence must describe how mechanically strong the rule match is.

```text
High: the rule found a narrow, mechanically recognizable shape
Medium: the rule found a useful pattern with plausible counterexamples
Low: the rule is exploratory and should normally be hidden outside explicit profiles
```

Evidence must identify source file, line, column, owner declaration when available, and rule-specific context. Rule-specific context may include matched arms, overlap counts, fallback/default-arm presence, callee labels, usage counts, body-shape summaries, or suggested replacement text.

Suggestions must be phrased as advice, not certainty. Risks must name the common counterexamples that would make the suggestion wrong.

Findings must be de-duplicated before output. Identical findings produced through multiple import roots must appear once.

### Rule categories

Architecture rules describe design pressure across declarations, modules, domains, or boundaries. Repeated match dispatch, growing literal domains, and operation-boundary pressure belong here.

Safety rules describe recoverability, fail-fast behavior, partial handling, unchecked assumptions, or public-boundary hazards. A public function that can panic on caller-provided data belongs here.

Idiom rules describe opportunities to use Incan language or stdlib features more directly. Result combinator candidates, iterator adapter candidates, generator/comprehension candidates, and compound assignment candidates belong here.

Smell rules describe local maintainability pressure. Single-use trivial helpers, repeated literals, unnecessary wrappers, long branch-heavy functions, and append-only builders belong here when detected conservatively.

Rules must not be categorized as architecture findings merely because they are emitted by `incan architect`.

### Rule authoring contract

Rules must declare metadata: code, category, default priority, default confidence, profile membership, required fact kinds, and a short explanation.

Rules must consume typed fact views rather than raw serialized facts. A rule that needs match dispatch sites, call sites, assignment shapes, helper usage counts, or loop-builder shapes should ask for those views directly.

Rules should be small and independently testable. Each rule should have positive and negative fixtures. Negative fixtures are required for common counterexamples named in the rule's risk text.

Rules must not require typechecked metadata when a syntactic fact is sufficient. Rules may use type facts when precision depends on type information, such as recognizing `Result[T, E]` match shapes.

Rules should prefer narrow body-shape facts over broad textual heuristics. For example, a comprehension candidate should be based on an append-only list-builder shape, not the mere presence of a `for` loop and `append`.

Rules must not emit findings for generated stdlib internals or known external code unless the user explicitly scans those sources.

### Codegraph fact requirements

The codegraph exporter must provide enough source facts for rules to avoid command-private AST walks. The first useful fact families are declarations, imports, public API metadata, match dispatches, call sites, references, assignment/update shapes, function body summaries, usage counts, loop-builder shapes, and result-match shapes.

Match dispatch facts must include the matched domain, explicit pattern labels, explicit pattern count, source arm count, and wildcard/default-arm context.

Call-site facts must include callee key, callee label, receiver shape when available, source location, and owner declaration.

Reference facts must support usage counting for private declarations and helper functions.

Assignment/update facts must make compound-assignment candidates expressible without string matching.

Function body summary facts should identify simple shapes such as single-return expression, pure expression wrapper, append-only list builder, and short result-match transform. These summaries must be conservative.

Result-match facts should identify branch-preserving transformations only when the matched expression is known to be a `Result[T, E]` or the syntactic shape is unambiguous enough for an idiom finding with appropriate confidence.

### Suppression and baselining

The command should support local suppression of a specific rule at a specific source location. Suppression syntax is unresolved by this draft.

The command should support project baselines so existing findings can be recorded and new findings can fail CI or be highlighted separately. Baseline storage is unresolved by this draft.

Suppressions and baselines must preserve rule code and evidence identity. A future change that moves or changes the evidence should not silently suppress an unrelated finding.

## Design details

### Profiles

Profiles let users choose the kind of advice they want. `architecture` should include cross-cutting design pressure. `safety` should include fail-fast and recoverability risk. `idioms` should include feature-usage opportunities. `smells` should include local maintainability findings. `all` should include every non-experimental rule.

Rules may belong to more than one profile only when that does not blur the category. For example, a public fail-fast boundary call is a safety finding even if it also has architecture implications.

Exploratory rules may exist behind an explicit experimental profile, but they must not be enabled by default.

### Severity calibration

Severity should be calibrated against evidence strength, public surface impact, and likely cost of ignoring the finding. Public API failures are generally higher priority than private helper smells. Repeated design pressure across files is generally higher priority than a local expression-level cleanup. Idiom suggestions are generally P3 or Info unless the shape creates repeated complexity or risk.

Rules should downrank or suppress known low-action cases. For example, fail-fast calls around trusted constants may be lower priority than fail-fast calls around caller-provided input. Exhaustive matches over a closed domain may be preferable to abstraction when the matched operation is local and the domain changes rarely.

### Examples of initial rules

`arch.repeated_match_dispatch` reports repeated match expressions that dispatch over the same domain and share multiple explicit arms. The rule should report overlap counts and wildcard/default context.

`safety.fail_fast_boundary_call` reports `unwrap`, `expect`, `panic`, `todo`, and `unreachable` inside public or internal boundaries. Public API boundaries should generally be P1. Internal boundaries should generally be P2 unless evidence shows trusted constants or invariant setup.

`idiom.result_combinator_candidate` reports obvious RFC 070 match shapes that can be expressed with `map`, `map_err`, `and_then`, `or_else`, `inspect`, or `inspect_err`.

`idiom.compound_assignment_candidate` reports assignments such as `i = i + 1` when the target and left operand are the same simple storage place and `i += 1` is equivalent.

`idiom.comprehension_candidate` reports append-only list builders that can be represented as eager list comprehensions.

`smell.single_use_trivial_helper` reports private, undocumented, undecorated helpers that are used once and only return a simple pure expression. The rule must mention that domain vocabulary can justify keeping the helper.

`smell.repeated_literal_domain` reports repeated raw string or scalar literal domains used as branch keys or dispatch keys across multiple sites.

## Alternatives considered

### Keep architect as architecture-only

This would preserve a narrow name, but it would force closely related idiom and smell findings into a separate command even though they need the same project-wide codegraph, evidence model, de-duplication, profiles, suppressions, and JSON output. The better boundary is category namespace, not separate infrastructure.

### Build a general linter instead

A general linter would fit small syntax-level advice, but it would understate the project-wide design-pressure use case. The command should remain broader than a linter while still identifying local smells as one category.

### Use a language model for rule detection

Model-based detection may be useful later for summarization, clustering, or explaining findings in pull requests. It is not the right foundation for v0.5 rule detection because findings need to be reproducible, testable, source-grounded, and suitable for CI.

### Let every rule walk the AST directly

This is the fastest way to add a first rule and the worst way to maintain many rules. It duplicates traversal logic, fragments fact extraction, and makes rule behavior harder to share with agents, editors, and other code-intelligence tools.

### Make findings auto-fixable from the start

Some findings will eventually support safe rewrites, such as compound assignment candidates. Making fixes part of the first version would expand the scope into formatter, semantic preservation, and edit application. The first version should focus on reliable findings and stable output.

## Drawbacks

This feature adds a new advisory surface that can become noisy if rule quality is poor. The command must earn trust by being conservative, showing evidence, and naming counterexamples.

The codegraph fact model will grow. If facts are added without a typed query layer, rules will become stringly and brittle. If facts are over-designed too early, implementation will slow down before the rule set proves itself.

Some code smells are subjective. A helper that looks unnecessary may carry important domain meaning. A loop that could be a comprehension may be clearer as a loop when side effects are about to be added. The finding model must make room for this uncertainty through confidence and risk text.

Project-wide scanning may be slower than entry-point scanning. The implementation should keep scans deterministic and should leave room for caching, but v0.5 should prioritize correctness and evidence over premature optimization.

## Implementation architecture

This section is non-normative.

The recommended internal shape is a layered pipeline: source collection, compiler-backed codegraph extraction, typed fact views, query indexes, independent rule modules, finding normalization, de-duplication, profile filtering, and text/JSON rendering.

The codegraph layer should remain the producer of source facts. The architect layer should not own parsing or typechecking behavior. Architect rules should operate over typed views such as match dispatch sites, call sites, references, assignment/update candidates, usage counts, loop-builder shapes, and result-match shapes.

The rule engine should provide a small metadata contract for rule authors. A rule should declare its code, category, default priority, confidence, profiles, required facts, and explanation. A rule should receive a query context and emit findings.

The report layer should be shared by all rules. Sorting, de-duplication, JSON serialization, text formatting, suppression matching, and baseline matching should not be implemented per rule.

The first version should ship with a small calibrated rule set rather than a large catalogue. New rules should be added only when they have clear positive fixtures, negative fixtures, and calibration evidence from real source.

## Layers affected

- **Parser / AST**: No new user syntax is required, but source traversal must expose enough body shapes for codegraph facts.
- **Typechecker / Symbol resolution**: Rules may need checked public API metadata, resolved imports, type facts for `Result` shapes, and symbol usage information.
- **IR Lowering**: No required impact.
- **Emission**: No required impact.
- **Stdlib / Runtime (`incan_stdlib`)**: No required runtime impact, though stdlib feature surfaces such as Result combinators and iterator adapters inform idiom rules.
- **Formatter**: No required impact unless future auto-fix support is added.
- **LSP / Tooling**: The JSON findings format should be usable by editors, agents, CI, and future diagnostics-style surfaces.
- **CLI / Project tooling**: `incan architect` needs project-wide scanning, profiles, stable text/JSON output, suppression support, and baseline support.
- **Documentation**: The CLI reference must document command behavior, profiles, categories, priorities, confidence, suppressions, and examples.

## Unresolved questions

- What is the default profile for `incan architect .`: architecture-only, architecture plus safety, or all stable rules?
- What suppression syntax should Incan use for architect findings, and should it share vocabulary with compiler diagnostic suppressions?
- Should baselines live in `incan.toml`, a separate lock-like file, or a generated artifact under project tooling state?
- Which finding fields are stable enough to commit as v0.5 JSON output, and which should remain experimental?
- Should code-smell findings use the namespace `smell.*` or `maintainability.*`?
- Should project-wide directory scanning include tests by default, and should findings from tests use a separate priority calibration?
- How should architect distinguish trusted-constant fail-fast calls from caller-input fail-fast calls in a deterministic, maintainable way?
- Should third-party rule packages be considered after v0.5, or should v0.5 explicitly restrict rule authoring to the Incan repository?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
