# RFC 146: Typed JSON decoding and complete validation

- **Status:** Draft
- **Created:** 2026-10-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes and implicit coercion)
    - RFC 024 (extensible derive protocol)
    - RFC 070 (Result combinators)
    - RFC 085 (field metadata and type constraints)
    - RFC 086 (schema descriptors and adapters)
    - RFC 087 (named checks and complete model contract composition)
    - RFC 088 (iterator adapters)
    - RFC 098 (implementation-chosen types in trait headers)
    - RFC 103 (secret values and Incan-wide non-disclosure)
    - RFC 115 (fallible iteration and combinators)
    - RFC 122 (model construction contracts)
    - RFC 128 (shared callable signatures)
    - RFC 131 (error declarations, umbrellas, hierarchy, and diagnostics)
    - RFC 142 (async iteration and source-owned failure policies)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Give standard typed JSON decoding a non-generic JsonDecodeError umbrella that preserves original validation failures, structured document paths, and available source positions. Derived decoding checks nested validated values and model validation, collects all independently discoverable input failures by default, and exposes a complete generated Result contract without requiring callers to enumerate validator error types. JSONL consumption separately supports lazy per-record outcomes and explicit collection that preserves successful records and distinguishes invalid records from source interruption. RFC 131 owns the general error mechanism; this RFC owns its JSON integration and migration from current string-returning typed decoding.

## Core model

1. Callers receive `Result[T, JsonDecodeError]`, rather than an authored union of every nested validator error.
2. A located validation failure retains its original concrete error, typed payload, hierarchy, and available diagnostics.
3. Document paths are immutable typed segments; readable paths are formatting, never the location representation.
4. Standard typed JSON decoding reports all independently discoverable failures by default and distinguishes blocked or interrupted checks from completed validation.
5. A JSON document and a JSONL source have separate consumption boundaries. Lazy record decoding does not by itself choose whether a consumer stops, collects, or ignores record failures.
6. Successful records remain available when a collector reports failures; successful decoding does not imply committing application effects.

## Motivation

The current typed JSON Deserialize contract returns `Result[Self, str]`. Converting a custom validator failure to text loses its concrete error identity and payload and makes precise recovery depend on interpreting messages. Existing Validate construction and validated newtypes also have different error contracts. These boundaries need an explicit integration contract before typed decoding can promise validated model instances with preserved domain failures.

Users should be able to fix all discoverable input problems in one pass. Returning only the first independent field failure makes repeated validation necessary. Complete reporting must still explain when malformed syntax, invalid prerequisites, source interruption, or retention limits prevented checks from completing.

JSONL adds a separate concern: a malformed record need not prevent later records from being decoded. A useful consumer can retain original record failures and valid records, while preserving the distinction between a bad record and a source that could not supply more data.

## Goals

- Preserve typed domain failures across standard JSON decoding and derive boundaries.
- Expose a simple JsonDecodeError umbrella without repeating nested validator error types in caller signatures.
- Preserve structured paths, available source positions, error hierarchy, causes, and available origin and stack diagnostics.
- Report all independently discoverable JSON input failures by default.
- Validate nested values and declared model invariants before exposing a successful instance.
- Support lazy JSONL outcomes and explicit collection with retained successes and honest interruption reporting.
- Specify migration from existing string-returning decoder contracts.

## Non-Goals

- Define another error declaration syntax, cause accessor, stack-trace policy, or general aggregation mechanism; RFC 131 owns those contracts.
- Replace Result propagation or change the behavior of `?` and existing early-return terminals.
- Select a new failure policy for every iterator, concurrent operation, or effectful callback.
- Execute checks whose required decoded values are unavailable, inspect unexecuted branches of an authored validator, or fabricate downstream failures after malformed syntax.
- Implicitly commit valid records, retry source reads, or roll back earlier application effects.
- Redesign JSON serialization, introduce separately authored string error codes, or expose sensitive input values automatically.

## Guide-level explanation

All new contracts below are proposed behavior. The `json` derive is an existing stdlib surface imported with `from std.serde import json`; Validate is an existing built-in derive. Named `@validate` checks and from-composed model contracts are proposed by RFC 087; their execution integration remains under review in RFC 122. Error declarations, umbrella retention, checked matching, and common diagnostic traversal follow RFC 131. IdentifierError, SymbolicId, Atom, PackageError, PackageSnapshot, and the validation helper are application definitions. JsonDecodeError and InputPath describe proposed stdlib contracts whose module placement remains open. The examples are design test drives, not claims that combining current derives already implements typed nested validation.

### Define application validation failures

```incan
from std.serde import json
from std.traits.error import Error

error IdentifierError:
    InvalidSymbol:
        value: str
        message = "Identifier must use lowercase letters, digits, '_' or '-'."


type SymbolicId = newtype str:
    def from_underlying(value: str) -> Result[Self, ValidationError]:
        if not identifier_text_is_safe(value):
            return Err(ValidationError.Invalid(
                original=IdentifierError.InvalidSymbol(value=value),
            ))
        return Ok(SymbolicId(value))
```

The application helper used above is defined below:

```incan
def identifier_text_is_safe(value: str) -> bool:
    if value == "":
        return false
    for character in value:
        if character not in "abcdefghijklmnopqrstuvwxyz0123456789_-":
            return false
    return true
```

The canonical newtype hook retains RFC 017's `Result[Self, ValidationError]` boundary. ValidationError is the proposed standard umbrella from RFC 131; its Invalid case above retains the original IdentifierError with its concrete identity, payload, hierarchy, and diagnostics. The wrapper spelling is the candidate shared representation from RFC 131, not a JSON-specific error declaration. Authored validation helpers may still return precise domain errors before capture at this canonical boundary. Explicit `from_underlying` calls remain recoverable, and JSON decoding must call that recoverable hook rather than invoke panic-producing implicit coercion. Ordinary implicit coercion retains RFC 017's existing behavior.

Model validation can use a domain error too:

```incan
error PackageError:
    BlankTitle:
        message = "Package title cannot be empty."


@derive(json)
model Atom:
    id: SymbolicId


@derive(json)
model PackageSnapshot:
    title: str
    atoms: list[Atom]

    @validate
    def title_present(self) -> Result[None, PackageError]:
        if self.title == "":
            return Err(PackageError.BlankTitle)
        return Ok(None)
```

`PackageSnapshot.new(title="", atoms=[])` returns `Result[PackageSnapshot, ValidationError]` under the proposed generated-construction contract. ValidationError is a non-generic umbrella that retains the original `PackageError.BlankTitle` failure rather than converting it to text. The authored `title_present` check declares its precise domain error type; the generated boundary captures it for callers. RFC 087 registered checks activate validated construction without requiring a separate Validate derive. The desired decoding integration must also check nested validated values and every retained field and whole-model contract check before exposing a successful instance, including rules from RFC 087 from composition. Combining the derive and proposed check declarations does not prove that this behavior exists today.

### Preserve custom decoding failures and field paths

Typed JSON decoding uses a standard, non-generic `JsonDecodeError` umbrella family that distinguishes invalid JSON, missing fields, unknown fields, wrong types, and domain validation failures. The located `InvalidValue` wrapper is settled: it retains the document path and original typed failure and exposes that failure through `caused_by()`. Document locations are structured paths, never formatted strings. In the following proposed stdlib declaration, `InputPath` names an immutable sequence of typed object-member and array-index segments; an empty sequence denotes the document root. The InputPath name and module placement remain provisional, but the structured contract is settled. Readable formatting such as `atoms[1].id` is a display operation. Source positions are separate structured data when available. The following remains proposed source syntax rather than an implemented stdlib API:

```incan
error JsonDecodeError extends JsonError:
    MissingField:
        path: InputPath
        message = f"Required field '{path}' is missing."

    UnknownField:
        path: InputPath
        accepted_fields: FrozenList[str]
        message = f"Unknown field '{path}'."

    DuplicateField:
        path: InputPath
        message = f"Field '{path}' was supplied more than once."

    WrongType:
        path: InputPath
        expected: str
        message = f"Field '{path}' must be {expected}."

    InvalidValue:
        path: InputPath
        cause: Error
        message = f"Invalid value at {path}."
        caused_by = cause

    Multiple:
        failures: FrozenList[JsonDecodeError]
        message = f"{len(failures)} decoding failures."
```

For this test drive, the decoding method returns `Result[PackageSnapshot, JsonDecodeError]`. An application boundary therefore needs only the umbrella:

```incan
def load_snapshot(source: str) -> Result[PackageSnapshot, JsonDecodeError]:
    return PackageSnapshot.from_json(source)
```

Here PackageSnapshot is the application model defined above; JsonDecodeError is the proposed stdlib umbrella. Callers capture the umbrella without enumerating every nested validator failure. Adding another nested validator does not change that outer error type; it can still change which inputs are accepted and which failures occur. The generated integration checks the declared validator return types and nested validation or decoding contracts, rather than guessing possible failures from function bodies. The original domain error must survive with its concrete identity and payload. Here `cause: Error` denotes an owned error value exposed through the common interface, not a selected spelling for general trait-object storage. Its concrete representation remains open; ordinary nested error patterns must support checked recognition of the retained concrete failure. The complete generated Result signature must remain visible in tooling and documentation. No additional derive-specific error-set constraint is introduced.

| Input                                 | Expected classification                                                                      |
| ------------------------------------- | -------------------------------------------------------------------------------------------- |
| Valid title and identifiers           | `Ok(PackageSnapshot(...))`                                                                   |
| Malformed JSON                        | `JsonError.Parse`                                                                            |
| Missing `title`                       | `JsonDecodeError.MissingField`, path `title`                                                 |
| Numeric `atoms[0].id`                 | `JsonDecodeError.WrongType`, path `atoms[0].id`                                              |
| Invalid identifier in the second atom | `JsonDecodeError.InvalidValue`, path `atoms[1].id`, original `IdentifierError.InvalidSymbol` |
| Empty title                           | `JsonDecodeError.InvalidValue`, path `$`, original `PackageError.BlankTitle`                 |

The table displays paths for readability; it does not specify string storage. `atoms[1].id` represents member "atoms", index 1, member "id". A literal member named "a.b" is one segment, distinct from member "a" followed by member "b". The root path on model-wide validation is intentional: a generic decoder cannot identify a more specific field unless the validator reports one. Derived decoders construct and prefix paths automatically. Field paths, source positions, and custom error identity remain separate structured data. Inspection, tooling, and selected serialization must use that structure rather than parse a message or rendered path.

The decoding error's own message supplies the location; traversal supplies the original validator's message. An application wrapper may add the fixture filename while retaining the decoder error. An illustrative combined report can read:

```text
Could not decode fixture 'package.json'.
  Invalid value at atoms[1].id.
    Validation failed.
      Identifier must use lowercase letters, digits, '_' or '-'.
```

Each level supplies its own message without embedding the entire upstream chain in it. The invalid value and concrete error identity remain retained independently. A caller can match the umbrella and read the retained cause through its common interface:

```incan
match PackageSnapshot.from_json(text):
    Ok(snapshot) => println("Package decoded.")
    Err(JsonDecodeError.InvalidValue(path, ValidationError.Invalid(IdentifierError.InvalidSymbol(value)))) =>
        println(f"Please correct the identifier at {path}: {value}")
    Err(JsonDecodeError.InvalidValue(path, original)) =>
        println(f"Please correct {path}: {original.message()}")
    Err(error) => println(error.message())
```

Umbrella capture preserves the declared error family, nested case identity, payload, and upstream hierarchy. Ordinary nested `match` patterns can optionally recognize a retained concrete error and bind its typed payload, as in the first failure arm above. Recognition must check the concrete error identity before exposing its fields. Patterns follow the explicit value structure; they do not implicitly search all upstream failures. The umbrella does not provide an exhaustive static list of every nested domain failure, so matches must retain a fallback for unhandled failures. RFC 131 owns this checked recognition behavior and its general runtime integration; this RFC requires the decoder to preserve the values and identities it needs.

The candidate stdlib JsonDecodeError declaration above is a contract sketch, rather than an application declaration that replaces a stdlib type. JsonError is the existing `std.json` public error name, proposed to migrate from its model-and-kind representation to a native error family under RFC 131. The sketch uses RFC 131's `extends` to include that shared family's cases alongside typed-decoding cases; included cases retain their original identities. `JsonError.Parse` is the proposed native case corresponding to the existing Parse category. JsonDecodeError is the selected typed-decoding umbrella name; its module placement and the exact shared case payloads remain open.

Dynamic JSON parsing and typed model decoding must share underlying JSON failures rather than define parallel parsing error hierarchies. Dynamic parsing remains `Result[JsonValue, JsonError]`; typed decoding returns `Result[Self, JsonDecodeError]`, widening shared JSON failures without converting them to strings or creating replacement identities. The typed umbrella adds located validation and aggregation where those contracts apply. Dynamic values do not acquire model-schema validation merely by participating in the shared error family. This follows the common JSON-error boundary used by [serde_json's generic deserialization function](https://docs.rs/serde_json/latest/serde_json/fn.from_str.html); the broader validation umbrella serves Incan's explicitly richer typed-validation contract.

### Diagnose unknown model fields

Standard typed model decoding rejects unknown fields by default. The failure is a distinct `JsonDecodeError.UnknownField` with the structured path of the unexpected member and the accepted input field names for that object. Accepted names follow the model's checked decoding contract, including supported wire aliases. Arbitrary keys inside a declared dictionary remain valid dictionary keys; the rule applies where a model defines a field schema.

```incan
from std.serde import json

@derive(json)
model JobOptions:
    timeout: int
```

For this application model, `{"timeout": 30, "timout": 60}` produces UnknownField at the member "timout". A clear diagnostic identifies the unexpected name, its document path and available source position, and the accepted field names. It should suggest a correction when a clear match exists, but a suggestion must not silently rename the input or choose a value for the caller. For example:

```text
Unknown field 'timout' at $.timout.
  Accepted field: timeout.
  Did you mean 'timeout'?
```

Applications consuming evolving external schemas may explicitly request tolerant decoding that ignores unknown fields. Tolerance does not disable JSON syntax checking, validation of declared fields, or collection of other discoverable failures. The option's exact public spelling remains part of the decoder API design. Strictness belongs to the proposed typed model boundary; existing dynamic JSON and decoder APIs still require their own explicit compatibility disposition.

### Follow Serde behavior with structured diagnostics

Serde's established JSON deserialization behavior is the baseline for ordinary decoding rules unless this RFC explicitly specifies a different contract. Standard failure categories must be adapted into typed JsonDecodeError cases with structured locations and clear diagnostics; the implementation must not recover categories by parsing Serde's rendered messages. Ordinary parser edge cases do not require independent language policy decisions merely because they need an error case.

For example, Serde reports a [duplicate struct field](https://docs.rs/serde/latest/serde/de/trait.Error.html#method.duplicate_field) when one model field is supplied more than once. Typed model decoding therefore reports `JsonDecodeError.DuplicateField` for `{"timeout": 30, "timeout": 60}` against the JobOptions model above. Equal values do not remove the duplication. Where multiple accepted wire spellings resolve to the same field, supplying that field through both spellings is also a duplicate. A diagnostic must identify the field and its structured path and should show both source occurrences when available. This model-field rule must not be silently generalized to change Serde's separate dictionary or dynamic JSON object behavior.

The explicitly different Incan contracts remain: typed retained domain failures, complete reporting of independently discoverable errors, strict unknown-model-field rejection with explicit tolerance, and strict typed JSONL blank-record handling with explicit skipping. Following Serde does not reduce those contracts to strings or first-failure reporting. Their compatibility with existing Incan APIs remains governed by the migration section.

### Preserve secret non-disclosure through decoding

Secrets must not leak through Incan-owned diagnostics. This is an Incan-wide commitment under RFC 103, not a JSON-specific option. Decoding must uphold it even when input fails before a secret wrapper can be constructed. Checked secret types and sensitivity contracts must govern input handling, retained failures, custom derive integration, and diagnostic presentation.

Diagnostics should retain safe structural facts such as the error category, expected type, and available source position while redacting protected material. Source excerpts, upstream parser messages, paths containing sensitive input, and other metadata must not provide another route to secret disclosure. If malformed input or unavailable sensitivity information prevents establishing that an excerpt or detail is safe, omit or redact it. Enabling verbose reports, stack traces, or audit export does not change this rule. Retaining the original typed failure must preserve its secret protections rather than introduce ordinary plaintext copies for reporting.

### Report complete JSON validation failures

Standard typed JSON decoding collects all independently discoverable input failures by default. It does not return after the first missing field, wrong type, or invalid nested value when other checks can safely proceed. One failure uses its ordinary JsonDecodeError case; several use the candidate `JsonDecodeError.Multiple` case with immutable retained failures. Both return the same JsonDecodeError umbrella, and each member retains its structured location, concrete domain error, and upstream diagnostics. Aggregate members must be available through the common diagnostic traversal and combined formatter; the generated aggregate integration must not pretend that a runtime collection is an authored fixed `caused_by` tuple.

For example, using the PackageSnapshot and SymbolicId definitions above:

```json
{"title": "Example", "atoms": [{"id": "!"}, {"id": "?"}]}
```

The decoder reports both invalid identifiers, at `atoms[0].id` and `atoms[1].id`, in one failure result. It does not expose an invalid PackageSnapshot or ask the caller to fix one identifier before discovering the other. The aggregate retains both IdentifierError.InvalidSymbol values; its report formats both original messages and paths rather than reducing the failures to a count.

Complete reporting means every applicable check the decoder can actually perform. A wrong-shaped subtree can prevent checks requiring its decoded values, but does not prevent independent sibling checks. A model-wide validator requiring a valid instance runs only after its required fields decode successfully. Authored validator control flow remains ordinary: returning Err ends that invocation, and generated validation cannot discover branches that never executed. Malformed JSON may prevent structural decoding altogether; the decoder must report the syntax failure without manufacturing values or downstream validation results. Blocked validation and interrupted or truncated collection must not be presented as successful checks or a completed validation report. Exact structured coverage and aggregate interfaces remain to be specified.

This default belongs to standard typed JSON decoding. Failure policy for concurrent work, lazy sources, callbacks, and batch side effects remains governed by those operations' own contracts; complete JSON validation does not make FailFast drain work or roll back effects.

### Parse all JSONL rows and surface failures at the end

A row result can be an ordinary iterator item. A bad row is then retained while the next row is attempted:

```incan
error JsonlRowError[E with Error]:
    BlankRecord:
        line: int
        message = f"JSONL line {line} contains a blank record."

    Invalid:
        line: int
        cause: E
        message = f"Could not decode JSONL line {line}."
        caused_by = cause


error JsonlError[T, E with Error]:
    InvalidRows:
        parsed: list[T]
        failures: list[JsonlRowError[E]]
        message = f"{len(failures)} JSONL rows failed."
```

An authored collection helper can express the policy without changing `?`:

```incan
def collect_jsonl[T, E with Error](
    source: str,
    decode: Callable[(str) -> Result[T, E]],
    skip_blank_lines: bool = false,
) -> Result[list[T], JsonlError[T, E]]:
    mut parsed: list[T] = []
    mut failures: list[JsonlRowError[E]] = []

    lines = source.split("\n")
    for index, line in enumerate(lines):
        # Empty input or the final split suffix is not another physical record.
        if index == len(lines) - 1 and line == "":
            continue
        if line.strip() == "":
            if not skip_blank_lines:
                failures.append(JsonlRowError.BlankRecord(line=index + 1))
            continue
        match decode(line):
            Ok(value) => parsed.append(value)
            Err(error) =>
                failures.append(JsonlRowError.Invalid(
                    line=index + 1,
                    cause=error,
                ))

    if len(failures) > 0:
        return Err(JsonlError.InvalidRows(parsed=parsed, failures=failures))
    return Ok(parsed)
```

The helper uses RFC 128's proposed callable spelling. Each nonblank physical line is decoded once; failure collection does not stop at the first invalid row. Blank or whitespace-only records produce the dedicated `JsonlRowError.BlankRecord` case with their one-based physical line number, without invoking the decoder or fabricating an upstream cause. They are not collapsed into a generic malformed-JSON message. This follows the [JSON Lines format](https://jsonlines.org/), which requires each record to contain a JSON value. A normal terminating newline does not create an extra blank record, and empty input contains zero records. The explicit `skip_blank_lines=true` option requests tolerant skipping while retaining subsequent physical line numbers. The helper's option spelling is illustrative; the default and distinct error identity are required for the proposed typed JSONL surface. For the candidate snapshot decoder:

```incan
return collect_jsonl(source, PackageSnapshot.from_json)
```

| Line | Outcome                |
| ---- | ---------------------- |
| 1    | Valid snapshot         |
| 2    | Invalid identifier     |
| 3    | Valid snapshot         |
| 4    | Missing required field |

The final `JsonlError.InvalidRows` contains two successful snapshots and two typed row failures. The caller can reject the entire batch, display all failures, or explicitly use the retained successes. No records are committed merely because decoding succeeded.

A blank line between two valid snapshots produces two retained successes and a `JsonlRowError.BlankRecord(line=2)` failure under collect-all consumption. Callers can match that case directly instead of interpreting its message. An `Invalid` row retains its single upstream decoding error; a `BlankRecord` is the record failure itself and has no invented cause. The aggregate retains a runtime-sized list of row failures as payload; that list is not accepted by the fixed tuple declaration. Shared traversal of aggregate members depends on the still-open runtime aggregate mechanism, while ordinary payload inspection remains available.

The helper above is an application-defined eager illustration of the collection policy. The standard `std.json.parse_jsonl` API instead changes to lazy iteration as an intentional breaking replacement of its existing eager `Result[list[JsonValue], JsonError]` contract. It accepts source text and yields one `Result[JsonValue, JsonlRowError[JsonError]]` per consumed physical record, subject to explicit blank-record skipping. Construction must not parse records or build the complete record list up front. An invalid record does not itself end iteration; stopping early is a caller decision. Blank-record errors retain the one-based physical line number, and invalid JSON records retain the original shared JsonError.

```incan
# Existing public name, proposed replacement contract.
from std.json import parse_jsonl

def inspect_records(source: str) -> None:
    for record in parse_jsonl(source):
        match record:
            Ok(value) => println("Valid record.")
            Err(error) => println(error.message())
```

Applications wanting eager results must explicitly consume a collection terminal. A corresponding lazy typed-decoding API can produce `Iterator[Result[T, JsonlRowError[E]]]` and reuse that collection contract:

```incan
outcomes = jsonl_rows[PackageSnapshot](source)
return collect_all(outcomes)
```

`jsonl_rows` and `collect_all` are candidate typed-decoding and collection API names; they do not replace the settled `std.json.parse_jsonl` name for lazy dynamic JSON records. The lazy row iterator does no decoding at construction; each advance attempts one row, or skips explicitly tolerated blank records before attempting the next yielded record. Collecting to the end accumulates failures and partial successes. Stopping consumption early means later rows have not been checked.

A file-backed source can use the existing FallibleIterator protocol from `std.derives.collection` and IoError from `std.io`, with the shape `FallibleIterator[Result[T, JsonlRowError[E]], IoError]`: an inner `Err` describes a row that was read but could not be decoded; an outer polling error describes failure to obtain more source data. RFC 115's early-return terminals remain unchanged. A source interruption must not be rendered as a completed batch, and accumulated information must remain available through the chosen collector's interruption outcome. The exact interruption payload and collector API remain open.

### Distinguish selected input from interrupted collection

An application that explicitly selects the first 100 records has chosen its input scope. Consuming that prefix to its end is ordinary completion of the selected input, even if the original source contains more records. It does not prove that the rest of the source was validated. Ordinary iterator scope selection must not become an error merely because the caller intentionally chose a prefix.

A collector asked to validate the whole supplied input has a different promise. Collection has no hidden outcome-retention cap by default. A caller may explicitly configure a protective collection limit; if that limit stops consumption before ordinary exhaustion is established, the collector must return an incomplete outcome preserving prior successes, original failures, and the reason it stopped. Reaching a limit does not prove whether additional records exist. The report must describe the observed coverage without claiming exhaustion or inventing failures in unconsumed records.

This follows the distinction between explicit iterator scope selection and validation failure policy illustrated by Rust and Python iterators and Pydantic's opt-in early failure. The concrete collection-limit options and outcome spelling remain API design work. Recognizing Pydantic as prior art does not implicitly add its annotation syntax or change Incan's default complete typed validation.

## Reference-level explanation

1. Standard typed JSON decoding must expose `Result[Self, JsonDecodeError]`. Callers must not need to enumerate every nested validator's error type or repeat those types in a separate derive annotation. The complete generated signature must be visible in tooling and documentation.
2. JsonDecodeError must distinguish malformed JSON, missing required members, wrong input types, and domain validation failures. The located InvalidValue wrapper must retain a structured document path and the original concrete domain failure. It must preserve the failure's typed payload and hierarchy and expose its upstream relationship through RFC 131's diagnostic interface.
3. Document paths must be immutable sequences of typed member and index segments, with an empty path denoting the root. Formatted text must not serve as the location representation. Derived decoders must construct and prefix paths automatically. Inspection, tooling, and selected serialization must preserve that structure. Available source positions must be separate structured data.
4. Standard typed JSON decoding must collect all independently discoverable input validation failures by default. Independent sibling checks must continue after a field failure. A single failure and a group of failures must share the JsonDecodeError umbrella, and every retained failure must remain individually inspectable and available to combined diagnostic reporting.
5. A decoder promising validated construction must check nested validated fields and the model's declared validation before exposing success. It must not bypass checks because construction was generated or return an invalid model as Ok.
6. A check requiring unavailable decoded values must not be reported as successfully executed. Authored validator control flow must remain ordinary; returning Err ends that invocation. Malformed syntax, source interruption, cancellation, and diagnostic truncation must not be presented as completed validation. Collection limits and coverage gaps must be observable through structured information.
7. Generated integration must use declared validator return types and nested validation or decoding contracts. It must not inspect function bodies to guess possible failures. Original failures must be retained before a textual adapter can discard them; reconstructing error identity by parsing a message is forbidden.
8. Canonical registered-check model construction and canonical newtype `from_underlying` hooks must expose `Result[Self, ValidationError]`. Authored validators may retain precise domain error return types; canonical boundaries capture those failures under ValidationError while preserving original typed identity, payload, hierarchy, and diagnostics. JSON decoding must use recoverable construction and retain the validation umbrella under its located JsonDecodeError failure. Ordinary implicit coercion keeps RFC 017's existing behavior; this RFC does not turn implicit construction into Result propagation.
9. A lazy JSONL iterator must decode a record only when consumed. A record-level Err must remain distinct from a failed source advancement. By default a blank or whitespace-only physical record must yield the distinct `JsonlRowError.BlankRecord` case with its one-based physical line number, rather than a generic decoder failure or silent skipping. An explicit option may request blank-record skipping; it must not renumber subsequent records. Lazy construction must not imply a collect-all policy, source retry, or validation of unconsumed records.
10. An explicit collect-all JSONL terminal must consume to ordinary exhaustion, retain successes in source order, and preserve each failure's original error and physical source location, including BlankRecord failures. A terminating newline must not create another record; empty input must contain zero records. A completed collection with failures must expose retained successes and individual failures without implying acceptance or commit.
11. Interrupted collection must preserve the source failure and prior outcomes and expose that it did not complete. Cancellation, consumption limits, or truncated diagnostic retention must remain distinguishable from completed collection. The behavior of `?` and RFC 115 early-return terminals must remain unchanged. The breaking replacement of `parse_jsonl` is explicit and must not implicitly collect its lazy records to satisfy an old eager caller.
12. Aggregate failures must use RFC 131's common hierarchy, traversal, and diagnostic preservation contracts. A runtime-sized collection must not be disguised as the fixed authored `caused_by` tuple. General aggregate representation is owned by RFC 131; this RFC owns the decoder and collector surfaces that use it.
13. Standard typed model decoding must reject unknown fields by default with a distinct UnknownField case retaining the structured input path and accepted input names for that object. Diagnostics must clearly identify the unexpected member, show accepted field names and available source positions, and should offer a correction when a clear match exists without applying it automatically. Model wire aliases must follow the checked decoder contract, and declared dictionary keys must not be mistaken for unknown model fields. An explicit tolerance option may ignore unknown fields but must retain syntax checking and normal validation of declared fields. Unknown-field failures must participate in the same collection of independently discoverable errors as other decoding failures.
14. Ordinary JSON decoding rules must follow established Serde behavior unless this RFC explicitly specifies a different contract. Standard Serde failure categories must be preserved as distinct typed errors with structured locations and clear diagnostics, without text-based reconstruction. Duplicate model-field inputs, including accepted aliases resolving to one field, must report DuplicateField rather than choose a value; diagnostics should identify both occurrences when source information is available. Dictionary and dynamic JSON behavior must retain their separately applicable contracts. The baseline must not weaken typed failure retention, complete validation, or the settled unknown-field and blank-record policies.
15. Deserialize must require `from_json(json_str: str) -> Result[Self, JsonDecodeError]` for both generated and handwritten implementations. This is an intentional breaking replacement of the string-returning contract. A handwritten implementation retaining `Result[Self, str]` must receive a clear diagnostic identifying the method, its actual return type, and the required return type. No implicit compatibility adapter may convert old string errors into purported structured or original domain failures.
16. All decoder and collector diagnostics must uphold RFC 103's Incan-wide secret non-disclosure invariant, including failures before secret construction. Checked sensitivity must survive derive and error boundaries. Retained failures, source excerpts, upstream messages, paths, and metadata must not bypass secret protection or create ordinary plaintext copies solely for reporting. Details that cannot be established safe to expose must be omitted or redacted in human diagnostics, machine inspection, verbose output, stack traces, and audit export alike.
17. Dynamic JSON and typed model decoding must reuse a shared JsonError family for underlying JSON failures, preserving the defining case identities, payloads, and diagnostics. JsonDecodeError is the broader typed-decoding umbrella that includes those shared failures alongside located validation failures and aggregation. Migrating the existing JsonError model-and-kind representation to native errors must not create a second parser-error hierarchy or impose model-validation requirements on dynamic values. Exact case payloads and compatibility details for existing dynamic operations remain to be specified.
18. `std.json.parse_jsonl` must intentionally replace its eager list-returning contract with lazy per-record outcomes. Construction must not decode records or build a complete record list; each consumed record must yield its own Result, preserving physical line numbers and original shared JSON failures. A record error must not terminate iteration by itself. Blank records follow the strict default and explicit skipping contract. Eager consumption must be explicit, and callers requiring the previous list-returning contract must receive migration guidance rather than implicit collection.
19. Explicit collect-all consumption must have no hidden cap that discards retained successes or failures. A caller may configure a protective collection limit; when that limit stops consumption before ordinary exhaustion is established, the collector must preserve prior outcomes and report incomplete coverage with a structured stopping reason. Explicit selection of a prefix is ordinary input scoping and may complete normally for that selected input. Neither prefix completion nor a protective limit must establish complete validation of an enclosing source or assert that additional records exist without evidence.

## Design details

### Ownership of the contracts

RFC 131 owns error declarations, closed error composition, umbrella capture, checked concrete matching, immutable cause relationships, runtime-sized aggregation, origins, stack traces, and combined diagnostics. This RFC consumes those facilities. JSON-specific categories, document path segments, validation scheduling, complete-report defaults, generated decoder signatures, and JSONL record contracts are specified here.

The candidate `Multiple` case and InputPath name are illustrative API spellings. The settled requirements are one JsonDecodeError umbrella, individually retained failures, structured locations, and complete reporting of applicable checks. Concrete aggregate and coverage surfaces remain open; the candidate case must not be mistaken for a separately implemented aggregation facility.

Uniform inspection of one or several retained failures is shared umbrella behavior owned by RFC 131. JsonDecodeError must reuse that contract while preserving its members' typed identity, hierarchy, payloads, causes, and diagnostics. Aggregate membership remains distinguishable from upstream causation. The inspection spelling, whether it is a method, and any typed JsonDecodeError view are deferred to RFC 131; this RFC does not select a JSON-only `failures()` API.

### Validated construction and nested failures

IdentifierError.InvalidSymbol describes why a value is invalid; JsonDecodeError.InvalidValue adds where it occurred in the document. PackageError.BlankTitle describes a model-wide invariant. Both remain available through the same decoder umbrella without an authored union or an extra conversion step. For model-wide validation, a root location is honest unless the validator supplies a more specific field location.

The JSON decoder does not need a distinct associated error type for each model. Its generated boundary remains `Result[Self, JsonDecodeError]` when nested validators change. Such a change can still reject previously accepted inputs or introduce new observable failures; a stable outer type does not promise behavioral compatibility. Authored trait adoptions still obey RFC 098's explicit binding rules.

The general validated-construction umbrella decision is supplied by RFC 131. RFC 085 and RFC 087 registered checks use the same umbrella without an extra Validate derive; model composition must preserve source validation and normalization. RFC 122 owns the detailed construction execution questions rather than this JSON RFC. It selects a runtime validation_policy scope, with models declaring rules rather than execution policy. Ordinary library and helper calls, including nested model construction, inherit the active runtime scope; a decoder boundary must not silently reset policy for registered checks. Coordination between explicitly selected early stopping, blocked-check coverage, and the complete JSON input walk still needs test driving under this RFC. The default remains complete reporting under CollectAll; active scope inheritance is not an implicit change to that default. This RFC must coordinate its JSON use with the existing constructor contract rather than assume that current Validate already returns that umbrella. Custom derives must publish checked callable contracts and preserve original errors; how those contracts are expressed through shared traits remains open.

### Structured locations and coverage

A literal member named "a.b" is one segment and differs from two nested member segments. Integer array indices remain distinct from member names containing digits. Source offsets and line or column information describe a position in input bytes or text; document paths describe data structure. Neither is recovered from a rendered message. Position indexing conventions and the concrete APIs remain open.

Complete validation is bounded by actual input structure and validator contracts. A wrong-shaped subtree can block checks requiring its decoded values while permitting independent sibling checks. Malformed JSON can prevent a schema walk altogether. The structured report must distinguish blocked or interrupted work from completed checks and must not claim to enumerate failures from unexecuted authored code.

### JSONL policy and resource use

Lazy parsing and delayed reporting are independent choices. A source may produce one Result item per record; an explicit terminal selects accumulation. Complete JSON document validation does not force a lazy JSONL consumer to read the entire source after its caller chooses to stop.

Lazy decoding over an already materialized `str` still retains the complete input text; it must not be advertised as streaming a large file with bounded input buffering. Incremental file- and reader-backed parsing is a follow-up design requirement. Such an API must read input incrementally without requiring callers to load the entire file into memory, preserve physical record locations across buffer boundaries, and distinguish record errors from source-read failures through the fallible-source contract. Buffering must be described separately from record size and from outcomes retained by an explicit collector: streaming input alone does not bound a single arbitrarily large record or an application's accumulated results. The reader and file API spelling, record-size controls, and lifecycle integration remain deferred; the source-text API is not a substitute for that work.

Accumulation can retain memory proportional to input size. Full collection has no hidden outcome-retention cap; protective limits are explicit. A protective limit that prevents observed exhaustion interrupts the full-input collection promise and preserves prior outcomes with an incomplete status and stopping reason. Intentional prefix selection instead defines the input being consumed and does not itself imply an error. Completion is relative to the supplied input scope, never evidence that an enclosing source was exhausted. Cancellation and source interruption likewise require honest outcomes. Any streaming delivery of successful values exposes application effects separately from the promise to report failures at the end. An aggregate does not roll back earlier effects.

### Compatibility and migration

The current typed Deserialize method returns `Result[Self, str]`; Serialize.to_json returns str. Deserialize changes directly to `from_json(json_str: str) -> Result[Self, JsonDecodeError]`. This intentional breaking change applies to generated and handwritten implementations; preserving the old string-returning trait contract is not required. Serialization is unchanged.

Generated decoders expose the new signature. Handwritten implementations must update their return annotation and failure construction; a contract-mismatch diagnostic must identify the affected method, show the old and required return types, and explain that errors now use JsonDecodeError. Generic callers and explicit result annotations must update their error type accordingly. Callers that need rendered text may explicitly format the typed error at their presentation boundary. There is no implicit string compatibility adapter, and previously discarded domain payloads cannot be recovered from their messages.

Current generated Validate construction returns the authored validator's `Result[Self, E]`. RFC 122 deliberately replaces that legacy registration with named `@validate` checks returning `Result[None, E]` and canonical `Result[Self, ValidationError]` construction. Authors must separate checks, transformations, and external operations, with actionable migration diagnostics rather than automatic adaptation or parallel invocation of both mechanisms. Domain failures remain precise and are retained under the validation umbrella without text conversion. Constructor callers and explicit result annotations must migrate to the shared umbrella. Canonical newtype hooks retain their `Result[Self, ValidationError]` signature while the umbrella gains typed retention under RFC 131. Existing hook implementations and callers must migrate from the legacy ValidationError representation with clear diagnostics. JSON construction uses these recoverable boundaries, retaining the validation umbrella and its original domain failure. Ordinary implicit coercion remains governed by RFC 017, including its existing failure behavior. Proposed source examples do not establish that these integrations exist today.

The existing `std.json.parse_jsonl(source)` changes from eager list construction, silent blank-line skipping, and first-error return to strict lazy per-record outcomes. This is an intentional breaking change. Eager callers must explicitly collect, choose how to handle failures, and request blank-line tolerance if desired. Diagnostics for old eager uses must identify the lazy iterator result and explain the need for explicit consumption; no implicit collector or blanket `?` rewrite is introduced. The new contract preserves earlier outcomes by yielding them as they are consumed rather than withholding successes until exhaustion.

Remaining dynamic JSON APIs, typed JSONL helper signatures, collector outcomes, and custom derive contracts need a scope audit before this RFC moves to Planned. Their migration must follow their actual existing contracts rather than assuming every API currently returns the same error type.

### Prior art

[Pydantic's validation reports](https://docs.pydantic.dev/latest/errors/errors/) provide a common validation exception with multiple located issues and readable output. This proposal borrows the aggregation and location experience while retaining Incan's typed error identity, Result-based handling, and checked matching rather than selecting string identifiers for dispatch.

The JsonDecodeError name is familiar but its scope is intentionally broader than [Python's JSONDecodeError](https://docs.python.org/3/library/json.html#json.JSONDecodeError), which represents malformed JSON and subclasses ValueError. Pydantic's [model_validate_json](https://docs.pydantic.dev/latest/api/base_model/#pydantic.BaseModel.model_validate_json) instead reports JSON parsing and model-validation failures through ValidationError; its [validation umbrella can contain multiple located failures](https://docs.pydantic.dev/latest/concepts/models/#error-handling). Incan's JsonDecodeError combines recognizable JSON decoding terminology with that broader validated-construction boundary while retaining original typed failures under RFC 131. It is not a claim that Python or Pydantic uses the same error hierarchy.

Pydantic also provides [opt-in FailFast for sequence validation](https://docs.pydantic.dev/latest/concepts/performance/#failing-early-with-failfast), explicitly trading visibility into later failures for earlier termination. It is useful prior art for keeping complete validation as the normal experience while making reduced coverage a deliberate choice. It is not a collection-memory limit or a guarantee of retained partial successes. Python jsonschema's [iter_errors](https://python-jsonschema.readthedocs.io/en/stable/validate/#jsonschema.protocols.Validator.iter_errors) separately shows lazy production of validation failures. Rust's [take](https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.take) and Python's [islice](https://docs.python.org/3/library/itertools.html#itertools.islice) select an intentional prefix without treating the selection boundary as a failure; Incan's collection report must preserve that distinction from interruption of a requested full-input validation.

## Alternatives considered

- **Return strings from typed decoding.** Rejected: rendered text cannot preserve the original error identity, typed payload, or structured location contract.
- **Require an authored union of every nested validation error.** Rejected for the default decoder surface: nested changes would require callers to rewrite signatures. A common JsonDecodeError umbrella preserves detailed failures underneath.
- **Keep document locations as formatted strings.** Rejected: punctuation in member names and index syntax can be ambiguous, and tooling would need to parse presentation text to recover structure.
- **Return only the first independent validation failure by default.** Rejected for standard typed JSON decoding: users should be able to fix all discoverable input problems in one pass.
- **Return a bare domain failure with location only in diagnostic metadata.** Rejected: the located decoder failure and original domain failure must remain independently inspectable through the typed error hierarchy.
- **Make lazy JSONL iteration imply collect-all behavior.** Rejected: advancement and terminal failure policy have separate owners. Callers may stop early, and a source advancement failure is distinct from a record failure.

## Drawbacks

Complete validation requires more work and storage than returning after the first failure. Retaining original error values and successful records extends their lifetimes. Coverage, cancellation, and retention limits require honest reporting rather than hidden truncation.

A stable umbrella simplifies signatures but leaves concrete domain matching open-ended. JSON integration depends on RFC 131's checked retained-error recognition and runtime aggregate contracts. Typed decoding and Validate migration affect existing code and custom derive implementations.

## Layers affected

- **Stdlib JSON and JSONL APIs:** typed decoder errors, record outcomes, collection, limits, and interruption contracts.
- **Derive protocol and validation integration:** checked generated signatures, nested validated construction, and preservation before textual adaptation.
- **Typechecker / callable metadata:** visibility of generated Result contracts and compatibility checks for handwritten trait implementations.
- **Runtime and diagnostics:** structured input locations, source positions, coverage, and integration with RFC 131 aggregate traversal and retained diagnostics.
- **LSP / documentation:** generated signature help, field-location inspection, diagnostics, and migration guidance.

## Design Decisions

- Standard typed JSON decoding uses the non-generic JsonDecodeError umbrella; callers do not enumerate nested validator failures.
- JsonDecodeError is the selected typed-decoding name. It includes the shared JsonError family rather than duplicating parser failures, and adds typed validation and aggregation. Dynamic JSON retains the JsonError boundary, with its model-and-kind representation intended to migrate to native errors under RFC 131. Existing case identities remain preserved when viewed through the broader umbrella.
- InvalidValue retains the structured document location and original concrete domain failure together, including its typed payload and hierarchy.
- Ordinary nested matching can recognize a specific retained failure through RFC 131's checked matching contract; callers retain a fallback for unhandled failures.
- Document paths are structured typed segments, never formatted string locations. Source positions are separate structured information when available. Derived decoding supplies paths automatically.
- Standard typed JSON decoding collects all independently discoverable validation failures by default. It preserves blocked or interrupted coverage honestly and does not rewrite authored validator control flow.
- Standard typed model decoding rejects unknown fields by default with a distinct UnknownField case, a structured member path, and accepted input names. Clear diagnostics show the unexpected member, accepted names, available source positions, and a correction when a clear match exists. Explicit tolerant ignoring is permitted without weakening other validation; declared dictionary keys remain governed by their dictionary type rather than a closed model field set.
- Serde supplies the baseline for ordinary decoding behavior. Distinct typed error categories, structured locations, and clear diagnostics are required; routine parser cases follow that baseline without becoming separate language design questions. Duplicate model fields report DuplicateField, including multiple accepted spellings for one field. Explicitly settled Incan differences and separate dictionary or dynamic JSON contracts remain intact.
- Generated decoding checks declared validator and nested type contracts rather than guessing error types from function bodies. Additional derive-specific error-set constraint syntax is not introduced.
- Deserialize directly replaces `Result[Self, str]` with `Result[Self, JsonDecodeError]` as an intentional breaking change for generated and handwritten decoding. Outdated handwritten signatures receive a clear contract-mismatch diagnostic. Callers explicitly update their error handling; no implicit string compatibility adapter is introduced.
- Secret non-disclosure follows RFC 103 as an Incan-wide invariant, including input failures before secret construction. Decode and derive integration must preserve checked sensitivity, and diagnostic surfaces must omit or redact details whose safe exposure cannot be established. Diagnostic richness never authorizes revealing secrets.
- Canonical registered-check construction and canonical newtype hooks use the ValidationError umbrella, retaining original domain failures rather than returning a bare domain error from the canonical hook. Authored checks may remain precise. RFC 122 replaces legacy Validate registration through a deliberate breaking migration with clear diagnostics rather than an automatic adapter. Explicit construction and JSON decoding are recoverable; ordinary implicit coercion retains RFC 017 behavior. Legacy validation representations require explicit migration with clear diagnostics.
- Lazy JSONL records and explicit collection preserve original record failures, successful records, and the distinction between an invalid record and source interruption.
- The existing `std.json.parse_jsonl` name adopts lazy per-record Result outcomes as an intentional breaking change. Eager collection is explicit; a record error does not itself stop iteration. Old list-returning callers receive clear migration guidance, and no implicit collection is introduced.
- Incremental file- and reader-backed parsing is recorded for follow-up design. Lazy decoding of a complete source string must not be described as bounded-memory file streaming. Future source-backed APIs must read incrementally, retain physical locations, and distinguish read failures from record failures without requiring whole-file materialization.
- Collect-all has no hidden cap that discards outcomes. Protective collection limits are explicit and return an incomplete outcome retaining prior successes, failures, and a structured stopping reason when exhaustion is not established. Intentional prefix selection may complete normally for the selected scope and does not claim full-source validation. Pydantic's aggregate validation and opt-in early failure are prior art, alongside ordinary iterator scope selection.
- Uniform inspection of retained failures reuses RFC 131's shared umbrella and aggregate contract. Its spelling and interface are deferred there; this RFC does not independently introduce a `failures()` method or direct error iteration.
- The proposed typed JSONL surface and the replacement `std.json.parse_jsonl` contract are strict by default. Blank and whitespace-only records produce a distinct BlankRecord error with the one-based physical line number and no fabricated upstream cause. Explicit tolerant skipping may be requested without renumbering later records. A final record terminator is not an extra record, and empty input contains zero records. Other JSONL entrypoints still require an explicit compatibility disposition before their defaults change.

## Unresolved questions

- How does complete JSON input traversal coordinate with an explicitly selected RFC 122 validation_policy scope? Registered checks inherit the ordinary runtime context and reuse RFC 122's typed-input eligibility rules; conversion failures, blocked checks, and policy-stopped evaluation must remain distinguishable.
- Which modules expose JsonDecodeError and the shared JsonError family, and what names and interfaces expose the aggregate case, structured document paths, and source positions? The JsonDecodeError name and reuse of shared underlying JSON failures are settled.
- What indexing conventions and source identities define offsets, lines, and columns?
- What JSON-specific coverage information exposes checks blocked by invalid prerequisites or interrupted validation? Shared umbrella and aggregate inspection, including its public spelling, is deferred to RFC 131.
- How do custom derives publish their checked validation contracts and preserve payload capabilities? Secret non-disclosure and preservation of checked sensitivity are settled requirements under RFC 103; the shared derive interface remains to be specified.
- What concrete adapters and diagnostics migrate legacy ValidationError construction and inspection? The canonical ValidationError umbrella, typed retention, recoverable JSON integration, and unchanged ordinary implicit-coercion behavior are settled.
- What names, signatures, limit options, and concrete interruption outcomes serve lazy JSONL records and collect-all terminals? No hidden outcome-retention cap, explicit protective limits, preserved prior outcomes on interruption, and normal completion of explicitly selected prefixes are settled.
- What future file- and reader-backed parsing APIs provide incremental input, record-size controls, and resource lifecycle integration? This follow-up must not be conflated with lazy decoding of an already materialized string.
- Which remaining dynamic JSON and JSONL APIs participate, and what compatibility policy applies to each? The shared JsonError family and the breaking lazy replacement of `std.json.parse_jsonl` are settled.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
