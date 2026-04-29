# Traits as language hooks (and overloading)

Traits let you describe **behavior contracts**: “a type supports X”.

Some common language features are easiest to understand as **desugaring** into trait methods. This is similar in spirit
to Rust using traits like `IntoIterator`, `Index`, and `Add` to power `for`, indexing, and operators.

This page explains the idea with one concrete example. For the authoritative reference pages, see:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)
- [Stdlib traits: collection protocols](../reference/stdlib_traits/collection_protocols.md)
- [Stdlib traits: indexing and slicing](../reference/stdlib_traits/indexing_and_slicing.md)
- [Stdlib traits: callable objects](../reference/stdlib_traits/callable.md)
- [Stdlib traits: operators](../reference/stdlib_traits/operators.md)
- [Stdlib traits: conversions](../reference/stdlib_traits/conversions.md)

## Why this matters

This is the mental model behind “ergonomic syntax without magic”:

- You can use pleasant syntax (`x[i]`, `for x in y`, `a + b`) **without** baking special cases into the language.
- User-defined types can become “first-class citizens” by implementing the same hooks as built-in types.
- The compiler can typecheck these capabilities explicitly (Rust-like), instead of relying on runtime duck typing.

## What “language hooks” means

A **language hook** is a method name that the compiler can call implicitly when you use a piece of syntax.

Examples:

- `len(x)` can desugar to something like `x.__len__()`
- `xs[i]` can desugar to `xs.__getitem__(i)`
- `obj()` can desugar to `obj.__call__(...)`
- `a + b` can desugar to `a.__add__(b)`

The important idea is that the *syntax* stays simple, while the *behavior* is defined by traits.

## Example: indexing is a hook

When you write:

```incan
value = xs[i]
```

Think:

```incan
value = xs.__getitem__(i)
```

So if you want a custom type to support `[]`, you implement the indexing hook method (and the corresponding trait
requirements as defined in the stdlib trait docs):

```incan
model Grid:
    data: list[int]

    def __getitem__(self, idx: int) -> int:
        return self.data[idx]
```

Rust analogy: `xs[i]` is powered by `std::ops::Index`, but the idea is the same: syntax → a trait-defined hook.

For model and class types, the same generic hook trait may be adopted more than once with different type arguments. That lets one type support multiple statically checked hook shapes, such as `Index[str, V]` and `Index[int, V]`, without runtime overloading.

## How this is Rust-like (not Python-magic)

In Python, “protocols” are often structural and dynamic (“if it quacks, it’s a duck”), and operator behavior is resolved
at runtime via dunder lookup.

In Incan, the intent is closer to Rust:

- **Static typing**: whether a type supports a hook is part of the type system.
- **Deterministic dispatch**: behavior is resolved at compile time (no dynamic MRO).
- **No runtime patching**: you can’t add behavior to types at runtime.

This is what keeps hook-based ergonomics predictable in large codebases.

## Overloading: what we mean (and what we don’t)

“Overloading” can mean different things:

- **Operator overloading**: `a + b` uses a hook like `__add__`.
- **Trait-based polymorphism**: generic code can accept “anything that implements Trait X”.

It does *not* necessarily mean “multiple functions with the same name and different signatures” (traditional overload sets).

In Incan, most extensibility is intended to flow through traits and explicit, checkable contracts.

## A concrete mental model

You can think of hook traits as giving user-defined types the same “surface ergonomics” that built-in types have.

For example, if a type can be indexed, you should be able to write `x[i]` without caring whether `x` is a built-in list
or a custom collection type.

That’s why the stdlib defines traits for:

- collection protocols (`len`, iteration, membership, truthiness)
- indexing and slicing (`[]`, `a:b:c`)
- callability (`obj()`)
- operators (`+`, `-`, `*`, `/`, etc.)
- conversions (`from`, `into`, `try_from`, `try_into`)

## See also

- [Stdlib traits overview](../reference/stdlib_traits/index.md)
- [Derives & traits (Reference)](../reference/derives_and_traits.md)
