# Derives: Validation (reference)

This page specifies `Validate`. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Validate

- **Provides**: `TypeName.new(...) -> Result[TypeName, E]`, validated construction.
- **Provided by**: `@derive(Validate)` on a model. On a class, enum or newtype it is refused (`INCAN-T0001`).
- **Behavior**: `new` constructs the model from its arguments, calls `validate(self)`, and returns its result.
- **Dunder**: none.
- **Requires**: the model defines `validate(self) -> Result[Self, E]`, where `Result[TypeName, E]` is the same return type and `E` is any type. A model without `validate`, or whose `validate` has another receiver, takes parameters, is `async`, or returns anything but a `Result` of the model, is refused (`INCAN-T0001`).

Rules:

- `new` takes the model's fields that declare no default, by name or positionally in declaration order; each field that declares a default takes it. Passing a field that declares a default to `new` is refused (`INCAN-T0001`).
- A model that defines its own `new` keeps it: `TypeName.new(...)` calls that method, and no `new` is derived.
- Constructing the model directly, `TypeName(...)`, is refused (`INCAN-T0001`), inside the model's own methods too.

```incan
@derive(Validate)
model EmailUser:
    email: str
    is_active: bool = true

    def validate(self) -> Result[EmailUser, str]:
        if "@" not in self.email:
            return Err("invalid email")
        return Ok(self)

def make_user(email: str) -> Result[EmailUser, str]:
    return EmailUser.new(email=email)                   # accepted

def make_inactive(email: str) -> Result[EmailUser, str]:
    return EmailUser.new(email=email, is_active=false)  # refused: is_active declares a default (INCAN-T0001)

def make_raw(email: str) -> EmailUser:
    return EmailUser(email=email)                       # refused: direct construction of a Validate model (INCAN-T0001)
```

## See also

- [Models: Validation (explanation)](../../explanation/models_and_classes/models.md#validation-derivevalidate)
- [Error handling (explanation)](../../explanation/error_handling.md)
