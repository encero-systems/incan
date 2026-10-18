# RFC 131: First-class error declarations

- **Status:** Draft
- **Created:** 2026-10-08
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (explicit Result-based error handling)
    - RFC 017 (validated newtypes and implicit coercion)
    - RFC 024 (extensible derive protocol)
    - RFC 027 (vocabulary registration and desugaring)
    - RFC 029 (union types)
    - RFC 032 (value enums)
    - RFC 050 (enum methods and trait adoption)
    - RFC 070 (Result combinators)
    - RFC 085 (field validation metadata)
    - RFC 087 (named validation and complete model contract composition)
    - RFC 088 (Iterator adapters)
    - RFC 103 (secret values and Incan-wide non-disclosure)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 115 (fallible iteration and combinators)
    - RFC 120 (canonical source symbol identity)
    - RFC 122 (model construction and validation integration)
    - RFC 126 (evidence-backed investigations)
    - RFC 128 (shared callable signature model)
    - RFC 132 (frozen classes for immutable generated error records)
    - RFC 137 (named ordinary enum payloads)
    - RFC 146 (typed JSON decoding and complete validation)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce `error` as a dedicated declaration for closed families of typed failures, with named variants, field payloads, and declarative messages. Error families can nest, select existing cases through `error Name = A | B`, and include existing cases while adding new ones through `extends`. These errors work with explicit `Result`, `?`, pattern matching, and the existing Result combinators. Umbrella capture preserves specific failures and their hierarchy without requiring callers to enumerate every underlying type. Common diagnostics and aggregation preserve original failures, structured locations, and available execution evidence. Library-specific decoding and collection APIs are defined separately.

## Core model

1. An error declaration defines a family of cases. Fields belong inside a named case, including in a family with only one case.
2. A case has an identity and structured payload independent of its human-readable message. Its authored payload fields are fixed at construction and cannot be reassigned.
3. Composition selects existing cases. Extension forms a larger family without changing the parent family's cases.
4. Propagation between compatible closed error sets preserves the original case and payload. Domain translation remains an explicit operation.
5. A payload can retain another error instead of reducing it to text. The common `caused_by()` accessor exposes direct upstream errors through an ordered, read-only borrowed view.
6. Early return and accumulation are explicit consumption policies. A row failure can be retained as data while later rows are processed.
7. Diagnostics are structured evidence as well as readable reports. Human and tooling views must agree on observed failures, available execution relationships, and explicit evidence gaps.

## Motivation

Typed failures are currently often represented by a kind enum and a model containing that kind plus a message. Richer errors require handwritten methods or a parallel collection of payload fields. A dedicated error declaration would keep the case, its payload, and its message together, and make their purpose visible in source.

Callers also repeatedly use transformations such as `.map_err((error) => error.message())`. This produces a readable string but discards the original error's structure. Other transformations replace a lower-level failure with a domain category while discarding the original cause entirely. Neither outcome is necessary when a declared error can retain that cause as typed data.

The same issue crosses generated boundaries. Validation and other library adapters need to group specific failures under familiar umbrella errors without reducing those failures to text. This RFC defines the shared error guarantees. RFC 146 owns typed JSON decoding, its complete-validation default, structured document paths, and the migration of its generated contracts.

Batch processing adds another requirement: one failed item need not prevent processing later items. Users should be able to retain individual failures and report an aggregate after consumption, while distinguishing item failures from an interrupted source. The operation owns its failure policy; error aggregation preserves the outcomes that policy produces.

Governance by design requires more than a useful terminal message. A failure should support questions about its origin, upstream relationships, task or thread handoffs, and the source and executable to which its observations belong. These facts must remain structured and inspectable by humans and tools. Diagnostic completeness can justify runtime and storage overhead; that cost must be measured rather than concealed by silently omitting evidence. RFC 104 owns capability and effect receipts, RFC 120 supplies checked source identities, and RFC 126 owns durable investigation evidence. Error diagnostics must connect to those contracts without inventing authority decisions or execution history from a message.

## Goals

- Provide a dedicated error declaration instead of requiring errors to be authored as models.
- Keep one enum-style structure for single-case and multi-case families.
- Support immutable error records with named payload fields, field defaults, model-style default rendering, and optional checked message expressions.
- Support nested families, reusable subsets, and extension with closed matching semantics.
- Preserve case identity and payload when propagating into a wider error set.
- Retain original errors as typed payloads when translating failures.
- Preserve failure origins and optional stack traces across propagation and wrapping.
- Retain observed task and thread handoff locations alongside failure diagnostics, without presenting them as captured stack frames.
- Support audit-like questions through structured diagnostic facts, checked identity correlations, and explicit coverage or retention gaps shared by human and tooling views.
- Integrate with the `Error` interface, user traits, and generated behavior.
- Preserve concrete failures and structured locations across umbrella and generated boundaries.
- Support lazy row outcomes and explicit accumulation with access to partial successes.
- Provide source-level examples for pipelines, sequential propagation, recovery, and batch collection.

## Non-Goals

- Introduce Python-style exceptions, ambient throwing, or a new unwinding model.
- Redefine `raise` as a failed Result return, or introduce new failure-return or success-return shorthand. Any such syntax belongs in a separate RFC.
- Make ordinary enums extensible or change existing class inheritance semantics.
- Infer omitted fallible return annotations or implicitly widen a declared return contract from the body.
- Guess domain-specific conversions or synthesize missing operation data.
- Make fields named `cause` or `source` acquire an implicit diagnostic role.
- Automatically serialize error payloads or append runtime-owned cause chains and stack traces to a case's own message.
- Retry a failed source poll or pretend that an interrupted batch reached exhaustion.
- Add a dependency on `anyhow` or `thiserror`.
- Define JSON decoder categories, document path APIs, validation scheduling, or JSONL collection helpers. RFC 146 owns that integration.

## Guide-level explanation

All new declarations and integration APIs in this Draft are proposed behavior. Existing `Result` combinators are specified by RFC 070; lazy and fallible iteration are specified by RFC 088 and RFC 115. Examples may assume imports and surrounding application types, and repeated declaration names belong to separate examples rather than one concatenated module. Candidate APIs are identified where their detailed contract remains open. Error payload fields have no field-level visibility modifiers; their inspection follows access to the case.

### Define cases with their fields and messages

```incan
error ConfigError:
    Missing:
        path: Path
        message = f"Configuration file '{path}' was not found."

    Invalid:
        path: Path
        detail: str
        message = f"Invalid configuration in '{path}': {detail}"
```

Construct the case that occurred:

```incan
return Err(ConfigError.Missing(path=config_path))
```

`message` is a rendering declaration, not another constructor argument. Callers can inspect `path` and `detail` independently of the rendered message.

Error cases are immutable records without an additional annotation. Their authored fields are fixed at construction. A mutable binding may be replaced with another error value, but does not permit assigning to a field of the existing value:

```incan
mut failure = ConfigError.Missing(path=config_path)
failure.path = other_path                          # rejected: immutable payload field
failure = ConfigError.Missing(path=other_path)      # allowed: replace the binding
```

This fixes the record's field bindings, rather than recursively freezing every object stored in them. A retained collection or legacy error keeps its own type's mutation rules. Runtime-owned diagnostic initialization is separate from authored payload mutation.

A message declaration is optional. Without one, the case reuses model-style structural rendering, as provided by the existing Debug-based derived Display behavior:

```incan
error ParticleError:
    InvalidInput:
        atoms: int
        quarks: int
```

The proposed default message for `ParticleError.InvalidInput(atoms=2, quarks=3)` is:

```text
InvalidInput { atoms: 2, quarks: 3 }
```

The case name takes the place of the model name, and the fields follow the existing declaration order, field formatting, and Debug capability requirements. An explicit message replaces this default. This RFC introduces no separate truncation, placeholder, or field-formatting policy for errors. Runtime-owned diagnostic metadata is not an authored payload field and is not appended by the structural fallback.

The declaration automatically supplies `message()` for callers using the common Error interface. It is a generated accessor, not an alternative authored message implementation. For complicated formatting, the expression can call an ordinary helper:

```incan
message = describe_invalid_input(atoms, quarks)
```

`describe_invalid_input` is an application helper that returns str. New error declarations must not define a handwritten `message()` override; existing legacy types implementing Error retain their methods. The author-facing construct remains `error`, independently of how its generated behavior is implemented.

A family with one case uses the same structure:

```incan
error SessionStateError:
    NoActiveSession:
        message = "No session is active."
```

`SessionStateError.NoActiveSession` is a payload-free value, following ordinary unit-variant construction. No base family or `extends` clause is required.

Authors can deliberately preserve an older case spelling using the existing enum-style variant alias form:

```incan
error ParticleError:
    InvalidInput:
        atoms: int
        quarks: int

    BadInput = alias InvalidInput
```

Here `BadInput` names the same case as `InvalidInput`; it introduces no additional failure category, payload, or message. Construction and matching through either name refer to that one case. This is proposed integration of existing alias semantics into error declarations, not an automatically created alias when a name changes.

### Choose the public namespace and visibility

An error's API includes its accessible namespace, family and case names, and visibility. Error family declarations use the existing `pub` and private declaration conventions; they do not become public merely because they describe failures. Case access follows the family's enum-style visibility. Callers with access to a case can inspect its authored payload fields and bind them in patterns without field-level `pub` declarations. Error fields do not inherit ordinary class fields' private-by-default access rules. Their immutable record semantics apply independently of accessibility. Aliases and re-exports follow the ordinary visibility and target checks.

A library can move a definition while deliberately retaining the old public access path:

```incan
# src/new_errors.incn
pub error ParseError:
    InvalidInput:
        detail: str


# src/errors.incn: the existing public module
pub from new_errors import ParseError
```

Consumers can continue importing `ParseError` from `errors` and referring to `ParseError.InvalidInput`. The old module exposes the same target rather than a copied error family. Removing that public binding or changing its visibility is a deliberate API change. This proposed error integration reuses the existing re-export mechanism; it does not introduce error-specific relocation behavior.

Private declarations remain subject to ordinary access checks. A public alias or re-export cannot bypass those checks to publish a private target. Two separately defined cases with the same short name in different namespaces remain distinct cases; an alias explicitly names one existing definition.

### Organize related cases with nesting

```incan
error SessionError:
    UnknownTable:
        name: str
        message = f"Table '{name}' is not registered."

    NoActiveSession:
        message = "No session is active."

    error Backend:
        Planning:
            detail: str
            message = f"Backend planning failed: {detail}"

        Execution:
            detail: str
            message = f"Backend execution failed: {detail}"
```

A function can return the whole family, a nested group, or one specific case:

```incan
def plan_query(query: str) -> Result[Plan, SessionError.Backend]:
    ...


def lookup_table(name: str) -> Result[Table, SessionError.UnknownTable]:
    ...
```

Nesting provides a namespace and a closed grouping. It does not make an error in the inner group a cause of an outer error, capture outer payload fields, or require a backend adapter to depend on a session-owned type.

### Name a subset or combination

```incan
error LookupError = SessionError.UnknownTable | SessionError.NoActiveSession

error QueryError = LookupError | SessionError.Backend.Planning

def prepare(query: str) -> Result[Plan, QueryError]:
    ...
```

The `=` form selects existing cases; it does not copy their definitions or wrap their values. Construction retains the original name, such as `SessionError.UnknownTable(name=name)`.

`error` remains the public declaration form. The compiler may share implementation machinery with ordinary types and unions without making `type` an equivalent spelling for an error composition declaration.

### Extend a family without changing its parent

```incan
error ImportError extends SessionError:
    InvalidSource:
        uri: str
        message = f"Unsupported import source '{uri}'."
```

`ImportError` admits every `SessionError` case plus `ImportError.InvalidSource`. A `SessionError` can propagate into `Result[T, ImportError]`; `ImportError.InvalidSource` cannot propagate into `Result[T, SessionError]`.

The parent remains closed. Adding this declaration does not change the cases a function returning `SessionError` can produce or invalidate an exhaustive match over `SessionError`. Inherited cases retain their defining names and identities.

### Retain the original failure when adding operation information

```incan
error FixtureError:
    ReadFailed:
        path: str
        cause: IoError
        message = f"Could not read fixture '{path}'."
        caused_by = cause


def read_fixture(path: str) -> Result[str, FixtureError]:
    return Path(path).read_text("utf-8", "strict").map_err(
        (error) => FixtureError.ReadFailed(path=path, cause=error)
    )
```

`map_err` receives the error inside `Err`, calls the supplied function, and wraps its returned value in `Err`. It leaves `Ok` untouched. The expression above has the same Result branch and payload behavior as:

```incan
match Path(path).read_text("utf-8", "strict"):
    Ok(text) =>
        return Ok(text)
    Err(error) =>
        return Err(FixtureError.ReadFailed(path=path, cause=error))
```

The original `IoError` is retained as a field. The rendering declaration adds the failed operation's path without automatically appending the cause's message. This Draft does not assign a shared traversal role based only on the field name.

The constructor overload expresses the same mapping without a closure:

```incan
return Path(path).read_text("utf-8", "strict").map_err(
    FixtureError.ReadFailed, path=path
)
```

Supplying `path` leaves exactly one required field, `cause`. The overload checks that it accepts the incoming `IoError` and constructs the replacement only on failure. The closure form remains available for more customized mappings. Both forms are proposed source behavior for declared error case constructors.

There are two ways to establish a diagnostic relationship. `caused_by = cause` declares that a stored field is an upstream error; the compiler checks the declaration, and the actual field value supplies the relationship at runtime. An error-to-error `map_err` conversion also knows which incoming failure produced the outgoing failure, so it is a candidate automatic linkage boundary. The word `cause` in this example is an ordinary payload field, not a reserved keyword or a second diagnostic declaration.

The declaration still matters when constructing a wrapper directly rather than through a combinator:

```incan
return Err(FixtureError.ReadFailed(path=path, cause=original))
```

It also expresses several upstream relationships, such as `caused_by = read_failure, fallback_failure`, where one conversion input is insufficient to describe the diagnostic. Both declarative and conversion-established relationships connect runtime error values. The distinction is where the relationship is specified, rather than whether the error itself exists at compile time or runtime. A relationship exposed through both mechanisms should appear only once in traversal; automatic retention and duplicate handling require the contract described under runtime provenance.

### Diagnose causes and execution locations together

A cause chain explains which failures led to an error. A captured stack trace shows the execution frames present at the capture point. A propagation trail records later return or conversion boundaries. These are related diagnostics, but neither a cause chain nor a propagation trail is a substitute for a captured call stack.

The intended diagnostic experience combines readable causes with execution locations. The following is illustrative output, not a shipped format or an actual recorded trace:

```text
Could not read fixture 'settings.txt'.
  Permission denied.

Captured stack at the read failure:
  Path.read_text  path.incn:18
  read_fixture   fixtures.incn:25
  load_content   content.incn:42
```

The initial failure location must remain distinguishable from the place where a higher-level wrapper was added or the error was eventually displayed. With lazy consumption, each retained item failure has its own origin; a stack captured only when the final aggregate is reported would describe the collector rather than those earlier failures. The aggregate may also have its own location, but must not overwrite the origins of its members.

The first entry of a fresh native error occurrence into `Err` establishes its failure origin and captures a stack when capture is enabled. Full stack capture is off by default and must be straightforward to enable through process runtime configuration. The common `origin()` and `stacktrace()` methods inspect these diagnostics. A failed application entrypoint reports the combined diagnostic automatically; handled failures can use `format_error()` explicitly. Supporting these diagnostics does not introduce exceptions, change `raise`, or require unwinding for ordinary Result failures.

Some failures have a cause only in some circumstances:

```incan
error CodecError:
    TruncatedPayload:
        detail: str
        cause: Option[IoError] = None
        message = f"{detail}"
        caused_by = cause
```

A failed bounded read can construct `CodecError.TruncatedPayload(detail=detail, cause=Some(error))`. A pure byte-length check can construct the same case without a cause. The language must not invent a lower-level error for the second situation.

The explicit `caused_by = cause` declaration accepts an optional error. Some(error) contributes the retained error to the upstream view; None contributes no member. The field remains typed as Option[IoError], while callers of `caused_by()` receive the same ordered, read-only view. The declaration selects the relationship once at construction and must not manufacture an upstream failure when the optional field is absent.

### Compose a pipeline with RFC 070

```incan
error ContentValidationError:
    Empty:
        message = "Content cannot be empty."


error ContentError = FixtureError | ContentValidationError


def require_content(text: str) -> Result[str, ContentError]:
    if text.strip() == "":
        return Err(ContentValidationError.Empty)
    return Ok(text)


def load_content(path: str) -> Result[str, ContentError]:
    loaded: Result[str, ContentError] = read_fixture(path)
    return (
        loaded
        .and_then(require_content)
        .map((text) => text.strip())
        .inspect((_text) => println("Content loaded."))
        .inspect_err((error) => println(error.message()))
    )
```

`read_fixture` and FixtureError are defined above. ContentValidationError and require_content are application definitions in this example. The annotated loaded binding explicitly widens the reader's failure into ContentError before chaining. The example uses existing Result combinators with the proposed error declarations and widening rules.

| Combinator    | Callback result | Effect                                          |
| ------------- | --------------- | ----------------------------------------------- |
| `map`         | A value         | Transform success and preserve failure.         |
| `map_err`     | An error value  | Transform failure and preserve success.         |
| `and_then`    | A `Result`      | Run another fallible operation after success.   |
| `or_else`     | A `Result`      | Attempt recovery after failure.                 |
| `inspect`     | `None`          | Observe success and retain the original result. |
| `inspect_err` | `None`          | Observe failure and retain the original result. |

If reading fails, content validation and trimming do not run. If validation fails, trimming does not run. Either failure reaches `inspect_err` and remains an `Err`. `and_then` retains RFC 070's shared error-type requirement; the new declarations do not silently infer a larger error union for the chain.

An explicit fallback illustrates `or_else`:

```incan
def read_fixture_with_fallback(primary: str, fallback: str) -> Result[str, FixtureError]:
    return read_fixture(primary).or_else(
        (_error) => read_fixture(fallback)
    )
```

This policy attempts the fallback after any primary read failure. If the primary read succeeds, the callback is not called. If both reads fail, the returned failure is the fallback failure; preserving both attempts requires an explicitly defined payload or aggregate.

### Use sequential propagation and local recovery

```incan
error ReadOrDecodeError = IoError | EncodingError

def decode_file(path: Path) -> Result[bytes, ReadOrDecodeError]:
    text = path.read_text("utf-8", "strict")?
    return Ok(hex.unhexlify(text)?)
```

This function needs no domain translation, so its declaration simply admits the original errors. The checked inclusion of each error type makes `?` valid. Existing error types explicitly implementing `Error` can participate as legacy leaves during migration; not every current stdlib error type already implements that interface.

Use `match` when recovery depends on the case:

```incan
match lookup_table(name):
    Ok(table) => return Ok(table)
    Err(SessionError.UnknownTable(missing_name)) =>
        println(f"Choose another table: '{missing_name}'.")
        return Err(SessionError.UnknownTable(name=missing_name))
```

Patterns bind payload fields in declaration order, following enum-style matching. Matching a restricted subset checks only the cases in that subset.

### Capture specific failures under an umbrella

A caller can handle a broad failure family without listing every concrete upstream error. Capture preserves the original family's identity, nested cases, typed fields, and upstream relationships. The following is a candidate standard ValidationError umbrella and an application validator showing that general behavior; the complete stdlib representation remains open:

```incan
from std.traits.error import Error

error ValidationError:
    Invalid:
        original: Error
        message = "Validation failed."
        caused_by = original


error LabelError:
    Blank:
        message = "Label cannot be blank."


def validate_label(value: str) -> Result[str, LabelError]:
    if value.strip() == "":
        return Err(LabelError.Blank)
    return Ok(value)


def checked_label(value: str) -> Result[str, ValidationError]:
    return validate_label(value).map_err(ValidationError.Invalid)


def explain_validation(failure: ValidationError) -> str:
    match failure:
        ValidationError.Invalid(LabelError.Blank) =>
            return "Please provide a label."
        _ => return failure.message()
```

Here `original: Error` denotes an owned retained error exposed through the common interface, not a selected spelling for general trait-object storage. A specific pattern checks concrete identity before exposing typed fields. Patterns follow the explicit retained value structure; they do not implicitly search every cause. The umbrella remains open to other original error types, so a match requires a fallback. This extends beyond RFC 029's closed-union narrowing; its storage and runtime identity mechanism remain open.

Validated construction uses the ValidationError umbrella while authored validators may retain specific domain return types. RFC 085 and RFC 087 registered field and named model checks return `Result[None, E]` and activate canonical `Result[Self, ValidationError]` construction without an additional Validate derive; the original E is retained. Complete model contract composition must retain these obligations. RFC 122 owns construction execution and the settled breaking migration replacing legacy Validate registration with named checks. Legacy checks, transformations, and external operations must be separated explicitly; no automatic adapter or dual dispatch is selected. Canonical constructor callers migrate from the authored error type to the shared validation umbrella with clear diagnostics. RFC 146 specifies JSON integration, including typed nested validation and complete input reporting, without making those library rules part of this declaration syntax.

Canonical newtype `from_underlying` hooks likewise keep `Result[Self, ValidationError]`, with original domain failures retained inside the validation umbrella. Authored helpers may expose precise domain errors before capture at the canonical boundary. Explicit hook calls and JSON integration remain recoverable; ordinary implicit coercion keeps RFC 017's existing failure behavior. RFC 146 records this integration contract and migration from legacy validation representations.

### Retain several failures without discarding successes

An aggregate can retain multiple independent errors as payload. This application-defined example collects already produced outcomes; it is not a proposed standard collector API:

```incan
error BatchError[T, E with Error]:
    FailedItems:
        accepted: list[T]
        failures: list[E]
        message = f"{len(failures)} items failed."


def collect_outcomes[T, E with Error](
    outcomes: list[Result[T, E]],
) -> Result[list[T], BatchError[T, E]]:
    mut accepted: list[T] = []
    mut failures: list[E] = []

    for outcome in outcomes:
        match outcome:
            Ok(value) => accepted.append(value)
            Err(failure) => failures.append(failure)

    if len(failures) > 0:
        return Err(BatchError.FailedItems(
            accepted=accepted,
            failures=failures,
        ))
    return Ok(accepted)
```

Callers can inspect retained outcomes without converting errors to strings. Retaining a successful value does not commit an application effect. Each original failure keeps its own identity, hierarchy, and diagnostics. Runtime-sized aggregate traversal requires a common integration contract; an ordinary list is not accepted by the authored fixed `caused_by` tuple declaration.

Umbrella inspection should provide a uniform, read-only way to visit retained failures whether the umbrella represents one failure or several. This shared contract serves ValidationError, JsonDecodeError, and application-defined umbrellas rather than introducing a separate JSON-only reading mechanism. Inspection must preserve each original failure's typed identity, payload, hierarchy, causes, and diagnostics without consuming or copying the retained values. Aggregate membership and upstream causation remain distinguishable; visiting aggregate members must not implicitly replace them with their deepest causes. The public inspection spelling and interface remain open: neither a `failures()` method nor direct iteration of an error value is settled.

Laziness determines when an outcome is produced. The consuming operation separately chooses whether to return early, collect failures, or ignore them. A failed source advancement remains distinct from a failed item, and cancellation or interruption must not be reported as complete consumption. RFC 115 and RFC 142 own iterator and concurrent consumption behavior; RFC 146 owns JSONL-specific APIs.

## Reference-level explanation

The rules below define the agreed declaration model and required integration outcomes. Candidate signatures in the guide do not settle the open APIs listed at the end of this Draft.

### Declarations, payloads, and namespaces

1. `error Name:` must define a closed family of named cases. A one-case family must use the same structure as a multi-case family; a flat field-only error body must not be accepted.
2. Cases may be payload-free or contain field declarations with explicit types and optional defaults. Payload construction must use the existing field argument and default checking rules. Authored payload fields must be fixed at construction; assignment to those fields must be rejected, including through a mutable binding or a method with a mutable receiver. Replacing a mutable binding with a new compatible error value must remain valid. This field-binding guarantee must not impose deep immutability on retained values. No implicit payload may be supplied by an outer family.
3. Unit cases must be values. Payload cases must expose constructors, support named arguments, and bind fields in declaration order in enum-style patterns. Case names must also be usable in type position to describe a single-case error set.
4. Nested `error` declarations must create qualified family names. The outer family must admit all of its local and nested cases; the nested family must admit only its own descendants.
5. Error declarations must support visibility, generic parameters and bounds, and explicit trait adoption through the existing declaration conventions. Ordinary methods must follow the existing method and receiver rules.
6. The parser must recognize `error` contextually at a declaration boundary without reserving it as an ordinary binding name. Existing expressions, parameters, and fields named `error` must remain valid. Symbol resolution must distinguish family names, case type names, constructors, and instance payload access. Duplicate declarations and unresolvable case paths must produce source-anchored diagnostics.
7. A case alias using `Name = alias Case` in an error family must name an existing case of that family and preserve its identity, payload type, constructor signature, and message. The alias must be usable wherever the case name is accepted, including construction, case type positions, and patterns. It must not add an admitted case or an additional exhaustiveness obligation. Aliases must follow existing name resolution, visibility, target checking, and cycle rejection rules.
8. Error families, compositions, and case aliases must participate in ordinary module imports, symbol aliases, and public re-exports. Declaration visibility must follow the existing `pub` and private rules; case visibility must follow enum-style family visibility. Authored payload fields must be inspectable wherever their case is accessible, including through payload patterns, and must not accept field-level visibility modifiers. Referencing a private target through error composition, extension, or a qualified path must not bypass its access checks. Public aliases and re-exports must obey the same target visibility restrictions as other declarations.
9. Case identity must distinguish separate definitions across namespaces and preserve checked generic type arguments rather than rely on the short case name or payload shape. An alias or re-export must resolve to its target's identity without adding a case. Public access paths and visibility are part of the source API; preserving an old access path after relocation must be an explicit author choice.

### Composition, extension, and propagation

1. `error Name = A | B` must define a dedicated error composition declaration. One or more members must be accepted, so `error Name = ExistingFamily` is also valid. Its members must denote declared error families, individual cases, other error compositions, or concrete legacy types explicitly recognized through the `Error` interface. An arbitrary type with a method named `message` must not qualify solely by that spelling. A concrete legacy type is an opaque leaf in the closed set; a bare trait does not enumerate all present or future implementers.
2. Composition must flatten closed case sets and deduplicate the same case identity. It must preserve original constructors, payloads, messages, and case identities. A composition must not introduce an additional runtime error category or semantic wrapper.
3. Recursive composition declarations and extension cycles must be rejected. Recursive payload types remain subject to the ordinary type and representation rules.
4. `error Child extends Parent:` must form a family containing the parent's closed case set and the child's new cases. It must not add cases to `Parent`, make a child-only case admissible as a parent value, or override an inherited case's identity or message. This rule is error-set inclusion, not a change to class inheritance or ordinary enum syntax.
5. A value whose possible cases are contained in a destination error set must be permitted to widen to that set at an explicitly typed boundary. An explicitly annotated `Result` boundary must permit that checked inclusion for its error component while preserving the success payload. `?` must permit the same checked inclusion when propagating a `Result` failure. No domain translation or string conversion may be inserted as part of that widening.
6. A wider error set must not narrow implicitly. Functions must handle excluded cases before returning a narrower set. The compiler must not enlarge a declared error set based on the function body.
7. Exhaustive matching must use the effective closed set. Extending or composing another family must not change the cases required to match the original family. Original case paths must remain usable after widening.
8. Existing non-error aliases retain their ordinary type rules. This RFC must not introduce `type` as an equivalent declaration spelling for its dedicated error composition surface.

### Messages and trait participation

1. Every declared error case and family, including compositions, must provide the common `Error` message contract without requiring authors to write `with Error` for each declaration.
2. `message = expression` inside a case must be a rendering declaration, not a payload field or constructor parameter. The expression must type-check as `str` in a scope containing the case's payload fields and ordinary resolvable names. Unknown fields must produce declaration-site diagnostics.
3. Rendering must preserve the error payload and must not consume a stored cause. A rendered message must not determine case identity, matching, or propagation compatibility.
4. A composition or extension must preserve the defining case's rendering. A wider view must not silently substitute a different message.
5. A case without a message declaration must receive a default message using the same structural formatting as model-derived Display: the case name, named payload fields in declaration order, and existing field rendering and Debug capability rules. A payload-free case must render its case name. This RFC must not introduce separate truncation or opaque-placeholder rules for error fields. The fallback must use structural formatting directly, rather than calling the error's own Display and recursing through `message()`.
6. An explicit message declaration must replace the structural fallback. Its expression may call ordinary formatting helpers. The `message()` accessor must be supplied automatically for native error declarations; handwritten overrides or competing authored implementations must not replace it. Existing legacy Error implementers retain their handwritten methods. Adoption of additional traits must satisfy the generated Error implementation rather than create a second message-authoring route, and distinct Display customizations must follow the existing display precedence and coherence rules without replacing the generated message accessor.
7. Payloads may retain original errors, optional errors, or aggregates. A field name alone must not give a payload a diagnostic traversal role. Defining `cause: E` must remain useful as ordinary typed storage independently of a `caused_by` declaration.
8. Native error classification and dispatch must use the defining case's typed identity. Diagnostic labels identifying a case must derive from its definition rather than require a separately authored string code. This RFC must not introduce an additional code declaration for error classification. Composition and widening must preserve the original case identity independently of message wording.
9. `caused_by = expression` must explicitly declare upstream diagnostic relationships. The accepted value must be one error, an optional error, or a tuple of errors conforming to the common error interface; tuple members may have different concrete error types. An optional error must contribute its retained member for Some(error) and no member for None, without changing the payload field's Option type. This rule must not erase the separately declared payload types or require a first-class trait-object type in source.
10. A single error must normalize to a one-member cause view. Comma-separated expressions must form the tuple without requiring parentheses: `caused_by = first, second` and `caused_by = (first, second)` must be equivalent. The declaration must be evaluated once when the error value is constructed, and its selected relationships must be retained. Traversal must preserve declaration order and must not permit adding, removing, or replacing declared members, or reevaluate the declaration to select different causes. This membership guarantee must not imply deep immutability of all retained payloads.
11. Runtime diagnostic retention should preserve known upstream failures at supported wrapping, decoding, and aggregation boundaries even without an explicit cause declaration. It must not infer causal relationships solely from field names or nearby control flow. Its interaction with declared causes and the shared traversal interface remains open.
12. Diagnostic metadata must distinguish an error's known failure origin, an available captured stack, and any recorded propagation or conversion sites. A cause declaration must not be presented as a stack frame. Missing, disabled, or unsupported trace capture must not be represented as a complete trace.
13. Propagation and wrapping must preserve available upstream origin and trace information. An additional wrapper location must not replace an upstream failure's origin. Aggregates must preserve each retained member's available diagnostics rather than substituting the final reporting location for every member.
14. The common `Error` interface must expose `caused_by()` as a method returning an ordered, read-only borrowed view of direct upstream errors. Reading the view must not consume or copy the retained errors or permit replacing its members. Each member must expose the common error interface so callers can traverse further upstream. An error with no exposed upstream relationships must provide an empty view. The existing `source() -> Option[str]` contract must remain available for compatibility; an old textual source must not silently become a synthetic error value. The concrete view type and remaining integration rules are open questions.
15. When declared relationships and automatic retention expose the same direct upstream failure occurrence, traversal must expose that relationship once. Equal messages, equal payloads, or a shared case identity must not merge independent failure occurrences. An error-to-error mapping that returns the incoming occurrence unchanged must not create a self-cause. This rule specifies relationship identity without selecting its runtime representation.
16. Automatic upstream retention at supported error-to-error conversions must preserve diagnostics rather than promise retention of the original typed payload. The retained diagnostic record must preserve the original message, available origin and stack information, and upstream diagnostic relationships. When the original error is not retained, its message must be captured before a consuming callback can discard it. Automatically retained entries must support the common diagnostic reading interface without pretending that a snapshot is the original typed error value. Keeping the complete original payload must remain an explicit author choice through a typed field; `caused_by` exposes that retained value for traversal. Automatic retention must not silently add a Clone requirement to the original error type.
17. A fresh native error occurrence must acquire its failure origin when it first enters `Err`; full stack capture, when enabled, must use that same boundary. Constructing an error value for a normal return must not itself mark a failure occurrence. Propagating an existing occurrence through `?` or another `Err` must preserve its original capture. Wrapping may record its own location separately but must not replace upstream origins. Independent uses of a payload-free case must receive independent occurrence diagnostics rather than share a captured stack solely because they name the same case.
18. Full stack capture must be disabled by default in both development and release builds. Available failure origins and upstream diagnostics must remain retained independently of that toggle. The process-wide runtime setting `INCAN_STACKTRACE=1` must enable capture for a supported compiled application without source changes or rebuilding, including when the binary is launched directly. The compiler's run command must offer `--stacktrace` as a corresponding convenience toggle for the launched application. Trace inspection must distinguish disabled capture, unsupported capture, and an available captured stack. Enabling capture must not manufacture stacks for earlier occurrences or promise unavailable source metadata.
19. The common diagnostic interface must expose `origin() -> Option[SourceLocation]` and `stacktrace() -> StackTrace`. A known source location must expose its file and line; unavailable origin metadata must produce None rather than a fabricated location. StackTrace must distinguish Disabled, Unsupported, and Captured(trace). A captured trace must expose read-only frame information and support display using available source metadata; incomplete metadata must not be filled with invented source names or locations. Reading these accessors must inspect retained diagnostics rather than capture a new stack at the inspection site. Native error authors must not need to declare diagnostic storage fields or implement the accessors themselves. Concrete diagnostic type placement and legacy integration remain open.
20. A standard `format_error(error)` helper must return a human-readable combined diagnostic containing the error's message, upstream failures, available origins, and captured stacks. It must inspect retained diagnostics rather than capture a new stack at the formatting site. A Result-returning application `main` that returns Err with an error supporting the common diagnostic interface must automatically report that diagnostic to stderr and terminate unsuccessfully. This behavior must apply to synchronous and asynchronous entrypoints. Handled failures must remain under application control; constructing or propagating Err inside an ordinary function must not itself print a report. Ordinary Display and `message()` behavior must remain unchanged. Existing entrypoints using non-Error Result payloads must remain valid and retain their existing reporting contract.
21. Supported task and thread boundaries must retain observed creation and result-handoff locations for a failure when that metadata is available. These records must preserve the failing worker's origin and stack and be labeled separately from captured stack frames. An unobserved boundary or unavailable mapping must remain an explicit gap rather than an inferred caller or causal connection. Native error authors must not need additional declarations to participate.
22. Failure diagnostics must provide structured facts usable by human-facing reports and tooling without requiring callers to parse formatted text. Available failure occurrence, task or thread, execution, source, and artifact identities must retain their owning contracts and correlation evidence; a source name or line number alone must not establish cross-build identity. Capture coverage, unavailable information, and any sampling, loss, redaction, or truncation must remain observable. Diagnostic export must respect declared sensitivity and redaction and must not automatically serialize arbitrary payload fields. Error diagnostic records must integrate with RFC 104 receipts and RFC 126 investigation evidence rather than claim authority decisions, durable retention, or complete execution replay solely from a captured stack. The shared data and export interfaces remain design work.

### Result and generated boundaries

1. RFC 070's callback contracts, branch behavior, evaluation order, and observer borrowing guarantees must remain unchanged. `inspect_err` must not recover, and `map_err` must not invent data for the replacement error.
2. Result must retain its existing generic behavior. Introducing an error declaration must not invalidate existing `Result[T, str]` merely because str is not an error composition member.
3. Umbrella capture must preserve the original error's concrete family and case identity, typed payload, nesting, upstream hierarchy, and available diagnostics. Callers must be able to handle the umbrella without enumerating all retained error types.
4. Ordinary nested error patterns must support checked recognition of a retained concrete failure and bind its typed fields only after identity is established. Open umbrella contents require a fallback. Matching must follow explicit value structure without implicitly searching a cause chain.
5. Canonical construction activated by registered validation checks must expose `Result[Self, ValidationError]`, using a non-generic validation umbrella. Authored checks return `Result[None, E]` with precise domain failures retained under that umbrella. RFC 122 specifies the breaking replacement of legacy Validate registration and migration diagnostics for declarations and constructor callers. Library-specific decoding and construction integration is specified separately in RFC 146.
6. Generated error boundaries must preserve checked public callable contracts and original error payloads before textual adaptation can discard them. Tooling and documentation must expose their complete Result signatures. The derive protocol's concrete shared contract remains open.
7. Locations accompanying retained failures must remain structured data, independently inspectable from messages. Display may render them as text; inspection, tooling, and selected serialization must not recover location structure by parsing rendered text. Domain-specific path and position APIs belong to their owning integration.
8. Error serialization must be opt-in and obey the chosen serialization trait's payload constraints. Derives must not automatically expose causes, input values, or diagnostics that were not selected for the wire representation.
9. `map_err` must support a declared error case constructor with explicitly supplied payload arguments as an alternative to its existing callback form. After supplied arguments are bound, exactly one required constructor field must remain, and it must accept the incoming error type. Zero or several remaining required fields, incompatible types, and a family name without a selected case must be rejected. Optional fields not supplied must use their declared defaults. The constructor must be invoked only for Err; ordinary supplied argument expressions must retain normal call evaluation behavior.

### Lazy evaluation and aggregation

1. Error declarations and aggregation must not silently change `?`, existing Result combinators, or early-return terminals into collect-all operations. Failure policy belongs to the consuming operation.
2. A retained failed item must remain distinct from a failed source advancement. Lazy production must not imply that unconsumed items were processed or validated.
3. An aggregate must retain each original failure's identity, typed payload, hierarchy, structured locations, and available diagnostics. Retained successes must remain accessible when the operation promises partial outcomes; their retention must not imply acceptance, commit, or rollback of effects.
4. Runtime-sized aggregate members must participate in common inspection and combined diagnostic reporting without being flattened into strings or replacing their origins with the aggregate's reporting location. The concrete traversal integration remains open.
5. Interruption, cancellation, limits, and truncated retention must remain distinguishable from completed collection. An operation must preserve the source failure and accumulated information promised by its callable contract, without implicit source retries.

## Design details

### Dedicated errors and existing enums

The public forms are `error Family:`, `error Selection = ...`, and `error Child extends Parent:`. All defining bodies use variants. An underlying representation may resemble enums, unions, and ordinary aliases, but that does not make errors ordinary type aliases in the source language.

Named field blocks are new syntax for errors. RFC 137 separately proposes compact named payloads for ordinary enums, such as `Finished(path: str, size: int)`. The shared field names, construction, matching, and metadata requirements should agree where applicable, while error declarations retain their dedicated case blocks and error-specific guarantees. Neither proposal makes ordinary enums extensible.

### Methods, derives, and custom messages

Error families should participate in existing trait adoption and derive checking through their declared payloads. `Clone`, equality, formatting, and serialization cannot be available regardless of payload capabilities. Error composition also needs a specified common-method surface; it must not promise a method merely because one member supports it.

A declarative message should be evaluated when rendering is requested, so construction stores typed data rather than a preformatted string. Ordinary helper calls make custom formatting possible without field decorators or handwritten message overrides. When no message declaration is supplied, the generated message reuses the structural rendering and payload capability rules already used by model-derived Display. This reuse does not make errors model declarations and does not route the fallback through an Error-to-message Display adapter, which would recurse. Normal Error display behavior can use the generated message. `message()` remains the common reading interface; new error authors supply an expression or accept the default, and must not write a competing method implementation. Existing non-native types implementing Error continue using their methods, while independent Display customizations retain the ordinary display rules.

### Typed payloads and cause traversal

Typed retention is already meaningful through normal payload access and matching. Shared diagnostic traversal uses the common `caused_by()` method, matching the terminology of the declaration. It exposes direct upstream errors through an ordered, read-only borrowed view; it does not consume or copy the retained errors or flatten the entire upstream graph. Each exposed member supports the same common error interface. No exposed relationships means an empty view. The declaration's fixed tuple remains its authored shape, while the accessor provides a uniform reading interface.

The following helper uses the proposed accessor with the existing Error trait:

```incan
from std.traits.error import Error

def show_direct_causes[E with Error](failure: E) -> None:
    println(failure.message())
    for upstream in failure.caused_by():
        println(f"  {upstream.message()}")
```

The current `Error.source()` returns `Option[str]`, so it cannot expose a retained error value. It remains available for compatibility rather than changing its return type or cardinality. Legacy implementations need an empty default for the new accessor unless they explicitly expose upstream errors; their existing textual source must not be presented as a fabricated error. The concrete borrowed view type, foreign-error adaptation, and runtime-sized cause integrations still require specification. Optional causes use the same view: Some(error) contributes one member and None contributes none.

This Draft reserves no implicit meaning for a field named `cause`, and proposes no `@source` or `@from` field annotations. Their functional goals remain useful: inspect original failures and declare conversions intentionally. The mechanism will be refined after reviewing this Draft.

The earlier FixtureError.ReadFailed case retains its IoError in `cause` and uses `caused_by = cause` to identify that upstream relationship. The field name remains ordinary authored data.

`caused_by` is a diagnostic declaration, not a constructor field. Its accepted shape is conceptually `Error | Option[Error] | Tuple[Error]`: one error, an optional error, or a tuple whose members provide the common error interface. Some(error) contributes one upstream member; None contributes none. This normalization preserves the authored optional field type and selects membership once at construction. This describes the accepted values rather than selecting trait-object or homogeneous tuple representation. `caused_by = original` is equivalent to `caused_by = (original,)`. Several causes use `caused_by = first, second`; parentheses remain valid but are unnecessary. Tuple members may retain different concrete error types. The cause view is ordered and immutable: traversal cannot append, remove, or replace its members. Tuple membership alone does not promise deep immutability of every retained payload.

The declaration is evaluated once when the error value is constructed, and fixes its declared upstream relationships. The error record's immutable fields prevent replacing a retained upstream payload field afterward. Computed expressions must likewise retain their selected causes rather than choose new ones on each traversal. The borrowed traversal interface still needs an ownership and lifetime contract for exposing those retained relationships. Messages on retained error values remain evaluated when rendering is requested; automatic diagnostic snapshots require the capture rule below.

Runtime-dependent aggregates need a separate mechanism; the explicit declaration does not accept arbitrary lists or require authors to expand a runtime collection into a statically sized tuple. Runtime and generated integrations should retain available upstream diagnostic information even when the relationship is not declared through `caused_by`. The exact aggregate mechanism and concrete borrowed view type remain open; optional single-error normalization is settled.

An explicit declaration identifies relationships from authored data. Runtime retention identifies relationships already known at supported failure boundaries, such as wrapping an incoming Result error or retaining failures during decoding and collection. It must not attach every recently encountered error to an unrelated failure. Automatic retention preserves a diagnostic record: the original message, available origin and stack information, and upstream diagnostic links. It does not guarantee that the original typed payload survives an arbitrary consuming callback or impose a hidden Clone requirement. Keeping that payload requires an explicit typed field, with `caused_by` declaring its diagnostic role. When explicit relationships and automatic retention identify the same direct upstream failure occurrence, traversal exposes that relationship once. Independent failures remain distinct even when their messages, payloads, and case identities are equal. Ordering between distinct declared and automatically retained relationships remains open.

If a consuming conversion does not retain the original error, its message must be captured before the callback can discard it. This is an explicit exception to rendering a live retained error's message on demand. A diagnostic snapshot supports message and upstream traversal through the common interface, but does not claim to expose the original error's typed fields or provide a reconstructed value for matching. Available upstream origins and captures are preserved rather than replaced with the translation location. The storage representation remains implementation work; the source contract must distinguish automatic diagnostic retention from explicitly retained typed payloads.

The intended first automatic linkage boundary is error-to-error `map_err`: the incoming Err and the produced replacement identify the actual conversion relationship without inspecting field names or guessing from control flow. Repeated conversions can preserve an upstream diagnostic chain. A declaration is still needed to expose stored relationships in a directly constructed error and to identify several independent causes. The compiler checks those declarations and conversion types; traversal follows retained runtime values or diagnostic records rather than manufacturing typed payloads. Case identity identifies a declared error category; occurrence identity distinguishes individual failures of that category. Mapping a failure to that same occurrence must not create a self-cause. The representation of occurrence identity, treatment of arbitrary non-error Result payloads, and concrete ownership of borrowed cause views remain open.

### Failure origins, stack traces, and propagation sites

The desired end state is an error diagnostic that explains both why an operation failed and where the failure occurred, with Incan function names and source locations when the relevant metadata is available. Cause relationships and execution traces must remain distinct. Metadata should not affect case identity, error-set membership, payload constructor arguments, or the case's own `message()` rendering. Serialization must not include it automatically.

The first entry of a fresh native error occurrence into `Err` captures its origin and, when enabled, its stack. Propagation through `?` or another `Err` preserves an existing occurrence's capture. Error wrapping preserves upstream captures and may record its own conversion location separately. Capturing only at the final display site is too late to recover frames from earlier execution. Capturing a fresh full stack at every propagation site can instead obscure the original failure and add unnecessary work. Separately recorded conversion or propagation sites must be labeled as such.

Constructing an error value for later use or returning one normally does not itself mark a failure occurrence. In a helper that constructs an error before a caller returns it through `Err`, the origin identifies the caller's failure boundary rather than the helper's construction site. A stack captured there does not include helper calls that have already returned. A payload-free case can be used on several independent occasions; fresh failures need independent occurrence diagnostics, while propagation preserves an already-established occurrence. Native interop and generated validation still require specified integration rules. When a foreign error lacks an origin trace, a later capture can identify the adaptation boundary but cannot reconstruct the unavailable foreign failure stack.

Runtime-dependent aggregates may retain several independent cause chains and captures. A single stack attached to the aggregate cannot describe all prior row failures. Async tasks, thread boundaries, and lazy consumption can separate a logical operation path from the native stack available at one capture point; diagnostics must not claim that one is the other. Supported task and thread boundaries retain observed creation and result-handoff locations when metadata is available, alongside the worker's preserved origin and stack. A report labels those handoffs separately. Missing boundary observations or mappings remain explicit gaps; a diagnostic must not invent a connection merely because two tasks ran near one another.

Full stack capture is off by default in both development and release builds. Available origins and upstream diagnostic links remain retained independently. Capture must be easy to enable through a process-wide runtime setting, including for directly launched compiled binaries, with a corresponding convenience option for `incan run`. Enabling capture on a supported binary requires neither an application source change nor rebuilding. Inspection must distinguish disabled, unsupported, and captured traces, and must not claim source locations that the build or runtime cannot supply. Earlier occurrences remain unchanged; enabling capture cannot reconstruct a stack that was never captured.

The toggle spellings are `--stacktrace` for `incan run` and `INCAN_STACKTRACE=1` for the application process. These are proposed user-facing controls, not existing CLI functionality. `pipeline` denotes the application's compiled executable:

```bash
# Enable capture when running Incan source through the compiler.
incan run --stacktrace main.incn

# Enable capture when launching the compiled application directly.
INCAN_STACKTRACE=1 ./pipeline
```

The common inspection methods are `origin() -> Option[SourceLocation]` and `stacktrace() -> StackTrace`. SourceLocation exposes a known file and line; missing origin metadata yields None. StackTrace has Disabled, Unsupported, and Captured(trace) cases. A captured trace provides read-only frame information and display using whatever source metadata is available. These are inspections of the retained failure occurrence, not requests to capture a fresh stack at the current call site. Native error authors do not declare storage fields or write these methods.

The following application helper demonstrates the proposed accessors. Error is an existing trait; StackTrace and the proposed `std.diagnostics` module placement are new API proposals:

```incan
from std.traits.error import Error
from std.diagnostics import StackTrace

def show_diagnostics[E with Error](failure: E) -> None:
    println(failure.message())

    match failure.origin():
        Some(site) => println(f"Origin: {site.file}:{site.line}")
        None => println("Origin unavailable")

    match failure.stacktrace():
        StackTrace.Captured(trace) => println(trace)
        StackTrace.Disabled => println("Stack capture was disabled")
        StackTrace.Unsupported => println("Stack capture is unsupported")
```

Configuration precedence and invalid-value diagnostics, ownership of retained traces, source mapping, concrete frame and diagnostic type placement, and legacy integration remain open. This RFC sets the preservation and availability requirements without selecting a new debugger or changing panic semantics.

### Combined diagnostics and application boundaries

A Result-returning application entrypoint reports a failure automatically. When `main` returns Err with an error supporting the common diagnostic interface, the process boundary prints the combined diagnostic to stderr and exits unsuccessfully. This applies to both synchronous and asynchronous entrypoints. Applications do not need to manually walk causes or call a formatter merely to obtain a useful terminal failure report. A minimal proposed program can deliberately report a startup failure:

```incan
error StartupError:
    MissingConfiguration:
        path: str
        message = f"Required configuration '{path}' is missing."

def main() -> Result[None, StartupError]:
    return Err(StartupError.MissingConfiguration(path="settings.json"))
```

The runtime reports that error's message, upstream diagnostics, origins, and any captured stacks using the same standard renderer available to application code. `format_error(error)` returns that human-readable report as a string. The helper and its candidate `std.diagnostics` module placement are proposed APIs:

```incan
from std.traits.error import Error
from std.diagnostics import format_error

def log_failure[E with Error](failure: E) -> None:
    println(format_error(failure))
```

The formatter reads retained diagnostics and must not capture a new stack at the logging site. Error authors still supply each case's own message; neither `message()` nor ordinary Display automatically expands into the combined report. Application code that handles a failure chooses whether to recover, print a short message, or request the full diagnostic. Returning or propagating Err inside an ordinary function does not itself print anything. The entrypoint behavior does not introduce Python-style exceptions or revisit the separate decision about `raise` semantics. Existing Result entrypoints whose error payload does not implement Error retain their existing reporting contract.

### Governance and inspectable evidence

Diagnostic completeness and auditability are acceptance requirements. The runtime may spend CPU and storage to preserve useful origins, upstream links, and supported handoff observations; performance evaluation must measure that work and disclose its coverage. Full stack capture remains opt-in under the settled toggle policy. A formatted report is a human view of structured diagnostic facts, not the only copy of those facts. Tools must be able to inspect available failure occurrences and relationships without extracting them from prose.

The evidence should support questions such as which failure occurrence began a chain, which worker produced it, where that worker was created and its result handed back, which source and executable the observations describe, and which parts of the history are unavailable. Case identity follows checked declarations and namespaces; occurrence identity distinguishes repeated failures. RFC 120's compilation-specific identities must be correlated with the relevant source and artifact evidence rather than assumed stable across edits or builds.

RFC 104 supplies authority decisions and effect receipts; an error record may correlate with them but must not infer an operation was authorized or completed merely because an error mentions it. RFC 126 supplies durable evidence and investigation contracts. A transient diagnostic is not itself a durable audit record, and a captured stack is not complete execution history or replay. Capture coverage, sampling, loss, truncation, redaction, and unavailable correlations must remain visible. Structured export respects declared sensitivity and selected exposure rather than automatically dumping retained payloads. The shared record, correlation, and export interfaces require further design with those owning RFCs.

Secret non-disclosure follows RFC 103 as an Incan-wide invariant across diagnostic capture, retention, traversal, formatting, stack traces, and human or machine export. Retaining an original typed failure does not authorize revealing secret payloads or copying them into ordinary diagnostic strings. Source excerpts, upstream messages, and diagnostic metadata must obey the same boundary, including failures before a secret wrapper exists. If a detail cannot be established safe to expose, the diagnostic must omit or redact it. Verbose output and audit completeness cannot override this invariant; reports must make redaction observable without disclosing the protected material.

### Conversion ergonomics

Closed-set widening requires no user conversion closure because it preserves the case. Wrapping adds a new case and possibly new data, so it needs an explicit mapping. A path-bearing read failure cannot be synthesized from an `IoError` alone, and two possible wrappers for the same source type cannot be chosen arbitrarily.

The current `map_err` callback form remains valid and reviewable. The constructor overload supplies operation data explicitly and applies a checked binding rule. No implicit `str(error)` fallback or guessed domain wrapping policy is selected here.

The overload accepts a specific error case constructor and explicitly supplied payload arguments:

```incan
result.map_err(FixtureError.ReadFailed, path=path)
```

For FixtureError.ReadFailed, binding `path` leaves one required field, `cause`, which accepts the incoming IoError. The overload constructs that case only on the Err branch. Its binding rule requires exactly one remaining required constructor field and checks its type against the incoming error; zero or several remaining required fields require an explicit closure. Optional fields use their defaults unless supplied. Passing a whole family does not select a case automatically. The existing closure form remains available for transformations and conditional mappings.

The overload extends the accepted call forms without changing existing callback behavior. Ordinary supplied argument expressions still follow call evaluation rules; lazy computation of operation data requires a closure or another explicitly lazy surface. Integration through library or callable machinery is an implementation detail; this Draft specifies the source contract without claiming an existing implementation.

### Umbrellas, generated boundaries, and aggregation

Umbrella capture preserves the original failure's concrete identity, payload, upstream hierarchy, and available origins and stacks. Broad handling and optional specific matching use the same retained error. Adding an underlying failure type does not force callers to enumerate it, but can still change the operation's accepted inputs and observable behavior. A stable outer error type is not a guarantee of behavioral compatibility.

A generic diagnostic snapshot retained from a consuming conversion does not satisfy full umbrella capture: it may preserve messages and provenance without retaining the original typed value. An umbrella promising specific matching must own or otherwise retain that actual value for the required lifetime. Its storage, payload capability requirements, and runtime identity checks remain open.

Generated boundaries must publish checked callable contracts and retain original failures before textual adaptation erases them. ValidationError is the intended standard validation umbrella; migration from the existing generated constructor error type requires an explicit contract. RFC 146 owns typed JSON decoding categories, complete validation reporting, structured document paths, source positions, and JSONL library signatures. Those APIs do not determine the general error declaration or cause-accessor shape.

Locations remain structured data through capture, wrapping, and traversal. Rendering supplies readable text without replacing the retained structure. The consuming domain owns its path and position vocabulary; unavailable positions must remain unavailable rather than invented.

An aggregate preserves multiple failure occurrences and their diagnostic hierarchies. Runtime-sized members need a common traversal mechanism distinct from the fixed authored cause tuple. Aggregation can extend data lifetimes and require memory proportional to retained outcomes. Complete, interrupted, bounded, or truncated collection must remain distinguishable, while the operation owns its policy and callable outcome. A combined error report does not imply that earlier application effects were rolled back.

### Compatibility and migration

This declaration surface is additive. Existing model-based errors, ordinary enums, string error results, explicit matches, and Result combinators remain valid. Existing types that explicitly implement `Error` need an interoperability route as legacy leaves, while their conversion into native declared cases remains a deliberate migration.

Changing the common error interface or generated validation boundary can affect existing code even though the declaration syntax is additive. Those migrations cannot be hidden inside syntax lowering. Existing serialized kind contracts, public message behavior, and declared API error sets must be audited at those boundaries. RFC 146 owns changes to serde and JSON-facing contracts.

Renaming a public error case, removing its public access path, or making a previously public declaration private is a deliberate compatibility change. A move to another namespace changes the API when the old public binding is removed; moving the implementation while deliberately retaining that binding through an alias or public re-export can preserve the source API. This RFC does not hide these choices behind a separate supposedly stable string code or automatically preserve removed names and namespaces. Aliases and re-exports refer to the same target rather than creating a parallel classification system, and must obey ordinary visibility checks. They preserve source bindings and do not automatically promise an unchanged serialized tag or rendered message.

Adding a case to an existing public closed family can require downstream matches to change. Authors needing a larger family without changing the old contract can use `extends` or composition. Recursive declarations, payload representation cycles, and generic error aliases follow checked type rules rather than opening a family dynamically.

### Prior art

[Anyhow](https://docs.rs/anyhow/latest/anyhow/) provides convenient propagation into a common error type and retains original causes. Its [context API](https://docs.rs/anyhow/latest/anyhow/trait.Context.html) attaches operation information, including lazy construction of that information on failure. This RFC takes inspiration from retaining original failures and reducing repetitive wrapping; it does not select Anyhow's erased container or context spelling as Incan's default.

Anyhow exposes [cause traversal and a backtrace separately](https://docs.rs/anyhow/latest/anyhow/struct.Error.html). Its diagnostics can display both, while capture depends on runtime configuration. Rust's [standard backtrace API](https://doc.rust-lang.org/std/backtrace/struct.Backtrace.html) also distinguishes disabled, unsupported, and captured traces. These distinctions inform the proposed diagnostic contract; neither library is selected as a dependency by this RFC.

[Thiserror](https://docs.rs/thiserror/latest/thiserror/) generates error formatting, source exposure, and conversion implementations from declarations. Its source and conversion attributes identify stored errors and generate mappings; they do not make arbitrary struct fields fail validation. Incan expresses those goals through its own `caused_by` declaration, common `caused_by()` accessor, and checked constructor mapping, with the concrete traversal representation and runtime retention contracts still open.

RFC 070 already supplies the Result composition vocabulary. RFC 115 distinguishes a failed source poll from an ordinary item. This RFC complements both by giving failures a dedicated declaration and showing how per-row failures can intentionally remain items during accumulation.

## Alternatives considered

- **Keep a kind enum plus an error model.** This remains compatible with existing code, but separates each category from its payload and message and makes an error look like general application data.
- **Support both flat record errors and variant families.** Rejected for this proposal: fields directly under `error` make readers determine whether the body describes one payload or a family. A one-case family keeps one structure.
- **Use ordinary `type` aliases as the public error composition spelling.** Rejected for this proposal: `error` expresses the author's purpose and permits dedicated member validation and diagnostics. Internal type machinery may still be reused.
- **Make extending a family add values admissible as the original parent.** Rejected: downstream extension would change the parent's effective cases and exhaustiveness. Extension instead creates a larger child family.
- **Use a universal erased error container for all APIs.** It offers convenient heterogeneous propagation, but loses the closed case contract that supports precise signatures and exhaustive matching. Foreign-error adaptation remains a separate interoperability requirement.
- **Turn retained errors into strings.** Suitable at an explicitly chosen display boundary, but insufficient for typed recovery, field-aware validation, and preservation of original payloads.
- **Choose a wrapper from the source type automatically.** The same source error may mean different domain failures, and operation fields may be unavailable. Any conversion convenience needs an explicit, unambiguous policy.
- **Add separately authored string codes for error classification.** Rejected: declared cases already provide typed identities and checked matching, and a recased string label adds no new classification semantics. Renaming is an explicit compatibility choice; authors can retain an old source name with an alias when they choose to.
- **Make all error collection lazy or accumulating by default.** Rejected: some callers require early return, others require all row outcomes, and laziness alone does not specify either failure policy.

## Drawbacks

The language gains a declaration kind, case types, nested namespaces, error composition, and error-set widening. These concepts require consistent constructors, patterns, diagnostics, and trait behavior across module and package boundaries. A single-case family also requires a family name and a case name.

The shared error contract reaches existing validation, derives, and runtime interfaces. An umbrella reduces signature bookkeeping but does not statically enumerate its retained domain failures for callers. Concrete error inspection therefore needs a checked narrowing contract. Declared contract visibility and migration decisions remain important.

Retaining causes and partial successes extends data lifetimes and increases storage. Diagnostic rendering and serialization must respect explicit payload capabilities and selected exposure. Accumulation adds resource and interruption policies that early return does not need.

## Implementation architecture

This section is non-normative. The source language should retain dedicated error declarations and checked error-set identities even if their runtime representation shares mechanisms with existing enums and unions. Widening must preserve observable values; it need not imply a particular internal layout or promise allocation-free representation for every combination.

Messages, trait participation, and derives should use the existing declaration and protocol machinery rather than a separate collection of ad hoc product-specific helpers. Generated error adapters must retain failures before textual adaptation erases them. Accumulation should be expressible by Incan-authored iterator and collection helpers, with source interruption represented explicitly.

The candidate implementation direction is vocabulary desugaring into generated Incan case classes and an error-backed enum. It separates three responsibilities:

- A shared `BaseError` supplies private diagnostic storage and runtime hooks for origins, stack traces, and upstream provenance.
- A generated concrete class represents each case's typed payload. A proposed `@derive(Error)` supplies its message and declared-cause behavior.
- A proposed `enum Family(Error)` binds the closed family names to those concrete case definitions and supplies family-level dispatch.

A schematic family representation could look like this, assuming its concrete case classes have already been generated:

```incan
enum LoadError(Error):
    InvalidInput = _LoadErrorInvalidInput
    ReadFailed = _LoadErrorReadFailed
```

The right-hand sides identify case definitions, not preconstructed error instances. A payload-bearing entry must expose its named constructor and also resolve in type position to that particular case. For example, `LoadError.ReadFailed(path=path, original=original)` constructs the case, while `Result[Config, LoadError.ReadFailed]` admits only that case. A payload-free entry must instead expose the bare unit value required by the source contract, while remaining usable as a case type. Generating a zero-field class must not make authors call a unit case as a constructor. A family-typed boundary admits the case into the family's representation through the checked widening rules. The generated class names remain implementation details rather than alternative public names.

The `Error` backing requires the common error behavior while retaining each case's concrete payload type. It does not mean that ordinary class inheritance supplies subtyping or that every payload is erased into one generic error object. This direction takes the existing `enum Name(str)` / `enum Name(int)` syntax as a precedent, but extends its semantics: current value enums associate variants with primitive literals, not payload-bearing case definitions, and do not introduce a distinct type for each variant. This RFC does not select general arbitrary-type value enums as an additional source feature.

Current Incan classes automatically require `Clone` and `Debug` support from their fields. The common `Error` interface alone does not establish those capabilities for an arbitrary retained error. The candidate lowering must reconcile those class requirements with error payload capabilities, particularly for foreign errors and runtime diagnostic handles. It must not assume that naming a generated class makes a non-cloneable or non-debuggable payload admissible, or that adding diagnostic storage automatically grants an error new capabilities.

A schematic generated case could have the following shape; the Error derive, BaseError, and error-backed enum in these sketches are proposals, not claims of current supported behavior:

```incan
@derive(Frozen, Error)
class GeneratedErrorCase extends BaseError:
    path: str
```

A reusable class-freezing mechanism is needed by this candidate lowering to enforce fixed instance field bindings through methods, inherited behavior, and aliases. Error cases would be constructed frozen, rather than require callers to freeze them afterward. RFC 132 proposes the general authoring syntax and semantics for frozen classes, including the `Frozen` marker shown here; this RFC specifies their required use for immutable error records. Under RFC 132's proposed inheritance policy, `BaseError` must also be frozen. Shallow freezing can retain a fixed private diagnostic handle whose contents support runtime initialization without replacing the error's payload fields. The exact access mechanism must preserve the contained-state and borrowing restrictions rather than assume unrestricted mutation through a frozen field.

The desugaring must retain each authored `message` and `caused_by` expression for generated behavior. A message declaration becomes a generated `message()` implementation evaluated when requested; it need not add a stored message string. A declared cause is selected at construction and exposed through the generated `caused_by()` accessor; it need not duplicate an upstream error already retained in an immutable payload field. Computed causes must remain retained for later traversal even when they are not separate authored fields. The concrete borrowed view type and ownership representation remain open. Missing messages use the structural fallback over the logical authored fields.

Authors would continue declaring `error` families and their cases; these sketches are underlying implementation forms. Private diagnostic state must not become a public payload constructor argument or appear in default structural messages and serialization merely because the generated class contains fields. Field privacy alone does not select which fields ordinary derives render or serialize, so the logical authored payload must remain distinguishable from implementation state.

Generated accessors or compiler mappings must implement the source-level payload access contract and reject authored field assignment. The field visibility or mutability of an ordinary generated class must not silently determine those properties for an authored error case. Runtime diagnostic fields remain implementation state rather than inspectable error payload; initialization at supported failure boundaries must not change authored fields or replace declared causes.

A module static is a candidate home for tracing policy or a capture provider, rather than one captured stack shared by every failure. Diagnostic state belongs to a failure occurrence: reused unit cases must not inherit an unrelated occurrence's origin or trace. Capture boundaries, enablement, and concurrency behavior remain subject to the preservation requirements and the open stack-trace questions.

The existing vocabulary mechanism can produce class declarations, but this does not establish that the full proposed error grammar, error-backed enum, and semantic contract are already expressible without further integration. Declaration and use-site transformations must agree on case construction, case types, payload patterns, and admission into family representations. Any desugaring must preserve closed case sets, checked propagation, namespace and visibility, original case identities, and diagnostics anchored in authored source. Ordinary class inheritance does not replace error-set inclusion. Representation and integration choices remain non-normative implementation work, rather than additional unresolved source-language questions.

## Layers affected

- **Parser / AST:** dedicated declaration forms, nested families, payload blocks, case rendering declarations, and error case names in type positions.
- **Typechecker / symbol resolution:** closed error sets, composition, extension, checked widening, generic constraints, constructors, patterns, and trait coherence.
- **IR lowering / emission:** preserved case identities, payloads, generated message behavior, and compatible propagation across family views.
- **Stdlib / runtime:** the common error interface, umbrella retention, legacy and foreign adaptation, aggregate traversal, and origin and stack preservation.
- **Derive protocol / serialization:** payload capability checking, preservation at generated boundaries, visible callable contracts, and opt-in wire representations.
- **Formatter:** readable and stable formatting for defining bodies, composition expressions, nesting, and messages.
- **LSP / documentation / signature metadata:** family and case completion, effective error sets, generated signatures, source-anchored diagnostics, and an explicit distinction between Draft proposals and shipped behavior.

## Design Decisions

- `error` is a dedicated public declaration construct; ordinary type machinery may be an internal implementation detail.
- Defining bodies are always enum-style families, including single-case errors. Payload fields belong inside a named case.
- Error cases are immutable records by default, without an additional annotation. Authored fields cannot be reassigned, even through a mutable binding; replacing the binding with another compatible error remains allowed. This does not recursively freeze retained values or prevent runtime diagnostic initialization.
- `error Name = A | B` selects or combines existing cases and preserves their original identities.
- `error Child extends Parent:` forms a larger closed family without changing the parent's admitted cases.
- Nested families provide qualified groupings and narrower signatures; they do not create cause chains or capture outer payloads.
- Messages are rendering declarations, not constructor data. Original failures may be retained in ordinary typed fields.
- Message declarations are optional. A case without a custom message uses the existing model-style Debug-based derived Display behavior for its case name and named fields, including the existing field capability rules.
- `message()` is generated as the common reading interface. New error declarations use `message = expression` or the structural default, with ordinary helper calls for complex formatting; they do not provide handwritten message overrides.
- Error classification uses typed case identity. No additional string code declaration is introduced; diagnostic case labels derive from the defining declaration.
- Renaming a public case is a deliberate compatibility change. Explicit enum-style case aliases can retain older source names without introducing a new case or a second identity.
- Namespace, public access paths, and visibility are part of the error API. Existing symbol aliases and public re-exports can deliberately retain an old public path after a definition moves; private access checks remain in force.
- Error payload fields have no field-level visibility modifiers. They are inspectable wherever the case is accessible; a generated class's private field defaults do not define the authored error's access rules.
- `caused_by` explicitly declares upstream diagnostic relationships. It accepts a single error, an optional error, or an ordered immutable tuple of errors, with no parentheses required for comma-separated members. Its declaration is evaluated once at construction and the selected relationships remain fixed; message expressions remain evaluated when requested.
- An optional error is accepted directly by `caused_by`: Some(error) contributes one retained upstream member and None contributes none. The authored Option field type is preserved, and the selected membership is fixed at construction. Callers receive the same read-only cause view.
- The common reading interface is the method `caused_by()`, not a property or a renamed `source()`. It exposes direct upstream errors through an ordered, read-only borrowed view without consuming or copying them; each member supports further traversal through the same interface. No relationships means an empty view. The existing textual `source()` interface remains available for compatibility.
- Runtime diagnostic retention should preserve available upstream failures without requiring authors to declare every relationship. Dynamic aggregates use a separate mechanism whose contract remains open.
- Automatic retention preserves diagnostics: the original message, available origins and stacks, and upstream diagnostic links. It does not guarantee retention of the original typed payload or silently require Clone. Full typed retention uses an explicit field and a `caused_by` declaration. A message that would otherwise be lost in a consuming conversion is captured before that callback; diagnostic snapshots support common reading without masquerading as original typed error values.
- Duplicate direct relationships are identified by failure occurrence, not message, payload equality, or error case. A declared relationship and automatic linkage to the same occurrence appear once; independent failures with equal messages remain distinct. Mapping an error to the incoming occurrence unchanged must not create a self-cause.
- Cause relationships, failure origins, captured stacks, and propagation sites are distinct diagnostics. Wrapping and aggregation preserve available upstream diagnostics; capture and rendering policies remain open.
- A fresh native failure captures its origin and, when enabled, its stack on first entry into `Err`, rather than ordinary error construction or final display. Propagation preserves the existing occurrence's capture; wrapper locations remain separate. Independent uses of a payload-free case have independent occurrence diagnostics.
- Full stack capture is off by default for development and release builds; available origins and upstream diagnostics remain retained. `INCAN_STACKTRACE=1` enables capture without changing source or rebuilding a supported binary, including direct execution; `incan run --stacktrace` provides the convenience option. Trace availability distinguishes disabled, unsupported, and captured states. Configuration precedence and invalid-value handling remain open.
- Common diagnostic accessors are `origin() -> Option[SourceLocation]` and `stacktrace() -> StackTrace`. SourceLocation exposes a known file and line; missing origin metadata yields None. StackTrace distinguishes Disabled, Unsupported, and Captured(trace), with read-only captured frames and display using available metadata. Access inspects retained diagnostics rather than capturing at the inspection site. Native error authors do not declare storage fields or implement these methods. Diagnostic type placement remains open.
- A Result-returning `main` reports a common-interface error automatically to stderr and exits unsuccessfully, for both synchronous and asynchronous entrypoints. Handled failures can explicitly use `format_error(error)` to obtain the same combined diagnostic as a string. Ordinary Display and `message()` remain unchanged; intermediate Err construction or propagation does not itself print. Non-Error Result entrypoints retain their existing reporting contract. The formatter's module placement remains open.
- Supported task and thread boundaries retain observed creation and result-handoff locations when available, alongside preserved worker origins and stacks. Handoff records are labeled separately from captured frames; missing evidence must not be replaced with inferred causal links.
- Governance by design makes structured diagnostic evidence, shared human/tooling views, and explicit coverage and retention gaps acceptance requirements. Useful provenance can justify measured runtime and storage overhead. Full stack capture remains opt-in. Identity correlation, receipt integration, durable evidence, and sensitivity follow their owning contracts in RFC 104, RFC 120, and RFC 126; a terminal report does not itself establish a complete or durable audit history.
- Secret non-disclosure is an Incan-wide invariant owned by RFC 103. Error retention, upstream traversal, source excerpts, stack traces, verbose output, and audit export must uphold it, including failures before secret construction. Details that cannot be established safe to expose are omitted or redacted.
- Existing Result combinators remain part of the intended source vocabulary. `map_err` additionally accepts a specific case constructor with supplied arguments when exactly one compatible required field remains; existing callbacks retain their behavior.
- Umbrella capture preserves the declared family, nested case identity, typed payload, and upstream hierarchy. Callers may optionally use ordinary nested match patterns to recognize a retained concrete failure; its identity is checked before fields are bound. Open umbrella contents require a fallback. Matching follows explicit value structure without implicitly searching a cause chain. Runtime identity checks extend RFC 029's closed-union narrowing; representation and compiler integration remain open.
- Uniform inspection of one or several retained failures belongs to the shared umbrella and aggregate contract. It preserves the retained hierarchy and distinguishes aggregate membership from upstream causes. RFC 146 consumes this contract; the public spelling and interface are deliberately deferred, with no `failures()` method or direct error iteration selected.
- Locations accompanying retained failures remain structured data through capture and traversal. Readable text is display only; tooling and selected serialization preserve structure. RFC 146 owns document path segments, source-position conventions, and automatic decoder path construction.
- Canonical registered-check construction uses a non-generic ValidationError umbrella; authored checks keep specific domain error types, retained with their typed payloads and available diagnostics. RFC 122 specifies the deliberate breaking migration from legacy Validate registration and constructor return types, with clear diagnostics rather than automatic adaptation. RFC 146 owns JSON integration. The new language version applies the new contract directly: no legacy migration framework, automatic adapter subsystem, or dual constructor dispatch is required. This decision does not remove explicit adaptation needed at a foreign error boundary.
- Canonical newtype hooks retain `Result[Self, ValidationError]` and capture domain failures inside that umbrella. JSON decoding uses recoverable hooks and preserves the retained hierarchy; ordinary implicit coercion continues to follow RFC 017. RFC 146 specifies the integration and validation migration contract.
- New `raise` semantics and failure-return or success-return shorthand are outside this RFC; explicit `Err(...)`, `Ok(...)`, and existing propagation remain the examples' return vocabulary.
- Lazy row outcomes and explicit accumulation belong in the design. Batch failures retain original row failures and partial successes, and source interruption remains distinguishable from completion.

## Unresolved questions

- **Cause-view integration:** what concrete borrowed view type implements the settled `caused_by()` accessor, how are computed expression results retained, how do runtime aggregates populate it, and how are foreign errors and the empty default for existing Error implementers adapted?
- **Runtime provenance:** which additional boundaries retain upstream diagnostics automatically, how are borrowed cause views owned, how are non-error Result payloads treated, and how are distinct declared and automatically retained relationships ordered? Diagnostic rather than implicit full-payload retention and duplicate direct relationships are settled.
- **Stack traces and origins:** what precedence and invalid-value rules define the runtime toggle; where do the diagnostic types and formatter live and how are frames represented; how do native interop and generated validation integrate with first-Err capture; and how are independent aggregate members and async or thread boundaries represented? First-Err capture, default-off full stacks, the `--stacktrace` and `INCAN_STACKTRACE` controls, inspection methods and availability states, automatic entrypoint reporting, and explicit `format_error()` are settled.
- **Evidence integration:** what structured record and export interfaces expose failure occurrences, task or thread handoffs, capture coverage and gaps, redaction, and verified correlations to execution, source, artifact, receipt, and investigation identities under RFC 104, RFC 120, and RFC 126? Structured evidence and retention of supported handoff observations are required; the interfaces remain open.
- **Umbrella representation:** how does an umbrella retain original typed values and implement runtime identity checks for the settled optional nested matching behavior, including legacy and foreign errors? Broad handling, retained hierarchy, and checked payload access are settled.
- **Generated error contracts:** how do custom derives publish checked callable error contracts, how are payload and diagnostic capability requirements checked, and how does the existing generated Validate constructor error type migrate? JSON-specific contracts and migration are owned by RFC 146.
- **Umbrella and aggregate inspection:** what shared interface uniformly exposes one or several retained failures, including runtime-sized aggregates, without consuming or copying them or erasing their typed identity and hierarchy? How are aggregate membership and upstream causes distinguished, and what structured information identifies completed, interrupted, or truncated retention? The public spelling, whether it uses a method, and any typed library views remain open; `failures()` is only an explored spelling. RFC 146 reuses this contract rather than defining a separate JSON mechanism. Specific iterator, concurrent, and JSONL collector policies belong to their owning APIs.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
