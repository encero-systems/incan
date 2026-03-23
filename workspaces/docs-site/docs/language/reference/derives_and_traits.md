# Derives and Traits

<!--
Link index

Use reference-style links like:
- [Debug][derive-debug]
- [Error Handling Guide][guide-error-handling]

So we can change the destination in one place if we move/rename sections.
-->

<!-- Built-in derives (anchors in this page) -->
[derive-debug]: #debug-automatic
[derive-display]: #display-custom-with-__str__
[derive-eq]: #eq-equality
[derive-ord]: #ord-ordering
[derive-hash]: #hash
[derive-clone]: #clone
[derive-copy]: #copy
[derive-default]: #default
[derive-serialize]: #serialize
[derive-deserialize]: #deserialize
[derive-validate]: #validate-models-only

<!-- Related sections (anchors in this page) -->
[auto-derives]: #automatic-derives

<!-- Other docs -->
[traits-doc]: #traits-authoring
[guide-error-handling]: ../explanation/error_handling.md

This page is a **Reference** for Incan derives, dunder overrides, and trait authoring. For guided learning, use:

- [The Incan Book: Traits and derives](../tutorials/book/11_traits_and_derives.md)
- [Error Handling Guide][guide-error-handling]

---

## Conflicts & precedence

Rules that resolve “derive vs dunder” (these are intentional and strict):

- **Dunder overrides are explicit behavior**: if you write a dunder (`__str__`, `__eq__`, `__lt__`, `__hash__`), that is
  the behavior for that capability.
- **Conflicts are errors**: you must not combine a dunder with the corresponding `@derive(...)`.
- **Auto-added traits are the exception**: some traits are automatically added to `model` / `class` / `enum` / `newtype`
  (see [Automatic derives][auto-derives]). You don’t need to spell them out.

---

## Automatic derives

The compiler automatically adds these derives:

| Construct | Auto-added derives                                             |
| --------- | -------------------------------------------------------------- |
| `model`   | `Debug`, `Display`, `Clone`                                    |
| `class`   | `Debug`, `Display`, `Clone`                                    |
| `enum`    | `Debug`, `Display`, `Clone`, `Eq`                              |
| `newtype` | `Debug`, `Display`, `Clone` (and `Copy` if underlying is Copy) |

Notes:

- `Debug` and `Display` are always available for these constructs.
- `Display` has a default representation (like Python’s default `__str__`) unless you define `__str__`.

---

## Derive catalog (quick index)

Only the items usable in `@derive(...)` are derives:

| Derive                            | Capability                | Override   | Notes                   |
| --------------------------------- | ------------------------- | ---------- | ----------------------- |
| [Debug][derive-debug]             | Debug formatting (`{:?}`) | —          | Auto-added              |
| [Display][derive-display]         | Display formatting (`{}`) | `__str__`  | Auto-added              |
| [Eq][derive-eq]                   | `==` / `!=`               | `__eq__`   | Conflicts are errors    |
| [Ord][derive-ord]                 | Ordering + `sorted(...)`  | `__lt__`   | Conflicts are errors    |
| [Hash][derive-hash]               | `Set` / `Dict` keys       | `__hash__` | Conflicts are errors    |
| [Clone][derive-clone]             | `.clone()`                | —          | Auto-added              |
| [Copy][derive-copy]               | Implicit copy             | —          | Marker trait            |
| [Default][derive-default]         | `Type.default()`          | —          | Baseline constructor    |
| [Serialize][derive-serialize]     | JSON stringify            | —          | `json_stringify(value)` |
| [Deserialize][derive-deserialize] | JSON parse                | —          | `T.from_json(str)`      |
| [Validate][derive-validate]       | Validated construction    | —          | Models only             |

Detailed pages:

**Derives**:

- [String representation](derives/string_representation.md)
- [Comparison](derives/comparison.md)
- [Copying/default](derives/copying_default.md)
- [Serialization](derives/serialization.md)
- [Validation](derives/validation.md)
- [Custom behavior](derives/custom_behavior.md)

**Stdlib traits**:

- [Overview](stdlib_traits/index.md)
- [Collection protocols](stdlib_traits/collection_protocols.md)
- [Indexing and slicing](stdlib_traits/indexing_and_slicing.md)
- [Callable objects](stdlib_traits/callable.md)
- [Operator traits](stdlib_traits/operators.md)
- [Conversion traits](stdlib_traits/conversions.md)

---

## Derive dependencies and requirements

Some derives imply prerequisite derives (the compiler adds them automatically):

| If you request | Compiler also adds              |
| -------------- | ------------------------------- |
| `Eq`           | `PartialEq`                     |
| `Ord`          | `Eq`, `PartialEq`, `PartialOrd` |

Note: you may also request `PartialEq` and `PartialOrd` explicitly via `@derive(PartialEq)` / `@derive(PartialOrd)`.

Semantic requirements (not “auto-added”):

- If you derive `Hash`, you almost always also want `Eq`. Prefer `@derive(Eq, Hash)`.
- If you provide custom equality via `__eq__`, your hashing (derived or custom) must remain consistent.

---

## Common compiler diagnostics

### Unknown derive

```incan
@derive(Debg)  # Typo
model User:
    name: str
```

Expected: “unknown derive” with a list of valid derive names.

### Deriving a non-trait

```incan
model User:
    name: str

@derive(User)  # wrong: User is a model, not a derive/trait
model Admin:
    level: int
```

Expected: “cannot derive a model/class” with a hint to use `with TraitName` for trait implementations.

---

## Decorators (`@staticmethod`, `@requires`)

Incan has four built-in decorators. `@derive(...)` is covered [above](#derive-catalog-quick-index); `@rust.extern` is a
Rust interop decorator (see [RFC 023]). This section covers the two method/trait decorators.

--8<-- "_snippets/rfcs_refs.md"

### `@staticmethod`

--8<-- "_snippets/language/decorators/staticmethod.md"

See also: [Classes: Static methods](../explanation/models_and_classes/classes.md#static-methods-staticmethod)

### `@requires(...)`

Covered below in [Traits (authoring)](#requires-adopter-contract).

---

## Traits (authoring)

Traits define reusable capabilities. Traits are always abstract: you opt concrete types in with `with TraitName`, and you may also use the trait name itself directly in annotations. Methods can be required (`...`) or have defaults.

Traits may adopt other traits with the same `with` syntax to form capability hierarchies. That means a narrower trait can refine a broader one, and any concrete adopter of the narrower trait is also accepted where the broader trait is expected.

```incan
trait Describable:
    def describe(self) -> str:
        return "An object"

class Product with Describable:
    name: str

def main() -> None:
    p = Product(name="Laptop")
    println(p.describe())
```

```incan
trait Collection[T]:
    def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
    def sorted(self) -> Self: ...

def first_item(values: Collection[int]) -> int:
    return values.first()
```

Rules to keep in mind:

- Traits are abstract and must not be constructed directly with `TraitName(...)`.
- A value annotated as `Collection[int]` may be any concrete adopter of that trait instantiation.
- Supertrait relationships are transitive: if `OrderedCollection[T]` adopts `Collection[T]`, adopters of `OrderedCollection[T]` also satisfy `Collection[T]`.

### `@requires(...)` (adopter contract)

`@requires(...)` is a decorator you can put on a `trait` to declare **which adopter fields must exist** (and what types
they must have).

Syntax:

```incan
@requires(field_a: TypeA, field_b: TypeB)
trait MyTrait:
    ...
```

What the compiler enforces:

- When a `class`/`model` adopts a trait (`with MyTrait`), it must provide **all required fields** with compatible types.
- Trait default methods may access adopter fields like `self.field` **only if** that field is declared in `@requires(...)`.
- Mutating adopter fields still requires `mut self` (same as normal methods).
- Required fields propagate through trait hierarchies: if `Sub` adopts `Base`, adopters of `Sub` must also satisfy `Base`'s `@requires(...)` contract.

Example:

```incan
@requires(name: str)
trait Loggable:
    def log(self, msg: str) -> None:
        println(f"[{self.name}] {msg}")

class Service with Loggable:
    name: str
```

Example (mutation):

```incan
@requires(count: int)
trait Counter:
    def bump(mut self) -> None:
        self.count += 1
```

See also: [Traits (authoring)][traits-doc].

---

## Debug (Automatic)

**Format**: `{value:?}` (Debug)

**Override**: not supported (Debug is compiler-generated).

```incan
model Point:
    x: int
    y: int

def main() -> None:
    p = Point(x=10, y=20)
    println(f"{p:?}")  # Point { x: 10, y: 20 }
```

---

## Display (Custom with `__str__`)

**Format**: `{value}` (Display)

**Default behavior**: custom types have a default Display representation.

**Custom behavior**: define `__str__(self) -> str`.

> Conflict rule: if you define `__str__`, you must not also `@derive(Display)`.

```incan
model User:
    name: str
    email: str

    def __str__(self) -> str:
        return f"{self.name} <{self.email}>"

def main() -> None:
    u = User(name="Alice", email="alice@example.com")
    println(f"{u}")    # Alice <alice@example.com>
    println(f"{u:?}")  # User { name: "Alice", email: "alice@example.com" }
```

---

## Eq (Equality)

**What it does**: enables `==` / `!=`.

**Custom behavior**: define `__eq__(self, other: Self) -> bool`.

> Conflict rule: if you define `__eq__`, you must not also `@derive(Eq)`.

```incan
model User:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id
```

---

## Ord (ordering)

**What it does**: enables ordering operators and `sorted(...)`.

**Custom behavior**: define `__lt__(self, other: Self) -> bool`.

> Conflict rule: if you define `__lt__`, you must not also `@derive(Ord)`.

```incan
model Task:
    priority: int
    name: str

    def __lt__(self, other: Task) -> bool:
        return self.priority < other.priority
```

---

## Hash

**What it does**: enables use as `Set` members and `Dict` keys.

**Custom behavior**: define `__hash__(self) -> int`.

> Conflict rule: if you define `__hash__`, you must not also `@derive(Hash)`.
>
> Consistency rule: if `a == b`, then `a.__hash__() == b.__hash__()`.

```incan
@derive(Eq, Hash)
model UserId:
    id: int
```

---

## Clone

**What it does**: enables `.clone()` (deep copy).

Auto-added for `model`/`class`/`enum`/`newtype`.

---

## Copy

**What it does**: enables implicit copying (marker trait).

Use only for small value types; all fields must be Copy.

---

## Default

**What it does**: provides `Type.default()` baseline construction.

**Field defaults vs `Default`**:

- Field defaults (`field: T = expr`) are used by normal constructors when omitted.
- `@derive(Default)` adds `Type.default()`; it uses field defaults when present, otherwise type defaults.

Constructor rule:

- If a field has **no default**, it must be provided when constructing the type.

```incan
@derive(Default)
model Settings:
    theme: str = "dark"
    font_size: int = 14

def main() -> None:
    a = Settings()               # OK: all omitted fields have defaults
    b = Settings(font_size=16)   # OK
    c = Settings.default()       # OK
```

---

## Serialize

**What it does**: enables JSON serialization.

**API**:

- `json_stringify(value)` → `str`

```incan
@derive(Serialize)
model User:
    name: str
    age: int

def main() -> None:
    u = User(name="Alice", age=30)
    println(json_stringify(u))
```

---

## Deserialize

**What it does**: enables JSON parsing into a type.

**API**:

- `T.from_json(input: str)` → `Result[T, str]`

```incan
@derive(Deserialize)
model User:
    name: str
    age: int

def main() -> None:
    result: Result[User, str] = User.from_json("{\"name\":\"Alice\",\"age\":30}")
```

---

## Validate (Models only)

**What it does**: enables validated construction for models.

**API**:

- `TypeName.new(...)` → `Result[TypeName, E]`

Rule:

- If a `model` derives `Validate`, you must construct it via `TypeName.new(...)`.
  Raw construction via `TypeName(...)` is a compile-time error.

```incan
@derive(Validate)
model EmailUser:
    email: str

    def validate(self) -> Result[EmailUser, str]:
        if "@" not in self.email:
            return Err("invalid email")
        return Ok(self)

def make_user(email: str) -> Result[EmailUser, str]:
    return EmailUser.new(email=email)
```

See: [Derives: Validation](derives/validation.md)

---

## Compiler errors (reference)

### Unknown derive (example)

```incan
@derive(Debg)
model User:
    name: str
```

```bash
type error: Unknown derive 'Debg'
```

### Deriving a non-trait (example)

```incan
model User:
    name: str

@derive(User)
model Admin:
    email: str
```

```bash
type error: Cannot derive 'User' - it is a model, not a trait
```

---

## Reflection (automatic)

Models and classes provide:

- `__fields__() -> FrozenList[FieldInfo]`
- `__class_name__() -> str`

Note:

- Field metadata like `[alias="..."]` and `[description="..."]` is **model-only**. For `class`, `FieldInfo.alias`
  and `FieldInfo.description` are always `None` and `FieldInfo.wire_name == FieldInfo.name`.

See [Reflection (Reference)](reflection.md) for `FieldInfo` structure details.

```incan
model User:
    name: str

def main() -> None:
    u = User(name="Alice")
    println(u.__class_name__())
    println([f.name for f in u.__fields__()])
```
