# RFC 144: Trait fields and type-owned constants

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (storage-free traits and field requirements)
    - RFC 008 (const bindings and frozen values)
    - RFC 021 (model field metadata and wire aliases)
    - RFC 025 (trait instantiations and method dispatch)
    - RFC 042 (abstract traits and supertrait requirements)
    - RFC 046 (computed properties)
    - RFC 083 (symbol and method aliases)
    - RFC 098 (native associated types)
    - RFC 120 (canonical source symbol identity)
    - RFC 122 (model construction hooks)
    - RFC 129 (bindings and writable aliases)
    - RFC 132 (frozen classes)
    - RFC 143 (cloning with field overrides)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Allow instance field requirements directly in trait bodies and type-owned constant requirements on traits. Models and classes may satisfy compatible requirements with one ordinary declaration or supply separate fields and constants through owner-qualified declarations. `settings from SnowflakeReader[B] as source: SnowflakeSettings` declares one adopter-owned field for that trait requirement; `pub const LIMIT from ReaderDefaults as READ_LIMIT: int = 100` supplies a trait constant with an optional ordinary alias. `from` selects the owner in declarations and member access. Traits remain storage-free, and collisions follow checked identities rather than declaration order or value equality. All new syntax is proposed; existing `@requires` remains a supported requirement spelling.

## Motivation

A trait's required fields belong beside the methods that use them. Today `@requires(name: str)` expresses that requirement outside the body. A direct `name: str` declaration makes the instance contract visible in the same place as method and property requirements, without giving a trait its own storage.

Other values belong to an implementation rather than to each instance: a codec's media type or a format's version. Repeating them in instance fields suggests that they vary per object. A checked constant requirement lets generic code obtain the implementing type's value without constructing an instance.

These members must compose predictably. Two traits may require one compatible field, two unrelated traits may use the same constant name, and a class may already declare a method or property with that spelling. The language must explain each case before accepting the syntax.

A transfer operation may also need two different configurations with the same shape: one for reading and one for writing. Sharing a model type should not force these configurations into one instance field. Owner-qualified declarations let the adopter make that choice explicitly while each trait's behavior keeps referring to its own contract.

## Goals

- Express stored field requirements directly in trait bodies while retaining `@requires` compatibility.
- Add model and class constants using the existing compile-time evaluation and frozen-value rules.
- Require implementations to supply typed constants without an instance.
- Allow separate same-named constant implementations for distinct trait owners, with optional aliases for ordinary member access.
- Allow separate stored fields for distinct trait requirements, with checked local names for construction, access, and model schemas.
- Preserve field privacy, storage-free traits, Frozen guarantees, and checked provider boundaries.
- Define duplicate, conflicting, inherited, and ambiguous members explicitly.

## Non-Goals

- Allocating trait-owned instance storage or injecting fields into adopters.
- Trait field initializers, trait constructors, mutable type-owned state, or default trait constant values in this revision.
- Treating a computed property or method as a stored field requirement.
- Adding const generic parameters, arbitrary compile-time function execution, or runtime reflection dispatch.
- Replacing RFC 025 method dispatch or RFC 098 associated-type ownership rules.

## Guide-level explanation

### Put field requirements beside their behavior

```incan
trait Named:
    name: str

    def greeting(self) -> str:
        return f"Hello, {self.name}"

class User with Named:
    name: str
```

The adopter declares and initializes `name`. The trait requires that storage; it does not supply another copy. This is the preferred spelling of the existing `@requires(name: str)` contract. Existing declarations remain valid.

Field requirements do not make an adopter's private fields public. Access through trait behavior retains the existing `@requires` permissions. A method that changes a required field still needs `mut self` and ordinary writable access.

```incan
trait Counter:
    count: int

    def bump(mut self) -> None:
        self.count += 1
```

RFC 132 frozen adopters must reject this mutating behavior when it replaces a frozen field. A trait that only reads a field can be adopted by a frozen class with compatible storage.

### Distinguish instance data from type-owned values

```incan
trait Codec:
    name: str
    const MEDIA_TYPE: FrozenStr

class JsonCodec with Codec:
    name: str
    pub const MEDIA_TYPE: FrozenStr = "application/json"

model ExportBatch:
    rows: list[str]
    pub const FORMAT_VERSION: int = 1

def media_type[C with Codec]() -> FrozenStr:
    return C.MEDIA_TYPE
```

`name` belongs to an instance. `JsonCodec.MEDIA_TYPE` belongs to the implementing type and satisfies the trait's constant requirement. `ExportBatch.FORMAT_VERSION` needs no trait at all. Constants are not constructor parameters or instance fields, and do not appear in a model's stored-field schema.

Ordinary class and model constants use normal public/private member visibility. Constants exposed by a public trait contract must be exposed compatibly by its implementations; a private implementation member must not silently satisfy a public constant requirement. The trait's own visibility controls access to its contract.

### Read a constant through ordinary member access

```incan
trait ReaderDefaults:
    const LIMIT: int

    def limit(self) -> int:
        return self.LIMIT
```

`self.LIMIT` reads the implementing type's constant. It does not require an instance field or a second stored copy. `Self.LIMIT` remains available when type-level spelling is useful, but methods do not have to switch receiver spelling simply because the selected member is a constant. The same convenience applies to `pipeline.LIMIT` when its checked receiver type identifies one accessible constant.

The method above is checked against `ReaderDefaults.LIMIT`. Adopting another trait with a same-named constant must not reinterpret or invalidate that checked reference. If several owners are already available when a body is checked, the ambiguous read needs explicit qualification. Access through a receiver preserves ordinary expression evaluation and visibility; it does not add dynamic attribute lookup or permit an instance to override the constant.

### Compatible field requirements may share one field

```incan
trait Named:
    name: str

trait Labeled:
    name: str

class User with Named, Labeled:
    name: str
```

One `name` field satisfies both compatible requirements in this example. The adopter may instead bind each requirement to a separate stored field using the declarations below. If `Labeled` requires `name: int`, one ordinary field cannot satisfy both requirements, but separate targeted fields may satisfy them with different types. A subtrait combining independent requirements retains their owners rather than rejecting their names alone. Conformance fails when the concrete storage mapping cannot satisfy a requirement; reordering traits never changes the mapping.

### Keep reader and writer settings separate

The following application example composes a reusable Step entrypoint with reader and writer behavior to copy rows between Snowflake tables with different connection settings. All models and traits below are application-defined, including the `SnowflakeBackend` connector contract; a concrete connector implementation is supplied by the caller. Its declarations show the complete boundary used by the reader and writer. The trait-targeted fields describe proposed language behavior.

```incan
model SnowflakeSettings:
    database: str
    schema: str
    warehouse: str

model Row:
    id: int
    value: str

trait SnowflakeBackend:
    def read_table(
        self,
        settings: SnowflakeSettings,
        table: str,
    ) -> Result[list[Row], str]: ...

    def write_table(
        mut self,
        settings: SnowflakeSettings,
        table: str,
        rows: list[Row],
    ) -> Result[None, str]: ...

trait Step:
    name: str

    def execute(mut self) -> Result[int, str]: ...

    def run(mut self) -> Result[int, str]:
        println(f"Running {self.name}")
        return self.execute()

trait SnowflakeReader[B with SnowflakeBackend]:
    backend: B
    settings: SnowflakeSettings
    table: str

    def read(self) -> Result[list[Row], str]:
        # These requirements belong to SnowflakeReader[B].
        return self.backend.read_table(self.settings, self.table)

trait SnowflakeWriter[B with SnowflakeBackend]:
    backend: B
    settings: SnowflakeSettings
    table: str

    def write(mut self, rows: list[Row]) -> Result[None, str]:
        # These requirements belong to SnowflakeWriter[B].
        return self.backend.write_table(self.settings, self.table, rows)

class CopyOrders[B with SnowflakeBackend] with Step, SnowflakeReader[B], SnowflakeWriter[B]:
    name: str = "copy_orders"
    backend: B  # One field deliberately satisfies both backend requirements.
    pub settings from SnowflakeReader[B] as source: SnowflakeSettings
    pub settings from SnowflakeWriter[B] as destination: SnowflakeSettings
    pub table from SnowflakeReader[B] as source_table: str = "ORDERS"
    pub table from SnowflakeWriter[B] as destination_table: str = "ORDERS_CURATED"

    def execute(mut self) -> Result[int, str]:
        rows = self.read()?
        row_count = len(rows)
        self.write(rows)?
        return Ok(row_count)

def copy_orders[B with SnowflakeBackend](backend: B) -> Result[int, str]:
    mut step = CopyOrders[B](
        backend=backend,
        source=SnowflakeSettings(
            database="LANDING",
            schema="PUBLIC",
            warehouse="EXTRACT_WH",
        ),
        destination=SnowflakeSettings(
            database="ANALYTICS",
            schema="CURATED",
            warehouse="LOAD_WH",
        ),
    )
    return step.run()
```

`Step.run()` prints the step's name and calls the concrete `CopyOrders.execute()`. That method connects the reader's `read()` and the writer's `write()`, propagates failures through `?`, and returns the row count only after the write succeeds. `read()` invokes the supplied backend with `LANDING.PUBLIC.ORDERS` and `EXTRACT_WH`; `write()` invokes that same backend with `ANALYTICS.CURATED.ORDERS_CURATED` and `LOAD_WH`. The concrete class supplies storage and transfer logic while the traits supply reusable behavior. The backend contract does not promise transactional rollback or define whether a failed write has partial external effects. The example uses `str` errors to keep its application-defined connector boundary small; choosing a domain error type is independent of the field-binding feature.

`CopyOrders` owns separate settings and table fields for each side. `source`, `destination`, `source_table`, and `destination_table` are their ordinary access and constructor names. `settings` and `table` remain each trait's requirement names. The reader's checked `self.settings` selects `source`, and the writer's selects `destination`; neither method needs knowledge of the adopter's chosen names. The `backend` field deliberately satisfies both compatible backend requirements with one shared slot. A plain `step.settings` read is ambiguous because those requirements select different slots. An owner-qualified read such as `step.settings from SnowflakeReader[B]` reaches the same slot as `step.source`, with the same access and mutation permissions.

A declaration such as `settings: SnowflakeSettings` would instead supply one shared field for both compatible requirements. Sharing the model type does not determine whether storage is shared. The binding declarations do.

These traits independently declare their `settings` requirements. If both inherited one requirement from a common supertrait, the diamond would retain one canonical contract, which must map to one slot. Different adoption paths alone do not split that contract into two requirements.

The local name is optional when the requirement name itself is a unique stored-field and constructor name: `settings from SnowflakeReader[B]: SnowflakeSettings`. Separate stored fields must have distinct ordinary names, so the two `settings` declarations in this example need `as source` and `as destination`. A diagnostic must suggest local names when the declarations would otherwise create duplicate construction inputs.

### A field and a constant cannot share a simple name

```incan
trait HasLimit:
    limit: int

class Client with HasLimit:
    pub const limit: int = 100  # rejected: constant cannot satisfy instance field
```

The same rule applies when the competing member is a method, computed property, or RFC 098 inferred type member. Matching spelling or result type does not turn one member kind into another. Owner qualification may bind separate same-kind requirements, but it does not make a constant satisfy a field requirement or hide a cross-kind name collision.

### Same-named constant requirements retain their owners

```incan
trait ReaderDefaults:
    const LIMIT: int

trait WriterDefaults:
    const LIMIT: int

class Pipeline with ReaderDefaults, WriterDefaults:
    pub const LIMIT: int = 100

def ambiguous[T with (ReaderDefaults, WriterDefaults)]() -> int:
    return T.LIMIT  # rejected: two owning trait contracts

def concrete_limit() -> int:
    return Pipeline.LIMIT  # accepted: the concrete declaration is unique
```

One explicitly declared concrete constant may satisfy both compatible requirements. That does not merge the two trait declarations into one identity. Generic code naming both owners must identify which contract it means, even if every current implementation uses equal values.

This RFC uses the proposed `from` qualifier shared with RFC 098, extended to value members:

```incan
def reader_limit[T with (ReaderDefaults, WriterDefaults)]() -> int:
    return T.LIMIT from ReaderDefaults
```

`from ReaderDefaults` identifies the owning trait contract. The same qualifier may select a constant reached through an instance, such as `value.LIMIT from ReaderDefaults`, when its checked type satisfies that contract. The shared grouping rule below applies consistently with RFC 098 type projections.

### Supply different constants for different traits

Separate trait implementations may need different values even when their requirements use the same name. `from` identifies the owning trait in both declarations and reads; optional `as` provides a convenient name for callers. This standalone proposed example uses application-defined traits and a class:

```incan
trait ReaderDefaults:
    const LIMIT: int

trait WriterDefaults:
    const LIMIT: int

class Pipeline with ReaderDefaults, WriterDefaults:
    pub const LIMIT from ReaderDefaults as READ_LIMIT: int = 100
    pub const LIMIT from WriterDefaults as WRITE_LIMIT: int = 25

def example() -> None:
    assert Pipeline.READ_LIMIT == 100
    assert Pipeline.WRITE_LIMIT == 25
    assert (Pipeline.LIMIT from ReaderDefaults) == 100
    assert (Pipeline.LIMIT from WriterDefaults) == 25
```

`READ_LIMIT` aliases the reader implementation's constant; it does not create another independent constant or rename the trait requirement. `WRITE_LIMIT` does the same for the writer implementation. `Pipeline.LIMIT` is ambiguous in this example because no unique ordinary concrete declaration supplies that name. Equal values would not remove the ambiguity.

The alias is optional: `pub const LIMIT from ReaderDefaults: int = 100` supplies the same reader contract without adding `READ_LIMIT`. A caller then uses owner qualification when the simple name is ambiguous. In a declaration, `from` identifies the contract being implemented; in a read, it selects that contract's implementation. A method already checked against `ReaderDefaults.LIMIT` retains that owner and reads the reader implementation's value when adopted by this class.

## Reference-level explanation

### Field requirements

1. A trait body must accept `name: Type` as a stored instance field requirement. Field initializers and field visibility modifiers must be rejected in that position in this revision.
2. This declaration must use the same conformance, generic substitution, access, and mutation rules as the corresponding `@requires(name: Type)` entry. It must not allocate storage, supply constructor defaults, or synthesize an adopter field.
3. Models and classes must satisfy required fields with compatible declared stored fields. Properties, constants, methods, and inferred type members must not satisfy the requirement. Existing restrictions on enum and newtype adopters must remain in force.
4. Identical requirements combined through `@requires` and the body must be deduplicated. A repeated direct body declaration must be rejected as a duplicate declaration. Incompatible declarations of the same canonical requirement must produce a conflict diagnostic; distinct owners must retain separate obligations even when their member names match.
5. Supertrait requirements must propagate transitively with canonical owners and ordinary generic arguments. One ordinary adopter field may satisfy several compatible same-named obligations. Separate targeted declarations may instead satisfy distinct owners, including requirements with different types. A diamond reaching the same canonical requirement must deduplicate that requirement and must not permit multiple storage bindings for it. A concrete mapping must be rejected when it cannot satisfy an obligation after substitution; combining independent same-named requirements alone must not force shared storage.
6. Field requirements must not grant new public access or writable capabilities. Default and authored methods must obey the adopter's Frozen restrictions, including through imported contracts.

### Trait-targeted stored fields

1. Models and classes must accept `[pub] NAME from TraitExpression [as LOCAL_NAME]: Type [= expression]` as a stored field declaration satisfying a particular adopted trait requirement. Brackets here denote optional syntax. A model must retain its ordinary field visibility rules; the optional `pub` follows existing class field syntax and does not add field visibility modifiers to model or trait bodies. The target must resolve to a stored-field requirement of an available adopted contract, retaining canonical ownership and ordinary generic arguments. A constant, method, property, unknown member, or unavailable adoption must be rejected as the target.
2. Each targeted declaration must allocate exactly one ordinary field in the adopter, not in the trait. Its type must satisfy the selected requirement under existing field conformance rules. Its explicit target must select the requirement it supplies; its local name must not silently bind other requirements by spelling. An untargeted ordinary field remains the form for automatically sharing storage across compatible same-named requirements. Distinct owners may map to independent fields of the same or different types. Defaults belong to the adopter and obey ordinary field initialization rules; no field initializer is introduced on traits.
3. `LOCAL_NAME`, when supplied, must be the field's ordinary source name, constructor input name, and canonical stored-field schema name. Otherwise `NAME` must serve those roles. Every stored slot must have a unique ordinary name. A pair of targeted fields with the same resulting ordinary name must be rejected with a diagnostic suggesting distinct `as` names. This RFC does not introduce qualified constructor keyword arguments or invent names for callers.
4. A local name and a qualified member access must identify the same stored slot. They must not allocate duplicate storage, forward through a computed property, copy the value on access, or bypass visibility. A trait body must bind its required-field references to canonical contract identities when checked; adoption must map those references to the selected slots without reopening lookup across the adopter's other traits. When multiple distinct contracts already make a field read ambiguous in the declaring trait or a generic body, an explicit qualifier such as `self.settings from SnowflakeReader[B]` must select the owner. Equal field types must not establish shared storage in a generic body that can receive separately bound implementations.
5. Compatible requirements may still share one ordinary adopter field. Each canonical requirement must map to exactly one stored slot. An ordinary field and a targeted declaration that both satisfy that same requirement must be rejected as duplicate bindings, as must two targeted declarations for the same owner, instantiation, and member. Source order must not select a winner. Distinct names on two paths to one inherited requirement must not split its identity or authorize two bindings.
6. Read, assignment, and borrow operations through a qualified member or its ordinary local name must use the same slot identity and existing ownership, access, and mutation checks. Mutation in trait behavior still requires `mut self`. RFC 132 freezing must reject replacement and writable aliases through either spelling, including inside trait methods. Separately bound fields must not be treated as the same slot merely because their trait member names or declared types agree.
7. Constructors, destructuring, stored-field reflection, and model serialization must see each stored slot once under its ordinary local name. The trait requirement name and owner mapping must not create another field or an additional wire key. RFC 021 model field metadata and explicit wire aliases must retain their ordinary meaning on that stored field; identifier `as LOCAL_NAME` here selects the local field name, whereas RFC 021 string aliases describe wire mapping. This RFC must not extend model wire metadata to classes.
8. RFC 122 construction hooks and RFC 143 keyword cloning must use checked stored-field identities and ordinary local input names. For a value supporting keyword cloning, an override such as `job.clone(source=replacement)` must select the source slot and its declared construction behavior; it must not replace the destination slot solely because both satisfy a requirement named `settings`. Retained slots, validation results, and freezing must follow those RFCs without additional copying or validation bypasses. This binding syntax must not grant Clone or keyword reconstruction to otherwise ineligible values.
9. Inherited stored fields must retain their slot identities, local names, visibility, and requirement bindings. A child must not override an inherited binding by redeclaring it under a different local name. Imports, aliases, and provider boundaries must preserve the same mapping rather than rediscovering storage by spelling.

### Owner-qualified projection grouping

`receiver.NAME from TraitExpression` must select the immediately preceding member projection and form one primary expression. The owner uses the existing trait name and generic-argument grammar. The qualifier must not consume a surrounding arithmetic, comparison, or comprehension clause, or apply to an unrelated whole expression. Ordinary call and collection delimiters do not require redundant parentheses; a further postfix operation on the selected result must parenthesize it, such as `(value.settings from Reader).database`. Any actual attachment ambiguity must produce a diagnostic requesting explicit grouping rather than choosing a meaning from imports or expected result types.

This rule also governs RFC 098 type projections, including inside generic arguments. For values, `(Pipeline.LIMIT from ReaderDefaults) + 1` adds one to the selected reader constant. In a comprehension, `[(p.LIMIT from ReaderDefaults) for p in pipelines]` selects the reader constant for each receiver; `for` remains the comprehension keyword. Normal receiver evaluation occurs once. No other operator precedence changes.

### Constants and evaluation

1. Models and classes must accept `const NAME [: Type] = expression`, with normal member visibility. Trait bodies must accept `const NAME: Type` as a required type-owned constant; the type is mandatory and an initializer is unsupported in this revision.
2. Constant initialization must obey RFC 008. Existing frozen representations of strings, bytes, and collections must be preserved; a `str` annotation in const context must not produce a mutable string capability. Explicit `FrozenStr` in the guide makes that representation visible.
3. Constants must be excluded from instance layout, constructor inputs, stored-field reflection, instance field cloning, and ordinary model serialization. This RFC does not add a reflection API for constants.
4. Constant reads must accept concrete type, checked type-parameter, and instance member access, such as `JsonCodec.MEDIA_TYPE`, `C.MEDIA_TYPE`, and `codec.MEDIA_TYPE`. An instance read must resolve through its statically checked receiver type and available trait contracts; it must not create instance storage, dynamic dispatch, or new visibility permissions. Its receiver expression must be evaluated once under ordinary member-access rules even if the constant's value can be folded.
5. Generic constant initializers may depend only on constants available through checked bounds and operations already permitted by RFC 008. Evaluation must occur when the dependencies are known; unresolved dependencies must not become runtime initialization. Cycles must be diagnosed across qualified declarations. Each concrete instantiation must evaluate its selected constant dependencies and initializer at compile time under the ordinary const-compatible rules. A valid generic definition may remain symbolic until its checked type arguments supply those constants; an unresolved, cyclic, unsupported, or runtime-dependent expression at required evaluation must produce an actionable diagnostic. No function-call interpreter, environment lookup, runtime fallback, or separate weaker generic-constant policy is introduced. Existing frozen string, byte, and collection results must retain their representations.
6. The selected implementation must provide a compatible constant and visibility. Equal evaluated values must not substitute for conformance or declaration identity.
7. Required constants of supertraits must propagate with their owners and ordinary generic instantiations. Omitting RFC 098 inferred arguments in a bound must not create another constant owner for the same implementation.
8. A trait method's constant references must resolve against the requirements available when its body is checked. The selected owner must be retained when the method is adopted, inherited, imported, or instantiated; extra traits on an adopter must not reopen that lookup. Ambiguity already present in the declaring trait or generic body must require a qualifier, such as `self.LIMIT from ReaderDefaults`.

### Trait-targeted constant declarations and aliases

1. Models and classes must accept `[pub] const NAME from TraitExpression [as ALIAS]: Type = expression` to supply a constant requirement of a particular adopted trait. Brackets in this grammar denote optional syntax, not source tokens. The target must resolve to an available adopted contract, including through supertraits, and must identify its canonical owner and ordinary generic arguments. An unknown owner, unavailable adoption, or target member of another kind must be diagnosed.
2. The declared type, evaluated value, and visibility must satisfy that selected requirement. Distinct owners may receive different values or different required types under the same member name. Targeted declarations must not create trait-owned instance storage or relax constant-evaluation rules.
3. An ordinary concrete constant may continue to satisfy multiple compatible same-named requirements. Each requirement must have exactly one implementation source: an ordinary declaration and a targeted declaration that both supply the same requirement must be rejected as duplicate implementations, as must two targeted declarations for the same owner, instantiation, and member. There is no source-order or targeted-over-common override rule.
4. Optional `as ALIAS` must introduce an ordinary member alias for that implementation's constant. The alias must preserve the selected implementation identity, type, value, and declaration visibility; it must not copy the constant into an independently defined member or rename the trait requirement. The alias name must obey ordinary member collision and inheritance rules, and must not hide an existing field, method, property, constant, inferred type member, or alias. This extends RFC 083's identity-preserving alias principle to these constant declarations; it does not change RFC 083's existing source forms or claim that they already accept constants.
5. A qualified read such as `Pipeline.LIMIT from ReaderDefaults` must select the corresponding trait implementation. An alias read such as `Pipeline.READ_LIMIT` must select that same constant without repeating the qualifier. Both forms must preserve normal visibility checks and work through checked imports and aliases of the declaring type.
6. Without a unique ordinary concrete declaration or alias for a name, unqualified concrete or instance reads that have multiple possible trait-owned constants must be rejected as ambiguous. Equal values or types must not select a winner. A generic read under multiple same-named trait contracts must still require owner qualification even when one ordinary concrete declaration happens to satisfy all of them for a particular implementation.

### Collision rules

Member identities must include their declaring owner and, for generic traits, their ordinary type arguments. Aliases and re-exports must preserve those identities. Source order, adoption order, import order, value equality, and capitalization must not choose a winner.

| Situation                                                                         | Required outcome                                                                                            |
| --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| Same inherited declaration reached through both sides of a diamond                | Deduplicate by canonical identity.                                                                          |
| Compatible same-named field requirements from different traits                    | One ordinary field may satisfy all, or targeted declarations may supply separate slots.                     |
| Distinct same-named field requirements have incompatible substituted types        | Retain the separate obligations; require compatible targeted storage rather than one shared field.          |
| Distinct targeted stored fields have the same ordinary local name                 | Reject duplicate stored-field and constructor names; suggest distinct `as` names.                           |
| Ordinary and targeted fields both satisfy the same canonical requirement          | Reject duplicate bindings; neither field overrides the other.                                               |
| A diamond reaches one field requirement through multiple supertrait paths         | Retain one contract and one slot mapping; path names do not authorize separate bindings.                    |
| Identical body and `@requires` field requirement                                  | Deduplicate; preserve both source locations for tooling.                                                    |
| Two direct declarations of the same member identity in one body                   | Reject; distinct targeted field or constant identities and existing RFC 025 method groups remain permitted. |
| A new field or constant collides with a method, property, or inferred type member | Reject the cross-kind collision; do not synthesize a conversion or choose a namespace by context.           |
| One concrete constant matches compatible requirements from distinct trait owners  | Accept conformance; retain each requirement's owner.                                                        |
| Same-named constant requirements have incompatible substituted types              | Require separate trait-targeted declarations; one ordinary constant cannot satisfy incompatible types.      |
| Distinct trait-targeted constants share a name                                    | Accept separate implementations; an unqualified read is ambiguous without a unique ordinary declaration.    |
| Ordinary and targeted declarations both implement the same constant requirement   | Reject as duplicate implementations; neither declaration overrides the other.                               |
| A targeted constant's alias collides with another ordinary member                 | Reject the alias collision; qualification does not hide the existing member.                                |
| Unqualified generic constant projection has distinct possible owners              | Reject as ambiguous, even when their types or values agree.                                                 |
| Unqualified generic field access has distinct possible owners                     | Require owner qualification; matching types alone do not prove a shared slot.                               |
| Concrete type has one explicit constant satisfying several traits                 | Its direct type-qualified read selects that concrete declaration.                                           |
| Subtrait redeclares an inherited constant name                                    | Reject; this revision provides no constant override mechanism.                                              |
| Class redeclares an inherited ordinary constant or the same trait-owned constant  | Reject; no implicit shadowing or constant override.                                                         |
| Alias and original expose the same declaration                                    | Treat as one owner when they resolve to the same canonical identity and instantiation.                      |

A class inheriting a constant without redeclaration must retain the original declaration identity and visibility, including the trait target and any alias. A private inherited constant must not become public through the child. A declaration targeting a different trait owner is a distinct implementation, not an override of the inherited contract; its alias must still satisfy ordinary collision rules. Inherited stored-field mappings likewise retain one slot identity and cannot be remapped by a child. The cross-kind collision rules apply when the combined surface is formed, including inherited members and adopted requirements, rather than only to declarations written side by side. Trait targeting permits separate same-kind field or constant identities; it does not let one kind satisfy or silently replace another.

Existing same-kind method dispatch, compatible method overrides, and trait-targeted method declarations remain governed by RFC 025 and RFC 042. This RFC must not impose its no-override constant rule on methods. RFC 098 type projections retain their separate ownership and ambiguity rules.

### Diagnostics and providers

Missing-member diagnostics must identify the required member kind and owning trait. Collision diagnostics must name both declarations, show their substituted types where relevant, and identify the declaration that combined them. An ambiguous projection must list the candidate trait owners and arguments; it must not recommend changing a constant's value to resolve ambiguity. Duplicate stored-field names must explain that distinct slots need distinct constructor names and suggest `as` names. A duplicate binding diagnostic must identify the already-satisfied requirement rather than silently select another field.

Checked API metadata must preserve member kinds, visibility, owners, field obligations and their stored-slot mappings, local construction and schema names, constant types and dependencies, targeted implementation identities, optional alias relationships, and inherited identities. Imports and aliases must not erase conflicts or turn unresolved generic dependencies into successful conformance. Diagnostics for a targeted declaration must identify the selected requirement; ambiguous reads should show accessible local names, aliases, or owner-qualified reads as remedies.

## Design details

### Sharing storage does not merge contracts

An ordinary field may satisfy several compatible trait requirements by name and type, preserving the existing `@requires` and RFC 042 storage contract. Owner-qualified declarations add an explicit choice of separate adopter-owned slots. Both choices preserve the requirement identities: shared storage does not merge contracts, and using one model type in two requirements does not force shared storage. Constant requirements similarly support either one compatible ordinary declaration or separate trait-targeted implementations.

This distinction explains why `Pipeline.LIMIT` and an instance read through a known `Pipeline` can be unambiguous when they select one explicit ordinary concrete declaration, while `T.LIMIT` under two unrelated trait bounds is ambiguous. When separate trait-targeted declarations supply different implementations, concrete callers also need an alias or owner qualification. Generic code must identify a contract independently of whichever implementation a caller later supplies. Qualification preserves that meaning across future implementations. A checked default method retains its original selection rather than looking up the name again across all of the adopter's traits.

### Construction and model schemas use stored names

For a targeted field, identifier `as source` supplies the adopter's ordinary field name while retaining the trait requirement identity. There is one stored slot and one constructor input named `source`, not another input named `settings`. Models expose that same canonical stored name to reflection, destructuring, construction hooks, and serialization, with any separately declared RFC 021 wire mapping applied normally. Classes gain this targeted naming surface without gaining model wire metadata.

Generic code must not assume that two compatible field requirements share a slot. A body checked under both owners may need explicit qualification even where older name-based reasoning would have treated compatible requirements as interchangeable. Diagnostics must identify both contracts and suggest the corresponding `from` reads. Existing concrete shared-field declarations and ordinary access to those concrete fields retain their meaning.

### No default values in traits yet

Requiring values in implementing types avoids a precedence rule for competing defaults and keeps traits storage-free. Adding trait constant defaults later would need an explicit override and diamond policy. Field initializers would additionally require a construction contract; this RFC does not infer one from declaration order.

### Prior art

Rust's [associated constants](https://doc.rust-lang.org/reference/items/associated-items.html#associated-constants) provide typed constants associated with traits and implementing types. Separate trait implementations can supply distinct same-named constants, and [qualified paths](https://doc.rust-lang.org/reference/paths.html#qualified-paths) select an implementation explicitly. This proposal adopts that ownership distinction with `from` in declarations and reads, plus optional aliases, while retaining Incan's existing frozen const representations. Direct trait field declarations and adopter-owned storage bindings extend Incan's established `@requires` contract; they are not a claim that Rust traits contain stored fields.

## Alternatives considered

- **Keep only `@requires`:** preserves the current capability but separates required fields from the behavior that consumes them.
- **Inject trait fields into adopters:** avoids repeated declarations, but introduces storage ownership, constructor composition, and inheritance questions absent from storage-free traits.
- **Force compatible field requirements to share storage:** keeps name-based lookup simple, but prevents reader and writer contracts from holding independent configurations. Explicit binding permits sharing or separation while preserving checked owner and slot identities.
- **Treat properties as fields:** broadens conformance but changes the current stored-field requirement and its mutation semantics.
- **Use static methods for every type-owned value:** remains useful for computed behavior, but obscures compile-time data and its immutability contract.
- **Resolve duplicate constants by first adoption or equal values:** makes API behavior order-dependent or dependent on an implementation detail rather than declared ownership.
- **Require separate adapter types for different same-named constants:** avoids targeted declarations but forces wrappers when independent traits choose the same name. Explicit trait targets express the intended implementations directly, and optional aliases give callers distinct ordinary names.
- **Separate field and constant namespaces automatically:** permits more spellings but makes collisions with methods, properties, and associated types harder to explain and can hide changes when member kinds evolve.

## Drawbacks

Direct field requirements repeat the field declaration in each adopter, deliberately preserving explicit storage ownership. Compatibility leaves two requirement spellings. Separate bindings add local names to the adopter's API, and distinct stored fields need distinct constructor names even when both trait contracts use the same spelling. Owner-sensitive generic field access may require qualification where matching types previously suggested shared storage.

Conservative cross-kind collision rejection limits some combinations of independently authored traits. Trait-targeted declarations, optional aliases, and qualified reads add syntax and additional public identities for tooling to preserve. The grouping of qualified projections must remain consistent with RFC 098. Constants also add public API members whose names, namespaces, visibility, types, and values can matter to consumers; moving or renaming them is not automatically compatibility-preserving.

## Layers affected

- **Parser / AST / formatter:** trait body field requirements, constant requirements, model/class constants, trait-targeted field and constant declarations, local field names, and optional constant aliases.
- **Typechecker / symbol resolution:** substituted requirements, owner-to-slot mappings, member kinds, visibility, constant dependencies, projection ownership, Frozen checks, and collision diagnostics.
- **Construction / derives / schemas:** local constructor and stored-field names, destructuring, model reflection and serialization, RFC 122 hooks, and RFC 143 clone overrides without duplicate slots or inputs.
- **Constant evaluation / lowering / emission:** type-owned constants without instance storage, distinct adopter-owned field slots, and trait behavior resolved to checked slot identities.
- **Providers / API metadata:** canonical member and storage identities, constant dependencies, targeted implementations and alias relationships, inherited members, and public contracts.
- **LSP / documentation:** member and constructor completion, ownership-aware navigation, shared-versus-separate storage explanations, and conflict locations.

## Design Decisions

- Traits remain storage-free. Models and classes may bind a required instance field through `NAME from TraitExpression [as LOCAL_NAME]: Type [= expression]`, declaring one adopter-owned slot. The local name is used for ordinary access, construction, and canonical stored-field schema identity; the trait requirement keeps its original owner and name.
- Compatible field requirements may share one ordinary declared field or use separate targeted fields. Independent owners may require different field types. One canonical requirement reached through a diamond remains one contract and maps to one slot.
- Trait methods retain their checked requirement owners and use the corresponding adopter slots. Qualified access and local names reach the same slot with the same visibility, borrow, mutation, and freezing rules. Generic bodies with multiple possible owning contracts must qualify ambiguous accesses rather than infer shared storage from matching types.
- Separate slots require unique ordinary names; the compiler diagnoses duplicate constructor names and suggests `as` names. Slot mappings and local names survive inheritance, imported metadata, construction hooks, model schemas, and eligible keyword cloning without duplicate fields or extra wire keys.
- One ordinary concrete constant may satisfy multiple compatible same-named trait requirements while retaining their separate contract identities.
- Distinct requirements may instead receive separate implementations through `const NAME from TraitExpression [as ALIAS]: Type = expression`. `from` identifies the owning contract in both declarations and reads. The alias is optional and preserves the implementation's identity.
- Separate trait-owned implementations may differ in value or type. Unqualified concrete reads remain ambiguous when no unique ordinary declaration or alias selects a constant, even if the implementations happen to have equal values. Generic reads under multiple owning contracts still require qualification.
- Each requirement has one implementation source. Duplicate targeted declarations, ordinary-plus-targeted implementations of the same requirement, and colliding alias names are rejected rather than resolved by declaration order.

- Type and value owner-qualified projections share the primary member-projection rule with RFC 098. Delimited uses stay compact; further postfix access and actual attachment ambiguities require explicit parentheses. Comprehension `for` keeps its existing role.
- Generic constants obey the same compile-time compatibility rules as ordinary constants. Checked dependencies may remain symbolic in a generic definition but must resolve and evaluate for the required concrete use; unsupported or runtime-dependent evaluation is diagnosed without a runtime fallback. No separate initial evaluator subset or arbitrary compile-time function execution is introduced.
