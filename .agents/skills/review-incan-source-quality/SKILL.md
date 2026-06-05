---
name: review-incan-source-quality
description: Review touched Incan source for Python-quality readability, idiomatic Incan structure, dogfooding integrity, public API polish, and source-level anti-patterns. Use for `.incn` stdlib, examples, language-surface implementations, or when the user asks whether Incan code reads like well-written Python rather than Rust-shaped scaffolding.
---

# Review Incan Source Quality

## Purpose

`/review-incan-source-quality` is a report-only reviewer for touched `.incn` source.

The standard is not merely "has comments" or "passes tests". The standard is:

- the source should honor the Zen of Incan: readability counts, safety over silence, explicit over implicit, visible performance costs, explicit namespaces, and one obvious way with documented escape hatches;
- the code should read like well-written Python;
- the source should use Incan as the implementation language, not as a thin facade;
- public APIs should feel authored for users;
- private helpers should sit near the public or external implementation they support when that makes the file easier to read;
- comments should explain intent, protocol invariants, boundary quirks, and non-obvious implementation depth.

Own:

- `.incn` readability and source organization
- stdlib dogfooding quality
- evidence that implementation choices were based on current Incan capability, not stale assumptions
- Pythonic naming, helper shape, and control flow
- complete and descriptive module, type, function, method, property, alias, partial, and helper docstrings
- public API examples where they clarify use
- inline comments that clarify non-obvious behavior for Python-oriented readers
- anti-patterns that make Incan source look generated, Rust-shaped, or backend-driven

Do not own:

- broad architecture placement decisions
- Rust prose or rustdoc quality
- test style outside `.incn` test fixtures/examples
- docs truthfulness outside comments/docstrings embedded in source
- final branch-clean judgment

## Output artifacts

Write a slice report at:

- `.agents/state/review-report.incan-source-quality.md`

Do not write to the canonical `.agents/state/review-report.md`.

When the source-quality report has findings, also copy the scope and findings into the shared lightweight central snapshot folder outside individual repo worktrees:

- `/Users/danny/Development/encero/.incan-review-findings/`

Use a deterministic, descriptive filename when possible:

- `YYYY-MM-DD-pr-<number>-incan-source-quality-<slug>.md`
- `YYYY-MM-DD-branch-<branch-slug>-incan-source-quality.md`
- `YYYY-MM-DD-review-incan-source-quality.md` when no PR or branch context is known

Do not create a snapshot for clean reviews. The snapshot is raw corpus for later analysis, not canonical guidance.
Treat `/Users/danny/Development/encero/.incan-review-findings/` as an append-only central corpus for local review work: create new snapshot files, but do not delete, overwrite, prune, or "clean up" existing snapshots unless the user explicitly asks for that exact maintenance.
Snapshot content is evidence, not a fix log. Preserve the original finding blocks verbatim, including severity, category, file path, line reference, and explanatory text. If the finding is fixed before the snapshot is written, keep the original finding as observed and add a separate `Resolution` note after it; do not replace the evidence with a resolved checklist item or a summary of the fix.

## Review standard

Treat touched Incan source as user-facing language showcase code, especially under `crates/incan_stdlib/stdlib/`, examples, fixtures that teach behavior, and RFC-backed language features.

Good Incan source should have:

- top-down structure that exposes the public contract before implementation details;
- local grouping that keeps internal helpers close to the public functions, methods, properties, or trait implementations they support when proximity improves comprehension;
- names that describe domain concepts rather than backend mechanics;
- small helpers that remove real complexity;
- direct `?`, `if let`, `while let`, `loop:` with `break <value>`, early return, and value-enum/model usage where those make the code simpler;
- public docstrings with `std.fs`-style shape: summary, semantic notes, and `Args`, `Returns`, or `Example` sections where useful, not one-line labels that restate the name;
- module, class, model, enum, trait, function, method, property, alias, partial, and helper docstrings on every authored declaration, including private/internal helpers;
- enough inline comments for readers coming from Python to understand deeper implementation code without reverse-engineering Rust/backend constraints;
- comments for bit layouts, protocol invariants, compiler boundary workarounds, surprising tradeoffs, ownership-sensitive choices, or intentionally non-obvious algorithm steps;
- ordinary Rust interop only where it imports existing primitives/crates and the `.incn` source still owns the behavior.

## Capability baseline to verify

Reviewers must assume the branch can use the current v0.2+v0.3 language and stdlib surface unless it proves otherwise. Do not accept Rust-shaped or pre-current scaffolding without evidence.

Before judging `.incn` source quality, read the generated feature inventory, current release notes, and relevant reference pages. At minimum check `workspaces/docs-site/docs/language/reference/feature_inventory.md`, `workspaces/docs-site/docs/release_notes/0_2.md`, `workspaces/docs-site/docs/release_notes/0_3.md`, and the detailed inventories in those release-note files. If the touched code claims a limitation, verify it against source, examples, tests, or a probe.

Baseline v0.2 surfaces to remember:

- namespaced stdlib imports and decorators through `std.*`, including `std.web`, `std.testing`, `std.async`, `std.derives`, `std.traits`, `std.serde.json`, `std.reflection`, and `std.math`;
- explicit Rust boundary forms: `from rust import ...`, `from rust::... import ...`, keyword-named Rust path imports with `as`, `rusttype`, `interop:` adapters, `[rust-dependencies]`, `@rust.extern`, and `@staticmethod` where that is the correct type-scoped Rust boundary;
- library and module surfaces: `pub::` imports, public API manifests, `pub from ... import ...` reexports, `const` vs `static`, `pub static`, source-root imports, and explicit `src/` project layout;
- callable and generic surfaces: first-class named function references, `Callable[...]` sugar, explicit call-site generics (`callee[T](...)`, `receiver.method[T](...)`, and `_` inference placeholders), generic instance methods, and direct trait-typed annotations;
- trait and derive surfaces: traits are always abstract, traits may adopt supertraits, `std.derives` and `std.traits.*` are source-defined capability contracts, generic `with` bounds use nominal trait conformance, and `@derive(...)` should read as language-surface adoption rather than ad hoc backend magic;
- Python-shaped collection and pattern behavior that already existed by v0.2: list concatenation, `list.extend`, `enumerate` with tuple unpacking in loops, strict `KeyError` / `IndexError` / `ValueError` runtime diagnostics, and precise f-string interpolation spans;
- docstring and tooling expectations: leading docstrings on model/class/enum/trait/newtype bodies round-trip through `incan fmt`, public type-like docs feed library tooling, and the formatter preserves source intent instead of requiring hand-shaped whitespace.

Current v0.3 surfaces to remember:

- control flow: `if let`, `while let`, pattern alternation with `|`, `loop:` as an expression, `break <value>`, direct `?`, and `Result` combinators (`map`, `map_err`, `and_then`, `or_else`, `inspect`, `inspect_err`);
- type and API shape: value enums, enum variant aliases, enum methods, enum trait adoption, computed `property` declarations, generic methods, direct trait annotations, supertraits, multi-instantiation trait adoption, associated type declarations, and method-level `for Trait` disambiguation;
- public-surface reuse: symbol aliases, enum variant aliases, same-type method aliases, and `partial` callable presets when a new name or preset should project an existing callable instead of adding a hand-written wrapper;
- collection and iteration: iterator adapters and terminal consumers, generator functions/expressions, tuple-unpack comprehension targets, variadic `*args` / `**kwargs`, call-site unpacking with `f(*xs)` / `f(**kw)`, spread entries (`[*xs]`, `{**kw}`), `list.repeat`, `List[T].clone()` only when semantically needed, and `std.collections` types when specialized collection behavior is the point;
- data and boundary types: exact-width numeric annotations, decimal/numeric precision-scale types, validated newtypes with checked coercion, union types with narrowing, stdlib `std.fs`, `std.io`, `std.tempfile`, `std.datetime`, `std.graph`, `std.logging`, and `std.telemetry` surfaces;
- testing and examples: inline `module tests:`, `assert expr[, msg]`, `assert ... raises ...`, fixtures, parametrization, markers, and async fixtures where the touched `.incn` file is test or example source;
- async: `Awaitable[T]`, `race for`, `std.async.race`, task/channel/time/sync helpers, cancellation-safety comments, and explicit `await` rather than hidden fire-and-forget calls;
- decorators and DSLs: typed user-defined decorators, decorator factories, method decorators, scoped DSL surface descriptors, scoped DSL symbols, and explicit `std.builtins.<name>` when a DSL or import shadows a builtin;
- interop: Rust trait adoption through Incan `with TraitName`, method-level `for Trait`, associated types, narrow `@rust.allow(...)`, imported extension-trait metadata, and ordinary Rust imports only where the `.incn` source still owns the behavior.

Flag stale source that:

- uses mutable accumulators, sentinel values, or trailing branch assignment where `loop:` / `break <value>`, `if let`, `while let`, `?`, or a `Result` combinator would make intent direct;
- adds detached helper functions for behavior that belongs as an enum method, computed property, trait default, same-type method alias, or partial preset;
- hand-rolls queue, counter, ordered map/set, sorted map/set, layered map, priority queue, graph, filesystem, IO, datetime, tempfile, logging, or telemetry behavior already covered by a stdlib module;
- writes wrapper callables that only bind keyword defaults when a `partial` declaration would expose the intended public API more clearly;
- keeps compatibility spellings as duplicated functions, methods, or enum variants when aliases would preserve identity and avoid duplicate behavior;
- uses bare or legacy stdlib/decorator names where the current `std.*` surface is expected;
- treats async/assert/testing/web/derive/trait behavior as compiler magic when the current surface is import-driven and source-authored;
- misses `pub::`, `pub static`, `pub from`, call-site generics, first-class function references, or `Callable[...]` when those are the established surface for the shape at hand;
- uses `.clone()`, `.to_string()`, `str(...)`, `.into()`, or Rust-facing conversion noise only to satisfy generated Rust rather than because the Incan semantics need ownership or conversion;
- hides substantial implementation depth behind terse one-line docstrings or sparse comments.

Flag Incan source that has:

- custom Rust backends hidden behind a thin `.incn` wrapper when the behavior should be dogfooded;
- `@rust.extern`, `rusttype`, or `rust.module` used to avoid writing expressible Incan behavior;
- design narrowing or backend fallback justified by “Incan cannot do this” without local examples, tests, or probe evidence;
- sentinel initialization such as `value = 0` only to satisfy later branch assignment;
- nested `match` ladders that only peel `Option`/enum variants before continuing, when `if let`, early returns, or a focused shallow `match` would state the same control flow more directly; also flag helper forests that merely hide the ladder one branch at a time;
- verbose `match` blocks that just rewrap a `Result` where `?` would read naturally;
- verbose `match` blocks that only transform one `Result` branch where RFC 070 combinators such as `map`, `map_err`, `and_then`, or `or_else` would state the intent directly;
- unnecessary type noise when inference or a local helper would be clearer;
- Rust-shaped names, ownership workarounds, `.clone()`, `.to_string()`, or manual conversion scaffolding leaking into `.incn`;
- helpers that hide one obvious operation without adding meaning;
- internal helpers collected far away from their only public/external caller when that forces readers to jump around the file;
- avoidable wrapper functions or methods where an alias, method alias, enum variant alias, computed property, trait default, or `partial` preset would state the API relation more honestly;
- stringly or byte-twiddling logic without named intent;
- dense implementation logic with no inline comments for readers who understand Python syntax but not the compiler/runtime machinery underneath;
- comments that narrate the next line instead of explaining why;
- missing, placeholder, stale, too-short, or non-descriptive docstrings on modules, classes, models, enums, traits, functions, methods, properties, aliases, partials, or helpers, even when the declaration is private;
- public APIs that expose compiler/backend vocabulary instead of a Pythonic user-facing surface;
- generated-looking code in authored stdlib or examples.

## Workflow

1. Derive scope from touched `.incn` files in the current worktree plus any `.incn` files named by the user.
2. Read the generated feature inventory and current release-note inventories before style judgment. Use v0.2 as the minimum modern baseline and v0.3 as the active capability baseline.
3. For stdlib or RFC-backed work, compare source shape against nearby established modules such as `std.fs`, `std.io`, `std.collections`, or the relevant domain module.
4. Check whether the implementation claims or implies a current Incan limitation. If so, verify that the branch records local precedent, tests, or probe evidence for that limitation.
5. Inspect declarations first: module docstring, public and private types, functions, methods, properties, aliases, partials, argument order, return shape, and examples.
6. Inspect file layout next: whether public/external implementations are easy to find, whether internal helpers are grouped near their use, and whether section ordering lets a Python-oriented reader build context naturally.
7. Inspect implementation helpers next: helper names, docstrings, control-flow readability, branch shape, conversion noise, and whether helpers clarify or obscure.
8. Inspect comments/docstrings last as part of source quality, not as a separate docs-only pass. Short or non-descriptive docstrings are findings even when every declaration technically has one.
9. For each finding, explain what a Pythonic/Incan-native version would make clearer. Do not demand style churn when the existing shape is already direct and readable.
10. Stay report-only unless the user explicitly asks for fixes.
11. If the report contains findings, create `/Users/danny/Development/encero/.incan-review-findings/` if needed and write a new snapshot containing only the review source metadata, Scope, and Findings sections. Copy the finding blocks verbatim from `.agents/state/review-report.incan-source-quality.md`; do not generalize them into policy or rewrite them as fix summaries. Preserve exact file:line evidence when the report has it; if a finding is only file-level, make that explicit in the finding text. Do not overwrite an existing snapshot path; add a suffix if needed.

## Slice report shape

Keep findings first. Only list clean surfaces when the clean call is useful because the file is a visible Incan surface or had prior risk.

```md
# Review Slice Report

- role: incan-source-quality
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - crates/incan_stdlib/stdlib/uuid.incn

## Findings

- [ ] warning | source-quality | Rust-shaped sentinel read | crates/incan_stdlib/stdlib/uuid.incn:117
  The function initializes a placeholder byte and overwrites it from a match arm. A direct helper returning `Result[u8, UuidError]` would read like authored Incan rather than generated Rust-shaped control flow.

## Reviewed Clean Surfaces
- crates/incan_stdlib/stdlib/fs/path.incn — used as style baseline
```

Finding severities:

- `blocker`: source violates an explicit dogfooding or implementation-boundary requirement.
- `error`: source is misleading, generated-looking, or exposes backend shape in a user-facing API.
- `warning`: source works but is below the Python-quality readability bar.
- `note`: cleanup is optional but useful if the file is already being edited.

If there are no findings, say so explicitly.

## Findings snapshot shape

Only write this file when findings are present.

```md
# Incan Source Quality Findings Snapshot

- source: PR #<number> / <branch-or-context>
- date: YYYY-MM-DD
- reviewer: review-incan-source-quality

## Scope
- assigned files:
  - crates/incan_stdlib/stdlib/uuid.incn

## Findings
- [ ] warning | source-quality | Rust-shaped sentinel read | crates/incan_stdlib/stdlib/uuid.incn:117
  The function initializes a placeholder byte and overwrites it from a match arm. A direct helper returning `Result[u8, UuidError]` would read like authored Incan rather than generated Rust-shaped control flow.

## Resolution
- <optional, only if this snapshot is written after a fix loop>
```
