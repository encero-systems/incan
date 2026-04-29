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

For model, class, and enum types, the same generic trait may be adopted more than once with different type arguments. That lets one type support multiple statically checked capability shapes without runtime overloading.

For example, a type can expose one conversion hook for more than one target type:

```incan
trait Snapshot[T]:
    def snapshot(self) -> T: ...

model Reading with Snapshot[str], Snapshot[bytes]:
    value: int

    def snapshot(self) -> str:
        return str(self.value)

    def snapshot(self) -> bytes:
        return b"reading"

reading = Reading(value=1)
text: str = reading.snapshot()
payload: bytes = reading.snapshot()
```

The important constraint is that the repeated methods still belong to one generic trait family. Incan can choose between `Snapshot[str]` and `Snapshot[bytes]` from the expected result type. It does not treat unrelated traits with the same method name as an overload set.

!!! note "Coming from Python?"
    Python protocol and dunder behavior is often structural and dynamic: if an object has the right method at runtime, the operation can work. Incan keeps the familiar hook names, but the contract is static.

    - **Static typing**: whether a type supports a hook is part of the type system.
    - **Deterministic dispatch**: behavior is resolved at compile time, with no dynamic MRO.
    - **No runtime patching**: behavior cannot be added to types at runtime.

    That is what keeps hook-based ergonomics predictable in larger codebases.

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
