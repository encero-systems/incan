# RFC 044: Open-Ended Trait Methods

- **Status:** Draft
- **Created:** 2026-03-27
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (trait-based operator overloading)
    - RFC 026 (Superseded — archival; Rust trait contracts on wrappers: RFC 043)
    - RFC 003 (traits and derives)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/201
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes to make the colon (`:`) optional after method declarations inside trait definitions. Currently, trait methods must end with `: ...` to indicate they are unimplemented (abstract). This RFC makes the colon optional, allowing authors to write `def example_method(value: str) -> str` without the trailing `: ...`, while still maintaining the semantic that the method has no body and must be implemented by concrete types.

## Motivation

### Current syntax is verbose for abstract methods

Today, trait methods require a colon followed by `...` to indicate they are unimplemented:

```incan
trait MyFavTrait:
    def example_method(value: str) -> str: ...
```

The `: ...` serves as a visual marker that the method has no body, but it adds visual clutter and is redundant with the trait context itself. Inside a trait, it's already clear that methods are abstract unless they have a body.

### Consistency with Python-like simplicity

Incan aims for Python-like simplicity where possible. In Python, method declarations in abstract base classes don't require a special marker — the absence of a body (or presence of `pass`) is sufficient. This RFC brings Incan closer to that simplicity while maintaining type safety.

### Reduces boilerplate in trait definitions

Traits are often used to define interfaces or contracts, and many traits have only abstract methods. Making the colon optional reduces boilerplate and improves readability:

```incan
# Current
trait Serializer:
    def serialize(value: Any) -> str: ...
    def deserialize(data: str) -> Any: ...

# Proposed
trait Serializer:
    def serialize(value: Any) -> str
    def deserialize(data: str) -> Any
```

## Goals

- Allow trait method declarations inside traits to omit the trailing `: ...` suffix while remaining semantically abstract.
- Maintain full backward compatibility with the existing `: ...` syntax.
- Keep the grammar unambiguous: the parser must distinguish abstract methods, default implementations, and docstring-bearing methods without context-free ambiguity.

## Non-Goals

- Removing or deprecating the `: ...` syntax.
- Allowing body-less method declarations outside trait definitions.
- Changing how trait method implementations work in `model` or `with` blocks.

## Guide-level explanation

### Defining traits with open-ended methods

You can now define trait methods without the trailing `: ...`:

```incan
trait MyFavTrait:
    def example_method(value: str) -> str
    def another_method(x: int, y: int) -> int
```

Both forms are valid and equivalent:

```incan
# Both are valid and identical
trait Trait1:
    def method1(x: int) -> int: ...

trait Trait2:
    def method1(x: int) -> int
```

### Implementing traits

Concrete types implement these methods using the `with` clause:

```incan
model MyType:
    value: int

model MyType with MyFavTrait:
    def example_method(self, value: str) -> str:
        return f"Result: {value}"
    
    def another_method(self, x: int, y: int) -> int:
        return x + y
```

### When to use which form

- Use `def method(...) -> ...` when the method has no body (abstract in a trait)
- Use `def method(...) -> ...: ...` if you prefer the explicit marker (backward compatible)
- Use `def method(...) -> ...: body` when the method has an implementation (in a `model` or `with` block)

### Docstrings require a colon

If a method has a docstring, the colon is mandatory to separate the signature from the docstring:

```incan
# Good — docstring with colon
def function(val: str) -> None:
    """This is a docstring"""

# Bad — docstring without colon (invalid syntax)
def function2(val: str) -> None
    """This is invalid"""
```

The colon serves as a delimiter between the method signature and its body (which includes docstrings).

## Reference-level explanation

### Syntax

The grammar for trait method declarations is extended to make the `: ...` suffix optional:

```text
trait_method_decl ::= "def" identifier "(" params ")" return_type [": ..." | ":" block]
```

Where:

- `params` is the parameter list
- `return_type` is the optional return type annotation
- `: ...` is the explicit abstract marker (optional)
- `: block` is the method body (for concrete implementations)

### Semantics

- A method declaration inside a `trait` without a body must be treated as abstract (no implementation).
- A method declaration inside a `trait` with `: ...` must be treated as abstract (no implementation); the two forms are semantically equivalent.
- A method declaration inside a `trait` with a body (`: block`) must be treated as a concrete default implementation.
- A method declaration inside a `model` without a body must be a type error; every method in a `model` must have an implementation.
- A method declaration inside a `model` with `: ...` must be a type error for the same reason.
- A method declaration followed by a docstring must use `:` to separate the signature from the body; the body-less form must not appear in that position.

### Type checking

The typechecker must enforce that:

1. Every abstract method declared in a trait (with or without `: ...`) is implemented by any type that `with`s that trait.
2. Concrete methods with default implementations may be overridden but must not be required to be overridden.
3. The `: ...` suffix, when present, is semantically equivalent to the absence of a body; the typechecker must not distinguish the two forms.
4. Methods followed by a docstring must use `:` as a separator; the parser must reject the body-less form in that position, and the typechecker must not treat a following docstring as a body.

### Interaction with existing features

- **Default implementations**: A trait method with a body provides a default implementation that implementors may override but need not.
- **Trait bounds**: Generic functions may still use trait bounds without change.
- **Derives**: `@derive(TraitName)` must continue to work for traits whose abstract methods use either form.
- **Docstrings**: A method with a docstring must use `:` to delimit the signature from the body; the body-less form must not be used in that position.

## Design details

### Compatibility / migration

- **Non-breaking**: The current `: ...` syntax continues to work
- **No migration needed**: Existing code continues to function
- **New syntax**: Authors can choose to omit the colon for cleaner syntax

## Alternatives considered

- **Keep current syntax**
    The current `: ...` syntax is explicit and unambiguous, but adds visual clutter. The trade-off is clarity vs. verbosity.

- **Require no colon** (breaking change)
    A more aggressive alternative would be to make the colon required only when there's a body, but this would be a breaking change for existing code.

## Drawbacks

### Reduced explicitness

Some may argue that the `: ...` makes it more explicit that a method is abstract. However, the trait context already provides this signal.

### Parser complexity

The parser must distinguish between trait and non-trait contexts to determine if a missing body is valid. This is already handled for other syntax rules, so the complexity is minimal.

## Layers affected

- **Parser / AST**: The parser must accept method declarations without `: ...` in trait contexts
- **Typechecker / Symbol resolution**: No change — abstract methods are already tracked
- **IR Lowering**: No change — abstract methods don't generate IR
- **Stdlib / Runtime**: No change
- **Formatter**: The formatter should handle both forms consistently (prefer no colon for new code)
- **LSP / Tooling**: Hover and diagnostics should work for both forms

## Unresolved questions

- Should the formatter prefer one form over the other? (Recommendation: prefer no colon for new code)
- Should lints warn about mixing both forms in the same trait? (Recommendation: no — both are valid)

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
