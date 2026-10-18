# RFC 132: Frozen classes and models

- **Status:** Planned
- **Created:** 2026-10-09
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 008 (const bindings and frozen values)
    - RFC 024 (extensible derive protocol)
    - RFC 027 (vocabulary registration and desugaring)
    - RFC 087 (complete model contract composition)
    - RFC 122 (canonical model construction and validation)
    - RFC 143 (clone with checked field overrides)
    - RFC 129 (shared bindings and the `mut` parameter contract)
    - RFC 131 (immutable first-class error records)
    - RFC 133 (literal types and compile-time value sets)
- **Issue:** [#2245](https://github.com/encero-systems/incan/issues/2245)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce frozen classes and models whose instance fields are fixed after successful construction, enforced by the compiler through methods, aliases, retained behavior, composition, and imported APIs. The authoring spelling is `@derive(Frozen)`. Freezing belongs to the declaration, so every completed instance is frozen; a mutable binding can still be replaced with a new instance. Freezing is shallow and does not grant equality, hashing, copying, serialization, or deep immutability. RFC 131's native error records would use this mechanism internally without requiring an annotation in authored error declarations.

## Motivation

A class's private fields protect access from callers, but its methods can still replace them. An immutable binding restricts access through one name, but a mutable binding can expose mutating behavior. Neither expresses that an instance's fields must remain fixed throughout its lifetime.

Error records make that distinction concrete. A failure should retain the payload and declared upstream relationships selected at construction, even when it passes through methods or aliases. At the same time, a fixed diagnostic handle may support runtime initialization of its contained state. That needs a reusable shallow freezing contract rather than a collection of error-specific assignment checks.

Other classes benefit from the same guarantee: coordinates, operation descriptions, and configuration values can offer behavior while requiring callers to construct a replacement instead of editing their fields. Authors should not need to use models merely to obtain this behavior.

## Goals

- Express fixed instance fields on ordinary classes and models with a concise declaration.
- Enforce the guarantee at compile time, including inside the declaring class and through aliases.
- Preserve normal construction, field visibility, methods, and ownership rules.
- Distinguish replacing a mutable binding from replacing a frozen instance's fields.
- Define inheritance and provider boundaries that preserve the guarantee.
- Provide the reusable mechanism needed by RFC 131's candidate class-based lowering.

## Non-Goals

- A runtime `freeze(value)` transition or thawing API.
- Recursive conversion of retained objects or collections into deeply immutable values.
- A read-only view type for otherwise mutable instances.
- Automatic equality, hashing, thread safety, or new serialization capabilities.
- Changing unmarked ordinary declarations, adding model inheritance, selecting mixed field-level freezing, or extending this marker to enums or newtypes.
- Compiler implementation or a claim that the examples already compile.

## Guide-level explanation

The examples describe proposed behavior. `Frozen` is a proposed built-in derive marker, available alongside the existing built-in derives. Its `@derive(Frozen)` authoring spelling is settled; the behavior remains proposed rather than implemented.

### Construct a value, then construct its replacement

```incan
@derive(Frozen)
class Coordinate:
    x: int
    y: int

    def describe(self) -> str:
        return f"({self.x}, {self.y})"

    def translated(self, dx: int, dy: int) -> Coordinate:
        return Coordinate(x=self.x + dx, y=self.y + dy)

def main() -> None:
    mut position = Coordinate(x=2, y=3)
    position = position.translated(1, -1)
    println(position.describe())  # (3, 2)
```

The constructor uses normal named fields. `translated` returns another frozen instance. Replacing the owned local `position` is allowed because the binding is mutable; the previous instance's fields were never edited.

Fields retain normal class visibility. These fields are private, so callers use methods. If an ordinary class author declares a field `pub`, freezing makes it readable but still prevents replacement. This does not add field visibility modifiers to RFC 131's separate error syntax.

### Methods cannot bypass freezing

This declaration is rejected:

```incan
@derive(Frozen)
class Coordinate:
    x: int
    y: int

    def translate(mut self, dx: int, dy: int) -> None:
        self.x += dx  # rejected: Coordinate.x is frozen
        self.y += dy  # rejected: Coordinate.y is frozen
```

`mut self` supplies normal mutation permission, but it does not remove the class's guarantee. Privacy does not exempt a field from freezing. The diagnostic should identify the frozen field and suggest constructing a replacement when appropriate.

### A writable alias would also replace the field

Under RFC 129, a mutable local initialized from a field borrows that field for writing. It does not make an independent copy. That makes the following declaration invalid:

```incan
@derive(Frozen)
class Coordinate:
    x: int
    y: int

    def replace_x(mut self, next_x: int) -> None:
        mut x = self.x  # rejected: writable alias to a frozen field
        x = next_x
```

The alias is rejected where writable access is requested, rather than letting a later assignment escape the freeze check. Read-only borrowing and explicit cloning retain their ordinary meanings. A clone of a frozen instance is another frozen instance.

### Shallow freezing fixes the field, not every retained value

A frozen class may contain a `list[T]` or an ordinary class. Their types do not automatically become frozen types. But shallow freezing also does not grant unrestricted writable access to their field storage: a writable alias to a list field could replace the whole list.

```incan
@derive(Frozen)
class RowBatch:
    rows: list[str]

    def count(self) -> int:
        return len(self.rows)

    def replace_rows(mut self, replacement: list[str]) -> None:
        self.rows = replacement  # rejected: replaces the frozen field
```

Contained state can change only through access permitted by both its own type and the existing mutation rules, without granting replacement of the frozen field. An interior mutable handle is a useful example: changing the state behind the handle need not replace the handle. Normal receiver, alias, and borrowing requirements still apply; freezing supplies no new permission to mutate through a read-only name.

Ordinary list and child-object contents may be mutated through checked access that cannot replace the containing field. For example:

```incan
@derive(Frozen)
class Batch:
    rows: list[str]

    def add_row(mut self, row: str) -> None:
        self.rows.append(row)  # allowed: changes contents, preserves the field

    def replace_rows(mut self, replacement: list[str]) -> None:
        self.rows = replacement  # rejected: replaces the frozen field
```

The compiler must establish that the called operation preserves the containing field and that ordinary receiver and ownership permissions permit the content mutation. Imported APIs must carry the checked contract needed to enforce that distinction. Unknown behavior is insufficient; a general writable alias remains rejected because it permits replacement. No new authoring syntax is introduced for the direct call shown here. Choosing FrozenList[str] instead prohibits collection mutation through its existing read-only API. A fixed field does not promise a stable snapshot of independently mutable contents.

### Compose with existing frozen types

Incan already has `FrozenStr`, `FrozenBytes`, `FrozenList[T]`, `FrozenSet[T]`, and `FrozenDict[K, V]`. These types supply read-only value or collection APIs, independently of the containing class. A frozen class adds a guarantee about its own fields; it does not replace those types.

```incan
const INPUT_LABELS: FrozenList[str] = ["atoms", "quarks"]

@derive(Frozen)
class InputSpec:
    labels: FrozenList[str]
    attempts: int

    def label_count(self) -> int:
        return len(self.labels)

def main() -> None:
    spec = InputSpec(labels=INPUT_LABELS, attempts=3)
    println(spec.label_count())  # 2
```

The two guarantees compose: `InputSpec` methods cannot replace `labels`, and the existing `FrozenList` API rejects appending or replacing an element. Declaring that field on an ordinary mutable class would still protect the collection itself, while a permitted method could replace the field with another `FrozenList`.

Existing frozen collections are distinct from mutable collections. Applying `Frozen` to a class with a `list[T]` field must not silently change the field to `FrozenList[T]`; authors choose the contained type explicitly. The existing frozen collection names are not callable Incan constructors, so this example uses a supported `const` value rather than inventing `FrozenList(...)` construction.

The existing `const` mechanism validates compile-time data and recursively freezes supported baked strings, bytes, and containers. That stronger guarantee comes from the const initializer and nested value types, not a rule that every `FrozenList[T]` recursively freezes an arbitrary `T`. Frozen classes may be constructed at runtime, and this RFC does not make their constructors const-evaluable, add runtime frozen-collection factories, or introduce a `FrozenClass[T]` view. For the current contracts, see [Frozen collections](../language/reference/frozen_collections.md) and [Const bindings](../language/explanation/consts.md).

RFC 133 separately proposes `Literal["atoms", "quarks"]` and `Literal[*INPUT_LABELS]` to constrain a field to selected values. Such a type governs which values are admissible; freezing governs whether the field can be replaced. Literal constraints and their runtime conversion rules do not form part of this RFC's freezing mechanism.

### Inheritance must preserve the guarantee

A frozen class may extend an ordinary base when the compiler can establish that all retained behavior preserves the frozen child's fields. The base need not adopt Frozen itself:

```incan
# point_lib.incn: ordinary application/library declaration.
pub class Point:
    x: int

    pub def x_value(self) -> int:
        return self.x
```

```incan
# app.incn: imports the library declaration above.
from point_lib import Point

@derive(Frozen)
class LocatedPoint extends Point:
    y: int

    def describe(self) -> str:
        return f"({self.x_value()}, {self.y})"
```

LocatedPoint fixes all inherited and local fields. The checker must verify its retained inherited methods, aliases and trait contracts under that guarantee, using checked provider evidence when the base is imported. The ordinary Point remains unchanged. Inheritance remains behavior reuse, not nominal subtyping or permission to expose a writable base view of a frozen child.

An inherited setter is a real conflict, rather than a reason to require the base's marker universally:

```incan
class MutablePoint:
    x: int

    def set_x(mut self, value: int) -> None:
        self.x = value

# Adding Frozen to a child retaining this setter is rejected.
# An ordinary explicit safe override can instead construct a replacement:
@derive(Frozen)
class Snapshot extends MutablePoint:
    y: int

    def set_x(mut self, value: int) -> None:
        self = Snapshot(x=value, y=self.y)
```

The override follows RFC 129's whole-receiver replacement contract; it does not edit a field of the old instance. It must satisfy ordinary override and inherited trait obligations. The compiler must not silently hide or rewrite an unsafe inherited method. If retained behavior cannot be verified or safely replaced, the frozen child declaration is rejected with a diagnostic identifying the operation, field and available source or provider evidence. Opaque imported behavior is not proof of safety.

A descendant of a frozen base must still explicitly retain the marker:

```incan
@derive(Frozen)
class NamedOperation:
    name: str

    def label(self) -> str:
        return self.name

@derive(Frozen)
class ReadOperation extends NamedOperation:
    path: str

    def describe(self) -> str:
        return f"{self.label()}: {self.path}"

# An unmarked descendant of NamedOperation is rejected.
```

ReadOperation constructs all inherited and local fields before exposure. Its parent methods retain ordinary privacy rules. An ordinary base can therefore feed an independently frozen child, while an already frozen base cannot lose its explicit guarantee in a descendant.

### Frozen models retain canonical construction

The same marker applies to a model's stored fields. RFC 122 normalization and checks run against pending construction state before a successful frozen value becomes available:

```incan
const INPUT_LABELS: FrozenList[str] = ["atoms", "quarks"]

@derive(Frozen, Clone)
model InputSpec:
    labels: FrozenList[str] = INPUT_LABELS
    attempts: int = 1

    @fieldmethod
    def attempts(mod, value: int):
        mod.attempts = value + 1

def main() -> None:
    mut spec = InputSpec(attempts=2)
    assert spec.attempts == 3
    updated = spec.clone(attempts=4)
    assert updated.attempts == 5
    spec = updated  # Whole mutable binding replacement remains allowed.
    # spec.attempts = 6 is rejected: completed frozen model field.
```

The example uses the proposed fieldmethod contract and RFC 143 keyword cloning; all declarations are application-defined. Frozen does not itself derive Clone, change defaults into explicit inputs, or bypass normalization. If registered checks or fallible factories require `Result[InputSpec, ValidationError]`, every construction and keyword-clone spelling retains that result and returns a frozen value only on success. Decoders and generated constructors obey the same finalization boundary. The guarantee remains shallow; contained values keep their own mutation and capability rules.

### Model composition preserves freezing

Model from composition is distinct from class extends. A frozen target may compose ordinary model sources, but a source already promising Frozen requires an explicitly frozen target:

```incan
model ConnectionDefaults:
    timeout: int = 10

@derive(Frozen)
model ImportSettings from ConnectionDefaults:
    retries: int = 3

@derive(Frozen)
model FrozenDefaults:
    timeout: int = 10

@derive(Frozen)
model ArchiveSettings from FrozenDefaults:
    retries: int = 3

# An unmarked model MutableArchive from FrozenDefaults is rejected.
```

RFC 087 complete contract composition must retain that declaration guarantee as well as stored fields, normalization and validation. Requiring Frozen on the target avoids a mixed field-level mode and preserves a simple whole-model contract. This does not introduce model inheritance, inherited utility methods, nominal subtyping, or implicit thawing.

### Native errors use freezing internally

RFC 131 authors continue to write dedicated error declarations:

```incan
error InputError:
    InvalidInput:
        atoms: int
        quarks: int
```

Its candidate desugaring may mark the generated case class with `@derive(Frozen, Error)` and use a frozen `BaseError`. Generated payload fields and retained declared causes stay fixed. A fixed private diagnostic handle can support runtime initialization of its contents through checked internal APIs. Freezing does not determine message formatting, cause traversal, or the public visibility of native error payloads; those contracts belong to RFC 131.

## Reference-level explanation

### Declaration and construction

1. A class or model marked with the proposed built-in `Frozen` derive must produce only frozen completed instances, including generic instantiations and instances constructed by factory methods or supported generated constructors.
2. Normal named constructor arguments, inherited or composed fields, defaults, canonical normalization and validation, and access rules must continue to apply. Freezing must not introduce an authored initialization method or a publicly accessible construction phase with writable fields.
3. Construction must establish all fields and fulfill its canonical finalization obligations before exposing a successful completed instance. Fallible model construction must retain its declared Result type; Frozen must not add an unchecked success path. No subsequent authored operation may replace or remove one of those fields.
4. Freezing must be a checked declaration property, not an ordinary trait implementation that an author can override to regain write access. Applying a similarly named library derive must not impersonate the built-in marker.
5. Repeated application of the built-in marker must have no additional effect. Applying it to an unsupported declaration kind must produce a source diagnostic.

### Writes, aliases, and receivers

1. Direct assignment, compound assignment, and any equivalent operation replacing a frozen field must be rejected, including in the declaring class's methods.
2. An operation must not obtain a writable alias, parameter access, or receiver access that permits replacing a frozen field. Derived aliases and closures must preserve that restriction. Unknown behavior must not be treated as proof that replacement is impossible.
3. A mutable binding or a `mut` parameter holding a whole frozen instance may be replaced with a new compatible instance where existing ownership and borrowing rules allow it. That replaces the containing place, not a field within the old instance.
4. A `mut self` method must not replace a frozen field. Replacement of the whole receiver must follow the same existing whole-value assignment rules as other caller-visible `mut` parameters; freezing must not introduce a special receiver exception. Such a method may perform other permitted effects, subject to the contained-state rules below.
5. Read access, read-only aliases, pattern inspection, and consumption of a whole value must retain existing semantics. Freezing must not introduce silent clones, allocation, or alias retargeting to make an otherwise invalid operation compile.
6. Clone and copy operations, where independently supported, must preserve the frozen declaration property. Neither operation may produce a thawed instance of that type. Keyword cloning must construct and finalize a complete replacement under RFC 143 rather than assign into completed frozen fields.

### Contained state

1. Freezing must be shallow. A field's declared type must not be recursively changed, and retained values must not acquire new capability requirements solely because the containing class is frozen.
2. Mutation of ordinary list or child-object contents must be permitted when checked access proves the operation cannot replace the containing frozen field and all ordinary mutation and ownership requirements are satisfied. Direct method calls may use that restricted access without new authoring syntax. Freezing must not itself grant mutation through a read-only receiver.
3. A general writable borrow of a frozen field must be rejected even when the author's intended operation is only a nested mutation: the borrow itself permits replacement. Checked content-only access must prove and preserve the narrower permission across calls, trait behavior, and imports. Provider metadata must retain the required guarantee; unknown behavior must be rejected rather than treated as preserving the field.
4. Interior mutable state must not exempt the enclosing field from freezing or grant mutation through a source access that otherwise lacks permission.
5. Runtime or generated initialization of contained diagnostic state must not grant authored code a way to replace frozen payload fields or declared causes. Generated code must obey the same field guarantee after construction.
6. Existing frozen field types must retain their own read-only contracts, conversions, and construction restrictions. Neither a mutable binding nor a mutable receiver may thaw them. The class marker must not implicitly convert mutable field types to frozen types or make a class constructor const-evaluable.

### Inheritance, traits, and providers

1. A frozen class may extend an ordinary base only when every retained inherited operation, alias and trait contract is verified to satisfy the frozen receiver restrictions. All inherited and local fields must be fixed. Unsafe retained behavior requires an explicit ordinary safe override satisfying its other obligations; otherwise the child declaration must be rejected. Unknown imported behavior must not be treated as safe. The compiler must not silently hide or rewrite inherited methods. A descendant of a frozen class must explicitly retain Frozen; an unmarked descendant must be rejected.
2. Inherited methods, overrides, adopted trait methods, and trait default methods must satisfy the frozen field restrictions. Adoption of a trait requiring writable replacement of a field must be rejected.
3. Privacy must remain an independent access rule. Freezing must not make private fields public or make existing public fields unreadable.
4. Imports, aliases, and re-exports must preserve the declaration's frozen property. Provider metadata and generic checking must not erase it or permit a consumer to regain writable access to a frozen field.
5. Interop must not expose a safe foreign API that replaces frozen fields of an existing instance. An unsupported operation must be refused at the Incan boundary rather than accepted and left to an incidental backend failure. This RFC does not define guarantees for foreign unsafe code that violates an API's contract.

### Model composition

1. Frozen must be accepted on models without adding model inheritance or changing their value and visibility contracts.
2. A frozen target may compose ordinary sources through RFC 087 from composition, provided the complete retained construction and behavior contract satisfies freezing.
3. Composing any frozen source must require an explicitly frozen target. An unmarked target must receive a diagnostic at the composition and source guarantee; neither schema metadata nor source ordering may erase it.
4. The target guarantee applies to every stored field, including local fields and fields from ordinary sources. This RFC must not introduce a mixed field-level freezing mode.
5. Model fieldmethods and canonical finalization operate on pending state before publication. Supported decoding, foreign construction and keyword cloning must preserve normalization, registered validation, fallibility and the frozen successful result. They must not publish an intermediate mutable or partially validated model.

### Derives and capabilities

`Frozen` must not imply `Eq`, `Hash`, `Copy`, serialization, or thread safety. Existing class `Clone` and `Debug` requirements must continue to apply; freezing must not make unsupported fields admissible. Other derives and handwritten methods must retain their own capability and conflict rules. In particular, shallow freezing alone must not be accepted as proof that hash-relevant contents cannot change.

## Design details

### Why a declaration property

Freezing every instance at construction avoids mixed mutable and frozen states for one type, runtime freeze failures, and outstanding writable aliases during a transition. There is no need to spell a second frozen type at each use site. A mutable binding still serves its existing purpose when an algorithm replaces one complete value with another.

The selected `@derive(Frozen)` spelling reuses an existing declaration surface. It requires compiler enforcement beyond generated methods: ordinary trait adoption cannot prevent assignments or writable aliases. It is not Rust's `Freeze` marker, and it does not inherit that marker's semantics.

### Relationship to RFC 129

RFC 129 gives writable aliases one consistent target: assigning through an alias replaces the borrowed place. This proposal preserves that meaning and refuses aliases whose target is a frozen field. It does not redefine such an alias as a copy or invent a read-only interpretation of `mut`.

Whole-value replacement remains available through owned mutable locals and permitted caller-visible parameters, including receivers under RFC 129's proposed parameter contract. A new instance replaces the old value; this does not grant access to replace individual fields of the old instance. Returning a newly constructed value is often the clearest API, but freezing does not impose a separate assignment meaning on receivers.

### Prior art

Python's [frozen dataclasses](https://docs.python.org/3/library/dataclasses.html#frozen-instances) emulate read-only instances by rejecting field assignment at runtime. They provide a familiar authoring model, but this proposal places the guarantee in Incan's checker and does not adopt Python's generated equality or hashing choices.

Rust's [interior mutability types](https://doc.rust-lang.org/std/cell/index.html) illustrate why a fixed field or shared reference need not mean recursively immutable contents. Rust's [experimental `Freeze` marker](https://doc.rust-lang.org/std/marker/trait.Freeze.html) concerns interior mutability and static placement; it is not the source-level frozen class contract proposed here. This RFC does not depend on that unstable trait.

### Compatibility

Existing unmarked classes and models retain their behavior. Adding freezing to a published declaration removes supported mutation operations and may break callers or descendants; it is an intentional API change. Moving a definition behind an alias or re-export must preserve freezing just as it preserves its other checked declaration properties.

This RFC introduces no new reflection API and does not make classes into schema models. Serialization support, where independently available, must construct complete frozen values rather than expose a thawed result for post-construction field filling.

## Alternatives considered

- **An immutable local binding:** restricts one access path and already exists, but does not prevent the declaring class or another permitted mutable path from replacing fields.
- **Private fields with getters:** preserves encapsulation, but internal methods and generated behavior still need a reusable no-write guarantee.
- **Runtime `freeze(value)`:** permits a mutable construction workflow, but needs transition semantics for aliases, outstanding borrows, and mixed states. Construction-time freezing avoids those obligations.
- **Deep freezing:** gives a stronger snapshot contract, but requires recursive treatment of collections, foreign objects, handles, and cycles. That is a separate feature rather than an implicit consequence of freezing fields.
- **`frozen class` or `@frozen`:** may describe a checked property more directly than a derive, but would add another declaration form. The selected `@derive(Frozen)` spelling reuses the existing authoring surface while requiring compiler enforcement of the checked property.
- **Require every base of a frozen child to be frozen:** easier to check, but excludes ordinary library bases even when retained behavior is safe. The selected policy verifies retained behavior and requires explicit safe overrides or rejects the child; it does not promise every base can be frozen.
- **Keep Frozen class-only:** narrower, but leaves model authors without the same fixed-field contract. The selected model extension preserves canonical construction and complete from composition rather than requiring models to stand in for classes or adding inheritance.

## Drawbacks

Shallow freezing is weaker than a stable snapshot and can surprise authors retaining mutable state. Checked content-only access requires the compiler and imported method contracts to distinguish contained mutation from field replacement. A general writable alias remains rejected even when the author intended only a harmless nested mutation. These checks must preserve ordinary receiver and ownership requirements without silent copies or unrestricted writable field access.

Freezing an ordinary base in a child requires checked inherited-behavior and provider evidence; not every mutable library base is usable. Frozen model composition also requires retaining the declaration guarantee alongside its field and construction contracts. Diagnostics and documentation also need to distinguish a valid replacement of a complete value from a prohibited replacement of one of its fields.

The derive spelling may suggest generated trait methods even though enforcement belongs in the checker. Imported metadata and generated declarations must carry the property reliably; a declaration-only check would leave aliases and consumer APIs unprotected.

## Implementation architecture

This section is non-normative. The recommended shape is a declaration property retained through vocabulary expansion, typechecking, generic instantiation, and provider metadata. Assignment and access checking enforce it in authored source. Generated error case classes can opt into the same property rather than use a parallel error-only rule.

Ordinary backend structs may represent frozen classes; backend binding mutability alone does not express the Incan guarantee. Emission and interop exposure must preserve checked access, including construction and the absence of safe field-replacement APIs. There is no need for a runtime flag or a dependency on Rust's unstable `Freeze` marker.

## Layers affected

- **Declaration / derive protocol:** a checked frozen class or model property and its selected authoring spelling.
- **Typechecker / ownership checking:** field replacement, receiver behavior, aliases, checked inherited and trait methods, complete model composition, contained access, and generic uses.
- **Provider metadata / symbol resolution:** preservation of freezing across compiled libraries, aliases, and re-exports.
- **Vocabulary / lowering / emission:** frozen generated declarations, completed construction, and preserved restrictions at foreign boundaries.
- **LSP / diagnostics / documentation:** visible declaration guarantees, source diagnostics for prohibited writes, and examples distinguishing field freezing from binding mutability.

## Design Decisions

- The authoring spelling is `@derive(Frozen)`, using the proposed built-in Frozen marker alongside existing derives. The compiler enforces the class or model property; generated methods alone do not establish the guarantee. No separate `@frozen` decorator or `frozen class` form is introduced.

- A frozen class may extend an ordinary base when checked inherited behavior, aliases and trait contracts preserve every frozen field. Unsafe retained operations need an explicit safe ordinary override or the child declaration is rejected. Unknown imported behavior is insufficient proof. Every descendant of a frozen base must explicitly retain Frozen.

- Frozen also applies to models after successful canonical construction, preserving normalization, validation, fallibility, cloning and shallow contained-state semantics. A frozen target may compose ordinary model sources; a target composing a frozen source must explicitly retain Frozen. No model inheritance or mixed field-level freezing mode is introduced.

- Shallow freezing permits mutation of ordinary list or child-object contents through checked access that cannot replace the containing field. Direct calls such as `self.rows.append(row)` retain familiar syntax, subject to ordinary receiver and ownership permissions. General writable aliases remain prohibited. The guarantee must survive trait calls and imported APIs through checked metadata; unknown behavior does not establish permission. FrozenList and other existing frozen value types retain their own read-only contracts.
