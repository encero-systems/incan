# RFC 137: Named enum payload fields

- **Status:** Planned
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (enum declarations and matching)
    - RFC 024 (aliases and derives)
    - RFC 049 (`if let` and `while let` patterns)
    - RFC 050 (enum methods and trait adoption)
    - RFC 071 (pattern alternation)
    - RFC 120 (canonical source identity)
    - RFC 131 (dedicated error declarations)
    - RFC 134 (pattern bindings with exiting fallbacks)
- **Issue:** [#2243](https://github.com/encero-systems/incan/issues/2243)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Allow an ordinary enum case to declare named payload fields, such as `Finished(path: str, size: int)`. The case remains one alternative in its enum, while field names document its data, support keyword construction, and permit named patterns that select only the fields a caller needs. Existing unit cases and unnamed positional payload cases retain their syntax and behavior. This does not introduce model declarations inside enums or replace dedicated error declarations.

## Motivation

An enum case often carries several values whose primitive types do not explain their roles:

```incan
enum DownloadState:
    Pending
    Finished(str, int)
```

At a construction site, `DownloadState.Finished("report.pdf", 4096)` requires readers to remember that the string is a path and the integer is a size. The problem grows with multiple strings, booleans, or collection payloads.

Compiler-adjacent code contains cases such as `Active(list[str], bool, DependencyDomain)`, carrying requested features, a default-feature flag, and a dependency domain. Payload names would make those roles explicit at both declaration and construction. Named patterns would also remove positional wildcard counting when a caller only needs one field.

## Goals

- Name an enum case's payload fields at their declaration.
- Support keyword construction using those names.
- Support named patterns with omitted fields treated as wildcards.
- Preserve existing positional enums and closed exhaustive matching.
- Keep case identity, field types, visibility, and ownership consistent across imports and compiled package boundaries.
- Provide useful completion, signature, and diagnostic information from the declared fields.

## Non-Goals

- Replacing enums with models or classes.
- Introducing fields shared by every case in an enum.
- Adding inheritance or extension to ordinary enums.
- Adding payload defaults, constructor methods, field aliases, or field-level visibility syntax.
- Making a field accessible on the whole enum when other cases lack it.
- Introducing new case subtypes, error messages, error causes, or error propagation.
- Changing Rust-backed enum metadata or assuming that foreign fields have authored Incan names.

## Guide-level explanation

Named payload declarations, construction, and patterns below describe proposed behavior.

### Explain each payload at its declaration

```incan
enum DownloadState:
    Pending
    Finished(path: str, size: int)
```

The value is still either `Pending` or `Finished`. Only `Finished` carries a path and size; there is no separate model to declare or wrap.

### Construct with meaningful names

```incan
state = DownloadState.Finished(
    path="report.pdf",
    size=4096,
)
```

Keyword order does not change which field receives a value:

```incan
state = DownloadState.Finished(size=4096, path="report.pdf")
```

Positional construction remains available in declaration order:

```incan
state = DownloadState.Finished("report.pdf", 4096)
```

Adding names therefore need not force every existing construction site to change. Names remain especially useful when argument types alone do not explain their meaning.

### Match only the fields that matter

```incan
def describe(state: DownloadState) -> str:
    match state:
        DownloadState.Pending => return "Still waiting"
        DownloadState.Finished(path=path) => return f"Downloaded {path}"
```

The omitted `size` matches any size. A field pattern can also check a value:

```incan
def is_empty_download(state: DownloadState) -> bool:
    match state:
        DownloadState.Finished(size=0) => return true
        _ => return false
```

That literal pattern does not cover every `Finished` value, so the fallback remains necessary. A named pattern selecting `path` without constraining it covers all `Finished` payloads.

Existing positional matching remains available in field declaration order:

```incan
match state:
    DownloadState.Finished(path, size) => println(f"{path}: {size} bytes")
    DownloadState.Pending => pass
```

### Clarify a multi-field case

```incan
enum SlotActivation:
    Active(features: list[str], defaults: bool, domain: DependencyDomain)
    Inactive(reason: InactiveReason)
```

With the application's existing domain and reason types, construction becomes:

```incan
activation = SlotActivation.Active(
    features=unique(requested),
    defaults=defaults,
    domain=domain,
)
```

A caller concerned only with the domain can match `SlotActivation.Active(domain=domain)` without spelling two leading wildcards.

### Use existing conditional matching

```incan
if let DownloadState.Finished(path=path) = state:
    println(path)
```

With RFC 134, the same pattern can establish a prerequisite:

```incan
def completed_path(state: DownloadState) -> Option[str]:
    let DownloadState.Finished(path=path) = state
    else:
        return None

    return Some(path)
```

The names become visible according to the surrounding pattern construct, not according to a new enum-specific scope rule.

## Reference-level explanation

### Declaration forms

The existing unit form `Case` and positional form `Case(T1, T2, ...)` must remain accepted. A named payload case must use `Case(field1: T1, field2: T2, ...)`, with at least one field. Field order is declaration order.

Every field of one case must be named or every payload entry must be unnamed. Mixing named fields and unnamed types within a case must be rejected. Different cases within the same enum may use different forms.

Field names must be unique within a case and must follow the existing identifier rules. Each field type must resolve under the enum's ordinary type and generic-parameter scope. Field defaults and field-level visibility modifiers must not be accepted by this proposal.

RFC 032 value enums backed by `str` or `int` must retain their existing literal-value and payload restrictions. Named payload syntax must not permit data-carrying cases in those value enums.

### Construction

A named payload case must accept positional arguments in declaration order and keyword arguments by declared field name, under the existing call argument ordering rules. Positional arguments must precede keyword arguments. Each field must receive exactly one value assignable to its declared type.

Missing, duplicate, and unknown fields, excess positional arguments, and invalid field types must produce source diagnostics. A value supplied positionally and again by name must count as a duplicate. Argument expressions must be evaluated exactly once in source order, including when keyword order differs from field declaration order.

Unnamed positional cases must not acquire generated keyword names such as `field0`. Unit cases must retain their existing bare-value spelling. Constructor identity and enum admission must remain unchanged by the choice of named or positional arguments.

### Patterns

A named payload case must accept patterns of the form `Case(field=pattern, ...)`. A pattern must not name a field more than once or name an undeclared field. Named pattern entries may appear in any order. Fields omitted from a named pattern must match any value and bind nothing.

A case with named fields must also accept a fully positional pattern in declaration order. A positional pattern must retain the existing exact-arity rule. A single case pattern must not mix positional and named entries.

For a named payload case, `Case()` in pattern position must be an empty named pattern matching every payload of that case and binding nothing. This does not permit `Case()` as a constructor when required fields are missing. A bare payload-bearing case name must not replace a payload pattern. Existing unnamed cases must retain their positional arity rules, including for an empty pattern.

Each supplied sub-pattern must follow the existing pattern typing rules. Omitted fields must behave exactly as wildcard sub-patterns. This must apply consistently in `match`, `if let`, `while let`, and RFC 134's proposed exiting binding. Alternation must retain RFC 071's binding-name and binding-type requirements.

### Coverage

Exhaustiveness must remain based on cases and the values their payload patterns cover. A named pattern with only wildcard or binding sub-patterns must cover the entire named case. A literal or otherwise restrictive sub-pattern must cover only its admitted payload values; omitting other fields must not make that pattern exhaustive for the case.

Adding a named payload case to an enum must continue to affect exhaustiveness as adding any ordinary case does. Names must not introduce open cases or runtime field dispatch.

### Access, ownership, and identity

Payload access must remain subject to matching the appropriate case. This proposal does not make `state.path` valid merely because one alternative declares `path`, or introduce independently constructible field records outside their case.

Fields must have the accessibility of the case's existing payload surface; the new syntax must not silently apply model or class private-field defaults. Matching and construction must preserve ordinary borrowing, ownership, and mutation permissions. A named pattern must not clone a field implicitly or change the effect of an equivalent positional pattern.

Case identity must remain qualified by its enum and namespace. Field names must be part of the public construction and pattern contract wherever that case is accessible. Renaming a field can break keyword calls and named patterns; moving a case or changing a payload type remains an API change under the existing language rules. This RFC does not add a stable string code or automatic compatibility alias.

Imported and compiled representations must preserve authored field names, declaration order, and field types wherever they expose a named case. Tooling must not reconstruct semantic names from generated backend fields or parameter position.

### Derives and serialization

Derives and reflection that inspect a case's payload must receive the declared field names and types. Existing derive capability requirements must remain applicable; naming a field must not provide Clone, equality, display, or serialization automatically.

Under the ordinary supported JSON derive, a named payload case must serialize as an externally tagged object whose payload contains the declared field names. For `Finished(path: str, size: int)`, a value carrying path "report.pdf" and size 4096 must encode as:

```json
{"Finished": {"path": "report.pdf", "size": 4096}}
```

The declaration must determine this representation even when the author constructs the named case positionally. Deserialization must use the corresponding named payload shape. Existing unnamed payload cases must retain their positional representation; `Finished(str, int)` carrying the same values retains `{"Finished": ["report.pdf", 4096]}`. Unit cases must retain their existing representation. Other supported derives must receive the same authored named-field metadata and retain their own format contracts.

Converting an unnamed payload declaration to a named one must be documented as a serialized schema change, even when existing positional source calls remain valid. The default named-case decoder must not silently accept the old positional representation as an automatic compatibility fallback. Constructors and patterns must not infer their names from serialized keys. This RFC introduces no field-alias or wire-migration mechanism.

### Diagnostics and tooling

Completion and signature help should present field names in declaration order and offer keyword arguments. Pattern completion should offer declared fields without requiring authors to bind every field. Diagnostics must distinguish constructor requirements from named-pattern omissions and identify the particular unknown, duplicate, or mistyped field.

## Design details

### Cases remain the unit of choice

`Finished(path: str, size: int)` defines one case and its data. It does not mix model fields into the enum body, define fields for other cases, or add a separate type declaration. The existing distinction between unit and payload cases remains visible.

### Named and positional use

Allowing both forms supports small local cases and existing positional call sites while giving larger or public cases a descriptive spelling. Field order still matters to positional users; field names matter to keyword users. Both are part of the API once published.

### Relationship to errors

RFC 131 uses the same named payload header spelling for dedicated error cases and deliberately owns immutable error fields, error-specific defaults, compact message expressions, optional declaration bodies, causes and error-set operations. This RFC provides a named payload surface for ordinary enums, without importing those error-specific guarantees. Shared compiler support for field metadata and matching is desirable, but the dedicated error source contract must remain separate.

## Alternatives considered

- **Keep every case positional.** Compact for simple payloads, but leaves repeated primitive types unexplained and makes partial extraction depend on position.
- **Wrap a model inside each case.** Expressive today, but requires another declaration and constructor layer solely to name the case's own data.
- **Give every case an indented model-like field body.** Could support more elaborate case bodies, but changes the enum's existing compact case syntax and introduces additional questions about methods, defaults, and declaration boundaries.
- **Require keyword-only construction and matching.** Rejected because it prevents a compatible path from existing positional case use. Both positional and named use remain available; declaration order and field names are both API commitments.
- **Infer names from local argument variables.** Unstable across call sites and unable to define a reusable import or package contract.

## Drawbacks

- Adds a declaration form and named pattern behavior that parsers and tooling must distinguish from ordinary calls.
- Field names become public API commitments alongside existing positional order.
- Partial named patterns can continue matching after a new field is added, which may hide a change a caller would otherwise reconsider.
- Supporting both positional and named use offers flexibility but permits inconsistent style within one codebase.
- Derives and serialization need an explicit named-case contract rather than accidental backend behavior.

## Layers affected

- **Parser / AST:** named field declarations in enum cases and named case argument and pattern forms.
- **Symbol resolution / typechecker:** field signatures, argument mapping, pattern typing, and exhaustive coverage.
- **Canonical metadata / package interfaces:** authored names, types, declaration order, and source identity.
- **Ownership analysis / IR lowering:** field-aware construction and extraction with existing ownership semantics.
- **Emission:** representation and calls preserving source names and evaluation order.
- **Derives / reflection / serialization:** logical payload metadata and the selected named-case contract.
- **Formatter / LSP:** declaration formatting, keyword completion, named patterns, and source diagnostics.

## Design Decisions

- Named payload cases support both positional and keyword construction, plus either fully positional or named patterns under the declared argument and pattern rules. Adding names to an existing positional declaration does not itself require callers to rewrite those uses. Declaration order and field names are both part of the accessible case's API; adding names makes no promise of serialized wire-format compatibility.

- Named payloads serialize by declared field names under the ordinary JSON derive, inside the case's existing external tag. Positional construction of a named case does not change that encoding. Unnamed and unit cases retain their existing representations. Changing an unnamed case to a named declaration changes its serialized schema without an automatic dual-format decoder.
