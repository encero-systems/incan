# RFC 141: Type matching with infer and named type operations

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (union types and value narrowing)
    - RFC 048 (checked contract metadata and interrogation tooling)
    - RFC 054 (explicit call-site generic arguments)
    - RFC 071 (pattern alternation and binding agreement)
    - RFC 083 (symbol and method aliases)
    - RFC 086 (structured schema descriptors and adapters)
    - RFC 098 (native associated types)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 107 (type-directed APIs and compile-time type tokens)
    - RFC 128 (one Callable trait and the shared signature model)
    - RFC 140 (typed holes and shared language-server diagnostics)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce type-shape matching using ordinary Incan type notation, with explicit `infer name` captures, `or` between alternative type patterns, and named compile-time type operations defined through matching. The same pattern language supports inspecting the structured field types required by RFC 086 and computing types from statically known inputs. Standard operations include `ElementOf`, `ReturnOf`, `SignatureOf`, and `ParametersOf`. Exact type references retain ordinary resolution, captures never depend on imports or capitalization, RFC 029 value narrowing remains unchanged, and RFC 107 type tokens retain their bounded dispatch role. Scala 3 match types provide the principal design precedent; TypeScript supplies the explicit `infer` capture notation.

## Core model

1. **Match types in their own notation:** `int`, `Option[str]`, and `list[...]` are type patterns when the subject is a reflected type or the input of a type operation.
2. **Captures are explicit:** `list[infer element]` introduces `element`; `list[Element]` refers to an existing type or an already introduced type-pattern capture.
3. **One pattern language, two evaluation contexts:** reflection matching binds inspectable descriptions of types and signature components, while compile-time operations bind the corresponding types and signature components.
4. **Named operations produce checked results:** type operations reduce to types or signature components without executing user code.
5. **Callable projections share RFC 128's contract:** parameter names, kinds, omission rules, and variadic element types must survive projections that claim to preserve them.
6. **Projections do not grant capabilities:** describing a callable or reflected field does not make a value callable, mutable, visible, or available as a new compile-time generic argument.

## Motivation

A schema adapter should be able to distinguish `Option[list[int]]` from `list[Option[int]]` without parsing printed type names. The former allows the whole list to be absent; the latter allows absent elements inside a list. Structured reflection alone supplies the information, but libraries also need a readable way to inspect it.

Generic library APIs have a related need: select a collection's element type, a callable's result, or its full caller-facing signature. Duplicating such relationships in annotations makes signatures harder to maintain. Named operations express those relationships directly.

Bare captures such as `list[element]` create a collision if an imported type also happens to be named `element`. Treating unknown names as captures can also turn a misspelled type into a successful broad match. Explicit `infer` preserves compact exact patterns while making every new binding intentional.

## Goals

- Match structured types using normal type syntax and explicit captures.
- Support nested patterns, wildcards, and repeated references to captured types.
- Expose reflection matching without string parsing or constructing a sample row.
- Define readable, user-authored compile-time type operations.
- Provide standard element, return, signature, and parameter projections.
- Preserve callable contracts through projections and package boundaries.
- Diagnose unresolved names, unsupported inputs, ambiguous callable surfaces, and non-terminating operations.
- Keep reflection matching, value narrowing, and type-token dispatch consistent without conflating their guarantees.

## Non-Goals

- Replacing RFC 029's value patterns or `isinstance` narrowing.
- Making RFC 107's `Type[T]` tokens general runtime type objects.
- Turning runtime-discovered type descriptions into compile-time generic arguments.
- Executing arbitrary functions while computing a type.
- Introducing general dependent types, variadic generics, or writable type metadata.
- Inferring every generic operation's result from its name or implementation body.
- Defining target-system type mappings in the compiler.
- Redesigning RFC 128's callable compatibility, receiver, or invocation rules.

## Guide-level explanation

The examples describe proposed behavior, not current compiler support. `std.reflection` is the public home for the read-only `TypeInfo` and `ParameterListInfo` descriptions and the standard named type operations. Application types and helpers have ordinary surrounding declarations unless shown explicitly.

### Import reflection surfaces explicitly

The proposed APIs are imported from the existing reflection module:

```incan
from std.reflection import TypeInfo, ParameterListInfo
from std.reflection import ElementOf, ReturnOf, SignatureOf, ParametersOf
```

These names must not be added to the automatic prelude. `TypeInfo` describes a type, while `ParameterListInfo` describes a reflected callable parameter list. Neither is RFC 107's `Type[T]` token. Ordinary import, alias, and visibility rules apply to these names and the named operations.

### Inspect a field's declared type

```incan
model Customer:
    age: int
    nickname: Option[str]
    tags: list[str]
    scores: list[Option[int]]


def describe_field_type(ty: TypeInfo) -> str:
    match ty:
        int => return "Integer"
        Option[str] => return "Optional string"
        list[str] => return "List of strings"
        list[Option[infer element]] =>
            return f"List of optional {element}"
        list[infer element] =>
            return f"List of {element}"
        _ => return "Another type"


def describe_fields[T]() -> None:
    for field in T.__fields__():
        println(f"{field.name}: {describe_field_type(field.type)}")
```

`field.type` is the proposed structured counterpart of the existing display-oriented `field.type_name`. The adapter reads the declaration, so it needs no `Customer` instance or example list. Type display in the strings above is diagnostic presentation, not a way to recover semantics.

No `type(...)` wrapper is needed because the subject already establishes type-pattern context. This does not change the meaning of `match` on ordinary values.

### Capture without name collisions

```incan
from domain import Element


def classify(ty: TypeInfo) -> str:
    match ty:
        list[Element] => return "List of the imported Element type"
        list[infer Element] => return f"List of {Element}"
        _ => return "Another type"
```

The second arm explicitly introduces a capture even though an existing type has the same name. Lowercase capture names are usually easier to read, but capitalization is not a semantic rule. Conversely, `list[Elemnt]` is an unknown-type error if no such type or capture exists; it is not an implicit capture.

Nested captures use the same notation:

```incan
match field.type:
    Option[list[infer element]] =>
        println(f"Optional list of {element}")
    list[Option[infer element]] =>
        println(f"List of optional {element}")
    dict[str, list[infer element]] =>
        println(f"String keys and lists of {element}")
    _ =>
        println("Another shape")
```

### Constrain a captured type by a trait

A capture may require checked trait conformance through `with`. Using the existing JSON serialization trait and proposed structured field types:

```incan
from std.serde.json import Serialize

def describe_fields[T]():
    for field in T.__fields__():
        match field.type:
            list[infer element with Serialize] =>
                println(f"List of JSON-serializable {element}")
            list[_] =>
                println("List without guaranteed JSON serialization")
            _ => pass
```

The first arm matches a list whose element type satisfies `Serialize`. This checks the declared type's trait conformance rather than serializing values or inferring a capability from similarly named methods. The captured component retains its exact type identity. In reflection, `element` remains a read-only type description; a requirement on the described type does not make the description itself implement that trait.

### Distinguish unions from alternative type patterns

Within a type pattern, `|` retains its ordinary union-type meaning. `or` instead selects between alternative patterns:

```incan
match field.type:
    int | str =>
        println("Declared as the union int | str")
    int or str =>
        println("Declared as int, or declared as str")
    _ => pass
```

The first arm matches the union itself, not an individual integer or string type. Alternatives can also capture matching components:

```incan
match field.type:
    list[infer element] or FrozenList[infer element] =>
        println(f"Sequence of {element}")
    _ => pass
```

Both alternatives introduce `element` as a reflected type. Each alternative must introduce the same binding names with agreeing kinds; a type component cannot stand in for a signature or parameter pack. This notation also applies in compile-time type-operation patterns. Ordinary expression `or` and value-pattern alternation retain their existing meanings.

### Use named type operations

```incan
type Names = list[str]
type Formatter = Callable[(str, int) -> bool]

type Name = ElementOf[Names]                         # str
type FormatterResult = ReturnOf[Formatter]          # bool
type FormatterSignature = SignatureOf[Formatter]    # (str, int) -> bool
```

`ElementOf` removes one element-bearing layer. `ElementOf[list[list[int]]]` is `list[int]`, and `ElementOf[list[Option[int]]]` is `Option[int]`. `ElementOf[Option[list[int]]]` is unsupported: it must not silently unwrap optionality.

`ElementOf` covers standard iterable collections and iterator contracts, rather than being restricted to lists. The result is the checked type of a yielded item: lists, sets, and their frozen forms yield their element type; dictionaries and frozen dictionaries yield their key type; strings yield `str`; bytes yield `int`. For a nonempty tuple, the result is the union of its position types under ordinary union normalization. An empty tuple has no element positions, so its projection produces a compile-time unsupported-input diagnostic. Custom iterables and iterators must expose one unambiguous checked item contract, using RFC 098's associated item types where applicable rather than guessing from storage or method names.

Using existing stdlib iterator contracts, the following results describe proposed type-operation behavior:

```incan
from std.derives.collection import Iterator, FallibleIterator
from std.fs import IoError

type Line = ElementOf[Iterator[str]]
type CheckedLine = ElementOf[FallibleIterator[str, IoError]]
```

Both results are `str`. A fallible source's error type remains separate from its yielded item type; an iterator whose items are themselves Results retains that Result as its element type. Type inspection must not construct, poll, or consume a source. Ambiguous item contracts receive a diagnostic rather than selecting one arbitrarily.

Tuple projection retains every possible element type:

```incan
type HomogeneousItem = ElementOf[tuple[int, int]]  # int
type MixedItem = ElementOf[tuple[int, str]]        # int | str
```

Ordinary union normalization removes duplicate member types while preserving checked identities and constraints. The projection must not choose only the first tuple position or replace a mixed result with Any.

Generic uses can reduce without knowing every concrete argument:

```incan
def first_or_none[T](items: list[T]) -> Option[ElementOf[list[T]]]:
    if len(items) == 0:
        return None
    return Some(items[0])
```

Here `ElementOf[list[T]]` reduces to `T` from the known outer list shape.

### Define an operation through matching

```incan
type ListElementOf[T] = match T:
    list[infer element] => element
    FrozenList[infer element] => element
```

This is an application-defined type operation, not an ordinary expression block. Each arm produces a type. `ListElementOf` illustrates the list-specific portion of the broader standard `ElementOf` contract rather than redefining the standard operation.

Libraries can define other transformations:

```incan
type SuccessOf[T] = match T:
    Result[infer success, _] => success

type OptionalOf[T] = match T:
    Option[infer inner] => Option[inner]
    infer other => Option[other]
```

`SuccessOf[Result[int, ParseError]]` is `int`. `SuccessOf[int]` is an unsupported application with a diagnostic naming `SuccessOf`, its input, and its admitted shapes. `OptionalOf` has an explicit fallback instead of relying on an invented success type.

### Apply a type operation recursively

An application may define an operation that repeatedly removes list layers:

```incan
type LeafOf[T] = match T:
    list[infer inner] => LeafOf[inner]
    infer leaf => leaf

type NestedValue = LeafOf[list[list[int]]]  # int
type SingleValue = LeafOf[list[str]]        # str
```

For the nested example, the compiler reduces `list[list[int]]` to `list[int]`, then to `int`. Each recursive step works on the captured element type, removing one list layer; the fallback returns the remaining type. This computes a type during checking rather than traversing list values. Straightforward structural descent requires no additional authored proof syntax. An operation that cannot establish progress must receive a diagnostic rather than loop indefinitely.

### Preserve the whole callable signature

```incan
type Formatter = Callable[(text: str, *, limit: int = ...) -> str]

type FormatterResult = ReturnOf[Formatter]         # str
type Signature = SignatureOf[Formatter]            # (text: str, *, limit: int = ...) -> str
type EquivalentContract = Callable[SignatureOf[Formatter]]
```

`SignatureOf` selects the full caller-facing function type. It preserves keyword names, positional-only and keyword-only boundaries, permission to omit arguments, explicit argument mutability, and variadic parameter kinds. It does not extract or evaluate the implementation's default expressions.

`ParametersOf` selects the parameter-list component. Its result is a parameter pack, not an ordinary tuple type:

```incan
type RequiredFormatter = Callable[(str, int) -> bool]
type Rebuilt = Callable[ParametersOf[RequiredFormatter] -> ReturnOf[RequiredFormatter]]

type Flexible = Callable[(*values: int, **options: str) -> bool]
type FlexibleSignature = SignatureOf[Flexible]
```

For `RequiredFormatter`, the reconstructed contract is `Callable[(str, int) -> bool]`. For `Flexible`, the signature remains `(*values: int, **options: str) -> bool`. Flattening that parameter pack into `Tuple[int, str]` would lose its meaning.

A parameter list can be named and reused with the same declaration spelling. Using the optional keyword-only `Formatter` contract above:

```incan
type FormatterParameters = ParametersOf[Formatter]
type Validator = Callable[FormatterParameters -> bool]
```

`Validator` accepts the same required `text` and optional keyword-only `limit` as `Formatter`, but returns `bool`. The compiler retains `FormatterParameters` as a parameter-pack alias, preserving names, order, parameter kinds, omission rules, types, and access markers. It is not an ordinary value type and does not provide runtime argument forwarding.

A full signature capture is also readable:

```incan
type CallSignature[T] = match T:
    Callable[infer signature] => signature
```

Component captures can select the result without discarding parameter information:

```incan
type CallResult[T] = match T:
    Callable[infer parameters -> infer result] => result
```

In that pattern, `parameters` captures the whole parameter pack. It does not mean one positional argument.

### Keep runtime inspection separate from static projection

```incan
match field.type:
    list[infer element] =>
        schema = build_schema(element)
        # decode[element](payload) is not admitted by this match.
    _ =>
        ...
```

The reflection capture is a `TypeInfo` value. In a compile-time type operation, the corresponding capture is a type that can appear in the result annotation. Reusing the pattern language does not make these bindings interchangeable.

## Reference-level explanation

### Type-pattern contexts and syntax

Type patterns must be admitted in `match` whose subject has the compiler-recognized reflected-type carrier and in the arms of `type Name[...] = match Input:` declarations. Ordinary value-pattern contexts must retain their existing meanings.

A type pattern must admit:

- an exact resolved type, including qualified names, transparent aliases, nominal newtypes, and constrained types;
- a type constructor applied to nested type patterns, such as `list[Option[infer element]]`;
- alternative type patterns separated by `or`, distinct from union types written with `|`;
- `_`, which matches a component without binding it;
- `infer name`, which binds the complete component in that position;
- `infer name with Trait`, which captures a type component only when its checked type satisfies the stated trait requirement;
- a function-type pattern whose parameter and result components use the shared signature model;
- `infer parameters -> infer result` in a function-signature pattern, with the first capture having parameter-pack kind;
- a reference to a capture introduced earlier in the same pattern, which requires equal checked type identity.

For example, `dict[infer item, item]` must match only dictionaries whose key and value types are identical. `dict[infer item, infer item]` must be rejected as a duplicate binding. Capture scope must be the containing arm, and earlier arms must not supply names to later arms. A qualified type name must continue to refer to its type even when a capture shadows its unqualified spelling.

`infer` must be contextual in type-pattern binding positions. This RFC admits `infer name` as a capture only where a type pattern is admitted. RFC 098 separately proposes contextual `infer Name` declarations in trait headers; those declarations must follow that RFC's scope and implementation-binding rules rather than pattern capture rules. Outside the explicitly admitted contexts, ordinary identifiers named `infer` must retain their existing meaning. The marker must not become an expression-hole notation, a general declaration keyword, or a request to synthesize a value.

### Trait-constrained captures

`infer name with Trait` must be admitted in type-pattern capture positions using the ordinary checked trait-bound syntax, lookup, visibility, and conformance rules. A concrete type satisfying the requirement must match and retain its exact checked identity; a concrete type known not to satisfy it must not match that alternative. Constrained captures must be available in both reflected-type matches and compile-time type-operation patterns.

Trait requirements must not infer adoption from method names, construct or inspect a sample value, execute validation, unwrap newtypes, or coerce the captured component into a trait representation. A trait requirement must apply to a well-kinded type capture rather than a parameter pack or other non-type component. For unresolved generic input, conformance must follow known bounds and pending obligations; absence of enough evidence must not be treated as proven nonconformance to select a later fallback speculatively.

In reflection, the requirement describes the matched type, not its read-only carrier. Success must not promote a reflected description into a static generic argument or manufacture a callable or serializable value. Capture-kind agreement and ordinary visibility boundaries remain applicable.

### Type identity and selection

Matching must use checked Incan type structure and identity, not display strings, generated target-language names, storage representations, or implicit conversions. An exact pattern must not silently match a subtype, erase constraints, or unwrap a newtype. Transparent aliases must follow existing alias identity rules.

Type constructors must resolve through ordinary type lookup. A typo or inaccessible type must produce a diagnostic. Only `infer` may introduce a new capture. Capture names must not be selected by capitalization or by failure to resolve a type.

Union and optional forms must use RFC 029's canonicalization. An exact union pattern must compare canonical member sets rather than source order. This RFC does not introduce ambiguous partition captures such as `int | infer rest`; that pattern must be rejected pending a defined partition contract. Capturing an entire union through `infer whole` remains valid.

Type-pattern alternatives must use `or`; `|` must continue to form a union type rather than alternate between patterns. Union formation binds within an alternative, so `int | str or bool` means the union `int | str` as one alternative and `bool` as the other. Parentheses may group alternatives. Alternatives must be checked independently and introduce the same binding names with agreeing kinds under RFC 071's binding-agreement principle. Agreement concerns the capture kind, not identical captured values across different alternatives: a type capture may receive whichever checked type the selected alternative matched. A parameter-pack capture must not agree with an ordinary type or full-signature capture. Bindings introduced solely by one alternative must not become available in a sibling alternative. Alternatives must be considered in source order, and an unresolved earlier alternative must not be bypassed to select a later one speculatively. This rule applies in reflection matches and compile-time operations without changing ordinary expression or value-pattern syntax.

Arms must be considered in source order. For a known concrete type, the first matching arm must win. Known unreachable arms should receive the ordinary redundancy diagnostic when such diagnostics are available. Reflection matches over the open universe of types must include a wildcard or whole-type capture unless exhaustiveness can be proved from a narrower checked subject domain.

### Compile-time type-operation declarations

`type Name[Parameters] = match Input:` must define a named compile-time operation with one type or signature-component result kind. Each arm must produce a well-kinded result using existing types, its captures, or other admitted operations. The declaration must not execute arbitrary expressions, call runtime functions, or bind runtime values.

An operation must have an ordinary module identity and visibility. Its definition and input constraints must be preserved across imports and checked package boundaries. The compiler must not infer behavior from names such as `ElementOf` or `ReturnOf` in user namespaces.

An application must reduce when the checked input proves which arm is selected. Generic input must not require every nested type to be concrete: `ElementOf[list[T]]` must reduce to `T`. Conversely, the compiler must not choose a fallback while an earlier arm could still match an unresolved type parameter. Such an application must retain its pending reduction and obligations until enough type information is available.

Operations may be partial. A concrete application matching no arm must fail with an unsupported-input diagnostic, not reduce to `Any`, `None`, a bottom type, or a guessed result. Pending operations must not escape a compilation boundary that requires a concrete layout or call contract. Generic checked metadata may retain an operation application and its obligations for later specialization.

User-defined operations may recurse when the compiler can establish termination. Structural descent through captured components, such as removing a list layer in `LeafOf`, must be an admitted basis for progress without additional authored proof syntax. Direct cycles, indirect cycles, and recursion without established progress must receive diagnostics rather than loop indefinitely. A compiler resource limit must produce a bounded diagnostic with the operation chain, not a crash or a fabricated type. The concrete analysis algorithm and resource accounting belong to implementation design under this contract.

### Standard named operations

`std.reflection` must provide the following standard type operations through explicit imports. These operations must not be automatic prelude names.

| Operation         | Accepted input                                                    | Result                                             |
| ----------------- | ----------------------------------------------------------------- | -------------------------------------------------- |
| `ElementOf[T]`    | Built-in iterable types or one checked iterable/iterator contract | Declared yielded item type, preserving constraints |
| `SignatureOf[F]`  | A function type or one unambiguous checked `Callable[S]` contract | Caller-facing function type `S`                    |
| `ReturnOf[F]`     | The same callable domain as `SignatureOf`                         | The result type from `SignatureOf[F]`              |
| `ParametersOf[F]` | The same callable domain as `SignatureOf`                         | The full parameter pack from `SignatureOf[F]`      |

Element projections must follow the checked iteration contract for the selected input, including standard iterable collections, iterator types, transparent aliases, and generic bounds that establish an item type. An associated item member must be resolved through its owning iteration trait under RFC 098, not by finding any unrelated member named Item. Multiple unresolved competing iteration contracts must produce an ambiguity diagnostic. Fallible source errors must not become part of the element type unless the yielded item itself has that type. Projection must not execute iteration or add an iteration capability to the input. For every nonempty tuple, the projected item type must be the canonical union of all position types under RFC 029, retaining checked type identities and constraints. The operation must not select the first position alone, infer a common supertype, or substitute Any. An empty-tuple projection must produce a compile-time unsupported-input diagnostic identifying the absence of element positions. It must not fabricate Any, None, or a bottom type. An empty runtime value of a typed collection such as `list[int]` must not change its declared element projection.

Callable adopters and generic bounds must be resolved according to RFC 128, not by a second structural `__call__` discovery rule. Where a generic bound provides only a required callable contract, projections must describe that contract; they must not claim knowledge of extra calls accepted by a concrete implementation.

If an input exposes multiple distinct admissible call signatures without a unique selected contract, these projections must fail with an ambiguity diagnostic. They must not select the last overload, union all results, or choose an arbitrary implementation. A caller may first supply an explicit checked function type or `Callable[S]` contract. No function-name-to-type conversion is implied by `SignatureOf[F]`.

`SignatureOf` must preserve every caller-facing parameter and result fact owned by RFC 128. Bound `self` and `cls` must remain absent from caller-supplied parameters. Receiver binding, mutable receiver access, consumption, and captured environment obligations must remain part of the original callable value's contract; projecting a signature must not manufacture a value, detach its receiver, or change an invocation guarantee. A projection is lossless for the declared caller-facing call shape, not a way to reconstruct the callable's state.

Omission metadata must preserve whether a parameter is optional without evaluating its default expression or embedding a new default value. `ParametersOf` must retain declaration order, names where promised, parameter kinds, types, omission rules, and explicit parameter access markers. Variadic types must continue to describe each variadic element, as in RFC 128.

### Parameter-pack kind

Parameter packs must be a distinct compile-time signature-component kind. They must not be silently converted into tuple types or runtime collections. `ParametersOf[F]` must be accepted in the parameter position of a function type, including `Callable[ParametersOf[F] -> R]`. `Callable[SignatureOf[F]]` must accept the projected function type directly.

A reusable parameter pack must be nameable through the existing type-declaration spelling, such as `type FormatterParameters = ParametersOf[Formatter]`. The checker must retain its parameter-pack kind rather than reinterpret the declaration as an ordinary value type. The name must be accepted wherever the corresponding direct parameter-pack projection is accepted, including `Callable[FormatterParameters -> R]`. Alias resolution, visibility, imports, and checked package metadata must preserve that kind and the complete parameter contract. Type operations returning a captured parameter pack must likewise retain its kind when named through this declaration spelling.

This narrow pack use must not imply arbitrary pack expansion into tuples, generic argument lists, calls, or collections. A parameter pack used where an ordinary type is required, including as a variable's annotation, must produce a kind-mismatch diagnostic. Naming a pack must not manufacture callable state, evaluate defaults, or provide runtime argument forwarding.

### Reflected-type carrier and fields

RFC 086's field descriptors must provide structured checked types to this feature. The structured surface must be `field.type: TypeInfo`, with `TypeInfo` imported from `std.reflection`, alongside `field.type_name` for display. Reflection matching must not require a live field value, a dummy model instance, source parsing, or interpreting type-name strings.

A reflected capture must be a read-only description of the matched component. Type components must bind `std.reflection.TypeInfo` values; a complete parameter-pack component must bind a read-only `std.reflection.ParameterListInfo` description. These descriptions must be passable to appropriate reflection consumers and inspectable again. They must not enter the ordinary static type namespace or authorize a runtime-selected generic instantiation. Function-type descriptions must preserve the same parameter and result facts when those facts are available in the checked reflected contract.

Type inspection must preserve existing visibility boundaries. Access to a description of a public field's type must not expose private fields or members of that type. Recursive nominal types must have finite identity-based descriptions; their internal graphs must not require infinite expansion merely to inspect or display them.

### Required diagnostics and tooling evidence

Diagnostics must distinguish an unresolved type reference, a duplicate capture, a capture-kind mismatch, a missing reflection fallback, unsupported concrete input, ambiguous callable input, pending generic obligations, and non-terminating reduction. They should show the input type, relevant pattern or operation, and the source responsible for the failed requirement.

Hover and checked metadata must distinguish compile-time captured types and parameter packs from reflected `TypeInfo` and `ParameterListInfo` values. For operation applications, tooling must expose the written operation, its reduced result when known, or its outstanding obligations. A hover over `ElementOf[list[int]]` must be able to identify `int` as the result and the checked list element relationship as its origin. Unresolved input must remain visibly pending rather than receive a guessed reduction.

The compiler must determine these facts through shared analysis. Editor and agent integration must go through the language server over that analysis, following RFC 140's integration approach rather than introducing a separate inference engine. Checked package metadata under RFC 048 must preserve operation definitions, capture kinds, reductions where applicable, and pending obligations across imports. RFC 106 projections must retain the same fact meanings, source-revision or analysis-snapshot evidence, visibility boundaries, and explicit incomplete status for dependent checks.

Human diagnostics, language-server results, and checked metadata must agree on established and pending facts. Structured additions must follow existing compatibility rules, using compatible additions or a guarded schema version as required. Exact protocol fields, schema version numbers, and wiring belong to implementation design under these contracts rather than a separate language-level protocol.

## Design details

### Fit with RFC 029

RFC 029 matches values of union types and binds narrowed values. `int(value)` remains a value pattern in its existing context. This RFC matches structured type descriptions or compile-time type inputs, and `int` denotes an exact type pattern only in those contexts. `isinstance` remains a value-narrowing construct; this proposal does not introduce runtime generic-element checks through that spelling.

RFC 029's union normalization remains authoritative for optional and union type identity. Type operations must not invent a second union system.

### Fit with RFC 086

RFC 086 owns schema descriptors, metadata overlays, adapter consumption, and extraction. This RFC owns the type-pattern and type-operation language used to inspect or transform their checked types. It gives the existing structured-descriptor requirement an inspectable surface without defining SQL, Arrow, JSON Schema, or other target mappings.

The proposal requires an additive structured field type surface. It does not remove `type_name`, reinterpret descriptive metadata as types, or change schema merge rules. Public reflection documentation must identify which structured surfaces exist when implementation ships; a Draft RFC is not evidence that those surfaces are available.

### Fit with RFC 107

RFC 107's `Type[T]` token remains a bounded carrier of static type evidence for dispatch. It is not the `TypeInfo` carrier, and a token must not become matchable merely because it carries a type parameter. The generic APIs may use named operations on their statically known `T` without converting tokens into reflection objects.

RFC 107 explicitly excludes general type-level functions and type switches. This RFC is the separate proposal for that excluded capability; it does not claim RFC 107 already admits it. If accepted, RFC 107's boundary text must point here while retaining its token restrictions and overload-based type-directed return mapping. A type operation must not synthesize missing overloads or weaken their call-site checks.

### Fit with RFCs 098 and 128

RFC 098 proposes implementation-chosen members in trait headers, such as `trait PullIterator[infer Item]`, with choices supplied by adoptions such as `PullIterator[str]`. That use declares a trait-owned relationship; an `infer element` type pattern here captures a component of an existing checked type. The shared marker must not merge their scopes or allow pattern syntax in trait headers. Associated types remain trait-owned relationships, while named type operations are explicitly applied transformations. `ElementOf` supports checked iterable and iterator contracts and must project their item type from the owning contract rather than inspect implementation storage or guess from method names.

RFC 128 owns the shared signature model and callable adoption. This RFC projects that model. `SignatureOf` and `ParametersOf` must not define a second, less expressive parameter system or discard receiver obligations to make forwarding appear valid. General argument forwarding and pack expansion remain separate capabilities.

### Observable acceptance

An accepted implementation must demonstrate the illustrated exact and nested matches, import-independent captures, repeated-capture equality, generic reductions, and unsupported-input diagnostics. Signature projections must preserve keyword-only, positional-only, optional, variadic, and argument-access facts through direct use and checked package boundaries. Reflection inspection must work without sample instances, preserve visibility, and reject use of reflected captures as static generic arguments. Ambiguous callable inputs and cyclic operations must fail with grounded diagnostics. Editor results must agree with compiler results. Documentation must distinguish implemented capabilities from this proposal's remaining design work.

### Prior art

[Scala 3 match types](https://docs.scala-lang.org/scala3/reference/new-types/match-types.html) are the principal precedent: named type computations use readable case analysis over type shapes and can reduce with partially known generic inputs. Scala's lowercase pattern-variable rule does not transfer directly to Incan's lowercase built-in type names.

[Scala quoted type patterns](https://docs.scala-lang.org/scala3/reference/metaprogramming/macros.html#type-patterns) also demonstrate inspecting a type and capturing its components within an explicit staging context. They illustrate why access to compile-time type evidence is a stronger capability than receiving a runtime description. This RFC does not import Scala's quote and splice syntax.

[TypeScript conditional types](https://www.typescriptlang.org/docs/handbook/2/conditional-types.html#inferring-within-conditional-types) provide the `infer` notation for introducing a type variable while matching structure. Incan borrows that explicit binding marker, not TypeScript's conditional-expression spelling or overload extraction rules.

## Alternatives considered

- **Bare identifiers become captures when lookup fails:** rejected because imports and misspellings could change exact checks into captures.
- **Capitalization distinguishes types and captures:** rejected because Incan's built-in types are lowercase and names should retain ordinary resolution.
- **`let` inside type arguments:** explicit but less descriptive of type extraction than `infer`.
- **`_ as element`:** unambiguous and compatible with ordinary capture ideas, but `infer element` states the type-pattern role directly and has established prior art.
- **Wrap every pattern in `type(...)`:** redundant when the subject already establishes type-pattern context, and it does not solve capture ambiguity.
- **Expose only descriptor constructors such as `TypeInfo.List(...)`:** usable internally, but it makes libraries translate ordinary Incan type notation into another vocabulary.
- **Copy TypeScript's conditional-type syntax:** more punctuation and nested branching than the match syntax already familiar to Incan users.
- **Make `ParametersOf` a tuple of types:** rejected as the normative signature projection because it loses keyword rules, optionality, variadic kinds, and parameter access markers.
- **Provide only compiler-known operations:** simpler, but prevents libraries from naming and sharing checked type transformations using the same pattern language.

## Drawbacks

Type operations add a new reduction and diagnostic problem to the typechecker. Partial and recursive operations need understandable failures, and unresolved generic inputs must not select the wrong fallback. Parameter packs introduce a compile-time kind beyond ordinary types even though this RFC deliberately limits their use to signatures.

The same `infer` notation has different binding kinds in static operations and reflection matches. Tooling and documentation must make that distinction visible. A reflected capture does not automatically provide the static capabilities developers may expect from Scala's staged type evidence.

Exact identity matching may require more explicit cases than subtype-oriented matching. Preserving that boundary avoids silently erasing newtype or constraint semantics, but future constrained patterns may still be useful.

## Layers affected

- **Parser / AST:** contextual `infer`, nested type patterns, match-based type declarations, and signature-component projection positions.
- **Typechecker / Symbol resolution:** exact type matching, capture scope and kind, operation reduction, partiality, callable projection, and termination diagnostics.
- **IR Lowering:** reduced static types and structured reflection matches, with no arbitrary type-operation execution crossing into runtime code.
- **Emission:** finite reflected-type descriptions and tests of their checked structure, without string-parsing dispatch.
- **Stdlib / Runtime:** read-only reflected-type support and the standard named operations, sharing schema and callable contracts.
- **Checked metadata / Packages:** operation definitions, pending generic obligations, canonical type identities, signature facts, and visibility boundaries.
- **Formatter:** stable formatting of type patterns, operation arms, and signature-pack expressions.
- **LSP / Tooling:** capture-kind hovers, reduced-result inspection, pending obligations, and source-grounded diagnostics.

## Design Decisions

- `std.reflection` exposes `TypeInfo`, `ParameterListInfo`, `ElementOf`, `ReturnOf`, `SignatureOf`, and `ParametersOf` through explicit imports rather than the automatic prelude. Reflected descriptions remain distinct from RFC 107's `Type[T]` tokens.
- Captures may require traits using `infer name with Trait`. Checked conformance and ordinary generic-bound rules determine matching without erasing exact type identity or inferring capabilities from method names. Unresolved obligations remain pending. Reflected captures remain descriptions rather than gaining the described type's runtime or static capabilities.
- `ElementOf` covers standard iterable collections, including tuples, and checked iterable and iterator contracts. It projects the yielded item type without executing iteration, guessing from storage, or conflating a fallible source error with its item type. Ambiguous item contracts receive diagnostics. Nonempty tuples project the ordinary normalized union of their position types, including a single member for repeated identical types. An empty tuple receives an unsupported-input diagnostic because it has no element positions; empty values of typed collections retain their declared element type.
- Reusable parameter packs use the existing type-declaration spelling, such as `type FormatterParameters = ParametersOf[Formatter]`. The checker and package metadata preserve their distinct compile-time kind and complete parameter contracts. Named packs may be reused in signature parameter positions without becoming value types or runtime argument-forwarding mechanisms.
- Type patterns use `or` for alternatives and retain `|` for exact union types. Alternatives must introduce the same binding names with agreeing capture kinds; ordinary expression `or` and value-pattern alternation remain unchanged. The rule applies to reflected-type matching and compile-time type operations.
- User-defined type operations may recurse when termination can be established. Structural descent through captured type components is supported without additional proof syntax; cycles or recursion without established progress produce diagnostics. Compiler resource limits report the reduction chain rather than crash or fabricate a type.

- The compiler owns type-operation and capture facts; the language server exposes the shared analysis to editors and agents. RFC 048 checked metadata preserves the contract across imports, and RFC 106 projections retain the same facts, source evidence, and incomplete status. Tools distinguish static captures from reflected descriptions and known reductions from pending obligations. Existing compatibility and schema-versioning rules apply; exact protocol fields and wiring belong to implementation design.
