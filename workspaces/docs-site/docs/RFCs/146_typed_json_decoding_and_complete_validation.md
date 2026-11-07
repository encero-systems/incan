# RFC 146: Typed JSON decoding and complete validation

- **Status:** Planned
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
    - RFC 094 (context managers and scoped cleanup)
    - RFC 142 (iteration consumers and failure policies)
    - RFC 158 (split and compressed byte readers, `skip_bytes`)
    - RFC 173 (walking and reconstructing typed values)
- **Issue:** [#2588](https://github.com/encero-systems/incan/issues/2588)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Give standard typed JSON decoding a non-generic JsonDecodeError umbrella that preserves original validation failures, structured document paths, and available source positions. Derived decoding checks nested validated values and model validation, collects all independently discoverable input failures by default, and exposes a complete generated Result contract without requiring callers to enumerate validator error types. An explicit FailFast policy stops decoding at the first failure. Source positions count lines and characters from one. JSONL sources are ordinary iterators of per-record results, read lazily from a string or from a `std.io.BinaryReader`, and collected through RFC 142's shared consumers, which keep successful records and distinguish invalid records from source interruption. A reader-backed source reports the position after each record and resumes from a saved position over any reader, failing when the input before that position has changed. RFC 131 owns the general error mechanism; this RFC owns its JSON integration and the typed decoding contract that replaces string-returning decoding.

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
- Stop at the first failure when an explicit FailFast policy is active.
- Read JSONL lazily from strings and from `std.io.BinaryReader`, one typed result per record, and collect it through RFC 142's shared consumers with retained successes and honest interruption reporting.
- Resume an interrupted JSONL import after a saved position, without decoding the input before it, and detect when that input has changed.
- Replace string-returning typed decoding with the typed contract.

## Non-Goals

- Define another error declaration syntax, cause accessor, stack-trace policy, or general aggregation mechanism; RFC 131 owns those contracts.
- Replace Result propagation or change the behavior of `?` and existing early-return terminals.
- Select a new failure policy for every iterator, concurrent operation, or effectful callback.
- Execute checks whose required decoded values are unavailable, inspect unexecuted branches of an authored validator, or fabricate downstream failures after malformed syntax.
- Implicitly commit valid records, retry source reads, or roll back earlier application effects.
- Wait for more lines at the end of a file that is still being written; a reader-backed source ends at the reader's end.
- Redesign JSON serialization, introduce separately authored string error codes, or expose sensitive input values automatically.

## Guide-level explanation

The `json` derive is an existing stdlib surface imported with `from std.serde import json`. Named `@validate` checks and from-composed model contracts come from RFC 087, with their execution governed by RFC 122. Error declarations, umbrella retention, checked matching, and common diagnostic traversal follow RFC 131. IdentifierError, SymbolicId, Atom, PackageError, PackageSnapshot, and the validation helper are application definitions. `JsonDecodeError`, `JsonError`, `InputPath`, and `SourcePosition` are exported from `std.json`:

```incan
from std.json import InputPath, JsonDecodeError, JsonError, SourcePosition
```

The examples are design test drives, not claims that combining current derives already implements typed nested validation.

A stored model representation follows RFC 173's model-owned reconstruction path: restore the declared stored fields and perform final validation without replaying constructor fieldmethods. Virtual construction inputs are not JSON fields by implication. An explicitly authored input adapter may accept fresh constructor inputs under RFC 122, but must not silently change the meaning of stored-state decoding. The decoder's location, error collection and coverage obligations apply to both explicit routes.

### Define application validation failures

```incan
from std.serde import json
from std.traits.error import Error

error IdentifierError:
    InvalidSymbol(value: str) = "Identifier must use lowercase letters, digits, '_' or '-'."


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

The canonical newtype hook retains RFC 017's `Result[Self, ValidationError]` boundary. ValidationError is the proposed standard umbrella from RFC 131; its Invalid case above retains the original IdentifierError with its concrete identity, payload, hierarchy, and diagnostics. The wrapper is the shared representation from RFC 131, not a JSON-specific error declaration. Authored validation helpers may still return precise domain errors before capture at this canonical boundary. Explicit `from_underlying` calls remain recoverable, and JSON decoding must call that recoverable hook rather than invoke panic-producing implicit coercion. Ordinary implicit coercion retains RFC 017's existing behavior.

Model validation can use a domain error too:

```incan
error PackageError:
    BlankTitle = "Package title cannot be empty."


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

Typed JSON decoding uses a standard, non-generic `JsonDecodeError` umbrella family that distinguishes invalid JSON, missing fields, unknown fields, wrong types, and domain validation failures. The located `InvalidValue` wrapper is settled: it retains the document path and original typed failure and exposes that failure through `caused_by()`. Document locations are structured paths, never formatted strings. In the following proposed stdlib declaration, `InputPath` names an immutable sequence of typed object-member and array-index segments; an empty sequence denotes the document root. Readable formatting such as `atoms[1].id` is a display operation. Source positions are separate structured data when available. The following is the proposed stdlib declaration:

```incan
error JsonDecodeError extends JsonError:
    MissingField(path: InputPath) = f"Required field '{path}' is missing."

    UnknownField(path: InputPath, accepted_fields: FrozenList[str]) = f"Unknown field '{path}'."

    DuplicateField(path: InputPath) = f"Field '{path}' was supplied more than once."

    WrongType(path: InputPath, expected: str) = f"Field '{path}' must be {expected}."

    InvalidValue(path: InputPath, cause: Error):
        message = f"Invalid value at {path}."
        caused_by = cause

    Multiple(failures: FrozenList[JsonDecodeError]) = f"{len(failures)} decoding failures."
```

For this test drive, the decoding method returns `Result[PackageSnapshot, JsonDecodeError]`. An application boundary therefore needs only the umbrella:

```incan
def load_snapshot(source: str) -> Result[PackageSnapshot, JsonDecodeError]:
    return PackageSnapshot.from_json(source)
```

Here PackageSnapshot is the application model defined above; JsonDecodeError is the proposed stdlib umbrella. Callers capture the umbrella without enumerating every nested validator failure. Adding another nested validator does not change that outer error type; it can still change which inputs are accepted and which failures occur. The generated integration checks the declared validator return types and nested validation or decoding contracts, rather than guessing possible failures from function bodies. The original domain error must survive with its concrete identity and payload. Here `cause: Error` denotes an owned error value exposed through the common interface, not a selected spelling for general trait-object storage. Its concrete storage is implementation work under RFC 131; ordinary nested error patterns must check the retained concrete identity before exposing typed fields. The complete generated Result signature must remain visible in tooling and documentation. No additional derive-specific error-set constraint is introduced.

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

The JsonDecodeError declaration above is the stdlib contract, not an application declaration that replaces a stdlib type. JsonError is the existing `std.json` public error name, proposed to migrate from its model-and-kind representation to a native error family under RFC 131. The sketch uses RFC 131's `extends` to include that shared family's cases alongside typed-decoding cases; included cases retain their original identities. `JsonError.Parse` is the native case corresponding to the existing Parse category; each existing JsonError category becomes one native case with the same meaning. Both errors are exported from `std.json`.

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

Applications consuming evolving external schemas may explicitly request tolerant decoding with `JobOptions.from_json(text, unknown_fields=UnknownFields.Ignore)`. Tolerance does not disable JSON syntax checking, validation of declared fields, or collection of other discoverable failures. Strictness belongs to the typed model boundary; dynamic `JsonValue` parsing has no model fields and is unaffected.

### Follow Serde behavior with structured diagnostics

Serde's established JSON deserialization behavior is the baseline for ordinary decoding rules unless this RFC explicitly specifies a different contract. Standard failure categories must be adapted into typed JsonDecodeError cases with structured locations and clear diagnostics; the implementation must not recover categories by parsing Serde's rendered messages. Ordinary parser edge cases do not require independent language policy decisions merely because they need an error case.

For example, Serde reports a [duplicate struct field](https://docs.rs/serde/latest/serde/de/trait.Error.html#method.duplicate_field) when one model field is supplied more than once. Typed model decoding therefore reports `JsonDecodeError.DuplicateField` for `{"timeout": 30, "timeout": 60}` against the JobOptions model above. Equal values do not remove the duplication. Where multiple accepted wire spellings resolve to the same field, supplying that field through both spellings is also a duplicate. A diagnostic must identify the field and its structured path and should show both source occurrences when available. This model-field rule must not be silently generalized to change Serde's separate dictionary or dynamic JSON object behavior.

The explicitly different Incan contracts remain: typed retained domain failures, complete reporting of independently discoverable errors, strict unknown-model-field rejection with explicit tolerance, and strict typed JSONL blank-record handling with explicit skipping. Following Serde does not reduce those contracts to strings or first-failure reporting.

### Preserve secret non-disclosure through decoding

Secrets must not leak through Incan-owned diagnostics. This is an Incan-wide commitment under RFC 103, not a JSON-specific option. Decoding must uphold it even when input fails before a secret wrapper can be constructed. Checked secret types and sensitivity contracts must govern input handling, retained failures, custom derive integration, and diagnostic presentation.

Diagnostics should retain safe structural facts such as the error category, expected type, and available source position while redacting protected material. Source excerpts, upstream parser messages, paths containing sensitive input, and other metadata must not provide another route to secret disclosure. If malformed input or unavailable sensitivity information prevents establishing that an excerpt or detail is safe, omit or redact it. Enabling verbose reports, stack traces, or audit export does not change this rule. Retaining the original typed failure must preserve its secret protections rather than introduce ordinary plaintext copies for reporting.

### Report complete JSON validation failures

Standard typed JSON decoding collects all independently discoverable input failures by default. It does not return after the first missing field, wrong type, or invalid nested value when other checks can safely proceed. One failure uses its ordinary JsonDecodeError case; several use the `JsonDecodeError.Multiple` case with immutable retained failures. Both return the same JsonDecodeError umbrella, and each member retains its structured location, concrete domain error, and upstream diagnostics. Aggregate members must be available through the common diagnostic traversal and combined formatter; the generated aggregate integration must not pretend that a runtime collection is an authored fixed `caused_by` tuple.

For example, using the PackageSnapshot and SymbolicId definitions above:

```json
{"title": "Example", "atoms": [{"id": "!"}, {"id": "?"}]}
```

The decoder reports both invalid identifiers, at `atoms[0].id` and `atoms[1].id`, in one failure result. It does not expose an invalid PackageSnapshot or ask the caller to fix one identifier before discovering the other. The aggregate retains both IdentifierError.InvalidSymbol values; its report formats both original messages and paths rather than reducing the failures to a count.

Complete reporting means every applicable check the decoder can actually perform. A wrong-shaped subtree can prevent checks requiring its decoded values, but does not prevent independent sibling checks. A model-wide validator requiring a valid instance runs only after its required fields decode successfully. Authored validator control flow remains ordinary: returning Err ends that invocation, and generated validation cannot discover branches that never executed. Malformed JSON may prevent structural decoding altogether; the decoder must report the syntax failure without manufacturing values or downstream validation results. Blocked validation and interrupted or truncated collection must not be presented as successful checks or a completed validation report. Every decoding failure exposes `coverage()`, which says which checks completed, which were blocked by an invalid prerequisite, which were stopped by policy, and which were interrupted, each with its path. A check that never ran appears only in coverage, never as an invented failure.

This default belongs to standard typed JSON decoding. Failure policy for concurrent work, lazy sources, callbacks, and batch side effects remains governed by those operations' own contracts; complete JSON validation does not roll back effects.

### Stop at the first failure when asked

Inside an explicit FailFast scope, decoding stops at the first failure it observes:

```incan
from std.validation import ErrorPolicy, validation_policy

with validation_policy(ErrorPolicy.FailFast):
    result = PackageSnapshot.from_json(text)
```

For `{"title": "Example", "atoms": [{"id": "!"}, {"id": "?"}]}`, decoding stops at `atoms[0].id` and does not parse `atoms[1]` at all. The failure is the ordinary `InvalidValue`, and its coverage reports the remaining checks as stopped by policy, never as passed. Outside such a scope the default stays complete reporting, and leaving the scope restores the outer policy.

### Read JSONL one record at a time

A JSONL source is an ordinary iterator with one `Result` per physical record, so a bad line does not stop the next one. Each record failure is a `JsonlRecordError`:

```incan
error JsonlRecordError[E with Error]:
    BlankRecord(line: int) = f"JSONL line {line} contains a blank record."

    RecordTooLarge(line: int, max_record_bytes: int) = f"JSONL line {line} exceeds {max_record_bytes} bytes."

    Invalid(line: int, cause: E):
        message = f"Could not decode JSONL line {line}."
        caused_by = cause
```

`std.json.parse_jsonl` yields dynamic records, and `jsonl[T]` yields typed records decoded through `T.from_json`:

```incan
from std.json import jsonl, parse_jsonl

def inspect_records(source: str) -> None:
    for record in parse_jsonl(source):
        match record:
            Ok(value) => println("Valid record.")
            Err(error) => println(error.message())

def snapshots(source: str) -> Iterator[Result[PackageSnapshot, JsonlRecordError[JsonDecodeError]]]:
    return jsonl[PackageSnapshot](source)
```

Construction decodes nothing; each step decodes one record. Blank or whitespace-only records produce `JsonlRecordError.BlankRecord` with their one-based physical line number, without invoking the decoder or inventing an upstream cause, as the [JSON Lines format](https://jsonlines.org/) requires each record to contain a JSON value. `skip_blank_lines=true` skips them while keeping later line numbers. A normal terminating newline does not create an extra record, and empty input contains zero records.

### Read large files without loading them

A string source already holds the whole input in memory, so it is not a way to read a multi-gigabyte file. For that, JSONL reads from the existing `std.io.BinaryReader`:

```incan
from std.fs import Path
from std.io import IoError
from std.json import jsonl

def scan(path: Path) -> Result[None, IoError]:
    with path.open("rb")? as file:
        for record in jsonl[PackageSnapshot](file, max_record_bytes=1_000_000)?:
            match record:
                Ok(snapshot) => index(snapshot)
                Err(error) => println(error.message())
    return Ok(None)
```

A `File` is a `BinaryReader`, and it is read record by record; physical line numbers survive across buffer boundaries. Any other reader works the same way, so RFC 158's `gzip.reader(file)` and `open_parts(paths)` read compressed and split logs with this call, and an RFC 142 chunk stream such as an HTTP response body reads as an async source. A record longer than `max_record_bytes` becomes `JsonlRecordError.RecordTooLarge` for that line, and reading continues with the next line. Over a reader, the source is a `FallibleIterator` whose advancement fails with `IoError`, so a failed read stays distinct from a bad record, and the loop header's `?` propagates it under RFC 115. The caller owns the reader, here through an RFC 094 `with` block.

### Resume an interrupted import

An import of a 10 GB log that fails at 7 GB should not start again from the first line. The import pairs each record with its position and saves both, and a later run continues after the saved position:

```incan
from std.fs import Path
from std.io import IoError
from std.json import JsonlPosition, JsonlResumeError, jsonl

error ImportError = IoError | JsonlResumeError

def resume_import(path: Path, saved: JsonlPosition, mut catalog: Catalog) -> Result[None, ImportError]:
    with path.open("rb")? as file:
        records = jsonl[PackageSnapshot](file, max_record_bytes=1_000_000, after=saved)?
        for record, position in records.with_positions()?:
            match record:
                Ok(snapshot) => catalog.add(snapshot, position)
                Err(error) => catalog.reject(error, position)
    return Ok(None)
```

`Catalog` is the application's own store, and it saves each record with the position after it. `with_positions()` pairs every record's result with the `JsonlPosition` after it, including after a record that failed, so a resumed import skips every record it already handled. A `JsonlPosition` holds the one-based `line` and `byte_position` at which the next record starts, with a check of the record before it, and has a stable JSON form. The first run reads without `after`; `after=saved` skips the reader to that byte without decoding anything before it and continues line numbering from that line, so later failures still report their physical lines. A `std.fs.File` skips by seeking; a decompressing reader from RFC 158 skips by decoding and discarding, so a compressed log resumes too.

Before reading, `after=` checks that the input still holds the record the position follows. A shorter input, or a different record before the position, fails with `JsonlResumeError.SourceChanged` rather than continuing at a byte offset inside different content:

```incan
error JsonlResumeError:
    Read(cause: IoError):
        message = "The JSONL reader could not be positioned."
        caused_by = cause

    SourceChanged(line: int) = f"The input no longer matches the saved position at line {line}."
```

Records appended after the position are read normally, so resuming a log that has grown since picks up the new lines; the source still ends at the reader's end. The check covers the record before the position, not all earlier content, so an input rewritten further back is not detected. When the catalog stores each record and its position in one transaction, a failure never imports a record twice; an application that saves them separately must tolerate rereading the records after its last saved position.

### Collect all records and failures

To get every good record and every bad one, collect with RFC 142's shared consumer:

```incan
outcome = jsonl[PackageSnapshot](source).collect(item_errors=ErrorPolicy.CollectAll)
```

| Line | Outcome                |
| ---- | ---------------------- |
| 1    | Valid snapshot         |
| 2    | Invalid identifier     |
| 3    | Valid snapshot         |
| 4    | Missing required field |

The outcome is `Collected.Completed(values, failures, progress)`: `values` holds the two snapshots in order, `failures` holds the two typed row failures with their line numbers and original errors, and `progress` counts four records seen. The caller can reject the whole batch, display all failures, or use the successes. No record is committed merely because it decoded. Collection keeps every outcome; there is no hidden cap.

### Distinguish selected input from interrupted collection

An application that selects the first 100 records with `take(100)` has chosen its input. Consuming that prefix to its end completes normally, even if the source contains more records, and does not claim that the rest was validated.

A protective limit is different. `collect(item_errors=ErrorPolicy.CollectAll, max_items=10_000)` stops after 10,000 records if the source has not ended, and returns `Collected.Stopped(values, failures, progress, ConsumptionFailure.Limit(max_items=10_000))` with everything read so far. A failed read over a reader stops with `ConsumptionFailure.Source(error)`, also keeping what was read. Neither outcome is presented as a completed batch, and neither claims that more records exist.

This follows the distinction between explicit iterator scope selection and validation failure policy illustrated by Rust and Python iterators and by Pydantic's opt-in early failure.

## Reference-level explanation

1. Standard typed JSON decoding must expose `Result[Self, JsonDecodeError]`. Callers must not need to enumerate every nested validator's error type or repeat those types in a separate derive annotation. The complete generated signature must be visible in tooling and documentation.
2. JsonDecodeError must distinguish malformed JSON, missing required members, wrong input types, and domain validation failures. The located InvalidValue wrapper must retain a structured document path and the original concrete domain failure. It must preserve the failure's typed payload and hierarchy and expose its upstream relationship through RFC 131's diagnostic interface.
3. Document paths must be immutable sequences of typed member and index segments, with an empty path denoting the root. Formatted text must not serve as the location representation. Derived decoders must construct and prefix paths automatically. Inspection, tooling, and selected serialization must preserve that structure. Available source positions must be separate structured data.
4. Standard typed JSON decoding must collect all independently discoverable input validation failures by default. Independent sibling checks must continue after a field failure. A single failure and a group of failures must share the JsonDecodeError umbrella, and every retained failure must remain individually inspectable and available to combined diagnostic reporting.
5. Inside an explicit RFC 122 FailFast scope, typed decoding must stop at the first observed failure for the whole decoding attempt. It must not decode later siblings, records, or fields to collect more failures, must return that failure through its ordinary case, and must report the checks it did not reach as stopped by policy. Leaving the scope must restore the outer policy, and nested model construction during decoding must inherit the active policy rather than reset it.
6. A decoder promising validated construction must check nested validated fields and the model's declared validation before exposing success. It must not bypass checks because construction was generated or return an invalid model as Ok.
7. A check requiring unavailable decoded values must not be reported as successfully executed. Authored validator control flow must remain ordinary; returning Err ends that invocation. Malformed syntax, source interruption, cancellation, and diagnostic truncation must not be presented as completed validation. Collection limits and coverage gaps must be observable through structured information.
8. Generated integration must use declared validator return types and nested validation or decoding contracts. It must not inspect function bodies to guess possible failures. Original failures must be retained before a textual adapter can discard them; reconstructing error identity by parsing a message is forbidden.
9. Canonical registered-check model construction and canonical newtype `from_underlying` hooks must expose `Result[Self, ValidationError]`. Authored validators may retain precise domain error return types; canonical boundaries capture those failures under ValidationError while preserving original typed identity, payload, hierarchy, and diagnostics. JSON decoding must use recoverable model reconstruction or an explicitly selected construction adapter and retain the validation umbrella under its located JsonDecodeError failure. Ordinary implicit coercion keeps RFC 017's existing behavior; this RFC does not turn implicit construction into Result propagation.
10. Every JsonDecodeError must expose `coverage()`, a structured record of which checks completed, which were blocked by an invalid prerequisite, which were stopped by policy, and which were interrupted, each with its document path. A check that did not run must never be reported as a failure or a success.
11. `std.json` must export JsonDecodeError, the shared JsonError family, InputPath, SourcePosition, JsonlPosition, JsonlResumeError, and the JSONL functions. A SourcePosition must carry the source identity, a one-based `line`, and a one-based `column` counted in Unicode characters; when the exact UTF-8 location is known, it must also carry a one-based `byte_position`. Rendered and machine-readable reports must use the same convention. Array index segments in an InputPath keep ordinary collection indexing and are not source positions. Missing position evidence must be reported as unavailable, never invented.
12. A JSONL source must be an ordinary iterator yielding one `Result[T, JsonlRecordError[E]]` per physical record and must decode a record only when it is consumed. Over a string it must be an `Iterator`; over a `std.io.BinaryReader` it must be a `FallibleIterator` whose advancement fails with `IoError`, so a record failure stays distinct from a failed read; over an RFC 142 `AsyncFallibleIterator[bytes, E]` it must be an async source of the same items whose advancement fails with `E`, reading only as the consumer advances. `std.json.parse_jsonl(source)` must yield dynamic records with `JsonError` causes, and `std.json.jsonl[T](source)` must yield typed records decoded through `T.from_json`.
13. By default a blank or whitespace-only physical record must yield `JsonlRecordError.BlankRecord` with its one-based physical line number, rather than a generic decoder failure or silent skipping. `skip_blank_lines=true` may request skipping; it must not renumber subsequent records. A terminating newline must not create another record, and empty input must contain zero records.
14. A reader-backed source must read incrementally, without loading the whole input, and must preserve physical line numbers across buffer boundaries. It must require a positive `max_record_bytes`; a longer record must yield `JsonlRecordError.RecordTooLarge` for that line, after which reading continues at the next line. Acquiring, owning, and closing the reader belong to the caller.
15. A reader-backed source must provide `with_positions()`, pairing every item, success or failure, with the JsonlPosition after it. A JsonlPosition must carry the one-based `line` and `byte_position` at which the next record starts and a check of the record before it, and must have a stable JSON form. `parse_jsonl` and `jsonl[T]` must accept `after=position` over a `std.io.BinaryReader`; that call must return a `Result` with JsonlResumeError, must reach the position through `skip_bytes` without decoding the input before it, and must continue physical line numbering from it. It must fail with `JsonlResumeError.SourceChanged` when the input is shorter than the position or the record before it differs, and must never continue at a byte offset inside different content. When the position follows a final record that had no terminating newline, resuming must accept a newline as that record's terminator and must fail with `SourceChanged` when the record continued instead. Records appended after a position must be read normally, and the source must end at the reader's end.
16. Collecting JSONL records must use RFC 142's shared consumers, with no JSON-specific collector. Under `item_errors=ErrorPolicy.CollectAll`, `collect` must return `Collected` with the successes in source order and every record failure with its original error and physical line, without implying acceptance or commit. A protective `max_items` limit, a failed read, or cancellation must return `Collected.Stopped` keeping everything consumed. A `take(n)` prefix completes normally for that prefix. Neither must claim validation of an enclosing source or the existence of further records.
17. Aggregate failures must use RFC 131's common hierarchy, traversal, and diagnostic preservation contracts. A runtime-sized collection must not be disguised as the fixed authored `caused_by` tuple. General aggregate representation is owned by RFC 131; this RFC owns the decoder surfaces that use it.
18. Standard typed model decoding must reject unknown fields by default with a distinct UnknownField case retaining the structured input path and accepted input names for that object. Diagnostics must clearly identify the unexpected member, show accepted field names and available source positions, and should offer a correction when a clear match exists without applying it automatically. Model wire aliases must follow the checked decoder contract, and declared dictionary keys must not be mistaken for unknown model fields. An explicit tolerance option may ignore unknown fields but must retain syntax checking and normal validation of declared fields. Unknown-field failures must participate in the same collection of independently discoverable errors as other decoding failures.
19. Ordinary JSON decoding rules must follow established Serde behavior unless this RFC explicitly specifies a different contract. Standard Serde failure categories must be preserved as distinct typed errors with structured locations and clear diagnostics, without text-based reconstruction. Duplicate model-field inputs, including accepted aliases resolving to one field, must report DuplicateField rather than choose a value; diagnostics should identify both occurrences when source information is available. Dictionary and dynamic JSON behavior must retain their separately applicable contracts. The baseline must not weaken typed failure retention, complete validation, or the settled unknown-field and blank-record policies.
20. Deserialize must require `from_json(json_str: str) -> Result[Self, JsonDecodeError]` for both generated and handwritten implementations. A handwritten implementation with any other return type is an ordinary type error, whose diagnostic names the method, its actual return type, and the required one. String errors must never be converted into purported structured or original domain failures.
21. Typed and dynamic JSON decoding must follow RFC 152's depth convention: they must accept `max_depth: Option[int]`, defaulting to 128, where only an explicit `None` lifts the limit, and must report nesting deeper than it as the shared `JsonError.TooDeep(path, max_depth)` case, with the path where the limit was reached, before allocating for the deeper levels. Typed decoding surfaces that case with its identity, as it does `JsonError.Parse`. Dynamic parsing into `JsonValue` must keep its nesting on a heap work stack and never recurse on the call stack.
22. All decoder and collector diagnostics must uphold RFC 103's Incan-wide secret non-disclosure invariant, including failures before secret construction. Checked sensitivity must survive derive and error boundaries. Retained failures, source excerpts, upstream messages, paths, and metadata must not bypass secret protection or create ordinary plaintext copies solely for reporting. Details that cannot be established safe to expose must be omitted or redacted in human diagnostics, machine inspection, verbose output, stack traces, and audit export alike.
23. Dynamic JSON and typed model decoding must reuse the shared JsonError family for underlying JSON failures, preserving case identities, payloads, and diagnostics. Each existing JsonError category must be one native case with the same meaning. JsonDecodeError is the broader typed-decoding umbrella that includes those shared failures alongside located validation failures and aggregation; dynamic values must not acquire model-validation requirements. Dynamic parsing keeps `Result[JsonValue, JsonError]`, and serialization is unchanged.
24. Custom derives that produce decoders must publish their checked validation contracts through RFC 131's derive metadata, preserving sensitivity under RFC 103. A contract the decoder cannot check must be diagnosed rather than guessed from annotations or function bodies.

## Design details

### Ownership of the contracts

RFC 131 owns error declarations, closed error composition, umbrella capture, checked concrete matching, immutable cause relationships, runtime-sized aggregation, origins, stack traces, and combined diagnostics. This RFC consumes those facilities. JSON-specific categories, document path segments, validation scheduling, complete-report defaults, generated decoder signatures, and JSONL record contracts are specified here.

The requirements are one JsonDecodeError umbrella, individually retained failures, structured locations, and complete reporting of applicable checks. The aggregate is the `Multiple` case, inspected through RFC 131's shared aggregate traversal, and coverage is the structured `coverage()` record.

Uniform inspection of one or several retained failures is shared umbrella behavior owned by RFC 131. JsonDecodeError must reuse that contract while preserving its members' typed identity, hierarchy, payloads, causes, and diagnostics. Aggregate membership remains distinguishable from upstream causation. RFC 131 supplies the shared diagnostics() -> DiagnosticReport reader and format_error overloads for one error or an ordinary tuple/iterable of errors. This RFC reuses those readers without a JSON-only failures() API; the typed view of a propagatable aggregate remains part of its own callable contract.

### Validated construction and nested failures

IdentifierError.InvalidSymbol describes why a value is invalid; JsonDecodeError.InvalidValue adds where it occurred in the document. PackageError.BlankTitle describes a model-wide invariant. Both remain available through the same decoder umbrella without an authored union or an extra conversion step. For model-wide validation, a root location is honest unless the validator supplies a more specific field location.

The JSON decoder does not need a distinct associated error type for each model. Its generated boundary remains `Result[Self, JsonDecodeError]` when nested validators change. Such a change can still reject previously accepted inputs or introduce new observable failures; a stable outer type does not promise behavioral compatibility. Authored trait adoptions still obey RFC 098's explicit binding rules.

The general validated-construction umbrella decision is supplied by RFC 131. RFC 085 and RFC 087 registered checks use the same umbrella without an extra Validate derive; model composition must preserve source validation and, for fresh construction inputs, source normalization; stored-state reconstruction follows RFC 173 without replaying that normalization. RFC 122 owns the detailed construction execution questions rather than this JSON RFC. It selects a runtime validation_policy scope, with models declaring rules rather than execution policy. Ordinary library and helper calls, including nested model construction, inherit the active runtime scope; a decoder boundary must not silently reset policy for registered checks. The default is complete reporting under CollectAll; an explicitly active FailFast scope stops the whole decoding attempt at its first failure, and coverage reports the checks it did not reach as stopped by policy. Custom derives publish their checked contracts through RFC 131's derive metadata and preserve original errors.

### Structured locations and coverage

A literal member named "a.b" is one segment and differs from two nested member segments. Integer array indices remain distinct from member names containing digits. Source offsets and line or column information describe a position in input bytes or text; document paths describe data structure. Neither is recovered from a rendered message. Lines and Unicode-character columns count from one, as does the optional `byte_position`, in rendered and structured reports alike; editor adapters convert to other conventions at their own boundary.

Complete validation is bounded by actual input structure and validator contracts. A wrong-shaped subtree can block checks requiring its decoded values while permitting independent sibling checks. Malformed JSON can prevent a schema walk altogether. The structured report must distinguish blocked or interrupted work from completed checks and must not claim to enumerate failures from unexecuted authored code.

### JSONL policy and resource use

Lazy parsing and delayed reporting are independent choices. A source produces one Result item per record; a consumer selects accumulation. Complete JSON document validation does not force a lazy JSONL consumer to read the entire source after its caller chooses to stop.

Collection is not a JSON concern. RFC 142's consumers, available on synchronous and asynchronous sources alike, already define policy-aware collection, retained failures, progress, protective limits, prefix scoping, and interruption outcomes. JSONL sources are ordinary iterators so that those consumers apply unchanged, and `std.json` defines no collector of its own.

Lazy decoding over an already materialized `str` still retains the complete input text; it is not a way to stream a large file. The reader-backed source reads `std.io.BinaryReader` incrementally, preserves physical record locations across buffer boundaries, and distinguishes record errors from source-read failures through the `FallibleIterator` contract. `max_record_bytes` bounds a single record; it is separate from the buffering of the reader and from the outcomes a collector retains. Acquisition, ownership, and closing of the reader stay with the caller, typically through an RFC 094 `with` block.

A JsonlPosition describes where the next record starts in one input, not the application's progress. Resuming goes through `BinaryReader.skip_bytes`, defined by RFC 158, so `after=` takes any reader: `std.fs.File` and `std.io.BytesIO` seek, and a decompressing reader decodes and discards. Checking only the record before the position keeps resuming proportional to one record, not to the input already read. Saving the position after the effects it covers, ideally in the same transaction, is the application's responsibility.

Accumulation can retain memory proportional to input size. Full collection has no hidden outcome-retention cap; protective limits are explicit and stop with everything consumed so far. Completion is relative to the supplied input scope, never evidence that an enclosing source was exhausted. Any streaming delivery of successful values exposes application effects separately from the promise to report failures at the end. An aggregate does not roll back earlier effects.

### The typed contract

Typed decoding is `from_json(json_str: str) -> Result[Self, JsonDecodeError]` for generated and handwritten implementations alike; serialization is unchanged. Canonical registered-check construction returns `Result[Self, ValidationError]` under RFC 122, and canonical newtype hooks keep `Result[Self, ValidationError]` with typed retention under RFC 131. JSON construction uses these recoverable boundaries; ordinary implicit coercion remains governed by RFC 017.

The public JSON entry points are:

- `T.from_json(text)` for typed decoding, returning `JsonDecodeError`;
- dynamic `JsonValue` parsing, returning the shared `JsonError`;
- `parse_jsonl(source)`, yielding lazy dynamic records;
- `jsonl[T](source)`, yielding lazy typed records, over a string or a `BinaryReader`;
- `after=position` on either JSONL function, resuming over any `BinaryReader` and returning JsonlResumeError;
- serialization, unchanged.

Code written against another signature, such as a handwritten `from_json` returning `Result[Self, str]` or a caller expecting `parse_jsonl` to return a list, receives an ordinary type error naming the actual and required types. Rendered text is produced only explicitly, at a presentation boundary.

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
- **Keep decoding independent fields under FailFast.** Rejected: a policy named FailFast that keeps parsing to find more errors does not fail fast.
- **Use zero-based offsets for machines and one-based positions for people.** Rejected: two conventions for one location confuse both; lines, characters, and byte positions all count from one.
- **Define a JSON-specific JSONL collector and outcome types.** Rejected: RFC 142's consumers already define collection, retained failures, limits, and interruption, so a second vocabulary would split the design.
- **Read a JSONL file with `read_text` and parse the string lazily.** Rejected: it loads the whole file; the reader-backed source reads incrementally.
- **Keep the string-returning decoder or a compatibility adapter.** Rejected: there is one typed contract, and a string cannot carry the original error identity.

## Drawbacks

Complete validation requires more work and storage than returning after the first failure. Retaining original error values and successful records extends their lifetimes. Coverage, cancellation, and retention limits require honest reporting rather than hidden truncation.

A stable umbrella simplifies signatures but leaves concrete domain matching open-ended. JSON integration depends on RFC 131's checked retained-error recognition and runtime aggregate contracts, and JSONL collection depends on RFC 142's consumers. The typed decoding contract changes existing decoder signatures and callers.

## Layers affected

- **Stdlib JSON and JSONL APIs:** typed decoder errors, `coverage()`, source positions, lazy string- and reader-backed JSONL sources, their record errors, and JSONL positions and resumption, exported from `std.json`.
- **Stdlib I/O:** `SourcePosition`, defined in `std.io` so that other formats can share it, with `std.json` re-exporting it under the same identity. `BinaryReader.skip_bytes` comes from RFC 158.
- **Derive protocol and validation integration:** checked generated signatures, nested validated construction, FailFast scope inheritance, and preservation before textual adaptation.
- **Typechecker / callable metadata:** visibility of generated Result contracts and type errors for implementations with other signatures.
- **Runtime and diagnostics:** structured input locations, one-based source positions, coverage, and integration with RFC 131 aggregate traversal and retained diagnostics.
- **LSP / documentation:** generated signature help, field-location inspection, and diagnostics.

## Design Decisions

- Standard typed JSON decoding uses the non-generic JsonDecodeError umbrella; callers do not enumerate nested validator failures.
- JsonDecodeError includes the shared JsonError family rather than duplicating parser failures, and adds typed validation and aggregation. Each existing JsonError category is one native case with the same meaning, and dynamic JSON keeps the JsonError boundary.
- `std.json` exports JsonDecodeError, JsonError, InputPath, SourcePosition, JsonlPosition, JsonlResumeError, and the JSONL functions: `from std.json import JsonDecodeError`.
- InvalidValue retains the structured document location and original concrete domain failure together, including its typed payload and hierarchy.
- Ordinary nested matching can recognize a specific retained failure through RFC 131's checked matching contract; callers retain a fallback for unhandled failures.
- Document paths are structured typed segments, never formatted string locations. Source positions are separate structured data with one-based lines and Unicode-character columns and an optional one-based `byte_position`, the same in rendered and machine-readable reports.
- Standard typed JSON decoding collects all independently discoverable validation failures by default. Inside an explicit FailFast scope it stops the whole attempt at the first observed failure and decodes nothing further.
- Every decoding failure exposes `coverage()` with completed, blocked, policy-stopped, and interrupted checks; a check that never ran is never reported as a failure or a success.
- Standard typed model decoding rejects unknown fields by default with a distinct UnknownField case, a structured member path, and accepted input names. `unknown_fields=UnknownFields.Ignore` tolerates them without weakening other validation; declared dictionary keys remain governed by their dictionary type.
- Serde supplies the baseline for ordinary decoding behavior. Distinct typed error categories, structured locations, and clear diagnostics are required. Duplicate model fields report DuplicateField, including multiple accepted spellings for one field.
- Generated decoding checks declared validator and nested type contracts rather than guessing error types from function bodies. Custom derives publish their checked contracts through RFC 131's derive metadata, and an uncheckable contract is diagnosed.
- Typed decoding is `from_json(json_str: str) -> Result[Self, JsonDecodeError]`, the one contract for generated and handwritten implementations; another signature is an ordinary type error. There is no string compatibility adapter.
- Secret non-disclosure follows RFC 103 as an Incan-wide invariant, including input failures before secret construction. Decode and derive integration preserve checked sensitivity, and diagnostic surfaces omit or redact details whose safe exposure cannot be established.
- Canonical registered-check construction and canonical newtype hooks use the ValidationError umbrella, retaining original domain failures. Explicit construction and JSON decoding are recoverable; ordinary implicit coercion retains RFC 017 behavior.
- JSONL sources are ordinary iterators of one `Result` per physical record, decoded only when consumed: `parse_jsonl` for dynamic records and `jsonl[T]` for typed ones, over a string or a `std.io.BinaryReader`. Over a reader they are `FallibleIterator`s whose reads fail with `IoError`.
- JSONL is strict by default: blank and whitespace-only records produce BlankRecord with the one-based physical line, `skip_blank_lines=true` skips them without renumbering, a final terminator is not a record, and empty input contains zero records. A reader-backed record longer than `max_record_bytes` produces RecordTooLarge and reading continues.
- JSONL collection uses RFC 142's shared consumers: `collect(item_errors=ErrorPolicy.CollectAll)` returns `Collected` with successes and every failure, `max_items` stops with everything consumed, and a `take(n)` prefix completes normally. `std.json` defines no collector of its own.
- A reader-backed JSONL source pairs each record with the JsonlPosition after it through `with_positions()`. `after=position` over any `std.io.BinaryReader` resumes without decoding earlier input, continues physical line numbers, reads appended records, and fails with `JsonlResumeError.SourceChanged` when the input is shorter or the record before the position differs. The source does not wait for more lines; it ends at the reader's end.
- Uniform inspection of retained failures reuses RFC 131's shared umbrella and aggregate contract, without a JSON-only `failures()` method.
- Typed and dynamic decoding follow RFC 152's depth convention: `max_depth` is 128 by default, only an explicit `None` lifts it, exceeding it is the shared `JsonError.TooDeep` case, and dynamic parsing never recurses on the call stack.
