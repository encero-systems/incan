# Derives: Validation (Reference)

This page documents `Validate` for models.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Error handling](../../explanation/error_handling.md)

---

## Validate (models only)

- **Derive**: `@derive(Validate)`
- **Requirement**: a `validate(self) -> Result[Self, E]` method must exist on the model
- **API**: `TypeName.new(...) -> Result[TypeName, E]`

Rule:

- If a model derives `Validate`, you must construct it via `TypeName.new(...)` (validated construction).
  Raw construction via `TypeName(...)` is a compile-time error.

Semantics:

- `TypeName.new(...)` constructs the model, then calls `validate(self)`.

### Example

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
    # Validated construction (required for @derive(Validate))
    return EmailUser.new(email=email)
```

### Defaults and parameters

`TypeName.new(...)` is generated from the model’s required fields (fields without defaults).
Fields with defaults use their declared default values.

Additional constructors can be expressed as helper functions or associated functions on the model.
