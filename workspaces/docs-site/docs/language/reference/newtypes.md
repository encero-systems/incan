# Newtypes

Newtypes declare a distinct nominal type around one underlying value:

```incan
type UserId = newtype int
```

Construct a newtype explicitly with one positional underlying value:

```incan
user_id = UserId(42)
```

The wrapped value is available as `.0`.

## Validated Construction

A newtype may define the canonical validation hook `from_underlying`:

```incan
type Attempts = newtype int:
    def from_underlying(n: int) -> Result[Self, ValidationError]:
        if n <= 0:
            return Err(ValidationError("attempts must be >= 1"))
        return Ok(Attempts(n))
```

The hook must be a static method with exactly one ordinary parameter whose type is the newtype's underlying type. Its
return type must be `Result[T, ValidationError]` or `Result[Self, ValidationError]`.

`ValidationError("message")` creates the canonical validation error. Use `ValidationError(message="...", code="...")`
when a stable error code is useful.

## Implicit Sites

The compiler inserts validated coercion only where the destination type is already explicit:

- Function and method arguments.
- Typed local initializers.
- Static initializers.
- Model and class constructor fields.
- Explicit `T(value)` construction.

Implicit coercion does not parse unrelated primitive types. A `str` does not become an `int` on the way into an
`int`-backed newtype.

Reassignment is not an implicit coercion site:

```incan
type Attempts = newtype int

def main() -> None:
    mut attempts: Attempts = Attempts(1)
    # attempts = 2  # type error
```

## Constraints

Primitive integer and float underlyings may use numeric constraints:

```incan
type PositiveInt = newtype int[gt=0]
type Percentage = newtype int[ge=0, le=100]
```

Supported constraint keys are `gt`, `ge`, `lt`, and `le`. Generated constraint checks use the same validated
construction sites as `from_underlying`.

## Aggregate Validation

Model and class constructors aggregate validated field errors before raising:

```incan
type PositiveInt = newtype int[gt=0]

model Bounds:
    low: PositiveInt
    high: PositiveInt

def main() -> None:
    bounds = Bounds(low=1, high=2)
```

If more than one validated field fails, the raised validation error includes the constructor target and each failed
field.

## Opting Out

Use `@no_implicit_coercion` when callers must construct the newtype explicitly:

```incan
@no_implicit_coercion
type Attempts = newtype int
```

Explicit `Attempts(value)` construction remains available.
