# RFC 042: Traits Are Always Abstract

- **Status:** Implemented
- **Created:** 2026-03-18
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language baseline)
    - RFC 023 (compilable stdlib and generic trait bounds)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 026 (user-defined trait bridges)
    - RFC 028 (trait-based operator overloading)
- **Issue:** [#179](https://github.com/dannys-code-corner/incan/issues/179)
- **RFC PR:** [#180](https://github.com/dannys-code-corner/incan/pull/180)
- **Written against:** v0.1
- **Shipped in:** v0.2

## Summary

This RFC makes two language-level changes to Incan's trait system. First, every `trait` is inherently abstract: a trait name is always a valid abstract annotation type, trait adoption always introduces assignability from the adopter to the adopted trait, and generic trait instantiations in type position follow the same abstract-supertype meaning. Second, traits may adopt other traits using the same `with` syntax that concrete declarations use, establishing supertrait relationships and enabling trait stratification. Together these changes give Incan a coherent, Pythonic model for capability hierarchies without introducing new keywords or declaration forms.

## Core model

Read this RFC as seven rules:

1. A `trait` is never concrete or directly constructible.
2. A trait name or trait instantiation is valid in type position.
3. `class`, `model`, `enum`, and `newtype` declarations that adopt a trait with `with` become nominal subtypes of that trait instantiation.
4. A trait may adopt one or more other traits with `with`, establishing a supertrait relationship.
5. A value typed as a trait is viewed through that trait's declared surface — including methods inherited from supertraits — not through ad hoc knowledge of its hidden concrete adopter.
6. Supertrait assignability is transitive: if `Sub` adopts `Mid` and `Mid` adopts `Base`, then any adopter of `Sub` is also assignable to `Mid` and `Base`.
7. No new syntax is required for this model; `trait` and `with` already carry the user-facing meaning that other languages label "abstract" and "extends".

Everything else in the RFC follows from those rules: assignability, generic trait annotations, trait stratification, diagnostics, and the removal of any need to explain a separate notion of a "non-abstract trait" to users.

## Motivation

### Traits already read as abstract to users

Incan documentation already teaches traits as reusable capabilities that concrete declarations opt into with `with`. That user model already implies abstractness: a trait is not something you instantiate, it is something concrete declarations satisfy.

Today that model is only partially reflected in the language semantics. A simple trait name can sometimes behave like an annotation supertype, but the language does not yet state the rule clearly and generic trait instantiations do not consistently inherit the same meaning. This creates an avoidable mismatch between what authors think a trait is and what the compiler currently guarantees.

### "Non-abstract trait" is not a useful user-facing category

If Incan introduced a distinction between `trait` and `abstract trait`, users would have to learn why one trait can appear in annotations as a supertype and another trait cannot, even though both are adopted with the same `with` syntax and both define reusable capability surfaces. That distinction is not obvious, not Pythonic, and not aligned with the existing teaching story for traits.

### Generic traits make the gap more visible

The confusion becomes sharper once traits carry type parameters. If a library writes a generic trait such as `Collection[T]`, authors naturally expect `Collection[Order]` to mean "any concrete adopter whose element type is `Order`". If the language instead treats that spelling as a nominal generic type unrelated to trait assignability, the surface syntax becomes misleading.

### The language should have one coherent trait story

This RFC makes the trait model explicit and uniform:

- traits are abstract by definition
- adoption creates subtype compatibility
- generic trait instantiations keep that same meaning
- the language does not introduce a second keyword just to restate what `trait` already implies

### Trait stratification requires trait-on-trait adoption

Libraries frequently need capability hierarchies: a base trait defines the shared surface, and narrower traits extend it with additional operations that only some adopters support. Without trait-on-trait adoption, every narrower trait is unrelated to the base trait and the compiler cannot verify that an adopter of the narrower trait is also assignable to the base trait. That forces users to adopt every level of the hierarchy separately and manually, which is error-prone and defeats the purpose of the hierarchy.

The same `with` syntax that concrete declarations use to adopt traits is the natural way for a trait to declare its supertraits. This keeps the language surface uniform and avoids introducing a separate "extends" or "inherits" keyword.

## Goals

- Make `trait` the canonical abstract capability declaration form in Incan.
- Make trait names and trait instantiations valid abstract annotation types.
- Define trait adoption as creating nominal assignability from adopter to trait.
- Extend that rule to generic traits and multi-instantiation trait adoption.
- Allow traits to adopt other traits with `with`, establishing supertrait relationships.
- Make supertrait assignability transitive.
- Preserve Incan's Python-first surface by avoiding Rust-style hidden generic syntax in ordinary user code.
- Clarify the user mental model for traits in docs, diagnostics, and future RFCs.

## Non-Goals

- Introducing `abstract trait` as a required new syntax.
- Introducing abstract `class`, abstract `model`, or abstract `enum` in this RFC.
- Making traits sealed or closed-world by default.
- Introducing structural protocols or duck-typed interface matching.
- Introducing associated types in this RFC.
- Requiring runtime trait objects or boxed dynamic dispatch as the only implementation strategy.
- Redesigning derives or the stdlib trait catalog.

## Guide-level explanation

The user-facing rule is simple: a trait is an abstract type that concrete declarations can adopt.

If a value is annotated with a trait, any concrete adopter of that trait is accepted.

```incan
trait Drawable:
    def draw(self) -> None

class Circle with Drawable:
    radius: float

    def draw(self) -> None:
        println("circle")

def render_one(value: Drawable) -> None:
    value.draw()
```

In the example above, `render_one` accepts `Circle` because `Circle` adopts `Drawable`.

This same rule applies to generic traits.

```incan
trait Collection[T]:
    def first(self) -> T

class BoxedList[T] with Collection[T]:
    items: List[T]

    def first(self) -> T:
        return self.items[0]

def first_order(values: Collection[Order]) -> Order:
    return values.first()
```

`first_order` accepts any concrete adopter of `Collection[Order]`. Authors should not have to rewrite that API as a hidden generic bound just to express the concept "any collection of orders".

Traits remain open to library authors unless another RFC says otherwise. A third-party library may define its own type that adopts `Collection[T]` and should receive the same assignability benefits.

### Trait stratification

Traits can adopt other traits using the same `with` syntax. This creates a capability hierarchy where narrower traits extend broader ones.

```incan
trait DataSet[T]:
    def filter(self, predicate: Expr[bool]) -> Self
    def select[U](self, projection: Projection[T, U]) -> DataSet[U]
    def limit(self, n: int) -> Self

trait BoundedDataSet[T] with DataSet[T]:
    def order_by(self, key: SortKey[T]) -> Self
    def collect(self) -> DataFrame[T]

trait StreamingDataSet[T] with DataSet[T]:
    def watermark(self, column: str, delay: Duration) -> Self
```

Concrete types adopt the trait that matches their capability level:

```incan
class DataFrame[T] with BoundedDataSet[T]:
    ...

class LazyFrame[T] with BoundedDataSet[T]:
    ...

class DataStream[T] with StreamingDataSet[T]:
    ...
```

Because `BoundedDataSet[T]` adopts `DataSet[T]`, any adopter of `BoundedDataSet[T]` is automatically assignable to `DataSet[T]` as well. Authors do not need to adopt both traits separately.

APIs can be precise about what they accept:

```incan
def write_to_sink(data: DataSet[Order]) -> None:
    # accepts DataFrame, LazyFrame, or DataStream
    ...

def write_sorted(data: BoundedDataSet[Order]) -> None:
    # accepts DataFrame or LazyFrame — DataStream is excluded
    sorted_data = data.order_by(SortKey("amount"))
    ...

def write_to_kafka(events: DataStream[Event]) -> None:
    # accepts only streaming carriers
    ...
```

`write_sorted` accepts `DataFrame[Order]` and `LazyFrame[Order]` because both adopt `BoundedDataSet[Order]`. It does not accept `DataStream[Order]` because `DataStream` only adopts `StreamingDataSet`, not `BoundedDataSet`. The `order_by` method is available because it is declared on `BoundedDataSet[T]`. No union type or runtime check is needed — the restriction falls out of the trait hierarchy.

This RFC does not change how authors write trait adoption:

```incan
trait Serializable:
    def to_json(self) -> str

model User with Serializable:
    name: str

    def to_json(self) -> str:
        return "{}"
```

What changes is the semantic guarantee: `Serializable` is always an abstract annotation type, never merely a bound-only capability name with ad hoc exceptions.

## Reference-level explanation

### Core rule

Every `trait` declaration defines an abstract nominal capability type.

A trait:

- must not be directly instantiated
- may appear anywhere a type annotation is allowed
- may appear in `with` adoption clauses
- may appear in generic bounds

No separate `abstract trait` spelling is required for those meanings.

### Adopted trait assignability

If a concrete declaration `C` adopts trait `T`, then a value of type `C` is assignable to an annotation of type `T`.

If a concrete generic declaration `C[A1, ..., An]` adopts trait instantiation `T[B1, ..., Bm]`, then a value of type `C[X1, ..., Xn]` is assignable to `T[Y1, ..., Ym]` exactly when the adopted trait instantiation under the concrete substitution is compatible with the expected trait instantiation.

At minimum, exact trait-name matching and pairwise compatibility of trait type arguments must be enforced. `Trait[A]` and `Trait[B]` must not be treated as interchangeable merely because they share the same trait name.

### Supertrait adoption

A trait may adopt one or more other traits using `with`:

```incan
trait Sub with Super1, Super2:
    ...
```

This establishes a supertrait relationship. `Super1` and `Super2` are supertraits of `Sub`.

When a trait `Sub` adopts supertrait `Super`:

- any concrete declaration that adopts `Sub` must also satisfy all requirements of `Super`
- the concrete declaration does not need to list `Super` separately in its own `with` clause — adopting `Sub` implies adopting `Super`
- a value typed as `Sub` has access to methods declared by both `Sub` and `Super`

Supertrait adoption may include generic trait instantiations:

```incan
trait BoundedDataSet[T] with DataSet[T]:
    ...
```

The type parameter `T` in the supertrait clause must refer to type parameters declared on the adopting trait or to concrete types. Free type variables in supertrait clauses are not permitted.

### Transitive assignability

Supertrait assignability is transitive.

If `Sub` adopts `Mid` and `Mid` adopts `Base`, then:

- any adopter of `Sub` is assignable to `Sub`, `Mid`, and `Base`
- a value typed as `Sub` has access to methods from all three traits
- a value typed as `Mid` has access to methods from `Mid` and `Base`, but not `Sub`

The compiler must compute the transitive closure of supertrait relationships and enforce all implied requirements on concrete adopters.

### Concrete adopter obligations

When a concrete declaration adopts a trait that has supertraits, the concrete declaration must satisfy the method requirements of the adopted trait **and** all of its transitive supertraits.

The compiler must report missing method implementations for any unsatisfied supertrait requirement, with diagnostics that identify which supertrait introduced the requirement.

### Trait types in annotations

A trait type in annotation position denotes an abstract interface view over some hidden concrete adopter.

When a value is known only as trait `T`, the operations available on that value must be restricted to:

- methods declared by `T`
- methods inherited from `T`'s supertraits (transitively)
- universal operations available on all values

The compiler must not assume access to methods that exist only on the hidden concrete adopter unless the value has been narrowed to that concrete type by some other language rule.

### Generic bounds

This RFC does not remove or replace generic trait bounds such as `T with Eq`.

Bounds and trait-typed annotations are distinct but compatible facilities:

- a bound constrains a named type parameter
- a trait annotation names an abstract accepted type directly

These two forms may lower through similar internal mechanisms, but they remain distinct at the language level.

### Constructibility

Trait declarations are abstract and must not be directly constructed. Code such as `Drawable()` or `Collection[int]()` must be rejected unless a future RFC introduces an explicit factory or adapter construct that names a concrete implementing type.

### Diagnostics

When an adopter is used where a trait annotation is expected, the compiler should accept the use without requiring the author to rewrite the signature as an explicit generic bound.

When a value does not satisfy an expected trait annotation, diagnostics should explain the missing trait conformance in trait terms. For generic traits, diagnostics should mention mismatched trait arguments when the trait name matches but the instantiated arguments do not.

## Design details

### Syntax

This RFC adds no new keywords or declaration forms. It extends the existing `with` clause to trait declarations.

`trait` keeps its existing declaration form:

```incan
trait Example:
    def run(self) -> None
```

Traits may now adopt other traits using `with`:

```incan
trait AdvancedExample with Example:
    def run_advanced(self) -> None
```

Concrete declarations keep their existing adoption syntax:

```incan
class Worker with AdvancedExample:
    def run(self) -> None:
        println("running")

    def run_advanced(self) -> None:
        println("running advanced")
```

`Worker` adopts `AdvancedExample`, which implies `Example`. The compiler must verify that `Worker` satisfies both.

Generic bounds keep their existing syntax from RFC 023:

```incan
def run_all[T with Example](items: List[T]) -> None:
    ...
```

The changes are semantic: the trait name itself is always an abstract annotation type, and `with` on a trait declaration establishes supertrait relationships.

### Semantics

Traits are nominal, not structural. A declaration satisfies a trait because it explicitly adopts that trait according to Incan's trait rules, not merely because it happens to define methods with similar names and signatures.

Trait-typed annotations are existential in surface meaning: `value: TraitName` means "some concrete type that adopts `TraitName`". For generic traits, `value: TraitName[A, B]` means "some concrete type that adopts `TraitName[A, B]`".

The language may lower trait-typed parameters to hidden generic bounds, a compiler-managed trait-view representation, or another equivalent strategy. The lowering strategy is non-normative as long as the language-level behavior defined by this RFC is preserved.

### Interaction with existing features

- **Traits and derives:** unchanged. Derives may continue to add or imply traits, and trait adoption remains the mechanism that establishes conformance. If a derive implies a trait that has supertraits, the implied supertrait obligations propagate normally.
- **Generic bounds:** clarified, not replaced. `T with TraitName` remains useful when the API needs to retain an explicit type parameter name. A bound `T with Sub` where `Sub` has supertrait `Base` implies that `T` also satisfies `Base`.
- **Multi-instantiation trait dispatch:** compatible with RFC 025. Trait argument matching must continue to distinguish different instantiations of the same trait. Supertrait relationships between different instantiations of the same generic trait (e.g. `trait Foo[A] with Bar[A]`) must preserve the instantiation-level distinction.
- **Operator overloading:** compatible with RFC 028. Operator traits such as `Add[Rhs, Output]` may participate in supertrait hierarchies if a library defines a composite trait that requires multiple operator capabilities.
- **Imports and modules:** imported public traits may appear in type annotations under the same rules as locally declared traits. Supertrait relationships defined in imported modules must be visible to the typechecker.
- **Rust interop:** this RFC does not require foreign Rust traits to become first-class Incan trait declarations automatically. Any mapping from imported Rust traits into Incan trait space remains future work.
- **Union types:** orthogonal. A union containing a trait type is still a union, not a new form of implicit narrowing.
- **`@requires` decorator:** trait field requirements propagate through supertrait relationships. If `Sub` adopts `Base` and `Base` has `@requires(name: str)`, then any concrete adopter of `Sub` must also provide a `name: str` field.

### Compatibility / migration

This RFC is source-compatible for ordinary trait authoring syntax.

The primary effect is semantic clarification plus broader acceptance of trait-typed annotations, especially for generic traits. Code that already treats traits as abstract interfaces becomes more consistently supported. Documentation and diagnostics should migrate toward the simpler statement that "traits are always abstract" and should stop implying that some traits are only bound-like while others are annotation-like.

This RFC does not require the `abstract` keyword. The keyword may remain reserved for future work such as abstract classes or redundant readability-only sugar, but trait semantics must not depend on it.

## Alternatives considered

1. Introduce `abstract trait`
    One alternative is to distinguish `trait` from `abstract trait`, with only the latter being valid as an annotation supertype.

    This was rejected because it creates a category users do not naturally want: the "non-abstract trait". In Incan's existing teaching model, traits already define reusable capabilities concrete declarations opt into. Requiring a second keyword to make that existing meaning fully real adds ceremony without improving the user mental model.

2. Introduce `@abstract` decorator
    Another alternative is decorator-based marking such as `@abstract trait Foo:`.

    This was rejected for the same semantic reason. Whether a trait is an abstract annotation type is a core type-system rule, not optional metadata. A decorator suggests an add-on capability rather than the fundamental meaning of the declaration form.

3. Keep traits as bound-only in some contexts
    Another alternative is to keep today's partial model: traits may be used in bounds and selected annotation positions, but generic trait annotations do not carry the same abstract-supertype meaning.

    This was rejected because it preserves the very ambiguity that motivates this RFC. It makes the syntax misleading and keeps the language harder to explain than it needs to be.

4. Introduce `abstract type` as a separate declaration form
    Another alternative is to introduce a new `abstract type` declaration that serves as a named supertype for a family of concrete types, separate from traits.

    This was rejected because the resulting `abstract type` would need method signatures, adoption syntax, and assignability rules — all of which traits already provide. The only genuinely new semantics `abstract type` could carry (sealed families, stateful abstract bases) are orthogonal to the core problem and can be addressed by separate future features without duplicating the trait system.

5. Model abstract accepted types only through explicit generic bounds
    Another alternative is to require authors to write every abstract API using hidden or explicit type parameters, never direct trait annotations.

    This was rejected because it imports implementation-oriented ceremony into ordinary API design. Incan should let users write the type they mean.

## Drawbacks

- The typechecker must track trait-typed annotations, generic trait compatibility, and transitive supertrait closures more explicitly than today.
- Trait method calls through abstract annotations raise tricky questions around `Self` in parameter and return positions.
- Supertrait cycles must be detected and reported, adding a graph-analysis step to declaration collection.
- Method name collisions between a trait and its supertraits must be defined and diagnosed. This RFC requires that collisions within a single supertrait chain are resolved by the most-derived declaration, but diamond-shaped hierarchies where two independent supertraits declare the same method name may need additional disambiguation rules.
- Future trait-system work such as associated types will have to fit this model cleanly.
- Some implementation strategies may need compiler-generated generic wrappers or view types behind the scenes, even though the source language remains simple.

## Implementation architecture

This section is non-normative.

The simplest implementation model is to treat trait annotations as surface-level abstract supertypes that lower to compiler-managed hidden generic bounds or equivalent nominal trait-view representations. The compiler should prefer static dispatch and preservation of the hidden concrete adopter when possible, rather than requiring boxed runtime trait objects for every trait-typed value.

For generic traits, the compatibility check should be based on the adopted trait instantiation under concrete substitution, not on plain name-only matching. This keeps generic trait annotations honest and aligns with the multi-instantiation dispatch direction of RFC 025.

Supertrait relationships map naturally to Rust's supertrait syntax. A trait declaration such as `trait BoundedDataSet[T] with DataSet[T]` would lower to something morally equivalent to `trait BoundedDataSet<T>: DataSet<T>` in Rust. The compiler should compute the transitive supertrait closure once during declaration collection and cache it for use during type checking and lowering. Cycle detection in supertrait graphs must be performed eagerly and reported as a compile-time error.

## Layers affected

- **Parser / AST:** the parser must accept `with` clauses on `trait` declarations using the same syntax already used for concrete declarations. The AST representation for trait declarations must carry the list of adopted supertraits.
- **Typechecker / symbol resolution:** the typechecker must treat every trait as a valid abstract annotation type, must treat trait adoption as creating assignability, must perform generic trait-instantiation compatibility checks, must compute transitive supertrait closures, must detect supertrait cycles, and must verify that concrete adopters satisfy all transitive supertrait obligations.
- **Lowering / IR emission:** lowering must preserve the abstract-supertype semantics of trait annotations while remaining free to choose hidden generic bounds or another equivalent backend strategy. Supertrait relationships must lower to Rust supertrait bounds on the generated trait definitions.
- **Stdlib / runtime (`incan_stdlib`):** stdlib traits should be documented and authored under the clarified rule that traits are abstract by definition. Existing stdlib traits that logically form hierarchies should be updated to use supertrait adoption where appropriate.
- **Formatter:** the formatter must handle `with` clauses on trait declarations, formatting them consistently with `with` clauses on concrete declarations.
- **LSP / tooling:** hover text, completions, and diagnostics should describe traits as abstract capability types, should surface trait-conformance mismatches clearly, and should show the full method surface including inherited supertrait methods.
- **Documentation:** guides and references must update the trait teaching model to state directly that all traits are abstract and that traits may adopt other traits to form capability hierarchies.

## Implementation Plan

### Phase 1: Parser + AST

- Accept `with` clauses on `trait` declarations using the same comma-separated surface style as concrete declarations, with optional generic arguments on each supertrait (e.g. `DataSet[T]`).
- Extend the trait declaration AST to carry the list of adopted supertraits as structured trait bounds.
- Teach the formatter to print `with` on traits consistently with models and classes.

### Phase 2: Typechecker — symbols and supertrait graph

- Record supertrait relationships on trait symbols during collection and resolve bound types against the declaring trait’s type parameters.
- Build the transitive supertrait closure, detect cycles, and emit a dedicated diagnostic for supertrait cycles.

### Phase 3: Typechecker — assignability and conformance

- Treat trait names and generic trait instantiations as abstract annotation types everywhere annotations are allowed; reject direct trait construction where applicable.
- Extend trait implementation and compatibility checks for transitive supertraits, generic trait annotations, `@requires` propagation, and diamond ambiguity where required by the spec.

### Phase 4: Lowering + emission

- Preserve supertrait relationships in IR and emit Rust supertrait bounds and generic parameters on generated traits; ensure adopting types lower to `impl` blocks that satisfy the full hierarchy.

### Phase 5: Tests, stdlib alignment, and docs

- Add parser, typechecker, snapshot, and integration coverage for supertraits, generic trait annotations, and error cases (cycles, ambiguity, invalid instantiation).
- Align stdlib trait hierarchies where appropriate; update docs-site trait guidance and release notes.

## Implementation log

### Spec / design

- [x] Confirm remaining edge cases for diamond conflicts and `Self` under trait-typed annotations are reflected in Design Decisions where needed.

### Parser / AST

- [x] Parser: `with` on trait declarations (single, multiple, generic supertraits).
- [x] AST: `TraitDecl` carries supertraits as `Spanned<TraitBound>`.
- [x] Formatter: `with` on traits matches concrete declaration style.

### Typechecker

- [x] Symbol table: store resolved supertraits per trait.
- [x] Transitive supertrait closure and cycle diagnostic.
- [x] `type_implements_trait` and `types_compatible`: generic trait annotations and transitivity.
- [x] Conformance: transitive supertrait method and `@requires` obligations; conflict/ambiguity diagnostics.
- [x] Reject trait constructor / invalid instantiation where specified.

### Lowering / IR

- [x] IR trait: supertraits and type parameters.
- [x] Lowering and emission: Rust `trait Name<T>: Super<T>` shape; impl coverage for hierarchies.

### Stdlib / runtime

- [x] Document and adjust stdlib traits under the “always abstract” model; add supertrait links where hierarchies exist.

### Tests

- [x] Parser unit test: trait `with` (multiple and generic supertraits).
- [x] Additional parser / formatter round-trip tests as later phases land.
- [x] Typechecker tests: assignability, transitivity, cycles, missing supertrait methods, diamond ambiguity, `@requires` merge conflicts.
- [x] Codegen snapshots and integration tests for hierarchy examples (e.g. DataSet-style APIs).

### Docs

- [x] Update docs-site trait pages and teaching model.
- [ ] Release notes entry when shipped.

## Design Decisions

- **How should `Self` in trait method parameter and return positions behave when a value is known only through a trait-typed annotation?** `Self` refers to the abstract trait type in annotation context — it denotes the trait view, not the hidden concrete adopter. When a concrete adopter implements a trait method, `Self` resolves to the adopter's own type. This is consistent with how Python ABCs and Rust `dyn Trait` handle self-referential return types.

- **Should redundant spellings such as `abstract trait Foo:` be accepted as optional sugar or rejected to preserve one canonical declaration form?** Rejected. One canonical form (`trait`) keeps the language simpler and avoids a meaningless distinction. The keyword `abstract` may remain reserved for potential future use on other declaration forms (classes, models) but is not required or accepted for traits.

- **Should trait adoption remain fully open-world by default, or should a future RFC add a separate sealing mechanism for library-controlled trait families?** Deferred to a future RFC. Traits remain open-world by default under this RFC. A sealing mechanism is orthogonal and can be layered on later without changing the core model defined here.

- **Should this RFC remain limited to traits, or should future work generalize the same abstract-supertype model to classes or models?** Remain limited to traits. Abstract classes and models involve different design concerns (partial implementation, constructor semantics, inheritance hierarchies) and are deferred to a future RFC.

- **How should diamond-shaped supertrait hierarchies be handled when two independent supertraits declare methods with the same name and compatible but non-identical signatures?** The most-derived trait or the concrete adopter must provide an explicit disambiguation. If two independent supertraits declare the same method name and no adopting trait or concrete declaration resolves the conflict, the compiler must report an ambiguity error at the adoption site.

- **Should a trait be allowed to override a default method inherited from a supertrait, and if so, which implementation wins when a concrete adopter does not provide its own?** Yes. A sub-trait may override a supertrait's default method. The most-derived default wins — the sub-trait's override takes precedence over the supertrait's default. A concrete adopter may always provide its own implementation, which takes precedence over all trait-level defaults.

- **Should supertrait `@requires` fields be merged additively, and what happens when two supertraits require the same field name with incompatible types?** Yes, merge additively. When two supertraits require the same field name with compatible types, the requirement is deduplicated. When the types are incompatible, the compiler must report a conflict error at the trait declaration site where the conflicting supertraits are adopted, not deferred to the concrete adopter.
