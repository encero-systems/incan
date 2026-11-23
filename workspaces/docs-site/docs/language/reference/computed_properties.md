# Computed properties

Computed properties are field-like members whose value is produced by a body each time the member is read.

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

Property declarations take no parameters, including an explicit `self`.

## Reads

Ordinary dot access evaluates the property body on every read. Call syntax is a type error.

```incan
value = account.dollars    # accepted
value = account.dollars()  # refused: property is read with dot access (INCAN-T0001)
```

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

Trait properties are abstract requirements. The adopting `model`, `class`, or concrete trait implementation supplies the body.

## Member names

Properties share the same member namespace as fields, methods, and trait members. A name can identify only one of those members on a type.

## See also

- [Model fields and computed properties](../explanation/models_and_classes/models.md#field-access)
