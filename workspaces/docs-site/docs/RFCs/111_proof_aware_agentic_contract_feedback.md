# RFC 111: Proof-aware agentic contract feedback

- **Status:** Draft
- **Created:** 2026-06-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 080 (AI assets, models, prompts, evals, and agent metadata)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 096 (declaration metadata blocks)
    - RFC 106 (compiler-backed agent context graph)
- **Issue:** #787
- **RFC PR:** —
- **Written against:** v0.4
- **Shipped in:** —

## Summary

This RFC defines proof-aware contracts for Incan: a source-level contract surface for refined values, preconditions, postconditions, invariants, and verification outcomes that lets humans state domain intent while agents implement ordinary Incan code against compiler-checkable obligations. The compiler classifies each generated obligation as proved, violated, unknown, unsupported, or runtime-enforced, exposes counterexamples and assumptions where available, and publishes structured verification facts through RFC 106 so agentic tooling can repair implementations against human-authored specifications instead of relying on prose-only feedback.

## Core model

1. **Humans own intent:** users write domain contracts in source using types, refinements, preconditions, postconditions, invariants, and named semantic declarations.
2. **Agents own implementation labor:** generated or assisted code is expected to iterate against compiler feedback until it satisfies the declared contract or exposes a contract gap for human review.
3. **Contracts are opt-in, then binding:** ordinary Incan code remains valid without function contracts or loop invariants, but any contract the user writes becomes a real checked obligation rather than documentation.
4. **Contracts are source facts:** contracts are not comments, prompt text, or hidden generated-Rust assertions; they are checked Incan declarations with source spans, metadata, and documentation presence.
5. **Verification is outcome-based:** the compiler reports whether each obligation is proved, violated, unknown, unsupported, or enforced at runtime instead of collapsing all proof failures into a generic type error.
6. **Runtime fallback is explicit:** unproved obligations may remain protected by generated runtime checks when that is sound and configured, but that fallback is visible in diagnostics, metadata, and graph facts.
7. **Proof checkers are providers:** SMT solvers, abstract interpreters, range analyzers, symbolic evaluators, or future theorem-prover bridges are verification providers behind a stable Incan outcome vocabulary; no provider owns the language surface.
8. **The graph carries repair context:** RFC 106 is the delivery substrate for contract declarations, generated obligations, outcomes, counterexamples, runtime checks, and proof receipts so agents receive targeted repair context.
9. **Proof is bounded:** Incan proves a clearly defined subset and records assumptions; it must not claim full program correctness beyond the authored contracts and supported reasoning fragment.
10. **Trust is inspectable:** every proof result must record enough provenance for tools to distinguish compiler-local checks, solver-backed checks, runtime checks, cached results, unsupported fragments, and degraded work-in-progress output.

## Motivation

Incan already leans into a future where agents generate more code and the language provides a smaller, typed, auditable surface. Types, `Result`, `Option`, newtypes, checked metadata, and Rust-backed emission catch broad classes of mistakes, but they do not fully capture human intent. A human can say "a percentage is between 0 and 100" or "this transfer never creates a negative balance," but today that knowledge is spread across validation code, tests, comments, and review conventions rather than represented as a compiler-owned contract that an agent can repair against.

Formal-methods languages and tools point at the missing layer. SPARK makes contracts such as preconditions, postconditions, type invariants, and assertions part of the language and uses deductive proof to establish absence of runtime errors and functional properties. Dafny is verification-aware and has native specifications, a static verifier, preconditions, postconditions, termination conditions, loop invariants, and read/write specifications. F* combines dependent types with SMT-backed automation and interactive theorem proving. Why3 provides a provider-oriented deductive verification platform where verification conditions can be discharged by external provers. Frama-C's ACSL shows the value of formal function contracts that are understandable by humans and manipulable by analyzers. The lesson is not that Incan should become Ada/SPARK, Dafny, F*, WhyML, or ACSL; the lesson is that source-level intent must become structured, analyzable, and tied to implementation feedback.

The agentic angle changes the ergonomics. Traditional formal verification often assumes that trained humans will repair failed proofs, supply invariants, and adjust specifications. Incan can reshape that workflow for this century: a human writes the domain contract, an agent writes or edits ordinary Incan code, the compiler reports structured verification facts and counterexamples, and the agent uses RFC 106 context packing to inspect only the relevant declarations, assumptions, tests, and failed obligations. Humans still decide the specification; agents handle much of the iteration needed to satisfy it.

This also keeps Incan honest about correctness. A spec can be wrong, incomplete, or too weak. A proof can only establish a property under declared assumptions and supported semantics. The value of this RFC is not magic correctness; it is a bounded, inspectable feedback loop where human-authored intent is mechanically checked and agent repairs are grounded in compiler facts.

## Goals

- Define a source-level contract model for refined values, preconditions, postconditions, invariants, loop invariants, and termination hints where they are needed.
- Keep explicit function contracts and loop invariants opt-in for ordinary code, while allowing project policy or CI profiles to require them for selected packages or modules.
- Extend the meaning of existing type-shaped constraints from RFC 085 and RFC 091 into proof-aware obligations without replacing validated newtype construction.
- Define verification obligations as compiler-visible facts generated from typed source semantics.
- Define stable verification outcomes: `proved`, `violated`, `unknown`, `unsupported`, and `runtime_enforced`.
- Define counterexample and assumption reporting suitable for human diagnostics and agent repair loops.
- Define when proved obligations may allow a generated runtime check to be skipped because proof made it unnecessary, and require visible proof or verification receipts for that skipped-check decision.
- Define how unproved obligations remain protected by runtime checks when runtime enforcement is sound and configured.
- Define RFC 106 graph fact families for contracts, obligations, outcomes, counterexamples, runtime checks, and verification receipts.
- Keep the Rust backend as the ordinary emission target while making proof facts backend-independent where possible.
- Learn from Ada/SPARK-like systems, Dafny, F*, Why3, and ACSL while preserving Incan's Python-like authoring surface and agent-native tooling model.

## Non-Goals

- This RFC does not make Incan a dependent type language.
- This RFC does not require users to write interactive proof scripts.
- This RFC does not require every function, loop, module, or package to carry contracts by default.
- This RFC does not replace the Rust backend or make Ada/SPARK an Incan code generation target.
- This RFC does not require a particular SMT solver, theorem prover, storage engine, MCP server, or hosted verification service.
- This RFC does not claim that all Incan programs become formally verified.
- This RFC does not prove correctness of specifications themselves.
- This RFC does not remove runtime validation from validated newtypes, model constructors, or library contracts unless a matching proof result and configuration justify skipping the redundant generated check.
- This RFC does not define every possible contract predicate in the first release.
- This RFC does not make agent output correct by default; it makes agent output more mechanically checkable against human-authored contracts.
- This RFC does not require remote model inference, remote solver execution, or remote graph indexing.

## Guide-level explanation

Contracts are optional in ordinary Incan code. A user can write normal functions, loops, models, and validated newtypes without adding `requires`, `ensures`, or `invariant` statements. When a contract is present, however, the compiler treats it as part of the program: it must be checked, proved, rejected, marked unknown/unsupported, or protected by a visible runtime fallback according to the selected verification mode.

The intended workflow starts with a human writing a contract where the domain needs one. A percentage type remains a normal domain type, but its bounds are compiler-visible:

```incan
type Percentage = newtype int[ge=0, le=100]

def average(a: Percentage, b: Percentage) -> Percentage:
    return Percentage((a.value + b.value) // 2)
```

The compiler already knows the input values satisfy `0 <= value <= 100`. A proof-aware checker can generate an obligation for the `Percentage(...)` construction: prove that `(a.value + b.value) // 2` is also between `0` and `100`. If the obligation is proved, the compiler can record a proof result and avoid generating a redundant runtime check at that construction site when skipped checks are enabled. This is sometimes called "elision": skipping generated code because proof showed it is unnecessary. If the obligation cannot be proved, generated code must still validate the `Percentage` construction unless strict verification mode rejects the program.

The same model is not limited to numbers. String-shaped domain values can also carry contracts, but the supported proof fragment is different. An email address type may prove local string facts such as length and the presence of an `@` separator, while real-world deliverability remains a runtime or external-system concern:

```incan
type EmailAddress = newtype str[pattern="^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$", max_length=254]

def local_part(email: EmailAddress) -> str:
    parts = email.value.split("@")
    result = parts[0]
    ensures "@" not in result
    return result
```

In this example, `result` is an ordinary local variable chosen by the author, not a hidden keyword. The `ensures` line checks the value that is about to be returned: after `local_part` returns, the returned string must not contain `@`. The contract does not claim that the mailbox exists, that the domain accepts mail, or that a confirmation link was clicked. It claims a source-checkable string shape: `EmailAddress` values satisfy the declared pattern and length bound, and the `local_part` result is the substring before the separator. A verifier may prove the postcondition when the string operations and pattern fragment are supported, keep the existing runtime validation when they are not, or report `unsupported` if the pattern is outside the supported regular-language fragment.

Function contracts make intent explicit when the result property is not just the target type. The next example is deliberately ordinary application logic: a withdrawal either returns an updated account or an insufficient-funds error. The reader should look at two layers at once: the contract says what every successful result must guarantee, while the branch in the body gives the verifier the local fact it needs before subtracting.

```incan
type Cents = newtype int[ge=0]

model Account:
    balance: Cents

def withdraw(account: Account, amount: Cents) -> Result[Account, str]:
    if amount > account.balance:
        return Err("insufficient funds")

    new_balance = account.balance - amount
    result = Ok(Account(balance=Cents(new_balance)))
    ensures result.is_ok() implies result.unwrap().balance <= account.balance
    ensures result.is_ok() implies result.unwrap().balance >= Cents(0)
    return result
```

The exact helper names in this example are illustrative, but the contract shape is the important point: the implementation binds the value it is about to return, and `ensures` states what that return value must satisfy on the `Ok` path. The `if amount > account.balance` guard establishes that the later `Ok` path only runs when `amount <= account.balance`. A proof-aware checker can turn those source facts into a successful obligation:

```text
verification[E1111]: postcondition proved
  contract: withdraw ensures result.is_ok() implies result.unwrap().balance >= Cents(0)
  obligation: successful withdrawal keeps balance non-negative
  established by:
    branch fact: amount <= account.balance on the Ok path
    type invariant: account.balance >= Cents(0)
    type invariant: amount >= Cents(0)
  runtime_check: Cents(new_balance) validation may be skipped for this path because the obligation was proved
```

The same diagnostic model becomes more useful when an agent writes an implementation that looks plausible but omits the guard:

```incan
def withdraw(account: Account, amount: Cents) -> Result[Account, str]:
    new_balance = account.balance - amount
    result = Ok(Account(balance=Cents(new_balance)))
    ensures result.is_ok() implies result.unwrap().balance <= account.balance
    ensures result.is_ok() implies result.unwrap().balance >= Cents(0)
    return result
```

Here the contract still says that every successful withdrawal must leave a non-negative balance, but the implementation returns `Ok` for every input. The verifier can now produce a real counterexample:

```text
verification[E1112]: postcondition may be violated
  contract: withdraw ensures result.is_ok() implies result.unwrap().balance >= Cents(0)
  obligation: successful withdrawal keeps balance non-negative
  counterexample:
    account.balance = Cents(5)
    amount = Cents(7)
    new_balance = -2
  note: the Ok path does not establish `amount <= account.balance`; establish that predicate before returning Ok, return Err when it is false, or add a requires clause if callers must prove it
```

This is the subtext of the feature: the compiler is not merely saying "type mismatch." It is explaining the gap between the human-authored contract and the agent-authored implementation, in terms precise enough for the agent to repair the missing branch. Diagnostic notes should be derived from predicates, control-flow shape, and source-authored names; domain wording such as "insufficient funds" may appear only when it comes from user-authored code, metadata, or an explicit contract label.

Loops need invariants when a property cannot be inferred from local statements:

```incan
type Natural = newtype int[ge=0]

def sum_non_negative(values: list[Natural]) -> Natural:
    total = Natural(0)
    for index, value in enumerate(values):
        invariant index >= 0
        invariant index < len(values)
        invariant total >= Natural(0)
        total = total + value
    return total
```

The loop invariants are not comments for a reviewer. They are obligations: the compiler must check that they hold before the loop, are preserved by each iteration, and are strong enough to justify the post-loop facts that later obligations need. An implementation assistant can use failed invariant preservation diagnostics to adjust the loop body or ask the human to strengthen the invariant.

Through RFC 106, both the proved obligation and the failed obligation become graph context. An agent can ask for the task context for a verification result and receive the contract declaration, the function body, the type invariants, nearby tests, related diagnostics, and the graph edges explaining which branch fact proved the property or which expression produced the violating value.

## Reference-level explanation

### Contract declarations

A contract declaration is a source-authored statement or type constraint that defines a checkable property. Contract declarations must have source spans and must be represented in checked metadata when they are part of a public or exported surface.

This RFC defines these contract families:

- refined primitive predicates such as `int[ge=0, le=100]`;
- validated newtype invariants inherited from constrained underlyings;
- function preconditions written as `requires`;
- function postconditions written as `ensures`;
- loop invariants written as `invariant`;
- loop termination hints written as `decreases`;
- model or declaration invariants when a future RFC or a resolved design decision admits that syntax.

Contract declarations must be typechecked before they produce verification obligations. A contract expression must not require arbitrary runtime side effects to evaluate. Contract expressions may refer to function parameters, visible immutable facts, fields of values in scope, prior values through an explicit old-value surface when supported, and local return bindings referenced by nearby `ensures` clauses. A future grammar may also admit a dedicated returned-value binding, but this RFC's examples use explicit locals.

An `assert` statement is not a contract declaration under this RFC. It retains RFC 018's always-on runtime assertion semantics: it checks an implementation fact at a specific program point, may produce a local verification obligation, and may become a fact available to later obligations only after that assertion is itself checked, proved, or runtime-protected. An implementation-local `assert` must not be exported as a caller-visible precondition or postcondition unless the source also declares `requires` or `ensures`.

### Verification obligations

A verification obligation is a compiler-generated question derived from checked source semantics and contract declarations. The compiler must generate obligations for at least:

- values constructed for refined or constrained types;
- assignments and returns into refined or constrained target types;
- calls where caller code must establish callee preconditions;
- function bodies where implementations must establish postconditions;
- loop entries where invariants must initially hold;
- loop bodies where invariants must be preserved;
- loop termination hints where the checker must establish progress under the supported fragment.

The compiler must keep obligations source-anchored. A diagnostic or graph record for an obligation must identify the source contract, the expression or control-flow site being checked, and the assumptions available at that site.

### Verification outcomes

Every obligation must receive one of these outcomes:

- `proved`: the compiler or a verification provider established the obligation under recorded assumptions.
- `violated`: the compiler or provider found that the obligation does not hold, with a counterexample or deterministic local explanation when available.
- `unknown`: the obligation is in scope for the provider, but the provider did not prove or disprove it within the configured budget.
- `unsupported`: the obligation uses language features, predicate forms, effects, theories, or backend assumptions outside the supported verification fragment.
- `runtime_enforced`: the obligation is not statically proved, but generated code or an existing validated construction path enforces the same property at runtime.

`runtime_enforced` may be reported alongside `unknown` or `unsupported` in detailed tooling, but user-facing summaries must make the fallback explicit. A build must not silently present an unproved runtime-enforced obligation as `proved`.

### Build and verification modes

The ordinary check/build workflow should remain usable without requiring every project to satisfy strict formal verification. Function contracts and loop invariants are opt-in unless a package, workspace, CI profile, or future policy requires them for a selected scope. In a permissive mode, an `unknown` or `unsupported` obligation may continue when a sound runtime check protects the property and the diagnostic policy allows it. In a strict verification mode, `unknown`, `unsupported`, and unprotected runtime-only obligations must fail unless explicitly allowed by a local policy.

The compiler must fail for `violated` obligations that are not intentionally guarded by a reachable error path accepted by the contract. For example, a failed callee precondition is a compile-time error in strict checked code, while a validated constructor may remain a fallible runtime path if the surrounding API contract explicitly returns `Result`.

### Runtime enforcement and skipped checks

Runtime checks must remain the conservative fallback. A runtime check may be skipped only when a corresponding obligation is `proved` and the proof result covers the same property, target value, and assumptions as the runtime check. This skipped-check decision must be visible in verification metadata or graph facts.

If a runtime check cannot be generated for a contract expression, an unproved obligation must not be treated as runtime-enforced. Such obligations must be `unknown` or `unsupported` and must fail in strict mode.

### Counterexamples and assumptions

When a verifier reports a counterexample, the compiler should translate it into source-level terms where possible. Counterexamples should prefer Incan identifiers, type names, model fields, enum variants, and literal values over provider-internal names.

Every proof or counterexample must record assumptions that materially affected the result. Assumptions may include function preconditions, type invariants, branch conditions, loop invariants, trusted external function summaries, provider configuration, arithmetic semantics, overflow policy, and runtime-library contracts.

### RFC 106 graph facts

The agent context graph should support these node kinds when this RFC is implemented:

- `contract_decl`: a source-authored contract such as a refined predicate, `requires`, `ensures`, `invariant`, or `decreases`.
- `verification_obligation`: a compiler-generated obligation tied to a contract and source site.
- `verification_result`: the outcome for an obligation.
- `counterexample`: a model or witness explaining a violation where available.
- `runtime_check`: a generated or preserved runtime enforcement site.
- `verification_receipt`: a cacheable proof or verification-result identity that records provider, configuration, assumptions, source identity, and checked fragment.
- `verification_assumption`: a fact used by one or more verification results.

The graph should support these edge kinds when this RFC is implemented:

- `generates_obligation`: from a contract declaration or checked type to an obligation.
- `checks_site`: from an obligation to the expression, statement, call, return, loop, or construction site it checks.
- `has_result`: from an obligation to a verification result.
- `has_counterexample`: from a failed result to a counterexample.
- `enforced_by`: from an unproved obligation or contract to a runtime check.
- `elides_check`: from a proved result to a runtime check that was removed or skipped.
- `assumes`: from an obligation or result to a verification assumption.
- `repairs_context_for`: from a verification result to task-ranked context packs or advisory records that explain likely repair targets.

These records must preserve RFC 106 provenance. A solver-backed result must not be marked as `compiler_checked` unless the compiler owns the validation of that result under the graph schema. Provider-specific facts should identify the provider while preserving a stable Incan outcome vocabulary.

## Design details

### Contract syntax

This RFC intentionally starts with readable keyword contracts instead of importing Ada/SPARK, Dafny, F*, WhyML, or ACSL syntax. The exact grammar remains open, but the source shape should look like ordinary Incan:

```incan
def normalize_score(raw: int) -> Percentage:
    requires raw >= 0
    requires raw <= 100

    result = Percentage(raw)
    ensures result >= Percentage(0)
    ensures result <= Percentage(100)
    return result
```

In the sketch above, `raw` is the ordinary function parameter. `result` is also ordinary source code: the author binds the value that will be returned, then writes `ensures` clauses over that value before returning it. This avoids a hidden keyword while still giving the verifier a clear postcondition target. The exact grammar remains open: Incan must decide whether `ensures` attaches only to the immediately following return, to the enclosing function, or to both through a dedicated contract block.

Contract statements should appear close to the code they constrain. Function-level `requires` should appear before executable statements. `ensures` should appear near the return value it checks, after the value is bound and before the corresponding `return`, unless a future grammar chooses a dedicated function-header contract block. Loop-level `invariant` and `decreases` should appear at the start of the loop body. The formatter should preserve that visual grouping.

`assert` is deliberately separate from `ensures`. An `assert` statement checks a fact at the point where it appears in the implementation; an `ensures` clause declares a postcondition of the function that callers, documentation, verification metadata, and RFC 106 graph context can rely on. A verifier may use a proved `assert` as local evidence for later obligations, but `assert` should not be used as a synonym for "this function promises this after return."

### Supported predicate fragment

The first proof-aware fragment should be small enough to be useful and predictable. It should prioritize:

- integer comparisons and bounded integer arithmetic;
- string length, prefix, suffix, containment, split, and pattern facts for a deliberately restricted regular-language fragment;
- boolean connectives over supported predicates;
- refined type bounds and validated newtype invariants;
- equality over values with deterministic equality semantics;
- option/result shape predicates such as `is_ok`, `is_err`, `is_some`, and `is_none`;
- list length facts and index bounds where the compiler can connect them to safe indexing;
- branch facts from `if`, `match`, and early returns.

The compiler must reject, defer, or mark unsupported predicate forms that require arbitrary I/O, mutation, async execution, nondeterministic functions, reflection, floating-point equality beyond a defined fragment, unconstrained nonlinear arithmetic, unbounded quantification, provider-specific syntax, or real-world claims that are not properties of the local program state. For example, a string predicate may establish that an `EmailAddress` has email-shaped syntax; it must not claim that the address is deliverable without an explicit runtime or external verification step.

### Provider model

Incan should support a provider model rather than embedding one solver identity into the language. A built-in range checker may prove simple bounds without any external tool. An SMT-backed provider may discharge richer arithmetic obligations. A future abstract interpreter may contribute runtime-error absence checks. A future theorem-prover bridge may validate specialized library contracts.

Provider output must normalize into the Incan verification outcome vocabulary. Provider-specific details may be preserved in metadata, but user-facing diagnostics and graph facts should remain stable across provider changes where possible.

### Performance model

Proof work primarily affects compile-time, check-time, editor, and CI latency. Parsing and typechecking contract expressions is ordinary compiler work, while provider-backed verification may add solver or analyzer cost. Implementations should therefore support deterministic budgets, incremental caching, and mode controls so ordinary development can choose between fast feedback and stricter proof.

Runtime impact depends on the verification outcome and mode. Code without authored contracts should not pay verification-specific runtime cost. A proved obligation may allow generated runtime validation to be skipped when that is safe and configured. An unproved obligation may add or preserve a runtime check in permissive mode, which can increase runtime cost on the checked path. Strict verification mode may instead fail the build rather than generate a runtime fallback. This RFC therefore must not claim that contracts are "compile-time only"; proof itself is compile-time, but runtime enforcement remains part of the safety story when proof is unavailable.

### Prior art lessons

[SPARK](https://www.adacore.com/languages/spark) demonstrates that contracts should be language constructs with execution semantics, compiler checking, proof support, and the ability to skip redundant runtime checks when proof establishes safety. Incan should borrow the contract-as-code and modular proof lessons, but it should not inherit Ada syntax, certification-heavy workflows, or the assumption that proof engineers are the primary repair loop.

[Dafny](https://dafny.org/) demonstrates a verification-aware language where specifications, preconditions, postconditions, termination conditions, loop invariants, and IDE feedback are central to development. Incan should borrow the idea that specifications are ordinary development artifacts, but it should keep a smaller application-language surface and integrate diagnostics with agent context rather than requiring users to become Dafny programmers.

[F*](https://fstar-lang.org/) demonstrates the power of dependent types plus SMT-backed and interactive proof, with extraction to practical targets. Incan should learn from its proof-oriented design and effect awareness, but this RFC explicitly avoids making dependent types or interactive proofs part of everyday Incan code.

[Why3](https://why3.org/) demonstrates a provider-oriented verification architecture where a verification language and generated conditions can be discharged by multiple automated and interactive provers. Incan should borrow the provider-neutral architecture lesson: verification facts and outcomes should be stable even if the proof engine changes.

[Frama-C ACSL](https://frama-c.com/html/acsl.html) demonstrates that function contracts can be precise, partial, human-readable, and analyzable, and that weakest-precondition style verification can be driven from source-level contracts. Incan should borrow the partial-contract and analyzable-contract lessons while avoiding comment-based specification syntax.

### Agentic repair loop

This RFC is primarily about feedback quality, not theorem-prover maximalism. A verification diagnostic should give an agent enough structured context to repair a failing implementation:

- the violated human-authored contract;
- the generated obligation;
- the implementation expression or control-flow edge being checked;
- the available assumptions;
- the counterexample when one exists;
- the runtime fallback status;
- relevant graph neighbors from RFC 106, such as type definitions, constructors, tests, and related diagnostics.

The graph should allow context packing to prioritize contract owners and failed obligations over broad file search. This is the "reshaped" part of formal methods for Incan: proof tools become a compiler-backed correction channel for implementation agents, while human authors remain responsible for deciding what the program should guarantee.

### Interaction with validated newtypes

Validated newtypes remain the ordinary way to name domain constraints. This RFC adds proof-aware reasoning around their construction and use. If an expression flowing into a validated newtype is proved to satisfy the newtype invariant, generated runtime validation may be skipped under the skipped-check rules. If the expression is not proved, construction must continue to validate at runtime or remain fallible according to the existing newtype contract.

### Interaction with Rust interop

Rust interop calls may participate in verification only through trusted summaries, checked metadata, or explicit contracts. The verifier must not infer arbitrary Rust behavior from generated code or Rust source scraping. If a Rust-backed function has no trusted summary, calls to it should be treated as opaque except for ordinary type information and declared effects.

### Interaction with effects and mutation

Contract expressions should be pure. Function bodies may be effectful, but obligations must be generated from a semantic model that records the relevant state facts. Contracts that depend on old values, mutated fields, or external state need explicit syntax and assumptions before they can be supported.

## Alternatives considered

### Target Ada/SPARK

Rejected as the primary strategy. Ada/SPARK is strong prior art for language-level contracts and industrial formal proof, but Incan's current product surface is Python-like authoring with Rust-native emission and Rust ecosystem access. A SPARK target would be a separate backend and ecosystem strategy, while this RFC is about making Incan contracts compiler-visible and agent-repairable.

### Use only runtime checks

Rejected. Runtime checks are necessary as a conservative fallback, but they do not give agents static repair feedback, they do not prove redundant checks safe to skip, and they cannot explain proof assumptions before code runs.

### Use only tests and generated examples

Rejected. Tests remain valuable, especially for examples and behavioral expectations, but they sample behavior. This RFC targets universal obligations within a bounded fragment, such as every value returned from a function satisfying a declared invariant.

### Add a full dependent type system

Rejected. Dependent types can express powerful specifications, but they would make everyday Incan authoring much heavier and would pull the language away from its readable application-code center.

### Require manual proof scripts

Rejected. Manual proof scripts are appropriate in some ecosystems, but they conflict with this RFC's agentic premise. Humans should write domain contracts; agents and tools should handle much of the implementation repair loop. Future advanced proof hooks may be possible, but they should not be required for the baseline.

### Make solver output the public API

Rejected. SMT-LIB, Z3 models, prover traces, or provider-specific proof objects are useful internal or advanced artifacts, but the Incan contract must be provider-neutral. User-facing diagnostics and RFC 106 graph facts should use stable Incan terms.

## Drawbacks

This feature adds a new mental model: users must distinguish typechecking, runtime validation, proof, unknown proof results, unsupported fragments, and runtime fallback. Poor diagnostics would make that distinction frustrating.

Verification can be slow or unpredictable if the proof fragment is too broad. The language must define conservative budgets, deterministic reporting, and a small first supported fragment rather than promising arbitrary proof power.

Permissive runtime fallback can make some checked paths slower when obligations are not proved. Conversely, skipped checks can make proved paths faster. The performance model therefore depends on verification mode, proof outcome, and where runtime enforcement is needed.

Contracts can be wrong. A proved implementation can still be wrong if the human-authored specification is incomplete or expresses the wrong property. The tooling must communicate that proof is relative to contracts and assumptions.

Skipping generated runtime checks creates safety risk if proof receipts are stale, assumptions are hidden, or provider bugs are over-trusted. Skipped-check decisions must therefore be auditable and conservative.

Agentic repair loops can overfit to diagnostics if the graph context is poor. RFC 106 integration reduces that risk, but the quality of context packing and counterexample translation will determine whether agents repair the right code or merely silence symptoms.

The implementation touches many compiler and tooling layers. A partial implementation could be useful, but an incoherent implementation that emits unverifiable claims would be worse than no proof-aware surface at all.

## Implementation architecture

This section is non-normative. The recommended architecture is to introduce a verification-facing semantic layer after typechecking and before backend-specific emission. That layer should preserve source-level contracts, typed expressions, branch assumptions, validated newtype invariants, loop structure, and effect summaries in a form suitable for generating obligations. Verification providers should consume this semantic layer or a provider-neutral obligation representation and return normalized outcomes. Emission should consume verification outcomes only through a conservative skipped-check and runtime-enforcement interface, not by embedding solver decisions ad hoc in generated Rust. RFC 106 graph export should read the same normalized contract, obligation, result, assumption, counterexample, and runtime-check records that diagnostics use.

## Layers affected

- **Parser / AST**: contract declarations such as `requires`, `ensures`, `invariant`, and `decreases` need syntax, source spans, and formatting-preserving representation when their final grammar is accepted.
- **Typechecker / Symbol resolution**: contract expressions must be resolved, typed, checked for purity and supported names, connected to refined type facts, and rejected when they refer to unavailable values, invalid old-value surfaces, or return bindings that are not actually returned on the checked path.
- **Semantic verification layer**: checked contracts, assumptions, branch facts, loop facts, type invariants, function summaries, and generated obligations must be represented independently of backend emission.
- **IR Lowering**: lowering must preserve enough source and semantic identity for runtime checks, skipped checks, generated artifacts, and graph records to link back to obligations.
- **Emission**: generated Rust must preserve runtime validation by default, emit runtime checks for runtime-enforced obligations where sound, and skip checks only when a proved result covers the same property under recorded assumptions.
- **Stdlib / Runtime (`incan_stdlib`)**: validation helpers, result/option predicates, assertion behavior, and domain constructors may need stable contract summaries that verification can trust.
- **Formatter**: contract statements should be formatted predictably and kept visually attached to the function or loop they constrain.
- **LSP / Tooling**: editors, CLI, and MCP consumers should show verification outcomes, counterexamples, assumptions, strict/permissive mode state, runtime fallback status, and graph-backed repair context.
- **Checked metadata and docs**: public contract declarations, supported predicates, proof outcomes where appropriate, and runtime-enforcement status should be exposed through checked metadata and generated documentation without exposing provider internals as the source of truth.
- **Agent context graph**: RFC 106 graph export and context packing should expose contract, obligation, result, counterexample, assumption, runtime-check, and verification-receipt records with provenance and source anchors.
- **Packaging / Workspaces**: packages may need to record verification configuration, provider requirements, cached receipt identities, and policy around strict verification or runtime fallback.

## Unresolved questions

- What exact grammar should Incan use for `requires`, `ensures`, `invariant`, `decreases`, old-value references, and explicit return bindings; should a special returned-value binding exist at all?
- What is the first supported proof fragment: bounded integer arithmetic only, linear arithmetic plus booleans, option/result shape predicates, list length/index facts, or a larger subset?
- Should permissive mode allow `unknown` obligations with runtime checks by default, or should unknown proof results always require an explicit project policy?
- What exact command surface should expose strict verification: an `incan verify` command, an `incan check --verify` mode, project policy, CI profile, or some combination?
- Which runtime checks are safe to skip in the first implementation, and which should remain even when an obligation is proved because the runtime check also documents an API boundary?
- How should trusted summaries for Rust interop, stdlib functions, and generated behavior be authored, reviewed, versioned, and exposed through metadata?
- Should proof receipts be cacheable across packages and machines, or should the first implementation limit receipts to local deterministic result caching?
- How much counterexample translation is required before this RFC can move to Planned status?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
