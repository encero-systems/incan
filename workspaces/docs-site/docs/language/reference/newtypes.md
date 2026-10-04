# Newtypes

This page specifies newtype declarations and type parameters, construction and `.0`, trait adoption, validated construction with `from_underlying`, constraints, implicit coercion, and `@no_implicit_coercion`. The derives a newtype carries are in [Derives and traits](derives_and_traits.md#automatic-derives), and its JSON form is in [Serialization](derives/serialization.md#type-mapping).

## Declaration

`type Name = newtype Underlying` declares `Name`, a nominal type distinct from `Underlying` that holds one value of it. A body after `:` declares methods.

- `type Name[T, ...] = newtype Underlying` declares type parameters. Each type parameter appears in the underlying type; one that does not is refused (`INCAN-T0001`).
- A newtype whose underlying type leads back to itself through other newtypes is refused (`INCAN-T0001`).

```incan
type UserId = newtype int        # accepted
type Box[T] = newtype T          # accepted
type Many[T] = newtype list[T]   # accepted
type Tag[T] = newtype str        # refused: T does not appear in the underlying type (INCAN-T0001)
```

## Construction and `.0`

- `Name(value)` constructs a newtype from one positional value of its underlying type. Any other argument list, a named argument included, is refused (`INCAN-T0001`), and so is a value of another type.
- `value.0` is the underlying value.

```incan
type UserId = newtype int

def main() -> None:
    user_id = UserId(42)        # accepted
    raw: int = user_id.0        # accepted
    named = UserId(value=42)    # refused: a newtype takes one positional value (INCAN-T0001)
    other = UserId("42")        # refused: str is not int (INCAN-T0001)
```

## Trait adoption

- A newtype adopts traits with `with TraitName` after its underlying type, and declares their required methods in its body (see [Traits (authoring)](derives_and_traits.md#traits-authoring)).
- When two adopted traits require the same method name, each method names its trait with `for TraitName` before the return arrow. The target names the adopted trait the method satisfies and is not part of the return type (see [Method-level trait targets](derives_and_traits.md#method-level-trait-targets)).
- A newtype that adopts a trait with `@requires(...)` is refused (`INCAN-T0001`).

```incan
trait ToInt:
    def convert(self) -> int: ...

type UserId = newtype int with ToInt:
    def convert(self) -> int:
        return self.0
```

```incan
trait ToInt:
    def convert(self) -> int: ...

trait ToStr:
    def convert(self) -> str: ...

type UserId = newtype int with ToInt, ToStr:
    def convert(self) for ToInt -> int:
        return self.0

    def convert(self) for ToStr -> str:
        return str(self.0)
```

## Validated construction

A newtype may define the validation hook `from_underlying`:

- Form: `def from_underlying(value: Underlying) -> Result[Name, ValidationError]`, where `Result[Self, ValidationError]` is the same return type. The hook has no `self` receiver and exactly one parameter, of the underlying type; `@staticmethod` on it is accepted. A hook with a receiver, another parameter list or another return type is refused (`INCAN-T0001`).
- `Name(value)` outside the newtype's own methods, and each [implicit coercion](#implicit-coercion) into `Name`, call the hook. An `Ok` result is the new value; an `Err` result raises `ValidationError`.
- Inside the newtype's own methods, `Name(value)` constructs the value without calling the hook.
- `ValidationError(message)` constructs a validation error, and `ValidationError(message, code=code)` one that carries a code; `message` may also be passed by name. Any other argument list is refused (`INCAN-T0001`).

```incan
type Attempts = newtype int:
    def from_underlying(n: int) -> Result[Self, ValidationError]:
        if n <= 0:
            return Err(ValidationError("attempts must be >= 1"))
        return Ok(Attempts(n))

type Code = newtype str:
    def from_underlying(value: str) -> Result[Code, str]:   # refused: the error type is not ValidationError (INCAN-T0001)
        return Ok(Code(value))
```

## Constraints

- An `int` or `float` underlying type may carry constraints in brackets: `newtype int[gt=0]`, `newtype float[ge=0, le=1]`.
- The keys are `gt`, `ge`, `lt` and `le`, each at most once, and each value is an integer literal, optionally negative. An empty bracket, another key, a repeated key or another kind of value is refused (`INCAN-P0001`), and so are constraints on a type other than `int` and `float`.
- A newtype without `from_underlying` checks its constraints wherever the hook would be called: a value that fails one raises `ValidationError`.

```incan
type PositiveInt = newtype int[gt=0]           # accepted
type Percentage = newtype int[ge=0, le=100]    # accepted
type Ratio = newtype float[ge=0, le=1]         # accepted
type Label = newtype str[gt=0]                 # refused: only int and float take constraints (INCAN-P0001)
```

## Implicit coercion

A value of a newtype's underlying type is accepted where the newtype is the declared type of:

- a function or method parameter, for an argument or a parameter default;
- an annotated local, `attempts: Attempts = 4`;
- a `static`;
- a `model` or `class` field, for a constructor argument or a field default.

Rules:

- The coerced value is validated as `Name(value)` is.
- A newtype over another newtype coerces a value of the inner newtype's underlying type through both: `type RetryAttempts = newtype Attempts` accepts an `int`.
- A value of any other type is refused (`INCAN-T0001`): a `str` does not coerce into an `int`-backed newtype.
- Assignment to an existing binding and a `return` are not coercion sites, and a generic newtype such as `Box[int]` takes no implicit coercion. A value of the underlying type there is refused (`INCAN-T0001`).
- A `model` or `class` construction validates every field it coerces before it raises: when any fail, it raises one `ValidationError` that names the model or class and each failed field.

```incan
type Attempts = newtype int

model Job:
    attempts: Attempts

def take(a: Attempts) -> None:
    pass

def main() -> None:
    take(3)                                # accepted
    job = Job(attempts=2)                  # accepted
    mut attempts: Attempts = Attempts(1)   # accepted
    attempts = 2                           # refused: assignment is not a coercion site (INCAN-T0001)
    take("3")                              # refused: str is not int (INCAN-T0001)
```

## `@no_implicit_coercion`

`@no_implicit_coercion` on a newtype declaration turns implicit coercion into it off: a value of the underlying type at a coercion site is refused (`INCAN-T0001`). `Name(value)` is accepted.

```incan
@no_implicit_coercion
type Attempts = newtype int

def take(a: Attempts) -> None:
    pass

def main() -> None:
    take(Attempts(1))   # accepted
    take(1)             # refused: Attempts takes no implicit coercion (INCAN-T0001)
```

## See also

- [The Incan Book: Newtypes](../tutorials/book/12_newtypes.md)
- [Generic methods on types (explanation)](../explanation/derives_and_traits.md#generic-methods-on-types)
