# Union types (reference)

A union type is an anonymous, closed set of member types; a value of the union is a value of one member. This page specifies union spellings, assignability, collection literals at a union destination, narrowing, and matching.

## Spellings

| Spelling | Position |
| --- | --- |
| `Union[A, B, ...]` | Any type position |
| <code>A &#124; B &#124; ...</code> | Any type position; the same type as `Union[A, B, ...]` |

- Nested unions flatten, duplicate members collapse, and member order does not matter: `Union[str, int]`, `int | str` and `int | str | int` are one type.
- A union with `None` as a member is an `Option` of the other members: `str | None` is `Option[str]`, and `int | str | None` is `Option[Union[int, str]]`.
- A member that names a model, class, enum or newtype is the declaration that name resolves to where the union is written. `Product | int` written in two modules that each declare `Product` is two different union types.

```incan
# first.incn
pub model Product:
    pub value: int

pub type Answer = Product | int                  # accepted
```

```incan
# second.incn
pub model Product:
    pub value: str

pub type Answer = Product | int                  # accepted
```

## Assignability

- A value of a member type, or of a type assignable to a member type, is assignable to the union, as a return value, an assigned value or an argument.
- A union is assignable to another union when each of its members is assignable to a member of the other; a numeric member is assignable only to its own type (see [Assignment between numeric types](numeric_semantics.md#assignment-between-numeric-types)). A union with a member assignable to no member of the target is refused (`INCAN-T0001`).
- A union in the signature of another package's function or method is the same type as a union with the same members in the calling package.

```incan
# querykit/src/lib.incn
pub model Box:
    pub value: int

    def answer(self) -> int | str:
        return self.value
```

```incan
# src/main.incn
from pub::querykit import Box

def show(value: int | str) -> str:
    match value:
        int(number) => return f"number {number}"
        str(text) => return f"text {text}"

def widen(value: int | str) -> int | str | bool:
    return value                                 # accepted

def narrow(value: int | str | bool) -> int | str:
    return value                                 # refused: bool is not a member of int | str

def main() -> None:
    println(show(Box(value=3).answer()))         # accepted
```

## Collection literals

- A list, dict or tuple literal at a union or `Option` destination has the type of the destination's one member of the literal's kind (for a tuple literal, a tuple of the literal's length) when each other member is a scalar, a tuple, a `list`, `dict`, `set`, `Result` or `Generator`, or a model, class or enum. An empty literal and a literal of `None` elements have that member's type.
- A literal at a destination with two or more members of its kind has the type its elements give. A literal whose elements leave part of its type open (`[]`, `[None]`, `{}`, `(None, 1)`) is refused there (`INCAN-T0001`).
- A literal at a destination with a type parameter, newtype, trait or frozen collection member has the type its elements give.

```incan
def main() -> None:
    mut values: list[int] | str = "none"
    values = []                                  # accepted
    mut names: Option[list[Option[str]]] = None
    names = [None]                               # accepted
    mut either: list[int] | list[str] = [1]
    either = ["a"]                               # accepted
    either = []                                  # refused: list[int] or list[str]
```

## Narrowing

- A union value has no member-specific methods or operators. Calling one, or applying an operator, is refused (`INCAN-T0001`).
- A union value has no printed form. `print(value)`, `println(value)`, `str(value)` and an f-string `{value}` over it are refused (`INCAN-T0103`); a narrowed member displays as its own type (see [Display](strings.md#display)).
- In an `if isinstance(value, T):` branch, `value` has type `T`. In the `else` branch and in each following `elif` branch, it has the members no earlier branch tested.
- An `Option` union narrows through `is None` and `is not None` as well.
- Narrowing ends with the conditional statement: after it, `value` has the union type again.

```incan
def size(value: int | str) -> int:
    if isinstance(value, str):
        return len(value)                        # accepted
    else:
        return value + 1                         # accepted

def shout(value: int | str) -> str:
    return value.upper()                         # refused: upper is not a method of int | str

def label(value: str | None) -> str:
    if value is not None:
        return value.upper()                     # accepted
    return "missing"

def show(value: int | str) -> None:
    match value:
        int(n) => println(n)                     # accepted
        str(s) => println(s)                     # accepted
    println(value)                               # refused: a union value has no printed form
```

## Matching

- A type pattern `T(p)` matches a value of member type `T` (see [Match patterns](match_patterns.md#type-patterns)).
- The unguarded arms of a `match` over a union cover every member type, or include `_` (see [Coverage](match_patterns.md#coverage)).
- An alternation of type patterns binds a name only when every alternative binds it at the same type.

```incan
def classify(value: int | str | None) -> str:
    match value:
        int(_) | str(_) => return "present"      # accepted
        None => return "missing"

def pick(value: int | str) -> str:
    match value:
        int(item) | str(item) => return "item"   # refused: item is an int in one alternative and a str in the other
```
