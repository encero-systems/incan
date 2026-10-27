# RFC 099: Generic trait-targeted methods

- **Status:** Draft
- **Created:** 2026-05-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized integers)
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 025 (multiple generic trait instantiations)
    - RFC 042 (traits are always abstract)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 056 (`std.io`)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 098 (native associated types for traits)
    - RFC 101 (`std.collections.OrdinalMap`)
    - RFC 131 (typed error umbrellas and retained validation failures)
    - RFC 144 (owner-qualified trait contracts)
- **Issue:** <https://github.com/encero-systems/incan/issues/581>
- **Blocks:** <https://github.com/encero-systems/incan/issues/596> (v0.5 RFC 101 trait-system bridge removal)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC introduces generic trait-targeted methods: methods declared inside the owning type body may carry their own type parameters, target a trait instantiation with `from Trait[...]`, and add `where` constraints that make the method define a checked family of trait adoptions rather than one concrete adoption at a time. It also defines how trait-owned capability families can make an existing deterministic type family satisfy a trait without adding Rust-shaped external implementation blocks. The primary motivation is numeric and storage-carrier dispatch: `std.io.BytesIO` should be able to define one `write[T ... from BinaryWrite[T] ...]` method family for exact-width integers and another for storage-backed integer newtypes, while traits such as `OrdinalKey` should be able to own deterministic key conformance for language scalar families.

## Core model

1. **Behavior lives on the owning type:** when a class, model, enum, newtype, or rusttype owns behavior, generic trait-targeted methods are written in that declaration body.
2. **The method header names the trait target:** `from TraitExpression` inside the method generic header says the method satisfies that trait instantiation, not merely that it is an inherent overload.
3. **Generic constraints are part of the adoption family:** bounds such as `T with StorageBackedInteger` and `where T.Storage with FixedWidthInteger` decide which static type arguments the adoption family covers.
4. **No Rust `impl` syntax leaks into Incan:** the backend may emit Rust `impl` blocks, but source uses class bodies, `with`, `from`, and method-local `where`.
5. **Storage dispatch is explicit:** a storage-backed newtype may delegate representation work through `T.Storage`, but the source-level type remains the nominal newtype and no ambient primitive conversion is created.
6. **Overlaps must be diagnosed:** if two generic trait-targeted methods could satisfy the same trait instantiation for the same receiver type and type arguments, the compiler must reject the ambiguity unless another accepted RFC defines specialization.
7. **Traits may own capability families:** when a trait defines a canonical contract over a finite or registry-backed language type family, the trait can expose that conformance through checked capability metadata instead of reopening each existing type declaration.

## Motivation

RFC 009 made exact-width numeric types first-class. RFC 056 then used trait-backed numeric I/O so `BytesIO` could read and write `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `u64`, `i64`, `u128`, `i128`, `f32`, and `f64` without exposing a public matrix of method names. That design is right, but the current language still pressures implementations toward repeated one-type-at-a-time adoptions, hardcoded builtin dispatch, or backend-specific escape hatches. The language has the vocabulary for generic bounds and method-level trait targeting, but it lacks a source form for saying "this class method satisfies a whole generic family of trait instantiations."

RFC 091 adds a second pressure point. A constrained integer newtype such as `Month = newtype int[ge=1, le=12, storage=u8]` is logically a `Month`, not a `u8`, but representation-oriented traits such as binary writing often want to use the compact storage carrier once validation has already happened. The feature needed here is not "treat `Month` as a `u8`." It is a checked way for a method family to say that storage-backed values can participate in a trait when their storage carrier participates in the corresponding representation trait.

Incan should solve owned behavior in the class-body model users already know. Python's class model teaches that a class bundles data and functionality, and Incan's class docs follow that direction: classes are behavior-first, methods are the primary API, and `extends` is reserved for inheritance and method reuse. Adding an external `impl` or `extend Type with Trait` block would solve the backend problem while making the authoring surface less Incan. The language should instead let `BytesIO` teach existing `BytesIO` values the generic trait behavior by putting the method on `BytesIO` itself.

RFC 101 exposed the complementary case. `std.collections.OrdinalKey` owns a deterministic key contract for existing language scalar families such as `str`, `bytes`, exact-width integers, decimal, UUID, dates, and stable value enums. Reopening each type declaration is the wrong mental model, and external `impl` blocks would import Rust's coherence surface into Incan source. The trait should own the canonical conformance family, while the compiler proves membership from language registries, associated type metadata, and the trait contract.

## Goals

- Allow methods in concrete type bodies to declare method-level generic parameters and target a generic trait instantiation in the same method header.
- Allow generic trait-targeted methods to establish a family of trait adoptions for the enclosing type.
- Add method-local `where` constraints for projected associated types and concrete receiver capability checks needed by those adoption families.
- Support numeric family dispatch for exact-width integer and float helper traits without requiring a hand-authored method per primitive type.
- Support storage-carrier dispatch for constrained integer newtypes without weakening RFC 091's logical-type versus storage-carrier boundary.
- Support trait-owned capability families for deterministic registry-backed contracts such as stable ordinal keys.
- Preserve the existing `with TraitName` adoption model, RFC 144’s `from` contract ownership, and RFC 043’s existing concrete method target from RFC 043.
- Reject overlapping adoption families clearly rather than adding specialization by accident.
- Preserve trait adoption families, where-constraint proofs, trait-owned capability-family membership, selected substitutions, and overlap diagnostics in checked metadata, generated docs, LSP views, and package manifests.

## Non-Goals

- This RFC does not add Rust-shaped `impl Trait for Type` source syntax.
- This RFC does not add external class reopening or monkey-patching as a general language feature.
- This RFC does not make `extends` mean trait adoption or method injection; `extends` remains class inheritance and behavior reuse.
- This RFC does not change the representation of bare `int`, which remains the RFC 009 alias for `i64`.
- This RFC does not make storage-backed newtypes subtype or implicitly convert to their storage carrier.
- This RFC does not introduce trait specialization or overlapping implementation precedence.
- This RFC does not require generic associated types.
- This RFC does not add arbitrary package monkey-patching for existing types. Trait-owned capability families are canonical contracts owned by the trait and checked by the compiler, not per-package alternate implementations.

## Guide-level explanation

All new forms and standard APIs described here are proposed, not evidence of current compiler support. Existing implicit validated newtype conversion remains governed by RFC 017 and RFC 091.

### One method, many checked trait instantiations

The following independent module defines every application trait and type. Abstract trait signatures deliberately use ellipses; the application method bodies are complete.

```incan
# Complete module. Render and Sink are application traits.
trait Render:
    def render(self) -> str: ...

trait Sink[T]:
    def put(mut self, value: T) -> str: ...

class Label with Render:
    pub text: str

    pub def render(self) -> str:
        return self.text

class Formatter:
    pub def put[T with Render from Sink[T]](mut self, value: T) -> str:
        return value.render()

def main() -> None:
    mut formatter = Formatter()
    label = Label(text="atoms")
    assert formatter.put(label) == "atoms"
```

`T with Render` limits which values the method accepts. `from Sink[T]` identifies the contract whose `put` this method supplies. `Formatter` therefore satisfies each eligible `Sink[T]` without listing every type in its header. `from` denotes contract ownership, not a value-conversion direction. This new generic-header form follows RFC 144; RFC 043's existing concrete `def method(...) for Trait -> ResultType` form remains valid. This RFC neither deprecates that form nor adds a second concrete `from` spelling.

### Keep ordinary validated input ergonomic

```incan
# Proposed storage-backed newtype and extraction contract.
type Month = newtype int[ge=1, le=12, storage=u8]

def bill_for(month: Month) -> None:
    println(month)

def carrier_for(month: Month) -> u8:
    return month.to_storage()  # Proposed checked storage extraction.

def main() -> None:
    bill_for(4)
    # RFCs 017/091 keep ordinary validated int-to-Month input ergonomics.
    # bill_for(13) is rejected/fails the checked domain construction contract.

    # No new rule makes an arbitrary int automatically fit into u8.
    # to_storage is only for an already validated Month value.
    # It extracts the declared carrier; it does not validate raw input.

# Existing implicit input conversion is retained. The new public operation
# is the reverse representation direction used by numeric writer families.
```

`bill_for(4)` retains ordinary validated input coercion. `to_storage()` is the opposite, representation-oriented direction on an already validated value. It returns the declared carrier and never turns an arbitrary `int` into `u8`. The standard storage capability described below provides it; application code does not restate its inferred Storage binding.

### Read storage without admitting an invalid domain value

`std.io` owns the existing `BytesIO`, `Endian`, `IoError`, `BinaryRead[T]`, and `BinaryWrite[T]` names. The following complete application uses the proposed storage-backed adoption family with that existing typed-read surface:

```incan
from std.io import BytesIO, Endian, IoError

type Month = newtype int[ge=1, le=12, storage=u8]

def read_month(raw: bytes) -> Result[Month, IoError]:
    buffer = BytesIO(raw)
    month: Month = buffer.read(Endian.Little)?
    return Ok(month)

def main() -> None:
    match read_month(b"\x04"):
        Ok(month) => println(month)
        Err(failure) => println(failure)
    # The carrier is 4 and checked Month construction succeeds.

    match read_month(b"\x0d"):
        Ok(month) => println(month)
        Err(failure) => println(failure)
    # 13 fits the u8 carrier but fails Month’s 1..12 constraint.
```

The reader first decodes the declared carrier, then converts it through the checked logical underlying and canonical `from_underlying` validation path. A transport or decoding failure retains its ordinary `IoError`. A failed domain construction returns the proposed `std.io.IoError.InvalidData(original: ValidationError)` case, retaining the original standard RFC 131 validation umbrella as its cause. ValidationError is the standard boundary from RFC 017/RFC 131, not an application-defined text error; its final defining module follows that owning contract. The additional I/O case belongs to `std.io`; it does not invent a second validation mechanism or change a failure into success. Existing I/O operation, position and path context must remain available through the owning I/O diagnostic contract. Current legacy string-backed I/O failures alone do not satisfy the new retained-failure requirement.

The carrier value 13 is representable; the semantic Month is invalid. Both checks matter. This proposed recoverable read must not use the panic behavior of ordinary implicit input coercion, perform a lossy cast, or expose a partially validated newtype.

### Which standard families exist, and who owns them

The proposed `std.derives.numeric` module owns `FixedWidthInteger`, `FixedWidthFloat`, `FixedWidthNumeric`, and `StorageBackedInteger`. The first three are standard marker traits whose automatic membership is checked from the numeric registry. `StorageBackedInteger` has the ordinary RFC 098 inferred-member shape:

```incan
# Proposed standard declaration in std.derives.numeric.
pub trait StorageBackedInteger[infer Storage]:
    def to_storage(self) -> Self.Storage: ...
```

Each supported constrained integer newtype receives its Storage binding from its checked `storage=` declaration. That binding must satisfy FixedWidthInteger; a consumer must not fabricate one from a similarly named member. For `Month`, the binding is `u8`.

| Contract | Concrete reason a type satisfies it | What does not count |
| --- | --- | --- |
| FixedWidthInteger | Canonical exact-width integer identity from the numeric registry | A name containing int or an arbitrary nominal wrapper |
| FixedWidthFloat | Canonical exact-width float identity from the numeric registry | An unrelated type with numeric-looking methods |
| FixedWidthNumeric | Membership in the checked integer or float family | An unchecked package assertion |
| StorageBackedInteger | A validated constrained integer newtype with a checked carrier | Treating every primitive as a storage-backed domain wrapper |
| std.collections.OrdinalKey | The canonical stable encoding contract and scalar metadata owned by RFC 101 | Reopening a scalar in a consuming package |

These are ordinary stdlib-owned traits with compiler-checked conformance facts. This RFC does not add a public decorator or callback that lets arbitrary packages assert automatic scalar-family membership. Users can still author the ordinary owned method families demonstrated by Formatter. The published facts must explain the owning trait, type identity, membership proof, associated bindings and required behavior; callers must not rediscover membership by parsing names. These type conformance facts are distinct from RFC 104 runtime authority grants.

Aliases such as int and i64 denote one canonical family member. Diagnostics retain the author's spelling as the primary explanation and show the canonical equivalence when useful; aliases do not create duplicate implementations.

## Reference-level explanation

### Method header form

A concrete type body may contain a generic trait-targeted method with the form `def name[TypeParams from TraitExpression where Constraints...](params...) -> ReturnType:`. The `TypeParams` portion follows the existing method generic parameter rules. The optional `from TraitExpression` inside the generic header targets the method at a trait instantiation that may refer to the method's type parameters. The optional `where` clauses introduce additional constraints used to decide whether the trait-targeted method applies for a particular substitution.

The `from TraitExpression` form inside the generic header is distinct from the existing concrete method target form `def name(params...) for TraitExpression -> ReturnType:`. The existing form remains valid when the trait target does not need method-local type parameters. A method must not use both target forms at once. If the trait target mentions a method-local type parameter, the target must appear inside the generic header so that the parameter is in scope for the target.

### Adoption-family semantics

A generic trait-targeted method in a concrete type body establishes a trait adoption family for the enclosing type. For each substitution of the method type parameters that satisfies the method's bounds and `where` constraints, the enclosing type is considered to adopt the targeted trait instantiation, and the method body satisfies the corresponding trait method. The compiler must check the method signature against the targeted trait method after substituting the method type parameters, associated type projections, and constraints.

The enclosing type does not need to list every covered trait instantiation in its declaration header. For example, `BytesIO` should not have to spell `with BinaryWrite[u8], BinaryWrite[i8], BinaryWrite[u16], ...` when a single generic trait-targeted method covers the exact-width numeric family. The adoption family is still explicit because it is attached to a method that names `from BinaryWrite[T]` and declares the constraints on `T`.

### Trait-owned capability families

Some traits define a canonical contract for an existing language type family rather than behavior owned by one class body. `OrdinalKey` is the motivating example: the trait owns the deterministic key encoding contract, and `str`, `bytes`, exact-width integers, decimal, UUID, civil date/time values, and stable scalar value enums satisfy that contract when their language-defined representation has a deterministic encoding.

The compiler must model these as checked trait conformance facts, not as ad hoc name checks in each caller. A trait-owned capability family has:

- a fully qualified trait identity;
- a finite or registry-backed type-family predicate;
- required methods, associated type bindings, or constants supplied by the trait contract and language metadata;
- deterministic overlap rules so a type has at most one visible conformance for the same trait instantiation.

Source should not spell Rust-style external implementation blocks for this. The trait owns the intent, the compiler proves that a concrete type is in the supported family, and backend lowering emits whatever target representation is required. If the proof depends on associated types, the associated type bindings must be exposed through the same implementation metadata described by RFC 098.

### Where constraints

A `where` constraint in a method generic header may express trait conformance for a type parameter, an associated type projection, or a concrete type expression. Examples include `where T.Storage with FixedWidthInteger` and `where BytesIO with BinaryWrite[T.Storage]`. A `where` constraint must be checked statically and must not be treated as a runtime guard. If the compiler cannot prove a `where` constraint for a candidate substitution, that substitution does not satisfy the adoption family.

This RFC defines method-local `where` constraints only for generic trait-targeted methods. A later RFC may generalize `where` clauses to functions, type aliases, class declarations, and trait declarations, but this RFC does not require that larger surface.

### Storage-backed integer dispatch

Storage-backed constrained integer newtypes must expose the standard `StorageBackedInteger` capability. That capability must include the RFC 098 inferred type member `Storage`, whose binding is the exact-width integer storage carrier declared by `storage=<integer-carrier>`, and `to_storage(self) -> Self.Storage` for already-validated values to expose their storage representation. A type such as `type Month = newtype int[ge=1, le=12, storage=u8]` therefore satisfies `StorageBackedInteger` with an inferred `Storage` binding of `u8` under RFC 098.

Using `T.Storage` in a generic trait-targeted method must not create ambient primitive conversion. A `Month` remains a `Month`, a `u8` remains a `u8`, and a method that delegates through `value.to_storage()` is explicitly choosing a representation path for an already-validated value. Reconstructing a storage-backed newtype from storage data must continue to use the checked newtype construction path so storage carrier values that are representable but semantically invalid still fail validation.

### Checked binary reading of storage-backed values

The standard storage-backed BinaryRead[T] family must preserve `read(self, endian: Endian) -> Result[T, IoError]`. BinaryWrite[T] likewise retains its published receiver and `write(self, value: T, endian: Endian) -> Result[None, IoError]` contract. A generic implementation's receiver must match the targeted trait; this feature must not silently add or remove mutation permission.

Reading must decode T.Storage, check conversion into T's logical underlying domain, and call the canonical checked construction path. An invalid carrier-to-domain conversion or an Err from from_underlying must remain a recoverable failure, retaining the original ValidationError under `std.io.IoError.InvalidData(original: ValidationError)`. Its causal relationship must identify original under RFC 131; standard I/O context must not be erased. The read must not cast through a truncated primitive, use implicit-coercion panic behavior, discard the typed failure into a message, or expose an invalid T. Transport/decoding errors retain their ordinary I/O identity. This is an additive retained-domain-failure contract for the existing I/O umbrella, not a replacement error framework.

### Coherence and ambiguity

The compiler must reject overlapping generic trait-targeted methods when two methods could satisfy the same trait instantiation for the same enclosing type and substituted type arguments. This RFC does not introduce specialization, priority ordering, or "more specific wins" selection. If a future RFC adds specialization, it may relax this rule with explicit precedence semantics, but the default behavior must be ambiguity rejection.

Call resolution must continue to treat trait-targeted methods as trait behavior, not arbitrary overloads. A direct method call may use the targeted method when existing trait-method resolution can identify the relevant trait instantiation from argument types, expected result type, or an explicit target. If no target can be proven, the compiler must report an ambiguity or missing-method diagnostic rather than trying every method with a matching name as a runtime overload set.

### Metadata and inspection contract

Generic trait-targeted methods must produce inspectable trait adoption facts. Checked API metadata, package manifests, generated documentation, semantic inspection, and LSP surfaces must be able to expose at least:

- the enclosing type that owns the method family;
- the targeted trait instantiation, including method-local type parameters;
- bounds and `where` constraints that decide whether a substitution belongs to the adoption family;
- associated type projections and storage-carrier relationships used by the family;
- trait-owned capability-family predicates and registry-backed membership sources;
- overlap-rejection facts when two adoption families would satisfy the same trait target;
- the concrete substitution selected for a resolved method call when enough type information is available.

These facts are part of the public semantic contract. A package consumer, generated-doc reader, LSP hover, or explanation command should be able to answer why `BytesIO` satisfies `BinaryWrite[Month]`, which storage carrier made that possible, and which constraint would fail if the same call were attempted for a non-storage-backed type. The metadata must preserve the distinction between authored adoption families, trait-owned capability families, and backend lowering choices so generated Rust does not become the only way to understand the behavior.

## Design details

### Syntax

The compact one-line form is:

```incan
def write[T with StorageBackedInteger from BinaryWrite[T] where T.Storage with FixedWidthInteger](...) -> Result[None, IoError]:
    ...
```

The multiline form is preferred when the header contains more than one constraint:

```incan
def write[
    T with StorageBackedInteger
    from BinaryWrite[T]
    where T.Storage with FixedWidthInteger
    where BytesIO with BinaryWrite[T.Storage]
](
    self,
    value: T,
    endian: Endian,
) -> Result[None, IoError]:
    ...
```

The `from` clause belongs inside the generic header because the target trait instantiation may use method type parameters. The `where` clauses also belong in the generic header because they constrain the adoption family, not the runtime body. No colon appears inside the bracket list; `:` remains the block introducer after the return type.

### Numeric family traits

`std.derives.numeric` is the public home of FixedWidthInteger, FixedWidthFloat, FixedWidthNumeric and StorageBackedInteger. Numeric membership must use canonical registry identities: signed and unsigned exact-width integer primitives for the integer family, exact-width float primitives for the float family, and their union for FixedWidthNumeric. Transparent aliases normalize before membership; nominal newtypes must not enter a primitive family merely because of their underlying type. StorageBackedInteger instead applies to the checked RFC 091 constrained-integer declarations carrying explicit storage. The primitive and nominal storage-backed families therefore do not overlap merely by representation. An authored with adoption of a standard automatic numeric/storage family must satisfy that same checked membership predicate; an empty marker body must not let an arbitrary nominal type assert membership.

A standard family uses ordinary trait declarations plus compiler-checked numeric, storage or encoding metadata. The compiler must preserve the source trait's canonical identity and supply every required conformance fact. This RFC admits no unchecked public family-membership annotation, external reopening, arbitrary predicate callback or consumer-specific alternate conformance. The initial standard contracts are the four numeric/storage families and RFC 101's std.collections.OrdinalKey family under its owning deterministic-encoding contract. New automatic families require a separately checked contract rather than a package-level assertion.

### Interaction with `std.io`

RFC 056 deliberately made numeric reads and writes trait-backed rather than a public matrix of method names. Generic trait-targeted methods are the missing authoring surface for that design. `BytesIO` can define one method family for fixed-width numeric writes, one for fixed-width numeric reads where the result type is expected, and one for storage-backed integer newtypes that delegate through their storage carriers.

`int` remains an alias for `i64`, so an `int` value should satisfy the same numeric family membership as `i64` after alias normalization. This does not create a separate `BinaryWrite[int]` implementation alongside `BinaryWrite[i64]`; the trait target should canonicalize through the same alias rules RFC 009 uses for type identity.

### Interaction with associated types

This RFC relies on RFC 098-style associated type projection for `T.Storage`. The Storage member uses RFC 098’s `infer Storage` trait-header declaration and T.Storage projection; a checked adoption supplies its carrier. No separate `type Storage` declaration is added to each newtype body. Generic associated types are not required because `Storage` is one concrete storage carrier per storage-backed integer type.

### Interaction with class inheritance

`extends` remains the class inheritance surface. It creates a new class that inherits fields and methods for behavior reuse and overrides; it does not define trait adoption for an existing class. Generic trait-targeted methods are written in the body of the type that owns the behavior. This keeps the Python-shaped class story clear: use `extends` when defining a new class from an existing class, and use methods in the class body when defining what the class itself can do.

### Interaction with Rust interop

The Rust backend may lower generic trait-targeted methods to generic Rust `impl` blocks when that is the natural target representation, but Incan source must not expose Rust `impl Trait for Type` syntax. Foreign trait and foreign type coherence rules from RFC 043 continue to apply. A generic trait-targeted method must be rejected if its target would require an invalid foreign-trait-for-foreign-type implementation after lowering.

### Diagnostics

Diagnostics should name the trait target, enclosing type, and failed substitution. Useful diagnostics include "generic trait-targeted method overlaps another adoption family," "where constraint `T.Storage with FixedWidthInteger` is not satisfied," "method target `BinaryWrite[T]` is not a trait method target," "method signature is incompatible with targeted trait method," and "storage carrier projection requires `T with StorageBackedInteger`." Diagnostics should avoid suggesting Rust `impl` syntax as the primary fix.

## Alternatives considered

An external `impl Trait for Type` block would be familiar to Rust users and easy to map to generated Rust, but it would violate the Incan direction set by RFC 043: users adopt capabilities with `with` and write behavior in type bodies. It would also split class behavior across declarations in a way that feels less Pythonic for owned types such as `BytesIO`.

An external `extend Type with Trait` block is more Incan-looking than `impl`, but it overloads `extend`/`extends` terminology that already belongs to class inheritance and method reuse. It also blurs whether the feature is reopening a class, adding methods, or declaring trait conformance. This RFC keeps the primary authoring model in the owning type body.

Repeating concrete adoptions for every numeric width would avoid new syntax, but it scales poorly and reintroduces exactly the boilerplate RFC 056 tried to avoid. It also leaves storage-backed newtypes without a principled representation-delegation path.

Letting storage-backed newtypes behave as their storage carrier would make many calls shorter, but it breaks RFC 091's core safety boundary. Storage is representation metadata, not the source-level type. This RFC requires explicit projection through `T.Storage` and value-level conversion through the storage-backed capability.

Adding general method overloading would also solve some call-site ergonomics, but it is too broad for this problem. The needed feature is checked trait conformance over a generic family, not arbitrary runtime-style overload selection by parameter shape.

## Drawbacks

Generic trait-targeted methods make method headers more expressive and therefore more complex. A header such as `def write[T with StorageBackedInteger from BinaryWrite[T] where T.Storage with FixedWidthInteger](...)` carries type parameters, a trait target, and constraints in one place. That is acceptable for capability-heavy stdlib code, but docs should continue to show ordinary methods first and reserve this form for APIs that truly need generic trait adoption families.

The trait solver also has more work to do. It must reason about adoption families, associated type projections, alias-normalized numeric family membership, and overlap detection. The benefit is that this work becomes central and checked instead of being duplicated across stdlib modules and backend codegen branches.

Storage-carrier dispatch may tempt users to think representation and semantics are interchangeable. The language and docs must be blunt: storage dispatch is a representation-oriented delegation tool, not a permission to bypass validation or treat domain newtypes as primitive numbers.

## Implementation architecture

This section is non-normative. A practical implementation should model generic trait-targeted methods as checked trait adoption families attached to the enclosing type's metadata. During typechecking, the method's type parameters, target trait expression, and `where` constraints should be stored together so trait conformance queries can ask whether a concrete trait instantiation is covered. Trait-owned capability families should be stored in the same conformance model, keyed by trait identity and type-family predicate, so generic bounds, method lookup, lowering, metadata export, and diagnostics read the same facts. Backend lowering may then emit a target-language generic implementation, a finite expansion for registry-known families, or another representation that preserves the same checked semantics.

Storage-backed constrained newtypes should expose their storage carrier through checked metadata produced by RFC 091. The storage-backed capability should be derived from that metadata rather than requiring users to restate an additional Storage member declaration manually for every storage-backed newtype.

## Layers affected

- **Parser / AST**: method generic headers must accept `from TraitExpression` and method-local `where` constraints.
- **Typechecker / Symbol resolution**: concrete type metadata must record generic trait-targeted adoption families, trait metadata must record trait-owned capability families, method signatures must be checked against targeted trait methods, `where` constraints must be evaluated, and overlapping families must be rejected.
- **IR Lowering**: lowered method metadata must preserve the targeted trait instantiation and generic constraints so backend dispatch remains checked.
- **Emission**: Rust emission should lower eligible generic trait-targeted methods to valid Rust trait implementations or equivalent generated code while respecting coherence.
- **Stdlib / Runtime (`incan_stdlib`)**: numeric family traits and storage-backed integer traits should be exposed where `std.io`, codecs, serializers, and related APIs need them.
- **Formatter**: multiline generic method headers with `from` and repeated `where` clauses must format stably.
- **Library manifests / checked API metadata**: public adoption-family metadata must preserve trait targets, constraints, capability-family predicates, substitutions, overlap diagnostics, and provenance.
- **LSP / Tooling**: hover, completion, go-to-definition, diagnostics, semantic inspection, and explanation commands should surface the trait target, adoption family constraints, concrete substitutions, and reasons a substitution was accepted or rejected.
- **Documentation**: trait, class, numeric, newtype, and `std.io` docs should explain when generic trait-targeted methods are appropriate.

## Design Decisions

- New generic targets use from inside the method generic header, next to their subject bounds. RFC 043's existing concrete method-level for target stays valid; this RFC does not deprecate it or silently add another concrete spelling.
- Method-local where constraints remain scoped to trait-targeted adoption families. General trailing where clauses on unrelated functions/types are not introduced.
- Standard numeric family names and registry-backed membership are retained. Numeric/storage traits live in std.derives.numeric, while OrdinalKey retains RFC 101's std.collections ownership. Standard automatic membership uses checked trait/registry facts rather than a new unchecked public annotation.
- Existing validated input coercions remain available. to_storage() explicitly extracts the declared carrier from an already validated value and preserves the nominal domain distinction; Storage is an implementation-chosen member under RFC 098.
- Binary reading is included. It decodes the carrier and performs checked logical construction, returning the retained validation failure through IoError.InvalidData rather than panic, text conversion or unchecked wrapping.
- Alias-equivalent types share one canonical adoption. Diagnostics prefer authored spelling and may explain its canonical equivalence without creating duplicate trait implementations.
- The family-membership explanation must distinguish authored owned method families from standard automatic scalar conformance and identify their actual declaring modules and proof sources.
