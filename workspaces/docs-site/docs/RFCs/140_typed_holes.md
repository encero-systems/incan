# RFC 140: Typed holes

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 048 (checked contract metadata and interrogation tooling)
    - RFC 070 (Result combinators)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 111 (proof-aware contract feedback)
    - RFC 120 (canonical source identity)
    - RFC 128 (callable types and invocation)
    - RFC 131 (first-class error declarations)
    - RFC 134 (pattern bindings with exiting fallbacks)
    - RFC 138 (chained pattern conditions)
    - RFC 139 (sequence patterns)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce `???` as an explicit missing expression that the compiler can parse and analyze without pretending the program is complete. Checking reports the hole's required type or remaining constraints, their origins, available local bindings, and checked in-scope callable candidates where compatible, while continuing to report independent errors in completed code. Holes remain distinct from valid values, bottom types, and runtime panic stubs. Every unresolved hole in the selected compilation graph blocks executable or distributable artifact production, including holes in unreachable code. The initial contract provides grounded implementation feedback, not automatic program synthesis.

## Motivation

Developers often know a function's structure before its implementation details. Ordinary temporary stubs distort that structure: a placeholder value can satisfy the wrong type, a panic body can appear to satisfy every return type, and malformed syntax can stop useful checking before surrounding obligations are visible.

An explicit missing expression lets a developer ask the compiler a narrower question: what must this part produce, and which values are available here? The same feedback gives coding tools a bounded implementation target without replacing human-authored intent with guessed code.

```incan
def load_port(source: Result[str, ReadError]) -> Result[int, ConfigError]:
    text = source.map_err(???)?
    return ???
```

The first hole must convert a `ReadError` into the error type required by propagation. The second must produce the function's declared Result. Neither is an instruction to panic or return a default port.

## Goals

- Parse explicit expression holes as intentional incomplete source.
- Report required types, unresolved constraints, and the reasons those requirements arise.
- Expose in-scope values with their types and relevant known ownership facts.
- Include deterministic checked in-scope callable candidates for callable holes, labelled separately from required facts and without selecting an implementation.
- Continue checking independent completed code without manufacturing successful checks.
- Provide equivalent human-readable and structured hole information.
- Prevent incomplete programs from becoming executable or distributable artifacts.
- Compose with generics, callbacks, error conversion, patterns, and optional proof-aware contracts.

## Non-Goals

- Synthesizing or automatically applying an implementation.
- Claiming a type-correct replacement satisfies the intended domain behavior.
- Introducing runtime placeholders, default values, or panic stubs.
- Allowing holes in declarations, names, type annotations, or patterns.
- Treating separate holes as the same missing expression.
- Evaluating user expressions or invoking external suggestion services to diagnose a hole.
- Producing an executable under an option that ignores unresolved holes.

## Guide-level explanation

The examples describe proposed behavior. Diagnostics are illustrative, not current compiler output. Application error types and helpers have their ordinary surrounding declarations; examples explicitly declaring `error` also use proposed RFC 131 syntax.

### Start with a function's return contract

```incan
def parse_port(text: str) -> Result[int, str]:
    return ???
```

Checking could show:

```text
Unresolved expression hole
  Required type: Result[int, str]
  Reason: declared return type of parse_port
  Available local values:
    text: str
```

This states the required interface. It does not decide which strings are accepted, which port range is valid, or what an error message should say.

### Ask for a callback rather than its result

```incan
def load_port(source: Result[str, ReadError]) -> Result[int, ConfigError]:
    text = source.map_err(???)?
    return ???
```

The two holes have different obligations:

```text
Hole 1
  Required callable: (ReadError) -> ConfigError
  Reasons:
    map_err receives the incoming Result's error
    ? must propagate an error compatible with ConfigError

Hole 2
  Required type: Result[int, ConfigError]
  Reason: declared return type of load_port
  Available local values:
    text: str
```

The callable spelling is diagnostic notation corresponding to RFC 128's callable surface. If existing type rules permit more than one compatible output, the diagnostic must report the compatible-type constraint rather than claim that exactly one callable type is forced.

Replacing the first hole with a closure lets checking describe the closure body more directly:

```incan
def load_port(source: Result[str, ReadError]) -> Result[int, ConfigError]:
    text = source.map_err((original) => ???)?
    return ???
```

Inside that closure, `original` is a `ReadError`, and the body must produce an error compatible with `ConfigError`. The compiler must not silently choose a conversion or attach diagnostic causes on the author's behalf; error construction and provenance remain governed by RFC 131 and the conversion actually written.

### Preserve useful shape without implementing the branch

```incan
def port_or_default(result: Result[int, str], fallback: int) -> int:
    match result:
        Ok(port) => return port
        Err(detail) => return ???
```

The hole requires an `int`. Its local context includes `detail: str` and `fallback: int`. Although `fallback` is type-compatible, choosing it would establish a recovery policy the compiler cannot infer from types alone. Suggestions, if supplied by tooling, must remain distinguishable from required facts.

### Show checked callable candidates

For a callable-typed hole, the report includes type-compatible named functions already in scope. These application functions use RFC 128's proposed callable syntax:

```incan
def plain(value: int) -> str:
    return f"{value}"

def tagged(value: int) -> str:
    return f"item-{value}"

def formatter() -> Callable[(int) -> str]:
    return ???
```

An illustrative report reads:

```text
Required: Callable[(int) -> str]

Type-compatible functions in scope:
  plain
  tagged
```

The candidates appear in deterministic order and come from checked callable contracts, not speculative execution or a search of unrelated libraries. Both functions fit the required type, but implement different behavior. The report must identify them as candidates rather than recommended implementations; nothing is selected or inserted automatically.

### Keep a typed error declaration while leaving payload work open

```incan
error ImportError:
    InvalidRow:
        row: int
        detail: str
        message = f"Invalid row {row}: {detail}"

def reject_row(row: int) -> Result[int, ImportError]:
    return Err(ImportError.InvalidRow(row=row, detail=???))
```

The hole requires `str` because it supplies the declared `detail` field. That field declaration is the most direct explanation; the enclosing Result type is not a substitute for it.

### Check code after a hole

```incan
def describe_port(text: str) -> str:
    port: int = ???
    return port.missing_method()
```

The explicit annotation fixes `port`'s type to `int`. The compiler can report the missing method independently of the hole's eventual value. It must not stop all checking at the first hole, and it must not treat the missing initializer as a diverging expression that makes the return unreachable.

### Be honest when the type is not determined

```incan
def inspect() -> None:
    candidate = ???
```

No use constrains `candidate` to one particular type. The diagnostic should say that the required type is undetermined. It must not pick `None`, `Any`, a default numeric type, or a bottom type merely to make checking finish.

An annotation can establish a useful obligation:

```incan
def inspect() -> None:
    candidate: Option[int] = ???
```

Now the required type is known, but the required implementation is still absent.

### Preserve an explicit exit around a missing value

```incan
def require_port(candidate: Option[int]) -> Result[int, str]:
    let Some(port) = candidate
    else:
        return Err(???)

    return Ok(port)
```

The fallback explicitly returns; its control-flow shape is established even though its error string is missing. The hole requires `str`. The compiler can therefore check the subsequent `port` binding under RFC 134's successful path while continuing to report the unresolved hole.

If the fallback instead contains only a bare `???`, the compiler cannot assume that the missing expression will return, break, or continue. The fallback's required exit remains incomplete until the author supplies it. A type requirement and an exit requirement are separate obligations.

### Let pattern scope shape the hole's context

```incan
def parse_assignment(parts: list[str]) -> Result[tuple[str, str], str]:
    let [name, "=", value] = parts
    else:
        return Err(???)

    return Ok((name, value))
```

This combines RFC 134 and RFC 139. The hole requires `str`, from the enclosing Result's error type. Its available local context includes `parts`; it must not include `name` or `value`, because the fallback runs when that pattern did not succeed. A useful hole report explains those scope boundaries instead of suggesting a replacement that only works on the successful path.

### Separate checking from artifact production

Checking an incomplete program reports holes and independent errors, with a nonzero exit status. Editors and structured consumers can still use the reported types and constraints. Building, running, testing through compilation, or publishing an artifact from that selected incomplete source must fail before executable output is produced.

A hole is still unresolved inside a branch that appears unreachable. Compiling an independent target whose selected source graph contains no holes remains possible; the compiler need not reject every other target in a workspace merely because an unselected file contains incomplete work.

## Reference-level explanation

### Syntax and identity

The contiguous source spelling `???` must denote a hole where a primary expression is required. It must be distinct from the postfix `?` propagation operator. In postfix position after an existing expression, consecutive question marks must retain their existing repeated-propagation interpretation. Recognizing a hole must not globally replace every three question marks with a hole token. Strings and comments containing those characters must retain ordinary behavior.

Consequently, `return ???` contains a hole, while `return nested???` retains three postfix propagation operations on `nested`, subject to ordinary Result typing. An author can write `(???)?` to apply propagation to a missing Result expression with an explicit visual boundary. The formatter must preserve these distinctions.

Each source occurrence must represent an independent missing expression, with its own source span and enclosing declaration context. Repeated occurrences must not implicitly share an implementation or impose equal types on each other. This RFC does not introduce named holes or a public identity stable across arbitrary edits.

A hole must be accepted wherever an expression is syntactically permitted, including callback bodies, arguments, return expressions, initializers, and declared defaults. It must not stand for a declaration, statement suite, pattern, identifier, or type annotation. Other syntactic requirements of the enclosing construct must still apply.

### Type constraints

A hole must participate in contextual type analysis as an unknown expression subject to the surrounding type rules. Expected types, argument types, field types, callback signatures, return contracts, and subsequent typed uses must constrain it according to ordinary compatibility rules.

When the surrounding code uniquely establishes a required type, the diagnostic must report it. When a generic relationship or compatibility constraint permits multiple types, the diagnostic must report that relationship and any known bounds rather than invent a unique type. When no useful type can be established, it must report that the required type is undetermined.

A hole must not be assigned `Any`, a valid default value, an error-recovery wildcard, or a diverging bottom type solely to suppress other diagnostics. Constraints that are inconsistent independently of the missing implementation must still produce ordinary type errors. An overload, generic instantiation, or trait selection depending on unresolved hole information must remain unresolved rather than being selected speculatively.

### Diagnostic content

Each hole diagnostic must include its source location, enclosing declaration when available, known required type or constraints, and the source reasons for those requirements. Type constraints must preserve declaration identities and relevant generic parameters rather than presenting only ambiguous short names.

Available local values must be presented with their known types and applicable known ownership or mutation restrictions. Out-of-scope, inaccessible, or already moved values must not be described as immediately usable. A name available before the hole does not establish that using it inside the unknown implementation would preserve all later uses.

For a callable-typed hole, the required report must also include deterministically ordered named callable candidates already in scope whose checked signatures satisfy the known callable requirement under ordinary type and ownership rules. A candidate must not be claimed compatible by guessing an unresolved overload or generic instantiation. Candidate search must not import or scan unrelated libraries, execute user code, invent call expressions, or select an implementation. Each candidate must remain identifiable by its declaration and must be labelled separately from compiler-established requirements.

Human-readable output may abbreviate large environments, provided it indicates that it has done so and tooling can request the relevant complete context within ordinary visibility and access limits. Diagnostic ordering should be deterministic for an unchanged source selection. A numeric label such as "Hole 1" is presentation within one report, not a persistent source identity.

### Partial checking and evidence

Checking must continue through completed source when the known constraints permit it. The compiler must distinguish independently established facts and errors from checks that depend on an unresolved hole. It must not advertise a declaration, package, or program containing holes as successfully checked merely because its surrounding interfaces are consistent.

The missing expression's runtime behavior, side effects, termination, borrowing, and resource use must not be assumed safe or pure. Ownership, reachability, effects, constant evaluation, validation, and proof obligations depending on that behavior must remain explicitly incomplete. Independently provable errors must still be reported without assuming that the hole will exit, repair earlier ownership violations, or supply arbitrary coercions.

A constant or default containing a hole must remain unevaluated where its value is unknown. User code, construction hooks, validators, or external services must not be executed to invent its missing value. Existing compile-time checks that can be completed without that value remain applicable.

Proof-aware integration under RFC 111 must not turn an assumed hole contract into a proof that the missing implementation satisfies it. Hole-dependent obligations must be identified as incomplete under the hole status, with ordinary verification outcomes retained for obligations independently established from completed source.

### Checking and artifact boundary

The normal checking path must recognize holes as incomplete source, report their constraints alongside independent diagnostics, and return a nonzero status when holes remain. Clients must be able to distinguish intentional holes from malformed syntax and ordinary completed-code errors.

Every unresolved hole in a selected compilation graph must prevent producing a runnable, linkable, or distributable artifact from that graph. This includes holes in uncalled functions, uninstantiated generic bodies, unreachable branches, defaults, and constants included in that selection. An ignored diagnostic setting must not bypass this requirement.

The compiler may produce explicitly incomplete editor state or internal analysis projections. Those outputs must be marked incomplete and must not be accepted as checked package metadata, complete API contracts, proof receipts, or reusable executable artifacts. Existing artifacts from a different source revision must not be reported as successful output for the incomplete current revision.

An independently selected compilation graph without holes must remain eligible for ordinary checking and artifact production. This rule does not make a hole an execution-time trap and does not require lowering it into executable IR or emitting a runtime replacement.

### Structured tooling

Structured diagnostics must preserve the same hole identity within the analyzed source revision, source spans, type constraints, requirement origins, available bindings, checked callable candidates, and incomplete status as the human-facing report. They must also distinguish unresolved dependent checks from ordinary type errors.

Responses must identify the source revision or equivalent analysis snapshot sufficiently to prevent applying stale hole information to edited source. RFC 106 integrations must carry the incomplete status and respect ordinary visibility, graph selection, and access boundaries. They must not publish incomplete conclusions as authoritative checked facts.

Editor and agent integration must go through the language server, using the same compiler analysis facts as ordinary checking rather than a separate inference engine. RFC 106 projections must reuse that shared fact model, retain explicit incomplete and source-revision evidence, and follow its schema compatibility rules: additions must be compatible or guarded by a schema version. Exact record fields and protocol wiring belong to implementation design under those contracts.

Implementations must not require code synthesis, remote inference, or speculative execution to provide this feedback. Optional tooling may propose replacements, but it must distinguish suggestions from compiler-established requirements and must recheck edited source before claiming completion.

## Design details

### A missing expression is not a panic

A panic stub describes executable behavior and can make later code unreachable. A typed hole describes missing behavior and keeps all dependent conclusions incomplete. Substituting one for the other would conceal exactly the information this feature is intended to expose.

### Required type versus a type-correct candidate

For a hole requiring `int`, an in-scope `fallback: int` is a possible value, not necessarily the desired value. For a hole requiring a Result, both an Ok and an Err may type-check while implementing opposite behavior. Required facts and suggested candidates must remain distinct in both prose and structured output.

### Generic feedback can remain symbolic

```incan
def preserve[T](value: T) -> T:
    return ???
```

The required type is the declaration's `T`, not an unspecified arbitrary concrete type. The diagnostic can show `value: T` without specializing the generic function or claiming that returning it is the only correct implementation.

Conversely, a hole passed into a generic callable may leave some parameters unresolved. The checker must preserve those variables and their bounds rather than selecting an instantiation merely because one candidate is convenient.

### Interaction with pattern proposals

Holes in RFC 138 condition terms remain missing expressions; they do not become patterns or successful conditions. A bare hole in an RFC 134 fallback body does not establish that the fallback exits. An explicit `return ???` or `return Err(???)` retains its authored return boundary while leaving its value incomplete. RFC 139 sequence shapes can constrain an initializer's collection type, but an exact-length pattern does not supply the missing initializer's value or prove that its runtime length will match.

### Compatibility and spelling

`???` makes an unfinished expression visible without reserving an ordinary identifier. It does require context-sensitive recognition alongside postfix propagation. The current parser accepts consecutive postfix propagation operations, so a global longest-token rule would change existing source. Primary-expression recognition preserves those postfix operations instead. The spelling is settled; preserving postfix parsing is a compatibility detail rather than a competing user-facing meaning for a primary-expression hole.

This proposal changes checking behavior for explicit incomplete source. It does not weaken ordinary completed-program type checks, artifact validation, or visibility boundaries.

## Alternatives considered

- **Use a panic or todo call.** Useful for deliberate executable stubs, but introduces runtime behavior and can hide downstream type or flow information through divergence.
- **Use an ordinary placeholder value.** May let the program build with unintended behavior and anchors inference to a value the author never meant to choose.
- **Rely only on parser error recovery.** Important for accidental broken syntax, but lacks an explicit missing-expression obligation and a reliable user-facing contract.
- **Use `_` as the hole spelling.** Concise, but overlaps wildcard patterns and existing identifier conventions. Dedicated punctuation makes expression incompleteness explicit.
- **Require an annotation on every hole.** Can provide useful context, but duplicates information already supplied by a field, argument, or return contract. Ordinary annotations remain available when inference is insufficient.
- **Generate a replacement automatically.** Can be an optional tool feature, but types alone cannot establish domain intent. The compiler contract must remain useful without synthesis.
- **Permit builds when holes appear unreachable.** Makes artifact completeness depend on reachability analysis and can silently ship unfinished source. Every selected hole blocks artifact production.

## Drawbacks

- Requires constraint-oriented feedback and careful separation between established and incomplete conclusions.
- Additional diagnostics can overwhelm users when one hole leaves many dependent expressions unresolved; grouping and suppression of redundant cascades matter.
- Large generic or overloaded contexts may yield constraints rather than one simple required type.
- Ownership and effects cannot be fully established from an unknown expression's result type alone.
- A punctuation spelling near `?` requires a compatibility audit and clear formatter behavior.
- Supporting incomplete analysis across compiler and tooling boundaries adds an additional state that must never be mistaken for checked completeness.

## Implementation architecture

This section is non-normative. A coherent implementation would retain intentional holes as distinct source nodes, attach contextual constraints and their origins during analysis, and preserve incomplete status through diagnostics and editor projections. Ordinary error recovery and intentional holes can share recovery infrastructure without sharing their public meaning. Executable lowering should require a completed-source boundary rather than teaching every backend to invent behavior for holes.

The same contextual facts should feed human-readable diagnostics and structured consumers, with editor and agent integration implemented through the language server over shared compiler analysis. Constraint origins should explain a requirement from the relevant field, parameter, return annotation, or propagation boundary, rather than reconstructing that explanation from a rendered type afterward.

## Layers affected

- **Lexer / Parser / AST:** explicit expression holes distinguished from postfix propagation and accidental malformed source.
- **Typechecker / Symbol resolution:** contextual unknowns, constraint origins, available bindings, generic relationships, and independent error checking.
- **Ownership / Flow / Contract analysis:** explicit incomplete outcomes for checks depending on missing expression behavior.
- **Compilation / Artifact boundary:** unresolved selected holes prevent executable and distributable output.
- **Diagnostics:** deterministic hole reports, related requirement spans, and grouped dependent unknowns.
- **Formatter:** preservation of the hole spelling and surrounding expression structure.
- **LSP / Tooling:** revision-bound hole context, incomplete analysis, navigation, and structured diagnostics.
- **Package / Graph metadata:** incomplete projections kept distinct from authoritative checked artifacts.
- **Documentation:** workflows for completing source without runtime placeholder semantics.

## Design Decisions

- `???` is the expression-hole spelling. Existing postfix propagation retains its parsing; this compatibility rule does not require an alternative hole keyword or punctuation and does not prohibit nested Result types.
- Callable holes include deterministic type-compatible candidates from checked named callables already in scope. Human and structured reports distinguish these candidates from requirements and intended behavior. The compiler does not search unrelated libraries, execute user code, choose a candidate, or insert an implementation.

- Editor and agent integration goes through the language server over shared compiler analysis. Human diagnostics and RFC 106 projections expose the same requirements, context, candidates, and incomplete status, bound to the analyzed source revision. Existing structured diagnostic and graph contracts are reused with compatible or schema-versioned changes; exact record fields and protocol wiring are implementation design.
