# Assignments (reference)

This page specifies binding and assignment statements.

## Forms

| Statement | Targets | Effect |
| --- | --- | --- |
| `let name = value` | A name | Declares an immutable binding in the current scope. |
| `mut name = value` | A name | Declares a mutable binding in the current scope. |
| `name = value` | A name | Reassigns the binding of `name` in this or an enclosing scope; declares an immutable binding when there is none. |
| `name: T = value` | A name | As `name = value`; `value` has type `T`. `let` and `mut` accept the annotation too. |
| `name op= value` | A name | Reassigns `name` (see [Compound assignment](numeric_semantics.md#compound-assignment)). |
| `a, b = value` | Names, fields, elements | Assigns the elements of the tuple `value` to the targets in order. |
| `let a, b = value`, `mut a, b = value` | Names | Declares each name, immutable or mutable. |
| `a = b = value` | Names | Assigns `value` to each target, from left to right. |

## Rules

- A `let` or `mut` declaration in an inner scope shadows an outer binding of the same name until the end of that scope.
- A reassigned binding is `mut`.
- In `a, b = value` and `a = b = value`, each name target follows `name = value`: an existing binding is reassigned, and any other name is declared.
- The right side of an assignment is evaluated once, before any target is written.
- In a chained assignment, each target receives the value in its own type.
- A field write inside a method requires a `mut self` receiver (`INCAN-T0102`).

## Refusals

| Statement | Refused when | Code |
| --- | --- | --- |
| Any | A reassigned binding is not `mut` | `INCAN-T0001` |
| Any | The value's type does not fit a target | `INCAN-T0001` |
| Any | A target is an element of a tuple or a `str` | `INCAN-T0001` |
| `a, b = value` | The tuple's length differs from the number of targets | `INCAN-T0001` |
| `a = b = value` | The targets have different types, and the value has no fully known type and is not built only from literals and empty constructors (`None`, `[]`, `{}`, `list()`, a number) | `INCAN-T0001` |
| `a = b = value` | A target is not a name | `INCAN-P0001` |
| `x: T = y = value` | Always | `INCAN-P0001` |

```incan
def examples(items: list[int], maybe: Option[int]) -> int:
    mut a = 0
    mut b = 1
    a, b = (b, a + b)                  # accepted
    mut result = items
    result[0], result[1] = (result[1], result[0])   # accepted
    mut limit: Option[int] = maybe
    a = limit = 5                      # accepted: limit is Some(5)
    let fixed = 10
    fixed = 11                         # refused: fixed is not mut
    x, y = (1, 2, 3)                   # refused: 3 elements for 2 targets
    c: int = d = 1                     # refused: annotated chain (INCAN-P0001)
    return a
```
