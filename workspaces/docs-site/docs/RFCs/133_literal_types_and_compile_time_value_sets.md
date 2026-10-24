# RFC 133: Literal types and compile-time value sets

- **Status:** Draft
- **Created:** 2026-10-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 008 (const bindings and frozen values)
    - RFC 017 (validated construction and conversion)
    - RFC 029 (union types and narrowing)
    - RFC 032 (named value enums)
    - RFC 085 (type-shaped constraints rather than field metadata)
    - RFC 107 (bounded type-use contexts rather than runtime type objects)
    - RFC 131 (typed errors and validation umbrellas)
    - RFC 146 (typed JSON decoding and validation integration)
    - RFC 132 (frozen class fields)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce structural literal types that admit only specified values: `Literal["atoms", "quarks"]`. Permit explicit expansion of supported compile-time collections through `Literal[*LABELS]`, so one const can supply both runtime choices and a checked type. Each Literal admits values of one underlying type. Explicit unions combine literal types from different domains. Literal types work in ordinary type positions, support reusable aliases and finite-set narrowing, and retain their underlying value behavior. Known values are checked statically; runtime input needs a proof from a guard or an explicit fallible conversion. The selected conversion spelling is `from_underlying`, returning a recoverable Result with the ValidationError umbrella rather than adding silent validation panics to literal-typed fields.

## Motivation

An API often accepts only a few values of an otherwise broad primitive type. A string field named `label` does not communicate that only `"atoms"` and `"quarks"` are valid. Repeating a membership test at every boundary duplicates the rule, while a named enum introduces variant names and a distinct domain type that the API may not need.

Incan already has const collections that can serve as a stable source of allowed values. Reusing that source in a type avoids maintaining one list for validation, another for documentation or completion, and a third set of values in a type declaration.

RFC 085 places membership and other value constraints in the type lane, but does not define literal types. RFC 017 supplies validated conversion machinery, while RFC 029 supplies structural unions and narrowing. This proposal adds finite value sets on those foundations. Freezing remains independent: a literal type restricts which values are admissible, and RFC 132 restricts replacement of a class's fields.

## Goals

- Express finite sets of supported values directly in type annotations.
- Reuse compile-time const collections through explicit unpacking.
- Preserve underlying values rather than require user-facing wrapper objects or new enum variants.
- Keep each Literal single-typed and require explicit unions for alternatives from different underlying types.
- Reject statically known invalid values and unsupported type arguments at compile time.
- Support checked runtime input with recoverable conversion and guard-based narrowing.
- Preserve finite-set facts through aliases, imports, matching, and supported decoding boundaries.
- Support large admitted sets with compact checked type uses and fast indexed membership, without an arbitrary member-count cap.

## Non-Goals

- General dependent types or execution of arbitrary code inside a type expression.
- Defining a type from a runtime list, configuration file, or mutable static.
- Replacing nominal enums or validated newtypes.
- Implicitly changing ordinary inference so every string or integer local has a singleton type.
- Automatically freezing a class, its fields, or a collection.
- New parsing between unrelated primitive types, such as converting a string into an integer.
- Implementing the proposal or claiming that these examples already compile.

## Guide-level explanation

The examples describe proposed behavior, including the built-in Literal form, its alias support, and its generated conversion surface. The `from_underlying` spelling and recoverable Result contract are settled; implementation remains proposed.

### Restrict an argument to specific values

```incan
def describe(label: Literal["atoms", "quarks"]) -> str:
    return f"Selected {label}"

def main() -> None:
    println(describe("atoms"))
    println(describe("quarks"))
    println(describe("electrons"))  # rejected: not an admitted value
```

The argument still behaves as a string. There is no `Atoms` wrapper or enum variant to extract before formatting it. A literal value can be passed where its ordinary underlying type is expected; an arbitrary value of that broad type cannot travel in the reverse direction without checking.

### Name the constraint and reuse its source

```incan
const LABELS: FrozenList[str] = ["atoms", "quarks"]
type InputLabel = Literal[*LABELS]

@derive(Frozen)
class InputSpec:
    label: InputLabel
    attempts: int

    def describe(self) -> str:
        return f"{self.label}: {self.attempts} attempts"

def main() -> None:
    spec = InputSpec(label="atoms", attempts=3)
    println(spec.describe())
```

`*LABELS` expands the collection's members at compile time. It is equivalent to writing `Literal["atoms", "quarks"]`. `Literal[LABELS]` does not mean the same thing: an unexpanded collection is not a supported literal member and is rejected.

`InputLabel` is a reusable structural alias, not a nominal newtype. Another alias admitting exactly the same typed values denotes the same literal type. Use a newtype or enum when distinct domain identity is part of the API.

`Frozen` fixes `InputSpec`'s fields at construction. An ordinary class or model can also use `InputLabel`: valid replacement values must still satisfy the literal type, independently of field mutability. This RFC does not extend frozen-class syntax to models.

### Combine different underlying types explicitly

Each Literal contains values of one underlying type. Mixing domains in one declaration is rejected:

```incan
type InvalidCount = Literal["auto", 1, 2, 4]  # rejected: str and int members

type WorkerCount = Literal["auto"] | Literal[1, 2, 4]
```

WorkerCount is an explicit union of a string literal type and an integer literal type. It admits the same intended choices while making the union visible. Operations available only on one underlying domain require ordinary narrowing. The single-type rule applies after const expansion too, and uses type identity: bool and int are distinct domains, as are different enum types. FrozenStr and FrozenBytes from const storage normalize to str and bytes as specified below.

### Check runtime input explicitly

```incan
def make_spec(raw: str, attempts: int) -> Result[InputSpec, ValidationError]:
    label = InputLabel.from_underlying(raw)?
    return Ok(InputSpec(label=label, attempts=attempts))
```

The generated `from_underlying` checks membership and returns `Result[InputLabel, ValidationError]` under this proposal. Invalid input remains a recoverable failure. The conversion checks the existing string value; it does not trim, change case, or parse another primitive type.

The following is rejected because a general string is not proof of membership:

```incan
def unchecked(raw: str) -> InputSpec:
    return InputSpec(label=raw, attempts=3)  # rejected: raw may be any string
```

RFC 017's existing validated-newtype coercions have their own failure policy. This proposal does not silently apply that policy to structural literal types. A call to a literal-typed constructor field or function argument does not itself insert a potentially failing membership conversion.

### Convert a finite union through one entry point

```incan
type WorkerCount = Literal["auto"] | Literal[1, 2, 4]

def parse_count(raw: str | int) -> Result[WorkerCount, ValidationError]:
    return WorkerCount.from_underlying(raw)
```

The alias exposes the same generated membership check. The string "auto" and integer 2 succeed; the string "2" and integer 3 fail through ValidationError. Each input is checked against the literal branches of its existing type. Conversion never parses a string into an integer, and each constituent Literal remains single-typed. This generated surface applies to aliases for finite unions of literal types, rather than granting a validator to every ordinary union.

### Use the literal type in ordinary membership checks

The literal type itself can supply the allowed values for `in` and `not in`, including through the InputLabel alias defined above:

```incan
def main() -> None:
    for label in ["atoms", "electrons"]:
        if label in InputLabel:
            println(f"Supported label: {label}")
        else:
            println(f"Unsupported label: {label}")
```

`"atoms" in InputLabel` is true and `"electrons" in InputLabel` is false. For membership, InputLabel behaves like the tuple `("atoms", "quarks")`, using the checked type's exact typed values. Authors do not need a separate runtime choices list. The same use is available for a direct Literal expression or an alias for an explicit finite union of literal types. `not in` negates that exact membership result.

Typed function parameters and fields remain the primary API contract. Boolean membership checks are available wherever application logic needs a condition; they do not require a wrapper function accepting a broad string solely to validate a typed call.

A successful recognized membership check can also establish the literal constraint for the unchanged tested value within its branch. Equality with a known scalar and membership against a supported const collection retain the same checked narrowing behavior. Replacing the value invalidates that knowledge. Arbitrary user-defined equality, contains methods, or functions returning bool do not establish a finite-set invariant merely by returning true.

Using a finite literal type as the right operand is a bounded type-use context, consistent with RFC 107's distinction between checked type names and general runtime type objects. It does not introduce arbitrary type iteration, indexing, tuple ordering, or mutable access to the admitted set. The check consumes the same checked finite-set facts as static assignability and explicit conversion.

### Match a closed set of values

```incan
def explanation(label: InputLabel) -> str:
    match label:
        case "atoms":
            return "An atom label"
        case "quarks":
            return "A quark label"
```

Both cases cover the complete finite set. Omitting one requires another matching case or a wildcard. A guarded arm does not establish full coverage when its guard might fail.

### Operations can leave the finite set

```incan
def upper_label(label: InputLabel) -> str:
    return label.upper()
```

`upper()` returns a normal string. It does not preserve the input literal type, because the transformed value need not be one of its members. Authors must check a transformed value again before placing it in a literal-typed destination.

### Writable access cannot widen the constraint

Under RFC 129's proposed caller-visible assignment contract, a `mut str` parameter can replace its caller's string. Passing a literal-typed place to that parameter would let the callee store an invalid label:

```incan
def overwrite(mut text: str) -> None:
    text = "electrons"

def invalid_mutation() -> None:
    mut label: InputLabel = "atoms"
    overwrite(label)  # rejected: writable str access could violate InputLabel
```

Reading or transferring the value as a normal string is valid; granting replacement access to the original literal-typed place is not. A callee taking `mut InputLabel` may replace it with another admitted label. The compiler must not silently copy the value to make a caller-visible mutation disappear.

## Reference-level explanation

### Supported members and const expansion

1. The built-in type form `Literal[...]` must accept one or more supported members or expanded supported const collections. The supported domains are strings, bytes, integers, booleans, `None`, and resolved payload-free enum variants. Every member of one Literal must have the same resolved underlying type after the specified frozen-string and frozen-bytes normalization. Mixed types must be rejected, including after const expansion; authors may combine separate literal types with an explicit union. Floating-point values and arbitrary objects must be rejected.
2. A direct scalar member must be a literal expression or a named scalar const whose value is available to the existing const evaluator. Signed integer literals must be accepted. Arbitrary calls, closures, comprehensions, mutable storage, and runtime values must be rejected in the type expression.
3. `*NAME` must resolve to a compile-time tuple, `FrozenList`, or `FrozenSet` with supported members. Expansion must be exactly one level: nested collections are not literal members, and a `FrozenDict` must not be implicitly interpreted as its keys or values.
4. A const expansion must obey existing const-evaluation and access restrictions. This RFC must not make new expression kinds or enum constructors const-evaluable merely to make them usable in an expansion.
5. A literal set must be nonempty after expansion and normalization. An empty expanded set must be rejected; this RFC does not introduce an uninhabited type spelling.
6. A `FrozenStr` or `FrozenBytes` obtained from const storage must contribute its text or byte value to the ordinary `str` or `bytes` member domain. Literal typing must not require runtime input to have static backing storage.
7. An enum member must preserve its defining enum and variant identity. All enum members within one Literal must belong to the same resolved enum type. A value enum variant must not become interchangeable with its associated string or integer merely because those values are equal. None is its own singleton domain and must not appear alongside another domain within one Literal; combinations use ordinary explicit unions or Option canonicalization.

### Identity, aliases, and assignability

1. Literal types must be structural finite sets of values of one resolved underlying type. Order and repeated members must not change the type. Type checking must reject mixed domains before value-based deduplication: `Literal[1, true]` is invalid rather than collapsing to one member. Integer and boolean members in an explicit union remain distinct typed values.
2. `type Name = Literal[...]` must declare an alias for the normalized literal type, including when expansion is used. The alias must not introduce distinct runtime or nominal identity.
3. A known supported value must be assignable to a literal destination exactly when its typed value is admitted. Direct literals and known scalar consts must be checked statically; known invalid values must be rejected.
4. A value of literal type `A` must be assignable to literal type `B` when every member of `A` is admitted by `B`. A broader literal set must not narrow implicitly to a smaller one.
5. Each literal type must be usable as its one ordinary underlying type without validation or wrapper extraction. An explicit union of literal types from different domains must follow existing union rules, including narrowing before operations unavailable on the whole union. It must not be normalized into a mixed-domain Literal.
6. A broad underlying type must not be implicitly converted to a literal type solely because of an expected field, parameter, return, or binding annotation. A statically established membership proof or explicit checked conversion is required.
7. Combining literal types with `|` must preserve their structural union. An ordinary underlying member subsumes its literal subsets: `Literal["atoms"] | str` admits all strings. `None` must follow existing `Option` canonicalization; `Literal[None]` admits only the existing `None` value.
8. Aliases, imports, and public re-exports must retain the complete checked finite set and applicable visibility. They must not erase the constraint or expose otherwise inaccessible enum variants.
9. Read or value assignability must not grant writable widening. A literal-typed place must not be passed or aliased with a writable type that admits values outside its declared set. Caller-visible parameters, receivers, and collection element access must preserve that restriction; no silent copy may detach intended writes to make a call valid. Assignability of a literal element to its underlying type must not by itself imply assignability of mutable containers holding those elements.

### Runtime conversion

The selected generated conversion has the following proposed contract for an alias of one literal type:

```incan
# Generated signature proposed for InputLabel:
def from_underlying(value: str) -> Result[InputLabel, ValidationError]: ...
```

1. Each literal type must expose the generated `from_underlying` conversion through its type expression and aliases, accepting its one underlying domain. This must not introduce a mixed-domain Literal. An alias for an explicit finite union of literal types must also expose one generated `from_underlying` conversion, accepting the ordinary union of its underlying domains. The conversion must check each input against branches of the same resolved underlying type and retain the original typed value. It must not coerce between domains or merge them into one Literal. Ordinary union assignability and Option representation must remain unchanged. This requirement does not add conversion methods to arbitrary unions that include broad nonliteral domains.
2. The conversion must validate exact typed membership and return the unchanged logical value on success. It must not normalize text or perform unrelated primitive parsing.
3. An invalid supported input must return `Err`; the membership check itself must not panic. An argument outside the underlying domain must produce a type error, rather than invent an ambient conversion.
4. The error must retain the rejected typed value and allowed set as structured information through the ValidationError umbrella. Integration follows RFC 131's typed failure and hierarchy contract and must preserve recoverability and available structured locations; rendered text alone is insufficient. Migration from the existing validation representation must not change the finite-set invariant or introduce a panic.
5. An alias must not permit a handwritten replacement validator that accepts additional values. A custom validator may construct a literal-typed result only after satisfying the same finite-set invariant.
6. This RFC must not add a panicking literal constructor or automatic fallible coercion at ordinary assignment or call boundaries. Authors needing a different policy may wrap the recoverable conversion explicitly.

### Narrowing and matching

1. A direct equality test against a supported literal or scalar const must narrow the tested local's supported domain to that value in the successful branch. A negative branch of an existing literal type must exclude the tested member.
2. The `in` and `not in` operators must accept a checked Literal type expression, an alias for one Literal, or an alias for an explicit finite union of literal types as the right operand. They must perform exact typed membership over the checked allowed values and return bool, without coercion between underlying domains or requiring a separately authored choices collection. This is a bounded type-use context, not general runtime reflection. A recognized membership test against that type or a resolvable supported const value set must establish the corresponding literal constraint in the branch where membership holds. For an existing literal type, narrowing must use intersection for membership and difference for nonmembership. Negation must reverse those branch facts soundly. Ordinary operand typing and Option canonicalization remain applicable.
3. Narrowing must respect existing operator typing and type-sensitive member identity. A predicate must not prove integer membership from boolean equality or otherwise weaken ordinary comparison rules. Guard proofs must depend on compiler-known comparison and membership semantics, not arbitrary user-defined equality or a custom `contains` method that can return true without exact membership. Enum patterns or recognized identity tests may establish enum-member facts; an overridable comparison alone must not do so.
4. Proofs must apply only while the tested value remains unchanged. Assignment, caller-visible replacement, or other invalidating access must discard the proof. Control-flow joins must retain only sound common facts, following existing borrowing and union rules.
5. Value patterns must narrow literal-typed bindings and participate in exhaustiveness checking over the finite set. Guards must not count as unconditional coverage. An arm proven impossible should produce a source diagnostic consistent with existing unreachable-code policy.
6. Ordinary unannotated inference must retain its existing broad types. Literal facts established by supported tests may refine them within a branch; this RFC must not silently infer every collection or mutable local as a finite literal set.
7. General arithmetic, string transformation, or user code must not be assumed to preserve a literal set. Results must follow the operation's ordinary type unless checked source facts establish a stronger result.

### Large sets, lookup, and diagnostics

1. Literal types must not impose a fixed semantic cap on the number of admitted members. Large sets, including generated game-action catalogs, must retain exact membership semantics. Overall compiler and host resource budgets still apply and must be diagnosed explicitly rather than implemented by silently dropping members or widening a type.
2. Individual member values must obey documented per-member representation and resource bounds. An oversized member must produce an actionable diagnostic identifying the offending member and applicable limit; truncating a member into a different allowed value is forbidden. Compact checked type-use nodes must refer to the shared finite-set definition rather than inline the entire set at each use. Increasing member count must increase owned backing data rather than the size of every reference to the type.
3. Membership lookup for large sets must use a prepared indexed representation rather than a linear scan of all members on each check. Boolean `in` and `not in` checks must not rebuild the set, copy all its members, or allocate a collection per lookup. Lookup performance must account for input size as well as member count; string or byte processing is not assumed constant regardless of key length.
4. Explicit conversion and supported decoding must reuse the same checked membership facts and prepared lookup representation where applicable. A membership check must not duplicate the full allowed set merely to construct a validation failure; a retained immutable reference or equivalent structured view may expose the complete set. A value already proved to satisfy the type must not require a redundant membership check at an ordinary typed call.
5. Compiled provider metadata and structured diagnostic access must preserve the complete admitted set. Human-facing compiler diagnostics may show a clearly identified summary with the complete set available through structured tooling. Summarization must not change the type, conceal dropped members, or introduce new truncation rules for ordinary error payload display under RFC 131.
6. Compiler resource exhaustion, incomplete metadata, or a failed index construction must produce an explicit failure. The compiler must not emit a partially checked type or fall back to a broader primitive type. Inspection of a large set may use bounded access to the complete shared definition, with any retrieval limit visible.
7. Acceptance must measure membership hits and misses across small and large sets and varying member/input sizes, including repeated game-action-style lookups. Measurements must also report index construction cost, compiler memory, emitted lookup data, provider metadata size, and diagnostic retrieval behavior. Fast lookup must be demonstrated rather than inferred from an algorithm name or a small example. Concrete index selection and tuning remain implementation choices within these requirements.

### Storage, capabilities, and decoding

1. Literal constraints must hold in supported type positions, including fields, parameters, returns, nested collection elements, and `Option` or `Result` payloads. Every value admitted to such a destination must satisfy the same assignability rules.
2. Freezing and literal membership must remain independent. This RFC must neither freeze containing classes nor grant writable access to frozen fields.
3. Formatting, equality, copying, hashing, and other capabilities must follow the admitted underlying domains and existing union rules. Literal typing must not grant a capability the underlying values lack.
4. Where an existing serialization surface supports the admitted underlying type, a literal value must serialize as that underlying value. Enum members must retain their existing enum serialization semantics. Unsupported domains, including domains without an existing JSON form, must remain unsupported.
5. A supported decoder constructing a literal-typed destination must validate membership before exposing a successful value. It must report an invalid member through its existing recoverable decoding contract and preserve available field or element location information. Encoding a type constraint as metadata without enforcing it on decode is insufficient.
6. Provider and tooling metadata must retain the finite-set facts for supported schema and documentation consumers. Projecting those facts must not add a second independently authored set of allowed values.

## Design details

### One source of allowed values

`Literal[*LABELS]` explicitly expands a collection instead of interpreting every collection-valued argument as an implicit spread. Inline members and expansion may be combined, such as `Literal["other", *LABELS]`. The resulting type has set semantics even when the source list has a deliberate presentation order. Every expanded and inline member must resolve to the same underlying type; expansion cannot bypass the single-type rule.

Adding or removing an admitted value changes a public contract. Reordering or repeating values does not. Expanding a const imported from another package makes that dependency part of the checked API; compiled providers must retain the resulting set, not require consumers to run module initialization.

### Literal types, enums, and newtypes

An enum supplies nominal domain identity and named cases. A validated newtype supplies nominal identity and can enforce richer predicates or normalization through its validation hook. A literal type supplies a structural finite value set while retaining the underlying value surface. Authors can select the mechanism that matches their API rather than use a model or class as a stand-in for validation.

RFC 017's validation naming is useful precedent for `from_underlying`, but its automatic coercion policy is not automatically inherited. A structural annotation such as `Literal["atoms", "quarks"]` should not introduce a hidden validation panic when passed a general string. The proposed explicit conversion shares the existing validation vocabulary and provides a recoverable path.

### Prior art

Python's [literal types](https://typing.python.org/en/latest/spec/literal.html) supply the familiar inline spelling and structural value restrictions. Python's `Literal` performs no runtime validation itself. This proposal adds a checked runtime conversion contract and expansion from supported Incan const collections; neither addition should be presented as Python behavior.

### Source compatibility and diagnostics

This is additive syntax and a new built-in type form. Existing broad annotations keep their meaning. Tightening a public field or parameter from `str` to a literal set may reject existing callers; narrowing a published set is a deliberate compatibility change.

Diagnostics should report the expected finite set and the offending typed value where known. For a broad runtime input, they should suggest an explicit conversion or a membership guard, without suggesting that a plain cast can assert an unchecked invariant. Completion should offer admitted values while preserving normal class field privacy.

## Alternatives considered

- **Mixed underlying types within one Literal:** rejected. An explicit union such as `Literal["auto"] | Literal[1, 2, 4]` makes the different domains and their narrowing requirements visible while preserving the intended finite choices.
- **`OneOf[...]`:** describes selection, but can be mistaken for a union of types. `Literal` is familiar to Python users and distinguishes exact values from ordinary type unions.
- **`Literal[LABELS]` with implicit expansion:** shorter, but makes a collection-valued argument silently mean its members. Explicit `*` keeps direct values and expansion distinct.
- **Always declare a value enum:** provides a good nominal domain model, but requires variant names and enum identity when an API only needs constrained strings or integers.
- **Always declare a validated newtype:** supports richer constraints, but adds nominal identity and a custom validator to a finite rule already available to the compiler.
- **Field metadata such as `choices=LABELS`:** attaches validity to one field rather than a reusable type, contrary to RFC 085's separation between metadata and type-shaped constraints.
- **Implicit runtime membership validation everywhere:** reduces conversion syntax, but hides failures inside ordinary calls and assignments. Explicit conversion or a recognized proof keeps failure handling visible.
- **General const expressions inside literal types:** could be convenient, but expands this feature into a broader type-expression evaluator. Named consts can use whatever evaluation the existing const mechanism already supports.

## Drawbacks

Finite-set checking adds more precise facts to the type system and provider metadata. Narrowing must remain sound across replacement and control-flow joins. A declaration parser that currently expects type arguments also needs to distinguish literal values and explicit const expansion.

Large sets can make index construction, metadata, exhaustiveness checks, and diagnostics expensive even when repeated membership lookup is fast. There is no arbitrary member-count cap, but individual members and overall compilation must obey documented resource budgets. Shared definitions keep type-use nodes compact; diagnostic summaries must not erase the complete checked set.

Sharing a const can broaden the impact of a seemingly small data edit. Structural aliases also do not create distinct domain identity: two libraries using the same set have compatible literal types even when their business meaning differs. Authors needing separation should use nominal types.

## Implementation architecture

This section is non-normative. A useful shape is a normalized finite set of typed constant values attached to the resolved type, with source provenance retained for diagnostics. Existing const evaluation supplies expansion inputs. Each Literal retains one checked underlying type; existing union machinery handles explicitly combined literal types from different domains. Static destination checks and supported guard proofs establish membership before ordinary value lowering.

The finite-set definition can own shared immutable member data and a prepared lookup index, while annotations and other checked type uses retain compact references. Small sets may use a different optimized strategy from large ones; the large-set requirement forbids a per-check full scan or collection reconstruction. Index representation is not a new type identity and must preserve order-independent typed membership. String and byte lookup costs include examining input data. Static membership proofs can avoid runtime checking where the invariant is already established.

Each literal type can use its normal underlying representation, while explicit unions of literal types follow the existing closed union representation. Conversion and decoding boundaries must share the same checked set instead of maintaining unrelated validators. A vocabulary spelling alone does not establish the required assignability, narrowing, exhaustiveness, or provider contracts.

## Layers affected

- **Parser / AST / formatter:** literal members, const expansion, aliases, and readable type expressions.
- **Const evaluation / symbol resolution:** safe scalar values, supported collection expansion, visibility, and compile-time dependency facts.
- **Typechecker / flow analysis:** finite-set identity, assignability, ordinary underlying capabilities, narrowing, invalidation, and exhaustiveness.
- **Lowering / emission / validation:** underlying representations and recoverable checked conversion.
- **Serde / derives:** enforced membership at supported decoding destinations, preserved field locations, and underlying wire forms.
- **Provider metadata / LSP / documentation:** imported finite-set contracts, admitted-value completion, diagnostics, and checked schema projection.

## Design Decisions

- Each `Literal[...]` admits values of exactly one resolved underlying type, including after const expansion. Strings, bytes, integers, booleans, None, and payload-free cases of one enum are supported domains. FrozenStr and FrozenBytes const members normalize to str and bytes. Mixed domains use explicit unions such as `Literal["auto"] | Literal[1, 2, 4]`. Boolean and integer domains remain distinct, and separate enum types cannot be mixed within one Literal. Floating-point values and arbitrary objects are excluded.

- Each individual Literal and its aliases expose `from_underlying` as the generated explicit membership check, returning `Result[LiteralType, ValidationError]`. Success retains the unchanged value with its checked literal type; failure retains the rejected value and allowed set as structured validation information. Conversion does not trim, normalize case, parse another primitive type, or introduce implicit fallible coercion. ValidationError follows RFC 131's umbrella contract.

- An alias for an explicit finite union of literal types exposes one generated `from_underlying` conversion over its underlying union. Membership is checked within the input's resolved type; success preserves that typed value. Strings are not parsed into integers, constituent Literal types remain single-typed, and arbitrary broad unions do not gain this generated conversion. The failure uses the same ValidationError umbrella.

- Literal types and their aliases, including aliases for explicit finite unions, may be used directly as the right operand of `in` and `not in`. These are ordinary boolean checks over the exact typed allowed values, with tuple-like membership behavior. They reuse checked finite-set metadata rather than a second authored list. Recognized equality and membership checks also support sound branch narrowing for unchanged values. Arbitrary user predicates do not provide that proof, and the bounded membership context does not make types general runtime collections.

- Literal sets have no arbitrary member-count cap. Individual members obey documented representation bounds, and checked type-use nodes stay compact through shared definitions. Overall compiler resource limits remain explicit failures rather than silent truncation or widening.

- Large-set membership requires prepared indexed lookup without per-check set reconstruction, whole-set copying, or collection allocation. Conversion and decoding share the checked definition, and already established type membership does not require repeated validation. Performance must be measured on repeated hits and misses, large game-action-style sets, and varying key sizes, alongside compiler and metadata costs.

- Human-facing compiler diagnostics may summarize a large set explicitly, while checked metadata and structured tooling retain access to the complete set. This does not change ordinary error payload display or permit partial membership semantics.
