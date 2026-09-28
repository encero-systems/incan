# Derives: Validation (reference)

This page specifies `Validate`. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Validate

- **Provides**: `TypeName.new(...) -> Result[TypeName, E]`, validated construction.
- **Provided by**: `@derive(Validate)` on a model. On a class, enum or newtype it is refused (`INCAN-T0001`).
- **Behavior**: `new` constructs the model from its arguments, calls `validate(self)`, and returns its result.
- **Dunder**: none.
- **Requires**: the model defines `validate(self) -> Result[Self, E]`.

Rules:

- `new` takes the model's fields that declare no default; each field that declares a default takes it. Passing a field that declares a default to `new` is refused (`INCAN-T0001`).
- Constructing the model directly, `TypeName(...)`, is refused (`INCAN-T0001`).

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
    return EmailUser.new(email=email, is_active=false)  # refused: is_active declares a default

def make_raw(email: str) -> EmailUser:
    return EmailUser(email=email)                       # refused: direct construction of a Validate model
```

## See also

- [Error handling (explanation)](../../explanation/error_handling.md)
