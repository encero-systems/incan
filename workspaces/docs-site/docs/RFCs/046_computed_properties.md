# RFC 046: Computed properties (`property name -> Type`)

- **Status:** Draft
- **Created:** 2026-03-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 042 (traits are always abstract)
    - RFC 044 (open-ended trait methods)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/203
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces **computed properties**: members declared with the `property` keyword, a name, **`->`**, a return type, and a body. They are **field-like at use sites** (no argument list, no call parentheses) but **execute a body** when read, like Scala’s parameterless `def name: T` methods or Python’s `@property`. The syntax is intentionally distinct from `def` so authors and tools can tell **cheap attribute access** from **general methods**.

## Motivation

### Today: everything is a method

Incan methods use Python-shaped declarations: `def name(self) -> T:`. Callers must use **`()`** unless the language later special-cases nullary methods. For APIs where a value is **logically an attribute** of an object (schema fields, dimensions, derived flags, cached views), requiring `()` is noisy and easy to get wrong when porting from languages with first-class properties.

### Intent at a glance

A dedicated keyword makes the contract obvious in the **definition**:

```incan
property schema_fields -> list[FieldSchema]:
    return self._fields
```

Readers see immediately: **no parameters**, **one typed result**, **field-like access** at use sites.

### Tooling and style

Properties invite **lighter** implementations (no side effects, O(1) or documented cost). Linters and docs can treat them differently from `def`. IDEs can surface them next to fields in outline views without conflating them with methods that take arguments.

## Goals

- Add **`property <identifier> -> <Type>:`** with an indented body; **no parameter list** (except as explicitly extended later for `self` on types that use it).
- At **use sites**, allow **`<expr>.<name>`** without `()` when `<name>` resolves to a property.
- Give properties **the same type-checking story** as a nullary instance operation returning `Type` (including generic inference where applicable).
- Specify the high-level execution and interop model for properties, including how they interact with **traits** and **Rust interop**.

## Non-Goals

- **Setters** (`property x` with assignment) in the first version; they may be a follow-up RFC.
- **`async property`** or properties that return `Awaitable` with special await syntax.
- **Static** or **class** properties (unless resolved as an explicit extension in Unresolved questions).
- Deprecating or removing any existing `def` syntax.
- Changing **stored** `model` / `class` fields: those remain data members with the existing grammar.

## Guide-level explanation

### Defining a property on a type with a body

```incan
pub class Dataset[T]:
    pub _fields: list[FieldSchema]

    property schema_fields -> list[FieldSchema]:
        return self._fields
```

### Using a property

```incan
def use(ds: Dataset[int]) -> None:
    cols = ds.schema_fields   # no ()
    for f in cols:
        _ = f.name
```

### Contrast with a method

```incan
    def row_count(self) -> int:
        return self._compute_row_count()
```

Call site: **`ds.row_count()`** — parentheses required.

### Optional: explicit `self` in the signature (design choice)

This RFC **prefers** inferring the receiver from context (same as `def` methods on classes) so the declaration stays minimal:

```incan
property area -> float:
    return self.width * self.height
```

If the language requires **`self`** for consistency with `def`, the reference section can mandate:

```incan
property area(self) -> float:
    return self.width * self.height
```

Only **one** of these forms should be normative; see **Unresolved questions**.

## Reference-level explanation (precise rules)

### Syntax (grammar-ish)

- **`property`** is a keyword introducing a **property declaration**.
- Form: **`property` *identifier* [`(` *receiver* `)`] `->` *type* `:` *newline* *block***
  - The bracketed receiver is optional; if allowed, it must match the containing type’s method convention (e.g. `self` on classes).
- **No** comma-separated parameters; properties are **nullary** readers in v1.
- The **body** is a **suite** (block) like a function body; it **must** produce a value compatible with the annotated return type (same rules as `return` in `def`).
- Properties may have **leading docstrings** in the block if the language allows the same as for `def`.

### Name resolution and use sites

- A **property access** is **`primary "." identifier`** where `identifier` names a property on the type of `primary`.
- **`()` must not** follow the identifier for a property access; if the user writes `obj.prop()`, that is either **invalid** or resolves to a **different** symbol (e.g. a method named `prop` if overloads exist — v1 should **forbid** the same simple name for both a property and a method on the same type to avoid ambiguity).

### Typing

- The property’s return type is **explicit** after `->` (required in v1; inference from body alone is non-goal unless specified later).
- **Variance / borrowing**: same rules as for methods returning `T`, following the existing Incan-to-Rust interop contract.

### Runtime and side effects

- Semantics are **call a function** when the property is read; implementations may **cache** only if the author does so inside the body (no implicit memoization in v1).
- **Normative style** (for docs/lints, not necessarily hard errors): properties **should** be cheap and **should not** perform surprising I/O; heavy work **should** remain `def` methods.

### Visibility

- Properties use the same **`pub` / default** visibility rules as methods and fields on the containing declaration.

### Traits

- **Trait** members may declare **abstract** properties (no body, or body `...` — aligned with RFC 044 conventions for trait methods) that implementors must define.
- Concrete **`with Trait` blocks** implement properties with the same `property name -> T:` syntax.

(Exact trait syntax for abstract properties should match whatever RFC 044 stabilizes for abstract methods.)

### Rust interop

- A public Incan property on a type that exports to Rust should appear as a **Rust method** with a stable name (for example `schema_fields` or a documented mangling) returning the mapped result type.
- **Calling from Rust**: use the generated method; there is no special Rust “field” unless the emitter explicitly documents one.

### Errors

- Diagnostic if **`()`** is used on a property.
- Diagnostic if **parameters** appear in a v1 property declaration.
- Diagnostic if **duplicate** names collide between a property and a field or method on the same type (v1 recommendation: hard error).

## Design details

### Why `property` plus `->`?

- **`property`** matches Python’s conceptual keyword and signals “field-like.”
- **`-> Type`** between **name** and **type** avoids overloading the post-parameter `-> Ret` of `def` in a confusing way: there is **no parameter list** before this arrow, so the parser can distinguish **`property foo -> T:`** from **`def foo(self) -> T:`**.

### Interaction with `model`

- **`model`** types emphasize **stored** fields; computed members may still be useful (e.g. derived attributes). This RFC **allows** `property` on **`model`** bodies where the language already allows methods, subject to the same restrictions as methods for `model` (if any).
- If **`model`** is restricted to data-only in some contexts, properties follow those rules.

### Decorators

- If Incan gains user-defined decorators (RFC 036), this RFC **defers** whether `@decorator` applies to `property` until decorator semantics are stable; v1 may **disallow** decorators on properties or treat them like method decorators consistently.

### Compatibility / migration

- **Not breaking**: purely additive keyword and declaration form.
- Existing code keeps using `def`; no automatic rewrite required.

## Alternatives considered

1. **Python `@property` on `def`**
   - Familiar to Python users but splits declaration across decorator + `def`, and `def` still looks like a method.

2. **Only `def name(self) -> T` with a “call without parens” rule for nullary methods**
   - Fewer keywords but **blurs** heavy methods vs attributes; harder for tooling and style guides.

3. **Scala-style `def name: T` without `property`**
   - Minimal but **less** obvious to Python-oriented readers; `property` is clearer in Incan’s ecosystem.

4. **`get name -> T:` or `let name: T`** forms
   - Rejected for now as less aligned with existing `def` / type syntax patterns.

## Drawbacks

- **New keyword** `property` (soft-keyword considerations if needed for migration from identifiers named `property`).
- **Two ways** to expose zero-argument getters (`def` vs `property`) — authors need guidance.
- **No implicit caching**: every read runs the body unless the author caches (same as Python).

## Layers affected

- **Surface syntax**: the language needs a distinct `property` declaration form, separate from `def`.
- **Type system**: member lookup must distinguish properties from methods, enforce access without `()`, and match abstract property requirements in traits.
- **Execution handoff**: property reads must preserve field-like use-site syntax while executing property bodies according to the declared contract.
- **Interop / emission**: emitted artifacts must preserve the property-vs-method distinction in a predictable way, including the Rust-facing method form.
- **Formatter**: `property` blocks should format consistently and preserve `->` spacing.
- **LSP**: completion should treat properties like fields; hover should show the return type; snippets should avoid inserting `()`.

## Unresolved questions

1. **`self` in the declaration**: Is **`property area -> T:`** (no `self`) the only form, or is **`property area(self) -> T:`** required for symmetry with `def`?
2. **Abstract properties in traits**: Exact syntax (`property x -> T` vs `property x -> T: ...`) and parity with RFC 044.
3. **Same name as a field**: Forbid entirely vs allow shadowing (forbid recommended).
4. **Setters**: Deferred — if added later, spell as `property name` with assignment or `@name.setter`-style sub-grammar.
5. **Inheritance**: `super` or base property overrides — same rules as method overrides.
6. **Generic properties**: `property items -> list[T]` on `class Box[T]` — ensure variance matches method rules.
7. Should we perhaps use **`prop`** instead of **`property`**?
8. Should we add an @property decorator anyway and make `prop`/`property` just desugaring calls?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
