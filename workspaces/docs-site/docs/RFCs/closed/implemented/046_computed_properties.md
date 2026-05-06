# RFC 046: Computed properties (`property name -> Type`)

- **Status:** Implemented
- **Created:** 2026-03-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 042 (traits are always abstract)
    - RFC 044 (open-ended trait methods)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/203
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

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

- Add **`property <identifier> -> <Type>:`** with an indented body and no parameter list.
- At **use sites**, allow **`<expr>.<name>`** without `()` when `<name>` resolves to a property.
- Give properties **the same type-checking story** as a nullary instance operation returning `Type` (including generic inference where applicable).
- Specify the high-level execution and interop model for properties, including how they interact with **traits** and **Rust interop**.

## Non-Goals

- **Setters** (`property x` with assignment); they require a separate RFC.
- **`async property`** or properties that return `Awaitable` with special await syntax.
- **Static** or **class** properties; they require a separate RFC.
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

### Receiver model

Properties infer the instance receiver from the containing type, so the declaration stays focused on the attribute-shaped contract:

```incan
property area -> float:
    return self.width * self.height
```

`self` is available inside the body by the same rules as instance methods on the containing type. A parameter list in a property declaration is rejected.

## Reference-level explanation (precise rules)

### Syntax (grammar-ish)

- **`property`** is a keyword introducing a **property declaration**.
- `property` is a **soft declaration keyword**: it introduces a property only in member-declaration positions where a property is valid. Outside those positions, existing identifier uses of `property` remain ordinary identifiers.
- Form: **`property` *identifier* `->` *type* `:` *newline* *block***
- **No** comma-separated parameters; properties are **nullary** readers.
- The **body** is a **suite** (block) like a function body; it **must** produce a value compatible with the annotated return type (same rules as `return` in `def`).
- Properties may have **leading docstrings** in the block if the language allows the same as for `def`.

### Name resolution and use sites

- A **property access** is **`primary "." identifier`** where `identifier` names a property on the type of `primary`.
- **`()` must not** follow the identifier for a property access. If the user writes `obj.prop()`, the typechecker reports a property-called-as-method diagnostic.
- A field, method, property, or trait member may not share the same simple name within the same type or trait implementation.

### Typing

- The property’s return type is **explicit** after `->`; inference from body alone is not part of this RFC.
- **Variance / borrowing**: same rules as for methods returning `T`, following the existing Incan-to-Rust interop contract.

### Runtime and side effects

- Semantics are **call a function** when the property is read; implementations may **cache** only if the author does so inside the body (no implicit memoization).
- **Normative style** (for docs/lints, not necessarily hard errors): properties **should** be cheap and **should not** perform surprising I/O; heavy work **should** remain `def` methods.

### Visibility

- Properties use the same **`pub` / default** visibility rules as methods and fields on the containing declaration.

### Traits

- **Trait** members may declare **abstract** properties that implementors must define.
- Abstract trait properties follow the abstract-member convention from RFC 044: `property name -> T` is preferred for new code, and `property name -> T: ...` is accepted if the trait-method grammar supports the same body marker.
- Concrete **`with Trait` blocks** implement properties with the same `property name -> T:` syntax and a body.

Trait properties do not introduce trait default implementations in this RFC; they are requirements, matching the “traits are always abstract” direction from RFC 042.

### Rust interop

- A public Incan property on a type that exports to Rust should appear as a **Rust method** with a stable name (for example `schema_fields` or a documented mangling) returning the mapped result type.
- **Calling from Rust**: use the generated method; there is no special Rust “field” unless the emitter explicitly documents one.

### Errors

- Diagnostic if **`()`** is used on a property.
- Diagnostic if **parameters** appear in a property declaration.
- Diagnostic if a property appears outside a class, model, trait, or concrete trait implementation context that supports members.
- Diagnostic if a property declaration uses unsupported modifiers such as `async`, `static`, class-level binding syntax, decorators, or setter forms.
- Diagnostic if **duplicate** names collide between a property and a field, method, or trait member on the same type (hard error).

## Design details

### Why `property` plus `->`?

- **`property`** matches Python’s conceptual keyword and signals “field-like.”
- **`-> Type`** between **name** and **type** avoids overloading the post-parameter `-> Ret` of `def` in a confusing way: there is **no parameter list** before this arrow, so the parser can distinguish **`property foo -> T:`** from **`def foo(self) -> T:`**.

### Interaction with `model`

- **`model`** types emphasize **stored** fields; computed members may still be useful (e.g. derived attributes). This RFC **allows** `property` on **`model`** bodies where the language already allows methods, subject to the same restrictions as methods for `model` (if any).
- If **`model`** is restricted to data-only in some contexts, properties follow those rules.

### Decorators

- Decorators are not part of computed property declarations. If RFC 036 or later decorator work wants decorators on properties, that interaction requires a separate RFC.

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

- **New soft declaration keyword** `property`.
- **Two ways** to expose zero-argument getters (`def` vs `property`) — authors need guidance.
- **No implicit caching**: every read runs the body unless the author caches (same as Python).

## Layers affected

- **Surface syntax**: the language needs a distinct `property` declaration form, separate from `def`.
- **Type system**: member lookup must distinguish properties from methods, enforce access without `()`, and match abstract property requirements in traits.
- **Execution handoff**: property reads must preserve field-like use-site syntax while executing property bodies according to the declared contract.
- **Interop / emission**: emitted artifacts must preserve the property-vs-method distinction in a predictable way, including the Rust-facing method form.
- **Formatter**: `property` blocks should format consistently and preserve `->` spacing.
- **LSP**: completion should treat properties like fields; hover should show the return type; snippets should avoid inserting `()`.

## Implementation Plan

### Phase 1: Parser, AST, and Formatter

- Parse `property name -> Type:` in class and model member bodies, trait declarations, and concrete trait implementations.
- Represent computed properties as first-class member declarations with spans for the keyword, name, return type, and body.
- Preserve `property` as a soft declaration keyword so non-member identifier uses remain valid.
- Format property declarations, abstract trait property declarations, and property bodies consistently with existing member formatting.
- Emit syntax diagnostics for parameter lists, unsupported modifiers, misplaced property declarations, and malformed return-type arrows.

### Phase 2: Symbol Model and Typechecker

- Store properties as a distinct member kind alongside fields and methods.
- Enforce duplicate-name rejection across fields, methods, properties, and trait members.
- Typecheck property bodies against the explicit return type and expose `self` according to the containing type's instance-member rules.
- Resolve `obj.property_name` as a read expression with the property's declared return type.
- Reject `obj.property_name()` with a property-called-as-method diagnostic.
- Enforce abstract trait property requirements in concrete adopters and trait implementation blocks.

### Phase 3: Lowering, IR, and Emission

- Lower property reads to an explicit computed-member call while preserving field-like source semantics.
- Lower property declarations to callable backend artifacts with the same receiver and return ownership rules as nullary methods.
- Emit public Rust-facing properties as stable Rust methods rather than fields.
- Ensure generic containing types substitute property return types and receiver types consistently through lowering and emission.

### Phase 4: Tooling, Docs, and Release Integration

- Teach LSP completion and hover to present properties as field-like members with return types and no call snippet.
- Add user-facing docs for declaring, reading, and choosing computed properties versus methods.
- Add active 0.3 release notes coverage.
- Bump the active `0.3.0-dev.N` version for the implementation.

## Implementation log

### Spec / lifecycle

- [x] Resolve RFC open questions into Design Decisions.
- [x] Move RFC 046 to In Progress for implementation pickup.
- [x] Keep RFC checklist synchronized as implementation phases land.

### Parser / AST / Formatter

- [x] Parser: accept `property name -> Type:` where member declarations are valid.
- [x] Parser: accept abstract trait property declarations in the chosen RFC 044-compatible form.
- [x] Parser diagnostics: reject property parameter lists, unsupported modifiers, malformed arrows, and misplaced declarations.
- [x] AST: represent property declarations as first-class member nodes with precise spans.
- [x] Formatter: round-trip class, model, trait, and concrete implementation properties.

### Typechecker / Symbols

- [x] Symbol table: store properties as a distinct member kind.
- [x] Typechecker: typecheck property bodies against explicit return annotations.
- [x] Typechecker: expose `self` inside property bodies according to instance-member rules.
- [x] Typechecker: resolve field-like property reads to the declared return type.
- [x] Typechecker diagnostic: reject `obj.property()` when `property` resolves to a property.
- [x] Typechecker diagnostic: reject duplicate names across fields, methods, properties, and trait members.
- [x] Traits: enforce abstract property requirements in concrete adopters and implementations.
- [x] Generics: substitute containing type parameters in property return types.

### Lowering / IR / Emission

- [x] Lower property declarations to callable backend artifacts with instance receivers.
- [x] Lower property reads to explicit computed-member calls.
- [x] Emit Rust methods for public computed properties.
- [x] Preserve ownership, borrowing, and generic substitution rules shared with nullary methods.

### Tooling / Docs / Release

- [x] LSP: complete properties as field-like members without `()`.
- [x] LSP: hover properties with their declared return type.
- [x] User docs: document computed property declarations and reads.
- [x] User docs: explain when to choose `property` versus `def`.
- [x] Release notes: add active `0.3` entry.
- [x] Version: bump the active development version from `0.3.0-dev.33` to the next dev increment.

### Tests

- [x] Parser tests for valid class, model, and trait properties.
- [x] Parser diagnostic tests for invalid property declaration shapes.
- [x] Formatter idempotency tests for property declarations.
- [x] Typechecker tests for property body return checking and property read typing.
- [x] Typechecker diagnostic tests for property calls and duplicate member names.
- [x] Trait conformance tests for abstract property requirements.
- [x] Codegen snapshot tests for property declarations and read expressions.
- [x] Integration test that compiles and runs property reads on concrete instances.

## Design Decisions

- The spelling is `property`, not `prop`, so the member kind is obvious in source and tooling.
- `property` is a soft declaration keyword in member-declaration positions, not a globally reserved identifier.
- Properties use the `property name -> Type:` form. `property name(self) -> Type:` and any other parameter-list form are rejected.
- Properties infer their instance receiver from the containing type, and `self` is available in the body under the same rules as methods on that type.
- Properties are instance readers only. Static, class-level, async, setter, and decorator forms are out of scope for this RFC and should produce explicit diagnostics rather than partial parser support.
- Properties are allowed on classes and on models where model bodies already allow executable members.
- Trait properties are abstract requirements. `property name -> T` is the preferred abstract trait spelling, with `property name -> T: ...` accepted only to the same extent RFC 044 accepts that marker for abstract methods.
- Concrete trait implementations use `property name -> T:` with a body; this RFC does not add default property implementations to traits.
- A property read executes the body each time. There is no implicit memoization, and any caching must be written by the author.
- A field, method, property, or trait member may not share the same simple name within one type or trait implementation.
- Calling a property with `()` is an error even if a call expression would otherwise typecheck structurally.
- Generic properties use the containing type’s generic parameters and follow the same return-type substitution, variance, and ownership rules as nullary methods.
- Property override and `super` behavior follows the existing method override model once inheritance support applies; this RFC adds no separate override rules.
- Rust-facing output exposes public properties as generated Rust methods, not Rust fields.
