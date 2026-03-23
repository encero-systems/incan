# 11. Traits and derives

Traits describe shared behavior. Derives automatically generate common behavior for your types.

## Derives: add behavior without boilerplate

Derives add behavior to a type. One of the most common is `Debug`, which lets you print a structured representation of a
value.

!!! tip "Coming from Python?"
    **Derives** are like getting common “dunder methods” (`__repr__`, `__eq__`, etc.) without writing them by hand.

```incan
@derive(Debug)
model Point:
    x: int
    y: int

def main() -> None:
    p = Point(x=1, y=2)
    println(f"{p:?}")
```

### Debug formatting (`:?`)

The `:?` inside an f-string means “debug formatting”.

!!! tip "Coming from Python?"
    **Debug formatting** (`:?`) is like Python’s `__repr__`: it shows the type name and fields in a structured format.

## Traits: shared contracts

Traits let you define a shared contract. In a trait, a method can either:

- Use `...` to mean “implementers must provide this” (required methds)
- Provide a default implementation (Rust-like default methods)

Traits are always abstract in Incan. That means two things:

- You do not construct a trait directly with `TraitName(...)`.
- You can use a trait directly in annotations to mean “any concrete adopter of this capability”.

!!! tip "Coming from Python?"
    **Traits** are like a typed interface (represented in Python by a `Protocol` or `abc.ABC`): “anything that implements
    these methods can be treated as this capability”.

### Trait hierarchies with `with`

Traits can also refine other traits:

```incan
trait Collection[T]:
    def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
    def sorted(self) -> Self: ...

def first_item(values: Collection[int]) -> int:
    return values.first()
```

Here, `OrderedCollection[T]` is also a `Collection[T]`, so any concrete adopter of `OrderedCollection[T]` can be passed to `first_item`.

### Default methods and adopter fields (`@requires`)

If a trait default method accesses adopter fields directly (for example `self.name`), the trait must declare those fields
in `@requires(...)`. Mutating fields still requires `mut self` (same as normal methods).

Example:

```incan
trait Greetable:
    # Required method: implementers must provide this
    def name(self) -> str: ...

    # Default method: implementers can provide this
    def greet(self) -> str:
        return f"Hello, {self.name()}!"

model User with Greetable:
    username: str

    def name(self) -> str:
        return self.username

def main() -> None:
    u = User(username="alice")
    println(u.greet())  # outputs: Hello, alice!
```

Mutation uses `mut self` (and the field must be declared via `@requires(...)`):

```incan
@requires(count: int)
trait Counter:
    def bump(mut self) -> None:
        self.count += 1

class Thing with Counter:
    count: int = 0

def main() -> None:
    t = Thing()
    t.bump()
```

## Try it

1. Add a second derived type and print it with `:?`.
2. Create a small trait (for example `Describable`) and implement it for a model.
3. Call the trait method.

??? example "One possible solution"

    ```incan
    @derive(Debug)
    model Point:
        x: int
        y: int

    trait Describable:
        def describe(self) -> str: ...

    model User with Describable:
        username: str

        def describe(self) -> str:
            return f"user={self.username}"

    def main() -> None:
        println(f"{Point(x=1, y=2):?}")
        println(User(username="alice").describe())
    ```

## Where to learn more

- Derives and traits (full reference): [Derives & Traits](../../reference/derives_and_traits.md)

## Next

Back: [10. Models vs classes](10_models_vs_classes.md)

Next chapter: [12. Newtypes (stronger types)](12_newtypes.md).
