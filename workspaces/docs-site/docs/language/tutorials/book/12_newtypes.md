# 12. Newtypes (stronger types)

A **newtype** is a “zero-cost wrapper” around another type. It lets you create **distinct types** that the compiler won’t let you accidentally mix up.

## Why newtypes?

If two values share the same underlying type (like `int`), it’s easy to mix them up by accident.

With newtypes, the compiler helps you keep them straight:

```incan
type UserId = newtype int
type ProductId = newtype int

def get_user_name(user_id: UserId) -> str:
    return f"user={user_id.0}"

def get_product_name(product_id: ProductId) -> str:
    return f"product={product_id.0}"

def main() -> None:
    user_id = UserId(42)
    product_id = ProductId(100)

    println(get_user_name(user_id))
    println(get_product_name(product_id))

    # If you accidentally swap them, the compiler can catch it:
    # println(get_user_name(product_id))
```

!!! tip "Coming from Python?"
    This is similar to creating a tiny wrapper class just to avoid mixing up values, but with less runtime overhead and better static checking.

## Constructing and unwrapping

You construct a newtype by calling it like a function: `UserId(42)`.

## Validated construction

Newtypes can optionally define a reserved validation hook:

```incan
type Attempts = newtype int:
    def from_underlying(n: int) -> Result[Attempts, ValidationError]:
        if n <= 0:
            return Err(ValidationError("attempts must be >= 1"))
        return Ok(Attempts(n))
```

If a newtype defines `from_underlying`, then calling it like `Attempts(5)` performs **checked construction** (it calls `Attempts.from_underlying(5)` and raises a validation error if it returns `Err(...)`).

The compiler also applies that checked construction at typed boundary sites where the destination type is already known:

```incan
def retry(attempts: Attempts) -> None:
    println(f"attempts={attempts.0}")

def main() -> None:
    retry(3)                  # checked as Attempts.from_underlying(3)
    attempts: Attempts = 4    # checked at the typed initializer
```

Implicit coercion does not parse unrelated primitive types. For example, `"3"` does not become an `int` on the way into `Attempts`; parse strings explicitly before constructing the newtype.

You can also write simple numeric constraints on primitive underlyings:

```incan
type NonNegativeInt = newtype int[ge=0]
type Percentage = newtype int[ge=0, le=100]
```

Generated constraints are checked at the same validated construction sites as `from_underlying`.

Use `@no_implicit_coercion` when a newtype should require explicit construction:

```incan
@no_implicit_coercion
type Attempts = newtype int

def retry(attempts: Attempts) -> None:
    return

def main() -> None:
    retry(Attempts(3))  # ok
    # retry(3)          # type error
```

To access the wrapped value, use `.0`:

```incan
def main() -> None:
    user_id = UserId(42)
    println(f"{user_id.0}")
```

## Validation with `Result`

Newtypes are great for values that should satisfy an invariant (like “must be positive”).

One common pattern is to provide a constructor helper that returns `Result`:

```incan
type UserId = newtype int

def make_positive_id(n: int) -> Result[UserId, str]:
    if n > 0:
        return Ok(UserId(n))
    return Err("ID must be positive")
```

You can then use `match` (or `?` if you’re inside a function returning `Result`) to handle failures.

## Methods on newtypes

Newtypes can also define methods:

```incan
type Email = newtype str:
    def from_underlying(v: str) -> Result[Email, ValidationError]:
        """Validate an email address by checking for the presence of an @ symbol"""
        if "@" not in v:
            return Err(ValidationError("missing @"))
        return Ok(Email(v.lower()))

def main() -> None:
    match Email.from_underlying("Alice@Example.com"):
        Ok(email) => println(f"email={email.0}")
        Err(_) => println("invalid email")
```

## Try it

1. Define `type CartItems = newtype List[str]`.
2. Write `def non_empty(items: List[str]) -> Result[CartItems, ValidationError]` that rejects empty lists.
3. In `main()`, call it with both `[]` and `["a"]` and print either the error or the first item.

??? example "One possible solution"

    ```incan
    type CartItems = newtype List[str]

    def non_empty(items: List[str]) -> Result[CartItems, ValidationError]:
        if len(items) == 0:
            return Err(ValidationError("must not be empty"))
        return Ok(CartItems(items))

    def main() -> None:
        match non_empty([]):
            Ok(items) => println(f"first={items.0[0]}")
            Err(e) => println(f"error: {e}")

        match non_empty(["a"]):
            Ok(items) => println(f"first={items.0[0]}")
            Err(e) => println(f"error: {e}")
    ```

## Where to learn more

- Longer example in the repository: `examples/intermediate/newtypes.incn`

## Next

Back: [11. Traits and derives](11_traits_and_derives.md)

Next: return to [Start here](../../../start_here/index.md) or continue in the [Language Guide](../../index.md).
