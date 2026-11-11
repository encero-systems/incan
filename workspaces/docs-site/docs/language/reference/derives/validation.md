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
- Importing a validated model, including through a standard package or a public facade, retains the same generated `new` signature and validation result. Raw construction remains refused; importing or reexporting the model never bypasses its validation.

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

## Registered final checks

On a model method, bare `@validate` registers a whole-model check with the signature `def check(self) -> Result[None, E]`. `@validate(field_name)` registers a field check with no receiver and one required ordinary parameter whose explicit type exactly matches that field, returning `Result[None, E]`. In both forms, `E` must implement `Error`, the check must be synchronous and nongeneric, and the successful result is `None`: a final check accepts or rejects completed state and cannot replace it.

A model with one or more registered checks has the canonical construction type `Result[ModelName, ValidationError]`. The compiler constructs the complete model once, runs field checks in field declaration order, then runs whole-model checks in method declaration order. It retains every failure in that order with its field or model location and original concrete error value. An `Ok` result contains the completed model; an `Err` result exposes no partially validated model.

Registered checks must be construction-safe. Calls to asynchronous work, mutable static state, C or host effects, uncertified native or builtin operations, and open or unverified method dispatch are refused. A source-absent imported helper is accepted only when its checked API metadata carries a positive construction-safety summary; older or missing metadata is unverified and therefore refused.

`@derive(Validate)` is the legacy value-returning contract described above. A model cannot combine it with registered final checks: remove the derive and make each registered check return `Result[None, E]`. There is no automatic adapter between the two contracts.

## See also

- [Models: Validation (explanation)](../../explanation/models_and_classes/models.md#validation-derivevalidate)
- [Error handling (explanation)](../../explanation/error_handling.md)
