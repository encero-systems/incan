# Derives and traits (Explanation)

This page explains the mental model behind **derives**, **dunder overrides**, and **traits** in Incan. For the exact
catalog of supported derives and signatures, see the Reference:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)

## The three mechanisms (and when to use each)

Incan uses three mechanisms to implement behavior:

- [`@derive(...)`: out-of-the-box behavior](#derive-out-of-the-box-behavior)
- [dunder methods: custom behavior](#dunder-methods-custom-behavior)
- [traits: domain capabilities](#traits-domain-capabilities)

### Derive (out-of-the-box behavior)

Use `@derive(...)` when the default, structural behavior is what you want (for example: field-based equality).

Incan derives are intentionally “Python-first”:

- You get common behaviors without writing boilerplate
- Behaviors are still explicit in your type definition

Read more about [`@derive(...)`: out-of-the-box behavior](../reference/derives_and_traits.md).

### Dunder methods (custom behavior)

Use dunder methods when you need **custom semantics** for a built-in capability:

- `__str__`: custom string output
- `__eq__`: custom equality
- `__lt__`: custom ordering
- `__hash__`: custom hashing

Incan treats “derive + corresponding dunder” as a **conflict**. The idea is to avoid ambiguity and keep the mental model
simple: “either it’s the default behavior, or it’s my behavior.”

Read more about [dunder methods: custom behavior](../how-to/customize_derived_behavior.md).

### Traits (domain capabilities)

Use traits for reusable, domain-specific capabilities:

- You define a contract once
- Multiple types can opt into it via `with TraitName`
- Traits can include default method bodies
- Traits are always abstract, so the trait name itself can be used directly in annotations
- Traits can build capability hierarchies with `trait Sub with Base:`

Traits are not “the derive system.” Derives are a convenience for a small set of **built-in capabilities**; traits are a
general language feature for authoring reusable behavior.

That gives Incan a simple mental model: a trait is both a capability declaration and an abstract accepted type. If a function says it accepts `Collection[Order]`, that means “any concrete adopter of `Collection[Order]`”, not “rewrite this API as a hidden generic bound first”.

Read more about [traits: domain capabilities](../tutorials/book/11_traits_and_derives.md).

## Debug vs Display: two string representations

Incan intentionally separates two kinds of “stringification”:

- **Debug** (`{:?}`): developer-facing, structured, and not user-overridable
- **Display** (`{}`): user-facing output; you can override via `__str__`

This mirrors the common “logs vs user output” split: Debug is stable and structural; Display is designed for
human-friendly formatting.

## `@rust.extern` (Rust-backed functions)

You may see `@rust.extern` used in stdlib sources and Rust-backed libraries. It marks functions whose body are provided
by a Rust module (declared via `rust.module()`).

The intended meaning is:

- the function's signature is defined in Incan (`.incn` source)
- the function's implementation lives in a Rust crate, mapped via `rust.module()`
- the compiler emits a call to the Rust implementation instead of compiling the `...` body

This lets the stdlib (and third-party libraries) wrap Rust crates with Incan-shaped APIs while keeping most logic in
pure Incan.

See also:

- [How derives work](how_derives_work.md)

## Field defaults and construction (pydantic-like ergonomics)

Field defaults (`field: T = expr`) are part of Incan’s “pydantic-like” ergonomics:

- If you omit a field and it has a default, the default is used
- If a field has no default, you must provide it at construction time

Separately, `@derive(Default)` provides `Type.default()` as a baseline constructor. It uses field defaults when present,
and otherwise falls back to type defaults.

## See also

- [Models and classes](models_and_classes/index.md)
- [How derives work](how_derives_work.md)
- [Strings and formatting (Book)](../tutorials/book/07_strings_and_formatting.md)
- [Traits and derives (Book)](../tutorials/book/11_traits_and_derives.md)
