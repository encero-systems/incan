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

Traits are not “the derive system.” Derives are a convenience for a small set of **built-in capabilities**; traits are a
general language feature for authoring reusable behavior.

Read more about [traits: domain capabilities](../tutorials/book/11_traits_and_derives.md).

## Debug vs Display: two string representations

Incan intentionally separates two kinds of “stringification”:

- **Debug** (`{:?}`): developer-facing, structured, and not user-overridable
- **Display** (`{}`): user-facing output; you can override via `__str__`

This mirrors the common “logs vs user output” split: Debug is stable and structural; Display is designed for
human-friendly formatting.

## `@compiler_expand` (stdlib marker)

You may see `@compiler_expand` used in stdlib sources. It is **not a user feature**.

The intended meaning is:

- the stdlib contains *stubs* that declare vocabulary and signatures
- the compiler is responsible for providing the actual implementation for those stubs

This lets the stdlib function as documentation and a vocabulary registry, without requiring users to learn a special
compiler-only mechanism.

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
