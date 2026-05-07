# Computed properties

Computed properties are field-like members whose value is produced by a body each time the member is read.

Use a computed property when callers should see a value as part of the object's attribute surface, but the value should be derived instead of stored.

```incan
model Account:
    cents: int

    property dollars -> int:
        return self.cents // 100

def show(account: Account) -> int:
    return account.dollars
```

## Declaration syntax

A computed property uses `property`, a name, `->`, an explicit return type, and a body:

```incan
property name -> Type:
    return value
```

Properties are valid in `model` and `class` bodies and in concrete trait implementation bodies. `self` is available in the body using the same immutable receiver rules as `def method(self) -> T`.

Property declarations do not take parameters. Write `property total -> int:`, not `property total(self) -> int:`.

## Reads

Read a property with ordinary dot access:

```incan
value = account.dollars
```

Do not call a property. `account.dollars()` is a type error because properties are selected as fields at the source level, even though the compiler lowers the read to a zero-argument backend call.

Each read executes the body. The compiler does not memoize property results; store or cache a value explicitly when that is the intended behavior.

## Traits

Traits can require computed properties:

```incan
trait Named:
    property label -> str

class User with Named:
    name: str

    property label -> str:
        return self.name
```

Trait properties are abstract requirements. The body belongs on the adopting `model`, `class`, or concrete trait implementation, not on the trait declaration.

## Choosing `property` or `def`

Prefer `property` when:

- the member is conceptually an attribute, derived field, or projection
- the read takes no arguments
- callers should use `obj.name`, not `obj.name()`

Prefer `def` when:

- the operation takes parameters
- the operation performs work that should be visibly action-like at the call site
- the result depends on options, external effects, or expensive computation that should not look like a simple member read

Properties share the same member namespace as fields, methods, and trait members. One type cannot declare a field, method, and property with the same simple name.
