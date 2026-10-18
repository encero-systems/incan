# RFC 098: Native associated types for traits

- **Status:** Draft
- **Created:** 2026-05-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 025 (multiple generic trait instantiations)
    - RFC 042 (traits are always abstract)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 054 (explicit call-site generic arguments)
    - RFC 065 (`std.hash`)
    - RFC 083 (symbol and method aliases)
    - RFC 088 (iterator adapter surface)
    - RFC 099 (generic trait-targeted methods)
    - RFC 101 (`std.collections.OrdinalMap`)
    - RFC 128 (one Callable trait and the shared signature model)
    - RFC 141 (type matching with infer and named type operations)
- **Issue:** [#580](https://github.com/encero-systems/incan/issues/580)
- **Blocks:** [#596](https://github.com/encero-systems/incan/issues/596) (v0.5 RFC 101 trait-system bridge removal)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Declare implementation-chosen types in trait headers with `infer`, as in `trait PullIterator[infer Item]`. An implementing type supplies its choice through ordinary trait adoption syntax, such as `class Lines with PullIterator[str]`. Trait methods and generic consumers refer to that choice through `Self.Item` and `I.Item`. Ordinary trait parameters remain independent inputs that may distinguish multiple implementations; inferred members are fixed outputs of each implementation. This proposal gives iterators, parsers, codecs, hashers, and adapters named type relationships without body-level `type Item` declarations or additional generic parameters in every consumer. All syntax in this RFC is proposed, except where an existing contract is explicitly identified.

## Core model

1. **Declare the relationship in the header:** `trait PullIterator[infer Item]` says that each implementation supplies one item type.
2. **Supply the choice at adoption:** `class Lines with PullIterator[str]` chooses `str` as its item type.
3. **Recover the choice through a projection:** `Self.Item` and `I.Item` name the implementation's selected type.
4. **Separate inputs from outputs:** in `trait Lookup[K, infer Value]`, `K` is an ordinary parameter; `Value` is an implementation-chosen member.
5. **Keep outputs unique for each implementation:** the implementing type, trait identity, and ordinary arguments determine all inferred members. Changing only an inferred member must not create another implementation.
6. **Check the relationship statically:** missing choices, incompatible methods, conflicting implementations, and ambiguous projections produce diagnostics. Associated types do not add runtime fields or values.

## Motivation

An iterator's item type, a parser's output type, and a streaming hasher's digest type are part of their capabilities. Generic consumers should be able to name those types without making the same choice again. A helper returning `H.Digest` can work with both a hasher producing `bytes` and one producing `u64`, while preserving the exact return type for each caller.

Ordinary generic trait parameters remain useful when distinct instantiations represent distinct capabilities, as with indexing by different key types. They do not, by themselves, express that one implementation determines a unique output type. RFC 025 deliberately allows multiple instantiations of an ordinary generic trait.

The original Draft spelling placed `type Item` inside a trait body and `type Item = str` inside the implementing type. The declaration named a requirement without making the implementation's choice visible in the header. This revision uses `infer Item` to identify the required type relationship where the trait's other type parameters already live, and uses the adoption's argument list to supply the choice.

RFC 023 deferred iterator item propagation requiring associated-type inference. RFC 042 identified associated types as future native trait-system work. RFC 043 supplies an existing Rust interop contract, but native Incan traits need their own readable authoring surface. The end-state is a shared checked model for these relationships, with source syntax that explains who supplies each type.

## Goals

- Declare native associated types through `infer Name` entries in trait headers.
- Bind those members through trait adoption arguments.
- Allow trait methods and generic signatures to project the selected types.
- Support traits mixing ordinary parameters and inferred members.
- Preserve RFC 025 multi-instantiation for ordinary parameters while preventing conflicting inferred outputs for the same implementation.
- Define missing-binding, ambiguity, inheritance, cycle, and signature-compatibility behavior.
- Preserve member identities, bindings, projections, and resolution provenance in checked API metadata, library manifests, generated documentation, and editor tooling.
- Keep the existing RFC 043 Rust interop surface compatible.

## Non-Goals

- Replacing ordinary generic traits or requiring existing traits to migrate.
- Inferring a missing adoption argument from method bodies or return annotations.
- Defining the complete generic constraint language, trait specialization, or overlapping implementation precedence.
- Introducing runtime type fields, runtime reflection dispatch, or structural duck typing.
- Defining generic associated types, associated constants, default associated types, or trait-object dispatch in this revision.
- Changing RFC 128's callable contract or its signature parameter into an inferred member.

## Guide-level explanation

### An iterator chooses its item type

The trait declares the name of the type an implementation must supply:

```incan
pub trait PullIterator[infer Item]:
    def next(mut self) -> Option[Self.Item]: ...
```

Read this as: “A pull iterator has an item type, chosen by its implementation. Its next method returns an optional item of that type.” `Item` is a type member, not an instance field. The `infer` marker describes how consumers obtain the type; the implementing declaration still supplies it explicitly.

An implementation supplies the choice beside the adopted trait:

```incan
pub class Lines with PullIterator[str]:
    def next(mut self) -> Option[str]:
        ...

pub class ByteChunks with PullIterator[bytes]:
    def next(mut self) -> Option[bytes]:
        ...
```

`Lines.Item` is `str`; `ByteChunks.Item` is `bytes`. No additional member declaration or alias is needed in either body.

A generic consumer can recover that choice without introducing a separate item parameter:

```incan
def next_or_default[I with PullIterator](
    mut source: I,
    default: I.Item,
) -> I.Item:
    match source.next():
        Some(value) => return value
        None => return default
```

For `Lines`, the default and result must be strings. Passing a byte default does not select a different implementation of `Lines`; it is a type error.

### A parser names both sides of its contract

A trait can declare more than one inferred member:

```incan
pub trait Parser[infer Input, infer Output]:
    def parse(self, value: Self.Input) -> Self.Output: ...

pub class IntegerParser with Parser[str, Result[int, ParseError]]:
    def parse(self, value: str) -> Result[int, ParseError]:
        ...
```

`ParseError` here stands for the library's existing error type. Putting the complete `Result` in `Output` preserves the parser's checked success and failure contract without adding error-handling syntax to this proposal.

```incan
def parse_one[P with Parser](parser: P, value: P.Input) -> P.Output:
    return parser.parse(value)
```

The parameter named `Input` is still an inferred member: the parser implementation determines it. The marker, rather than the English meaning of the name, determines its role.

### A hasher chooses its digest representation

```incan
pub trait StreamingHash[infer Digest]:
    def update(mut self, chunk: bytes) -> None: ...
    def finalize(mut self) -> Self.Digest: ...

pub class Xxh64Hasher with StreamingHash[u64]:
    def update(mut self, chunk: bytes) -> None:
        ...

    def finalize(mut self) -> u64:
        ...
```

A byte-digest implementation would instead adopt `StreamingHash[bytes]`. A shared helper preserves whichever representation its hasher supplies:

```incan
def hash_chunks[H with StreamingHash](
    mut hasher: H,
    chunks: list[bytes],
) -> H.Digest:
    for chunk in chunks:
        hasher.update(chunk)
    return hasher.finalize()
```

These are illustrative trait sketches, not claims about existing stdlib signatures. A fallible hasher can similarly make its declared output a `Result`, or use a separate inferred error member if its API needs to name that relationship independently.

### Ordinary parameters and inferred members can coexist

```incan
pub trait Lookup[K, infer Value]:
    def lookup(self, key: K) -> Option[Self.Value]: ...

pub class Directory with Lookup[str, User], Lookup[int, User]:
    def lookup(self, key: str) -> Option[User]:
        ...

    def lookup(self, key: int) -> Option[User]:
        ...
```

`User` stands for the library's user type. Different ordinary arguments allow the two lookup capabilities, following RFC 025. For the `str` capability, however, the directory must have exactly one selected `Value` type. Adding `Lookup[str, Team]` to this same class must fail even if a return annotation could distinguish the calls.

A consumer supplies the ordinary argument and recovers the inferred member:

```incan
def lookup_one[K, L with Lookup[K]](
    directory: L,
    key: K,
) -> Option[L.Value]:
    return directory.lookup(key)
```

Within this signature, the bound selects `Lookup[K]`, so `L.Value` is unambiguous. A concrete `Directory.Value` without a selected trait instantiation is ambiguous, even if both implementations currently choose `User`.

### Constrain an implementation's chosen type

A bound can explicitly require a selected inferred type:

```incan
def next_byte_chunk[I with PullIterator[bytes]](
    mut source: I,
) -> Option[bytes]:
    return source.next()
```

Here `PullIterator[bytes]` checks an existing choice. It does not let the caller change an iterator's item type.

For a capability requirement instead of an exact type, the related generic constraint design can keep that requirement beside its owner:

```incan
def encode_all[
    I with PullIterator where I.Item with TextEncodable,
](mut source: I) -> list[str]:
    mut output: list[str] = []
    while let Some(item) = source.next():
        output.append(item.encode())
    return output
```

This sketch assumes a `TextEncodable` trait with `def encode(self) -> str`. The placement illustrates the ongoing generic constraint design; this RFC requires associated members to be usable in checked constraints but does not settle the full `where` grammar. A fully explicit generic formulation is also possible:

```incan
def encode_all[T with TextEncodable, I with PullIterator[T]](
    mut source: I,
) -> list[str]:
    ...
```

### Identical member names belong to their respective traits

```incan
pub trait TextParser[infer Value]:
    def parse_text(self, text: str) -> Self.Value: ...

pub trait BinaryParser[infer Value]:
    def parse_bytes(self, data: bytes) -> Self.Value: ...

pub class DualParser with TextParser[str], BinaryParser[bytes]:
    def parse_text(self, text: str) -> str:
        ...

    def parse_bytes(self, data: bytes) -> bytes:
        ...
```

The adoption headers make both choices explicit without targeted body declarations. `DualParser.Value` is ambiguous because two distinct traits own that name. The proposed qualified projection, `DualParser.Value from TextParser`, selects `str`. `from` identifies the owning trait without repurposing comprehension syntax. Generic code bounded only by `TextParser` can use `P.Value` directly. Exact precedence and grouping remain Draft grammar questions, shared with RFC 144's value-level projections.

## Reference-level explanation

### Header declarations and scope

A native trait header may contain ordinary type parameters followed by one or more `infer Name` entries. The ordinary parameters must form a prefix and the inferred members must form a suffix. An ordinary parameter after an inferred member, duplicate names, or an inferred member that collides with an ordinary parameter must produce a diagnostic. Header bounds may use the existing ordinary parameter bound syntax; a richer constraint syntax for inferred declarations is not introduced here.

Each inferred member must have a checked identity owned by the trait. It must be available in type positions in that trait's body through `Self.Name`; its bare header name must denote that same type within the trait declaration. It must not create an instance field, runtime type object, alias declaration, or independent ordinary generic input.

The `infer` marker must be contextual in trait header member declarations. It must not become a general expression keyword, change ordinary identifiers named `infer` elsewhere, or authorize `infer` declarations on classes, functions, or ordinary type aliases. RFC 141 separately admits `infer` captures in type patterns.

### Adoption arguments and implementation uniqueness

An authored concrete adoption must supply all ordinary arguments and all inferred member choices, positionally in declaration order. For `trait Lookup[K, infer Value]`, `Lookup[str, User]` selects `K = str` and binds `Value = User`. Missing choices must produce a diagnostic naming the trait and each missing member. The compiler must not infer missing adoption choices from method signatures or bodies.

A choice must be an ordinary checked type expression. It may use the adopting type's own generic parameters; for example, a generic `Buffer[T]` may adopt `PullIterator[T]`. Every choice must satisfy the trait's checked requirements. A choice that cannot be resolved or represented by the target backend must be rejected.

The implementation key must consist of the implementing type, trait identity, and ordinary trait arguments. Inferred members must not be part of that key. Two adoptions with the same key and conflicting inferred choices must be rejected; inferred choices must not supply a second dispatch dimension. Repeated equivalent requirements through supertrait paths must merge as one conformance fact, rather than create competing implementations.

For a trait with no ordinary parameters, a concrete implementing type must therefore have one binding for each inferred member of that trait. A mixed trait may have different bindings for different ordinary arguments. This must preserve RFC 025 multi-instantiation of ordinary generic traits and must not introduce specialization or precedence between overlapping adoption families.

RFC 099 trait-owned capability families may supply inferred member bindings from checked language metadata. Such bindings must obey the same implementation key, uniqueness, and projection rules as authored adoptions. Metadata-provided bindings are an explicit capability-family mechanism, not permission to guess omitted source arguments.

### Bounds and explicit output requirements

A bound must supply all ordinary trait arguments. It may omit the entire inferred suffix, leaving those members to be recovered from the selected implementation. It may instead supply the complete inferred suffix, requiring exact type equality with the implementation's selected members. Partial inferred suffixes must be rejected; this revision does not introduce placeholders or named adoption arguments.

For `PullIterator[infer Item]`, `I with PullIterator` exposes `I.Item`; `I with PullIterator[bytes]` additionally requires `I.Item` to equal `bytes`. For `Lookup[K, infer Value]`, `L with Lookup[K]` exposes `L.Value`, while `L with Lookup[K, User]` additionally fixes the required value type. Ordinary argument omission must not be inferred from this rule.

Explicit inferred arguments in bounds must check equality rather than conversion, subtyping, or runtime validation. A bound must not mutate an implementation's choice. These requirements must be preserved during generic substitution and in checked metadata. A public API change from an unconstrained inferred member to an exact requirement must be treated as a contract change.

### Projection and resolution

A type projection must resolve to one trait-owned inferred member in one selected ordinary instantiation. `Self.Name` in a trait must resolve through that trait and its supertraits. `T.Name` in generic code must resolve through active bounds; on a concrete type, it must resolve through known conformance metadata. An ordinary trait parameter must not automatically become a projected member.

Distinct owners or distinct ordinary instantiations exposing the same member name must make an unqualified projection ambiguous, even when their current bindings normalize to the same type. A compiler must not choose based on declaration order, imports, capitalization, or convenient equality of the resulting types. The proposed trait-qualified projection `T.Name from TraitExpression` must identify the owner and ordinary instantiation. The qualifier applies to a member projection, not an arbitrary expression. Its exact precedence and grouping must be settled before this RFC can advance beyond Draft, consistently with RFC 144's value-level projections.

Projections must be accepted in type positions, including parameters, return types, generic arguments, type operations, and checked constraints. They must not be interpreted as value-field access. A generic projection that is justified by its bounds may remain symbolic until substitution; an unjustified projection must produce a diagnostic instead of becoming `Unknown` or `Any`.

### Methods, inheritance, and cycles

Trait method compatibility must substitute the inferred bindings. An adopter of `StreamingHash[u64]` must satisfy a `Self.Digest` return requirement with `u64`; a method returning `bytes` must be rejected. An implementation may spell the resolved concrete type or use a valid `Self.Name` projection in its own body and signatures.

Supertraits must retain their ordinary arguments, inferred bindings, and member ownership. A subtrait may inherit an unbound inferred requirement, which concrete adoption must satisfy through an explicit adoption of the owning supertrait. A subtrait may also bind a supertrait's inferred suffix in its adoption header. Conflicting bindings reached through direct or inherited adoptions must be rejected. Inherited members must remain projectable when they resolve uniquely; a subtrait must not silently redeclare a member with an inherited name.

For example, `trait ResettableHash with StreamingHash` inherits the digest requirement. A concrete type may adopt `ResettableHash, StreamingHash[bytes]` to supply it. `trait ByteDigestHasher with StreamingHash[bytes]` instead fixes that requirement for every adopter. The latter is an exact inherited requirement, not a new definition of `Digest` or an override of the owning trait.

Trait cycles, alias cycles, and inferred binding cycles that prevent resolution must produce bounded diagnostics. A circular binding such as an iterator choosing its own unresolved `Self.Item` must not satisfy the requirement by referring back to itself. A finite projection into an independently resolved associated member may be used as a binding.

### Metadata, visibility, and tooling

Checked API metadata, library manifests, generated documentation, semantic inspection, and editor tooling must preserve ordinary parameter roles separately from inferred member roles. They must expose trait-owned member identities, declaration order, concrete or symbolic bindings, the ordinary arguments selecting an implementation, supertrait provenance, exact output requirements, and the source of each resolved projection.

Generated documentation should display the `infer` header and supplied adoption arguments together with method contracts. Hover and go-to-definition should connect `I.Item` to the owning declaration and its concrete binding when known. Diagnostics and semantic queries must use the same checked identities. Tolerant tooling may preserve an explicit unresolved or ambiguous marker, but must not turn that marker into a successful conformance fact.

Existing visibility rules must govern projected members and their bindings. A public conformance or public signature must not expose an otherwise private type merely by routing it through an inferred member. Inferred members must not introduce a new field visibility syntax or grant access to private instance data.

## Design details

### Reading the same brackets in different contexts

The header declares which positions are ordinary inputs and which are inferred members:

```incan
trait Lookup[K, infer Value]:
    def lookup(self, key: K) -> Option[Self.Value]: ...
```

The adoption supplies all positions. A generic bound may leave the inferred suffix unspecified. This context-sensitive difference is deliberate: declarations identify the relationship, implementations supply it, and consumers either recover it or require a particular choice. Documentation and diagnostics must show the parameter roles rather than treating every bracket position as an ordinary generic parameter.

The restriction to an ordinary prefix and inferred suffix keeps positional omission unambiguous. Arbitrary interleaving would require named arguments, placeholders, or an additional argument grouping syntax, none of which is needed for the initial contract.

### Relationship to RFC 141

In RFC 141, `infer element` captures a component of an existing type pattern. Here, `infer Item` declares a member whose binding is recovered from an implementation. Both uses introduce a type name determined through another checked relationship; neither requests runtime value synthesis. They have different scopes and must be represented as distinct constructs in checked metadata.

RFC 141 named operations may consume a valid projection, such as `ElementOf[I.Item]` when the selected item type is an accepted container. This must not make an inferred member a match pattern or permit pattern syntax inside trait declarations. No new structural matching rules for trait adoption arguments are implied by the shared marker.

### Relationship to RFCs 025, 099, and 128

RFC 025 remains the authority for dispatch among ordinary trait instantiations. An inferred member is excluded from implementation identity and must not serve as an overload discriminator. This revision does not change method targeting syntax or permit arbitrary freestanding overloads.

RFC 099 capability families can expose inferred members, such as a storage carrier, using the same checked binding model. Projected requirements in generic method headers must recover those members through active conformance facts. This RFC does not replace RFC 099's ownership or coherence boundaries.

RFC 128 retains `Callable[S]` as its ordinary signature contract. This proposal does not turn that signature into an inferred output, revive the older `Callable[T, U]` shape, or change how callable signatures are selected.

### Relationship to Rust interop

RFC 043's existing body-level associated type declarations for imported Rust traits remain valid. This revision proposes the native trait header surface; it does not silently remove or reinterpret the interop spelling. A header-based facade for imported associated members would require a separately specified metadata and migration contract.

Native inferred members should lower to Rust associated types where possible, while ordinary trait parameters remain Rust generic parameters. For example, the source adoption `Lookup[str, User]` expresses an implementation of `Lookup[str]` whose associated `Value` is `User`; it must not become a Rust trait with both `str` and `User` as independent generic inputs. A backend unable to preserve the relationship must reject it with a clear diagnostic.

### Compatibility and migration

Existing traits without inferred members retain their semantics. Adding or removing `infer` from a parameter changes implementation identity, projection availability, and allowable adoptions; it must be treated as an API change. Public inferred names and their namespace are API as well. Renaming `Item` or changing a binding can break consumers.

Existing generic traits should migrate only when a unique implementation-chosen member is the intended contract. Native body-level declarations from the earlier RFC 098 Draft are superseded by this proposed surface; this statement does not claim that the revised syntax is implemented or deprecate RFC 043 interop support.

### Adjacent trait-system features

Generic associated types would let an inferred member itself accept parameters, choosing a type family rather than one type. Their declaration, projection, and borrowing rules need an additional contract; `infer Item` does not implicitly admit `infer Item[Borrow]`. Associated constants instead expose values, such as a digest width, and require constant-evaluation rules beyond checked type relationships.

Default associated types would allow an implementation to omit a choice. They need explicit rules for default substitution, inherited defaults, and compatibility; this revision requires supplied choices so omission has one meaning. Specialization would choose among overlapping implementations, whereas this revision requires unique bindings without implementation precedence. Trait-object dispatch would additionally require object-safety and erased-representation rules. The header spelling alone settles none of these features. These features remain outside this RFC under its Non-Goals; their scope is not reopened by the infer header spelling. Associated constants are a separate value-level contract under RFC 144. Generic associated types, defaults, specialization, and trait-object dispatch require their own explicit design rather than adoption by implication.

### Diagnostics

Diagnostics should identify the trait owner, selected ordinary arguments, and inferred member involved. Required categories include missing or extra adoption arguments, an ordinary parameter after an inferred member, duplicate or colliding member names, conflicting choices for one implementation key, incompatible method signatures, invalid or ambiguous projections, unsatisfied exact output requirements, and resolution cycles.

A missing-choice diagnostic should suggest an adoption such as `with PullIterator[YourItemType]`, not a native body-level `type Item` declaration. An ambiguity diagnostic should identify the competing trait instantiations. Rust-only corrections should be reserved for explicit interop contexts.

### Expected acceptance

These cases describe required behavior for a future implementation; they are not compiler test results. They assume the trait declarations from the guide.

| Case                                                                                | Expected behavior                                                             |
| ----------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| A concrete adoption of `PullIterator[str]` with a matching `next` method            | Accept; its `Item` is `str`.                                                  |
| A concrete adoption of bare `PullIterator`                                          | Reject; the implementing declaration must supply `Item`.                      |
| One type adopting both `PullIterator[str]` and `PullIterator[bytes]`                | Reject; there are no ordinary arguments distinguishing the implementations.   |
| One type adopting `Lookup[str, User]` and `Lookup[int, User]` with matching methods | Accept; the ordinary key arguments differ.                                    |
| One type adopting `Lookup[str, User]` and `Lookup[str, Team]`                       | Reject; the same ordinary argument cannot select conflicting inferred values. |
| A `Lines` value passed to a parameter bounded by `PullIterator[bytes]`              | Reject; an output requirement cannot change `Lines.Item` from `str`.          |
| `Directory.Value` without selecting one of its lookup instantiations                | Reject as ambiguous, even when both bindings equal `User`.                    |
| A trait header listing `infer Value` before an ordinary `K`                         | Reject; inferred members must form a suffix.                                  |

## Alternatives considered

### Body-level required type declarations

The previous Draft used `type Item` and `type Item = str` in trait and implementation bodies. Those forms align with Rust associated type items but leave the required implementation choice less visible in the trait header. Adding `= ...` makes the missing definition more explicit while retaining a separate declaration mechanism. This revision instead places both the requirement and its supplied argument beside the capability declaration.

### Ordinary generic parameters with an alias

A declaration such as `trait PullIterator[T]` with a named alias for `T` can preserve the convenient `Item` vocabulary. Ordinary generic semantics, however, permit multiple item instantiations for one implementing type under RFC 025. Restricting those adoptions without a visible marker would change ordinary generics by implication. Naming `infer Item` directly states the distinct role and avoids a redundant alias. RFC 083's existing RHS-oriented symbol alias syntax does not itself define projected trait-owned type members.

### Treat every header entry as inferred

Making all trait parameters implementation-chosen would remove the distinction needed by existing capabilities such as multi-key indexing and conversions. The explicit marker lets native associated types coexist with ordinary trait parameters.

### Shape-specific traits or union outputs

Separate traits for byte, 64-bit, and other digest shapes multiply helpers and adapters. Union outputs require runtime narrowing even when the implementing type already fixes the representation. Neither gives generic signatures the exact named relationship expressed by `H.Digest`.

## Drawbacks

Readers must learn that bracket entries can have two roles and that a bound may omit only the inferred suffix, while a concrete adoption must supply it. The `infer` spelling may initially suggest that the compiler also fills missing implementation arguments; examples and diagnostics must explain that implementations supply their choices explicitly.

The marker also has a distinct proposed use in RFC 141 type patterns. A shared intuition does not remove the need to explain each context. Traits with several ordinary instantiations or colliding member names still need disambiguation, whose final projection syntax remains open.

The typechecker and tooling must preserve implementation identity, output uniqueness, substitutions, inherited requirements, visibility, and provenance. The surface is compact, but its semantic checks are substantial. Rust interop temporarily retains a separate source spelling, and migration of ordinary traits is a deliberate API change.

## Implementation architecture

This section is non-normative. Checked metadata should represent ordinary trait parameters and inferred members as distinct roles with stable trait-owned identities. Adoption should resolve supplied arguments into ordinary implementation inputs and associated member bindings. Projection resolution should use active bounds, selected ordinary arguments, and supertrait provenance. Backend lowering should preserve associated relationships until it can emit associated type items or substitute known concrete bindings.

Inferred members should not be represented as runtime fields, synthetic methods, or unchecked strings. Compiler and editor queries should consume the same checked relationships so diagnostics, hover, manifests, and generated documentation agree.

## Layers affected

- **Parser / AST:** contextual `infer` in native trait headers, distinct header parameter roles, adoption arguments, and projections in type positions.
- **Typechecker / Symbol resolution:** implementation keys based on ordinary parameters, inferred output uniqueness, bound argument omission, exact output requirements, inheritance, projection resolution, and method compatibility.
- **IR Lowering:** preservation of checked associated members, selected instantiations, and symbolic projections for backend lowering and diagnostics.
- **Emission:** native inferred members lower to associated type items where possible, with ordinary inputs retained as generic parameters.
- **Stdlib / Runtime:** iterator, hasher, parser, codec, and adapter APIs may adopt the feature; associated types themselves add no runtime storage.
- **Formatter:** trait headers, mixed parameter lists, adoption arguments, and projected types must format consistently.
- **Library manifests / checked API metadata:** preserve parameter roles, member owners, bindings, exact requirements, implementation selection, and degraded-state markers.
- **LSP / Tooling:** completion, hover, go-to-definition, diagnostics, semantic inspection, and explanation commands expose the same type relationships.

## Design Decisions

- Preserve the existing Non-Goals: no generic associated types, default associated types, specialization, or trait-object dispatch is introduced here. Required inferred choices and unique implementation bindings remain authoritative. Value-level associated constants belong to RFC 144.

## Unresolved questions

- What precedence and grouping should `T.Item from TraitExpression` use when composed with other type operations and RFC 144's value-level projections?
- Should this revision admit declaration-time bounds on inferred members, and if so how should they compose with the separate generic constraint syntax?
- Should an adopting subtrait offer a shorthand for supplying inherited inferred members without repeating the owning supertrait in the concrete adoption header?
- Should inferred member names follow nominal type naming conventions only as style, or should any convention be enforced?
- Should a future interop revision expose imported associated types through native header arguments, and what checked metadata and compatibility rules would that require?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
