# RFC 111: Proof-aware agentic contract feedback

- **Status:** Planned
- **Created:** 2026-06-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 076 (project mutation policy)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 080 (declared AI assets)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 096 (declaration metadata blocks)
    - RFC 104 (receipts and reports)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 118 (`oven verify`)
    - RFC 122 (construction-safe callables and model checks)
    - RFC 124 (unit identity)
    - RFC 128 (callable types and `__signature__`)
- **Issue:** #787
- **RFC PR:** —
- **Written against:** v0.4
- **Shipped in:** —

## Summary

This RFC defines proof-aware contracts for Incan: a source-level contract surface for refined values, preconditions, postconditions, invariants, expectations, and verification outcomes that lets humans state domain intent while agents implement ordinary Incan code against compiler-checkable obligations. The compiler classifies each generated obligation as proved, violated, unknown, unsupported, or runtime-enforced, exposes counterexamples and assumptions where available, and publishes structured verification facts through RFC 106 so agentic tooling can repair implementations against human-authored specifications instead of relying on prose-only feedback.

## Core model

1. **Humans own intent:** users write domain contracts in source using types, refinements, preconditions, postconditions, invariants, and named semantic declarations.
2. **Agents own implementation labor:** generated or assisted code is expected to iterate against compiler feedback until it satisfies the declared contract or exposes a contract gap for human review.
3. **Contracts are opt-in, then binding:** ordinary Incan code remains valid without function contracts or loop invariants, but any contract the user writes becomes a real checked obligation rather than documentation.
4. **Contracts are part of the signature:** contracts are not comments, prompt text, or hidden generated-Rust assertions; they sit in a block on the return, are part of a callable's type, and are checked Incan declarations with source spans, metadata, and documentation presence.
5. **Verification is outcome-based:** the compiler reports whether each obligation is proved, violated, unknown, unsupported, or enforced at runtime instead of collapsing all proof failures into a generic type error.
6. **Runtime fallback is explicit:** unproved obligations may remain protected by generated runtime checks when that is sound and configured, but that fallback is visible in diagnostics, metadata, and graph facts.
7. **Proof checkers are providers:** SMT solvers, abstract interpreters, range analyzers, symbolic evaluators, or theorem-prover bridges are verification providers behind a stable Incan outcome vocabulary; no provider owns the language surface.
8. **The graph carries repair context:** RFC 106 is the delivery substrate for contract declarations, generated obligations, outcomes, counterexamples, runtime checks, and proof receipts so agents receive targeted repair context.
9. **Proof is bounded:** Incan proves a clearly defined subset and records assumptions; it must not claim full program correctness beyond the authored contracts and supported reasoning fragment.
10. **Trust is inspectable:** every proof result must record enough provenance for tools to distinguish compiler-local checks, solver-backed checks, runtime checks, cached results, unsupported fragments, and degraded work-in-progress output.
11. **Contract authority and proof scope are explicit:** source presence, a passing verification result, and a project approval are distinct facts. A proof establishes only the named formal obligation under recorded assumptions; it does not establish business correctness, operational suitability, or human approval.

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
- Preserve contract declaration identity, origin, review or approval status when a project supplies it, and the exact scope of every verification result without treating any one of those facts as another.
- Keep the Rust backend as the ordinary emission target while making proof facts backend-independent where possible.
- Record expectations and their outcomes, so a project can answer what a function expects, promises and did in production.
- Learn from Ada/SPARK-like systems, Dafny, F*, Why3, and ACSL while preserving Incan's Python-like authoring surface and agent-native tooling model.

## Non-Goals

- This RFC does not make Incan a dependent type language.
- This RFC does not require users to write interactive proof scripts.
- This RFC does not require every function, loop, module, or package to carry contracts by default.
- This RFC does not replace the Rust backend or make Ada/SPARK an Incan code generation target.
- This RFC does not tie the language surface to a particular solver or theorem prover, or require a storage engine, MCP server, or hosted verification service.
- This RFC does not claim that all Incan programs become formally verified.
- This RFC does not prove correctness of specifications themselves.
- This RFC does not infer that a source contract was human-approved, policy-approved, complete, suitable for a particular business purpose, or true about the external world merely because it parsed or was proved.
- This RFC does not define Rust-side contract syntax.
- This RFC does not remove runtime validation from validated newtypes, model constructors, or library contracts unless a matching proof result and configuration justify skipping the redundant generated check.
- This RFC does not define every possible contract predicate.
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

def local_part(email: EmailAddress) -> str {
    ensures:
        part: "@" not in part
}:
    return email.value.split("@")[0]
```

A contract sits in a `{ }` block on the return, so it is part of the function's signature rather than its body. `requires:` is what a caller must guarantee, and `ensures:` is what the function promises. Each `ensures:` line is a pattern matched against whatever the function returns, a colon, and a condition, so `part` above is just a name for the returned string, using the patterns Incan already has. The contract does not claim that the mailbox exists, that the domain accepts mail, or that a confirmation link was clicked. It claims a source-checkable string shape. A verifier may prove the postcondition when the string operations and pattern fragment are supported, keep the existing runtime validation when they are not, or report `unsupported` if the pattern is outside the supported regular-language fragment.

Function contracts make intent explicit when the result property is not just the target type. A withdrawal either returns an updated account or an insufficient-funds error. A pattern such as `Ok(updated)` says the promise applies only to successful returns:

```incan
type Cents = newtype int[ge=0]

model Account:
    balance: Cents

def withdraw(account: Account, amount: Cents) -> Result[Account, str] {
    ensures:
        Ok(updated):
            updated.balance <= account.balance
            updated.balance >= Cents(0)
}:
    if amount > account.balance:
        return Err("insufficient funds")

    return Ok(Account(balance=Cents(account.balance - amount)))
```

The `if amount > account.balance` guard establishes that the `Ok` path only runs when `amount <= account.balance`. A proof-aware checker can turn those source facts into a successful obligation:

```text
verification[E1111]: postcondition proved
  contract: withdraw ensures Ok(updated): updated.balance >= Cents(0)
  obligation: successful withdrawal keeps balance non-negative
  established by:
    branch fact: amount <= account.balance on the Ok path
    type invariant: account.balance >= Cents(0)
    type invariant: amount >= Cents(0)
  runtime_check: Cents(...) validation may be skipped for this path because the obligation was proved
```

The same diagnostic model becomes more useful when an agent writes an implementation that looks plausible but omits the guard. The signature and its contract stay as they were, and only the body changes:

```incan
}:
    return Ok(Account(balance=Cents(account.balance - amount)))
```

Here the contract still says that every successful withdrawal must leave a non-negative balance, but the implementation returns `Ok` for every input. The verifier can now produce a real counterexample:

```text
verification[E1112]: postcondition may be violated
  contract: withdraw ensures Ok(updated): updated.balance >= Cents(0)
  obligation: successful withdrawal keeps balance non-negative
  counterexample:
    account.balance = Cents(5)
    amount = Cents(7)
    account.balance - amount = -2
  note: the Ok path does not establish `amount <= account.balance`; establish that predicate before returning Ok, return Err when it is false, or add a requires clause if callers must prove it
```

This is the subtext of the feature: the compiler is not merely saying "type mismatch." It is explaining the gap between the human-authored contract and the agent-authored implementation, in terms precise enough for the agent to repair the missing branch. Diagnostic notes should be derived from predicates, control-flow shape, and source-authored names; domain wording such as "insufficient funds" may appear only when it comes from user-authored code, metadata, or an explicit contract label.

### A larger function

On a larger function the contract stays in the signature, clear of the docstring and the code:

```incan
def transfer(
    mut source: Account,
    mut target: Account,
    amount: Cents,
    limits: TransferLimits,
) -> Result[TransferReceipt, TransferError] {
    requires:
        amount > Cents(0)
        source.id != target.id
    ensures:
        Ok(receipt):
            receipt.amount == amount
            source.balance == old(source.balance) - amount - receipt.fee
            target.balance == old(target.balance) + amount
        Err(_):
            source.balance == old(source.balance)
            target.balance == old(target.balance)
}:
    """Move an amount between two accounts, within the daily limit."""
    if source.frozen or target.frozen:
        return Err(TransferError.Frozen)

    remaining = limits.daily - source.spent_on(limits.today)
    if amount > remaining:
        return Err(TransferError.OverLimit(remaining=remaining))

    fee = limits.fee_for(amount)
    total = amount + fee
    if total > source.balance:
        return Err(TransferError.InsufficientFunds(short_by=total - source.balance))

    source.balance -= total
    target.balance += amount

    return Ok(TransferReceipt(amount=amount, fee=fee, day=limits.today))
```

`old(...)` is a value at entry to the function. A line with no pattern is a plain condition, such as `self.count == old(self.count) + 1`.

### A contract is part of the type

A function's contract is part of its type automatically. Nobody writes a separate type, and asking the function shows it:

```incan
divide.__signature__()
# Callable[(a: int, b: int) -> int] {
#     requires:
#         b != 0
# }
```

The same block works on methods and closures, since a function is a named closure:

```incan
class Ledger:
    def post(mut self, entry: Entry) -> Result[None, LedgerError] {
        ensures:
            Ok(_):
                self.len() == old(self.len()) + 1
    }:
        ...

def build_divider() -> Callable[(a: int, b: int) -> int]:
    return (a: int, b: int) -> int {
        requires:
            b != 0
    } => a // b
```

The written-out form only appears where a callable crosses a signature. A plain `Callable` asks only about the shape, so any function of that shape fits it, and the contract is checked when the function starts, because the caller cannot see it:

```incan
def apply(op: Callable[(int, int) -> int], a: int, b: int) -> int:
    return op(a, b)

apply(divide, 10, 0)    # compiles, then divide stops with a contract error
```

A function that depends on what its callback promises writes the contract inline, and the check moves to compile time:

```incan
def apply(op: Callable[(a: int, b: int) -> int] { requires: b != 0 }, a: int, b: int) -> int {
    requires:
        b != 0
}:
    return op(a, b)     # proved: apply's own requires gives b != 0

apply(divide, 10, 2)    # fine
apply(add, 10, 2)       # fine: add asks for less
apply(divide, 10, 0)    # compile error: apply requires b != 0
apply(picky, 10, 2)     # flagged: picky requires b > 100, more than op allows
```

### Loops

Loops need invariants when a property cannot be inferred from local statements. A loop is not a declaration, so its invariants go at the start of its body:

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

### Expectations

`requires:` and `ensures:` must hold. `expects:` is softer: it states what should normally be true, and a failure is recorded while the function carries on:

```incan
def load_orders(rows: list[Order]) -> OrderBatch {
    requires:
        len(rows) > 0
    expects:
        all(row.amount >= 0 for row in rows)
}:
    ...
```

A failed expectation leaves a record under RFC 104 and an event on the current span. Together with the outcomes of `requires:` and `ensures:`, this answers the questions governance asks: what does this function expect, what does it promise, was that proved, did it ever fail in production, and who approved it.

### Checking a project

An ordinary build keeps working when something cannot be proved. It checks the contract at runtime and says so, and a contract that is provably broken is always a compile error. `oven verify` is the strict check, for CI:

```text
oven verify
```

It fails on anything not proved, unless policy allows it. The toolchain pins a solver, Z3 by default, so a contract gets the same outcome on every machine. A project can choose another in `loaf.toml`:

```toml
[incan.verification]
solver = "cvc5"
```

Through RFC 106, both the proved obligation and the failed obligation become graph context. An agent can ask for the task context for a verification result and receive the contract declaration, the function body, the type invariants, nearby tests, related diagnostics, and the graph edges explaining which branch fact proved the property or which expression produced the violating value.

## Reference-level explanation

### Contract declarations

A contract declaration is a source-authored statement or type constraint that defines a checkable property. Contract declarations must have source spans and must be represented in checked metadata when they are part of a public or exported surface.

This RFC defines these contract families:

- refined primitive predicates such as `int[ge=0, le=100]`;
- validated newtype invariants inherited from constrained underlyings;
- function preconditions in `requires:`;
- function postconditions in `ensures:`;
- recursion termination hints in `decreases:`;
- recorded expectations in `expects:`;
- loop invariants written as `invariant`;
- loop termination hints written as `decreases`;
- model invariants, which are a model's RFC 122 registered checks and need no further syntax.

### The contract block

1. A `def`, `async def` or closure may carry a block after its return type, or after its parameters when it has none, and before the `:` or `=>` that starts its body. A declaration with no body, such as an abstract trait method, may carry one too.
2. A block must contain the sections `requires:`, `ensures:`, `decreases:` and `expects:`, each at most once and in that order. `requires`, `ensures`, `invariant`, `decreases` and `expects` must be contextual, and stay usable as ordinary names elsewhere.
3. Each `requires:`, `decreases:` or `expects:` line must be a condition. A section with one condition may sit on one line, as in `{ requires: b != 0 }`.
4. Each `ensures:` line must be a condition, or `<pattern>: <condition>`, or a pattern followed by an indented list of conditions. A pattern must be an ordinary match pattern, matched against the returned value, and may have a guard.
5. Every arm whose pattern matches must apply, unlike `match`, and arms need not cover every case. There is no special result name, because a binding pattern names the result.
6. `a and b` at the top of a condition must be checked as two separate obligations, so a counterexample names the part that failed.
7. A condition must be construction-safe under RFC 122, inferred through what it calls. It may refer to parameters, `self`, type parameters and visible immutable facts, and must not refer to the body's local variables. `old(expr)` must be accepted only in `ensures:`, and means the value of `expr` at entry.
8. The `requires:` of a `pub` function must mention only what its callers can see, because otherwise they could not satisfy it.

An `assert` statement is not a contract declaration under this RFC. It retains RFC 018's always-on runtime assertion semantics: it checks an implementation fact at a specific program point, may produce a local verification obligation, and may become a fact available to later obligations only after that assertion is itself checked, proved, or runtime-protected. An implementation-local `assert` must not be exported as a caller-visible precondition or postcondition unless the source also declares `requires:` or `ensures:`.

### Contracts and types

9. A callable's contract must be part of its type. RFC 128's `__signature__()`, generated documentation, editor hover and `incan inspect` must report it, in the notation `Callable[(a: int, b: int) -> int] { ... }`. Parameter names used by a contract must appear in that notation.
10. The type checker must consider only a callable's shape. Contracts are the verifier's concern, and overload resolution must ignore them.
11. A callable fits a plain `Callable` of its shape, and its contract must then be checked at runtime when it starts, reported as `runtime_enforced`.
12. A callable fits a type with a contract when it asks for no more and promises no less. Whether it fits is an ordinary obligation, with the ordinary outcomes.
13. A method adopting a trait method must inherit its contract, and may ask for less and promise more under the same rule.
14. Changing a public function's contract is an API change, and must be distinguishable from a change to its body in checked metadata and tooling.

### Expectations

15. An `expects:` line must not stop execution by default. A failure must be recorded through RFC 104's report and as an event on the current span, naming the expectation and its source. `[incan.verification] expects = "stop"` may make a failure stop the task instead.
16. An expectation that is proved must not be checked at runtime. A proved or failed expectation is never a promise to a caller, and `oven verify` must report failed proofs of expectations as warnings.
17. An expectation is outside the proof fragment when the verifier cannot reason about it, and must still be checked at runtime when it can be evaluated. One that cannot be evaluated must be refused.

### Contract provenance and approval boundary

Every contract declaration must have a stable declaration identity, source anchor, and source revision or digest suitable for its checked metadata and RFC 106 graph facts. The compiler may record a declaration origin such as source-authored, generated, imported through a checked interface, or agent-proposed when that fact is available. It must not infer a human author, reviewer, approver, or business owner from source presence, a commit identity, a passing check, or a provider result.

A project may attach an explicit review or approval record through a trusted project-policy or checked-metadata boundary. That record must identify the exact contract declaration or immutable contract set, the applicable policy, its scope, and its revision. Approval metadata is not part of logical proof and must not change whether an obligation is `proved`, `violated`, `unknown`, `unsupported`, or `runtime_enforced`. Conversely, a proof result must not upgrade an unreviewed contract to approved. Agent-generated contract text must remain distinguishable from an explicit approved declaration until the project records a separate approval.

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

### Scope of proof

Every verification result must identify the exact contract declaration or obligation, checked source and semantic revision, verification provider and configuration, material assumptions, supported-fragment version, and runtime-enforcement or skipped-check status. A result classified as `proved` means only that the named obligation was established under that recorded scope. It does not establish that the contract captures the intended business rule, that external systems behave as modeled, that a user approved the contract, that a model-generated implementation is safe for every deployment, or that a required effect was authorized. Tooling and generated documentation must preserve this distinction in their vocabulary.

### Build and verification modes

Function contracts and loop invariants are opt-in unless a package, workspace, CI profile, or policy requires them for a selected scope.

- **An ordinary build** must continue when an obligation is `unknown` or `unsupported` and a sound runtime check protects the property, with a visible note. An obligation that cannot be checked at runtime either must be an error.
- **A `violated` obligation** must be a compile error in every mode, unless it is intentionally guarded by a reachable error path the contract accepts. A validated constructor may remain a fallible runtime path if the surrounding API contract explicitly returns `Result`.
- **`oven verify`** must be the strict check under RFC 118. It must exit non-zero for `violated`, `unknown`, `unsupported`, and runtime-only obligations, unless `[incan.verification]` or RFC 076 policy allows them. It must support `--format json`.

### Runtime enforcement and skipped checks

Runtime checks must remain the conservative fallback. A runtime check may be skipped only when a corresponding obligation is `proved` and the proof result covers the same property, target value, and assumptions as the runtime check. This skipped-check decision must be visible in verification metadata or graph facts.

The `requires:` check of a `pub` function must not be skipped, because a caller in another Loaf or behind a plain `Callable` cannot be proved. A proved call to a function that is not exported may skip it.

If a runtime check cannot be generated for a contract expression, an unproved obligation must not be treated as runtime-enforced. Such obligations must be `unknown` or `unsupported` and must fail under `oven verify`.

### Counterexamples and assumptions

When a verifier reports a counterexample, the compiler must translate it into source-level terms: Incan identifiers, type names, model fields, enum variants, and literal values. A result that cannot be translated must be reported as `unknown`, never as raw provider output.

Every proof or counterexample must record assumptions that materially affected the result. Assumptions may include function preconditions, type invariants, branch conditions, loop invariants, contracts on declarations with native implementations, provider configuration, arithmetic semantics, overflow policy, and runtime-library contracts.

### Proof results as facts

A proof result is a fact about a unit, keyed by the unit's identity under RFC 124 together with the solver and its version. It must be stored and published with the Loaf like other facts, so a consumer does not prove it again.

### Review and approval

Review and approval stay outside the compiler. A changed contract is a signature change that RFC 076 policy can gate, and the approval is recorded in the receipt. A contract's identity must be derived from its exact content, so an approval covers only the text it approved.

### RFC 106 graph facts

The agent context graph must support these node kinds:

- `contract_decl`: a source-authored contract such as a refined predicate, `requires`, `ensures`, `invariant`, or `decreases`.
- `verification_obligation`: a compiler-generated obligation tied to a contract and source site.
- `verification_result`: the outcome for an obligation.
- `counterexample`: a model or witness explaining a violation where available.
- `runtime_check`: a generated or preserved runtime enforcement site.
- `verification_receipt`: a cacheable proof or verification-result identity that records provider, configuration, assumptions, source identity, and checked fragment.
- `verification_assumption`: a fact used by one or more verification results.
- `contract_provenance`: an optional project-supplied origin, review, or approval record attached to an exact contract identity without changing proof semantics.

The graph must support these edge kinds:

- `generates_obligation`: from a contract declaration or checked type to an obligation.
- `checks_site`: from an obligation to the expression, statement, call, return, loop, or construction site it checks.
- `has_result`: from an obligation to a verification result.
- `has_counterexample`: from a failed result to a counterexample.
- `enforced_by`: from an unproved obligation or contract to a runtime check.
- `elides_check`: from a proved result to a runtime check that was removed or skipped.
- `assumes`: from an obligation or result to a verification assumption.
- `repairs_context_for`: from a verification result to task-ranked context packs or advisory records that explain likely repair targets.
- `has_provenance`: from a contract declaration to an optional contract-provenance record.

These records must preserve RFC 106 provenance. A solver-backed result must not be marked as `compiler_checked` unless the compiler owns the validation of that result under the graph schema. Provider-specific facts should identify the provider while preserving a stable Incan outcome vocabulary.

## Design details

### Contract syntax

This RFC uses readable keyword contracts instead of importing Ada/SPARK, Dafny, F*, WhyML, or ACSL syntax, and the block follows RFC 096's metadata blocks: it attaches to the declaration right before it.

The block is in the signature rather than the body because a contract is a promise to callers. Testing the alternatives on larger functions showed why:

- **Inside the body, next to a return:** the promise is buried in the code, only sits beside one return, and an agent editing the body is standing on the human's contract.
- **After the docstring, or above it:** better, but only position separates the contract from the code, so a rule has to enforce it, and a trait method with no body has nowhere natural to put it. Above the docstring also breaks the "first string" convention.
- **Sections instead of repeated keywords:** a keyword on each line repeats itself, and indented arms under a pattern avoid both the repetition and long lines.

`assert` is deliberately separate from `ensures:`. An `assert` statement checks a fact at the point where it appears in the implementation; an `ensures:` clause declares a postcondition of the function that callers, documentation, verification metadata, and RFC 106 graph context can rely on. A verifier may use a proved `assert` as local evidence for later obligations, but `assert` should not be used as a synonym for "this function promises this after return."

### Supported predicate fragment

The proof-aware fragment is small enough to be useful and predictable, and its list is versioned. It must include:

- integer comparisons and bounded integer arithmetic;
- string length, prefix, suffix, containment, split, and pattern facts for a deliberately restricted regular-language fragment;
- boolean connectives over supported predicates;
- refined type bounds and validated newtype invariants;
- equality over values with deterministic equality semantics;
- option/result shape predicates through `ensures:` patterns and `is_ok`, `is_err`, `is_some`, and `is_none`;
- list length facts and index bounds where the compiler can connect them to safe indexing;
- branch facts from `if`, `match`, and early returns.

Anything outside the list must report `unsupported`, so the list can grow without breaking anyone. The compiler must reject or mark unsupported predicate forms that require arbitrary I/O, mutation, async execution, nondeterministic functions, reflection, floating-point equality beyond a defined fragment, unconstrained nonlinear arithmetic, unbounded quantification, provider-specific syntax, or real-world claims that are not properties of the local program state. For example, a string predicate may establish that an `EmailAddress` has email-shaped syntax; it must not claim that the address is deliverable without an explicit runtime or external verification step.

### Provider model

The toolchain must pin a solver, as it pins `rustc`, so a contract gets the same outcome on every machine. Z3 is the default. `[incan.verification] solver` in `loaf.toml` may select another provider, and a built-in checker may settle simple bounds without calling a solver at all. A verification budget must be counted in solver steps rather than seconds, so a result does not depend on how fast a machine is.

An SMT solver works by being asked the opposite of what is wanted: "is there any input where this promise fails?" Values mean `violated`, with those values as the counterexample. No such values mean `proved`. Giving up within the budget means `unknown`. Providers exchange questions in a standard format, which is what makes them replaceable.

Provider output must normalize into the Incan verification outcome vocabulary. Provider-specific details may be preserved in metadata, but user-facing diagnostics and graph facts should remain stable across provider changes where possible. Every result must record the provider and its version.

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

Contracts are an Incan feature. There is no Rust-side contract syntax, and a call into Rust is opaque apart from ordinary type information and declared effects, so an obligation depending on one reports `unknown`. A standard-library function is an Incan declaration and may carry a contract. If its implementation is native, that contract must be recorded as an assumption in every result that relies on it.

### Interaction with effects and mutation

Contract conditions are pure. Function bodies may be effectful, but obligations must be generated from a semantic model that records the relevant state facts. `old(...)` is the surface for a value at entry, and a contract cannot depend on external state.

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

Rejected. Manual proof scripts are appropriate in some ecosystems, but they conflict with this RFC's agentic premise. Humans should write domain contracts; agents and tools should handle much of the implementation repair loop. Advanced proof hooks are not part of this design.

### Make solver output the public API

Rejected. SMT-LIB, Z3 models, prover traces, or provider-specific proof objects are useful internal or advanced artifacts, but the Incan contract must be provider-neutral. User-facing diagnostics and RFC 106 graph facts should use stable Incan terms.

### Put contracts in the body, or use decorators

Rejected. "Contract syntax" compares the body positions. A decorator such as `@ensures(...)` takes an ordinary value, and a condition is not a value, so decorators would need special handling.

### Make the contract block ordinary type identity

Rejected. If a contract changed what fits a plain `Callable`, a function with a contract could not be passed to `map`, `.then(...)` or a decorator, and every library would have to spell out contracts in its callback types.

### Ship no solver, or make one optional

Rejected. A built-in checker alone leaves arithmetic relations `unknown`, and an optional solver makes the same contract `proved` on one setup and `unknown` on another. The measured cost of pinning one is small next to the toolchain.

## Drawbacks

This feature adds a new mental model: users must distinguish typechecking, runtime validation, proof, unknown proof results, unsupported fragments, and runtime fallback. Poor diagnostics would make that distinction frustrating.

Verification can be slow or unpredictable if the proof fragment is too broad. The language must define conservative budgets, deterministic reporting, and a small supported fragment rather than promising arbitrary proof power.

Permissive runtime fallback can make some checked paths slower when obligations are not proved. Conversely, skipped checks can make proved paths faster. The performance model therefore depends on verification mode, proof outcome, and where runtime enforcement is needed.

Contracts can be wrong. A proved implementation can still be wrong if the human-authored specification is incomplete or expresses the wrong property. The tooling must communicate that proof is relative to contracts and assumptions.

Skipping generated runtime checks creates safety risk if proof receipts are stale, assumptions are hidden, or provider bugs are over-trusted. Skipped-check decisions must therefore be auditable and conservative.

Agentic repair loops can overfit to diagnostics if the graph context is poor. RFC 106 integration reduces that risk, but the quality of context packing and counterexample translation will determine whether agents repair the right code or merely silence symptoms.

The implementation touches many compiler and tooling layers. A partial implementation could be useful, but an incoherent implementation that emits unverifiable claims would be worse than no proof-aware surface at all.

`}:` is new punctuation, and a short function's contract takes several lines. A vocabulary could shorten this, and the foundation would not change.

The toolchain carries a pinned native solver, which it has to track and patch. SMT results can also be sensitive: a small, unrelated change can turn `proved` into `unknown`. The small supported list, the pinned version and a budget in solver steps limit that, but do not remove it.

An `expects:` line runs at runtime unless it is proved, so an expensive expectation costs what it costs.

## Implementation architecture

This section is non-normative. The recommended architecture is to introduce a verification-facing semantic layer after typechecking and before backend-specific emission. That layer should preserve source-level contracts, typed expressions, branch assumptions, validated newtype invariants, loop structure, and effect summaries in a form suitable for generating obligations. Verification providers should consume this semantic layer or a provider-neutral obligation representation and return normalized outcomes. Emission should consume verification outcomes only through a conservative skipped-check and runtime-enforcement interface, not by embedding solver decisions ad hoc in generated Rust. RFC 106 graph export should read the same normalized contract, obligation, result, assumption, counterexample, and runtime-check records that diagnostics use.

## Layers affected

- **Parser / AST**: contract blocks on functions, methods, closures and `Callable` types, and loop `invariant` and `decreases`, with source spans and formatting-preserving representation.
- **Typechecker / Symbol resolution**: contract expressions must be resolved, typed, checked for purity and supported names, connected to refined type facts, and rejected when they refer to unavailable values, invalid uses of `old(...)`, or local variables of the body.
- **Semantic verification layer**: checked contracts, assumptions, branch facts, loop facts, type invariants, function summaries, and generated obligations must be represented independently of backend emission.
- **IR Lowering**: lowering must preserve enough source and semantic identity for runtime checks, skipped checks, generated artifacts, and graph records to link back to obligations.
- **Emission**: generated Rust must preserve runtime validation by default, emit runtime checks for runtime-enforced obligations where sound, and skip checks only when a proved result covers the same property under recorded assumptions.
- **Stdlib / Runtime (`incan_stdlib`)**: validation helpers, result/option predicates, assertion behavior, and domain constructors may need stable contract summaries that verification can trust.
- **Formatter**: contract blocks and loop invariants should be formatted predictably and kept visually attached to what they constrain.
- **LSP / Tooling**: editors, CLI, and MCP consumers should show verification outcomes, counterexamples, assumptions, strict/permissive mode state, runtime fallback status, and graph-backed repair context.
- **Checked metadata and docs**: public contract declarations, declaration identity and optional explicit approval provenance, supported predicates, proof outcomes where appropriate, proof scope, and runtime-enforcement status should be exposed through checked metadata and generated documentation without exposing provider internals as the source of truth or turning proof into a business-correctness claim.
- **Agent context graph**: RFC 106 graph export and context packing should expose contract, obligation, result, counterexample, assumption, runtime-check, and verification-receipt records with provenance and source anchors.
- **Packaging / Workspaces**: `[incan.verification]` in `loaf.toml`, the pinned solver in the toolchain, and proof results published as Loaf facts.
- **CLI**: `oven verify`.

## Design Decisions

- **Contracts are part of the signature.** A `{ }` block on the return, with `requires:`, `ensures:`, `decreases:` and `expects:` sections, on functions, methods and closures. Loop `invariant` and `decreases` stay at the start of the loop body, because a loop is not a declaration.
- **`ensures:` uses patterns.** `Pattern: condition`, or a pattern with indented conditions, using ordinary match patterns. All matching arms apply, and no special result name exists.
- **A contract is part of the callable's type.** It is read by asking the callable, and nobody writes a separate type. A plain `Callable` accepts any function of the right shape, with the contract checked at runtime on entry, and a function that depends on a callback's promises writes the contract inline. The type checker considers only shape, so Incan does not become a dependent-type language.
- **One fit rule.** Asking for no more and promising no less, for callbacks and for trait methods alike.
- **What can be proved.** The versioned list in "Supported predicate fragment". Anything outside it reports `unsupported`.
- **A contract that cannot be proved.** The build continues and checks it at runtime with a visible note. It is an error if it cannot be checked at runtime either. A violated contract is always a compile error.
- **The strict check is `oven verify`.** Builds stay usable, and CI runs the strict check, the same split as `oven audit`.
- **Skipped checks.** Only when the same property is proved, and never the `requires:` of a `pub` function.
- **Contracts are an Incan feature.** No Rust macro, and a call into Rust is opaque. A contract on a declaration with a native implementation is a recorded assumption.
- **Proof results are facts.** They are stored and published with the Loaf, keyed by unit identity and solver.
- **Counterexamples use source terms.** A result that cannot be translated is `unknown`.
- **Approval is outside the compiler.** A proof never counts as approval, and a changed contract is a signature change policy can gate.
- **The toolchain pins Z3,** and `loaf.toml` can select another solver. Z3's release archive is roughly 10 to 15% of a toolchain archive as measured on 2026-11-05 (51 to 66 MB against 440 to 571 MB, compressed).
- **`expects:` records rather than stops,** and a project can tighten that. Every contract line is also a stated, versioned expectation with a recorded outcome, which is what governance needs.
- **Model invariants are RFC 122's registered checks,** with no new syntax.
