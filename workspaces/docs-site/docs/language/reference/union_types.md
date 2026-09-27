# Union types (reference)

A union type is an anonymous, closed set of member types; a value of the union is a value of one member. This page specifies union spellings, assignability, narrowing, and matching.

## Spellings

| Spelling | Position |
| --- | --- |
| `Union[A, B, ...]` | Any type position |
| <code>A &#124; B &#124; ...</code> | Any type position; the same type as `Union[A, B, ...]` |

- Nested unions flatten, duplicate members collapse, and member order does not matter: `Union[str, int]`, `int | str` and `int | str | int` are one type.
- A union with `None` as a member is an `Option` of the other members: `str | None` is `Option[str]`, and `int | str | None` is `Option[Union[int, str]]`.

## Assignability

- A value of a member type is assignable to the union, as a return value, an assigned value or an argument.
- A union is assignable to another union when each of its members is a member of the other. A union with a member the target lacks is refused (`INCAN-T0001`).
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
    return value                                 # accepted: every member of int | str is in the target

def narrow(value: int | str | bool) -> int | str:
    return value                                 # refused: bool is not a member of int | str

def main() -> None:
    println(show(Box(value=3).answer()))         # accepted: the package's int | str is this int | str
```

## Narrowing

- A union value has no member-specific methods or operators. Calling one, or applying an operator, is refused (`INCAN-T0001`).
- In an `if isinstance(value, T):` branch, `value` has type `T`. In the `else` branch and in each following `elif` branch, it has the members no earlier branch tested.
- An `Option` union narrows through `is None` and `is not None` as well.

```incan
def size(value: int | str) -> int:
    if isinstance(value, str):
        return len(value)                        # accepted: value is str here
    else:
        return value + 1                         # accepted: value is int here

def shout(value: int | str) -> str:
    return value.upper()                         # refused: upper is not a method of int | str

def label(value: str | None) -> str:
    if value is not None:
        return value.upper()                     # accepted: value is str here
    return "missing"
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
