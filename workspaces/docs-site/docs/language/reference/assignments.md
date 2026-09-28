# Assignments (reference)

This page specifies binding and assignment statements.

## Forms

| Statement | Targets | Effect |
| --- | --- | --- |
| `let name = value` | A name | Declares an immutable binding in the current scope. |
| `mut name = value` | A name | Declares a mutable binding in the current scope. |
| `name = value` | A name | Reassigns the binding of `name` in this or an enclosing scope, or else the module's `static` named `name`; declares an immutable binding when there is neither. |
| `name: T = value` | A name | As `name = value`; `value` has type `T`. `let` and `mut` accept the annotation too. |
| `target.field = value` | A field | Writes `value` to the field of the model or class value `target`. |
| `target[index] = value` | An element | Writes `value` to the element of a `list` at an `int` index, to the entry of a `dict` at a key, or through the `__setitem__` method of `target`'s type. |
| `name op= value`, `target.field op= value`, `target[index] op= value` | A name, a field, an element | Writes `target op value` to the target (see [Compound assignment](numeric_semantics.md#compound-assignment)). |
| `a, b = value` | Names, fields, elements | Assigns the elements of the tuple `value` to the targets in order. The tuple may be written without parentheses: `a, b = b, a`. |
| `let a, b = value`, `mut a, b = value` | Names | Declares each name, immutable or mutable. |
| `a = b = value` | Names | Assigns `value` to each target, from left to right. |
| `let a = b = value`, `mut a = b = value` | Names | Declares each name, immutable or mutable, holding `value`. |

## Rules

- A `let` or `mut` declaration in an inner scope shadows an outer binding of the same name until the end of that scope.
- A reassigned local binding is `mut`. A `static` of the module is reassigned without `mut` (see [Static storage](static_storage.md)).
- In `a, b = value` and `a = b = value`, each name target follows `name = value`: an existing binding is reassigned, and any other name is declared.
- In `a, b = value`, when the first target is a name, every target is a name.
- The right side of an assignment is evaluated once, before any target is written; a chained assignment of a literal value to targets of different types, below, is the exception.
- In a chained assignment, each target receives the value in its own type. When the targets have different types, a value built only from literals and empty constructors (`None`, `[]`, `{}`, `list()`, a number) is evaluated once for each target, in that target's type.
- A field or element write through `self` inside a method, compound forms included, requires a `mut self` receiver (`INCAN-T0102`).
- A field write, a `list` element or `dict` entry write, and a call of a method that changes its receiver (a method declared `mut self`, or a `list`, `dict` or `set` method that changes the collection), through any other name, compound and tuple forms included, requires that name to be declared `mut`: a binding declared with `mut`, or a parameter marked `mut`. A local bound directly to a module `static` writes through to the static (see [Static storage](static_storage.md#aliases)).
- A `for` loop, list comprehension or pattern binding changes the item it binds in the place the item belongs to. A `for` loop reads its items in place when its body changes one through its variable and it iterates a place: a binding, a field or element of one, `enumerate` or `zip` over one, or `values()` of a dict. A list comprehension reads its items in place when its element or filter changes the item its one name binds and it iterates a place: a binding, or a field or element of one (see [Changes through bound names](match_patterns.md#changes-through-bound-names) for pattern bindings).
- `d[key]` in a write or a changing method call names the value `d` stores at `key`, and changes it in place; a missing key raises `KeyError`.

## Refusals

| Statement | Refused when | Code |
| --- | --- | --- |
| Any | A reassigned local binding is not `mut` | `INCAN-T0001` |
| Any | A reassigned name is a marked `mut` parameter (see [`mut` parameters](functions.md#mut-parameters)) | `INCAN-T0001` |
| Any | A reassigned name is a `const`, or a `static` imported from another module | `INCAN-T0001` |
| Any | The value's type does not fit a target | `INCAN-T0001` |
| Any | A target is an element of a tuple or a `str` | `INCAN-T0001` |
| Any | A declared name is `print` or `println` | `INCAN-T0001` |
| `target.field = value` | `target`'s type declares no field `field`, or `target` is a tuple | `INCAN-T0001` |
| `target.field = value`, `target[index] = value` | The name the write goes through (`p` in `p.x` and in `p.rows[0]`) is a local declared without `mut`, by `let` or a first plain assignment, or a parameter not marked `mut`; an element write only when it writes a `list` element or a `dict` entry | `INCAN-T0001` |
| `target.method(...)` | The method changes its receiver, and the name the call goes through (`p` in `p.rows.append(x)`) is a local declared without `mut` or a parameter not marked `mut` | `INCAN-T0001` |
| `for item in iterable:`, `[... for item in iterable]` | The body, element or filter changes `item`, by a write, a changing method call or advancing a `Generator` item, and the loop reads its items in place from a binding declared without `mut`, a parameter not marked `mut`, a static, or `self` in a plain `self` method | `INCAN-T0001` |
| `target[index] = value` | `target` is a `str` or a tuple, the index is not an `int` for a `list` or not a key for a `dict`, or `target`'s type has no `__setitem__` | `INCAN-T0001` |
| `a, b = value` | `value` is not a tuple, or the tuple's length differs from the number of targets | `INCAN-T0001` |
| `a, b = value` | A target is not a name, a field or an element | `INCAN-T0001` |
| `a, b = value` | The first target is a name and a later target is not | `INCAN-P0001` |
| `a = b = value` | The targets have different types, and the value has no fully known type and is not built only from literals and empty constructors (`None`, `[]`, `{}`, `list()`, a number) | `INCAN-T0001` |
| `a = b = value` | A target is not a name | `INCAN-P0001` |
| `x: T = y = value` | Always | `INCAN-P0001` |

```incan
model Point:
    x: int
    y: int

def examples(items: list[int], maybe: Option[int]) -> int:
    mut a = 0
    mut b = 1
    a, b = (b, a + b)                  # accepted
    a, b = b, a                        # accepted
    mut result = items
    result[0], result[1] = (result[1], result[0])   # accepted
    result[0] = 5                      # accepted
    result[1] += 1                     # accepted
    mut point = Point(x=1, y=2)
    point.x = 3                        # accepted
    let origin = Point(x=0, y=0)
    origin.x = 1                       # refused: origin is not mut
    fixed_items = [1]
    fixed_items.append(2)              # refused: fixed_items is not mut
    rows = [[1]]
    for row in rows:
        row.append(2)                  # refused: rows is not mut
    mut limit: Option[int] = maybe
    a = limit = 5                      # accepted
    let fixed = 10
    fixed = 11                         # refused: fixed is not mut
    x, y = (1, 2, 3)                   # refused: 3 elements for 2 targets
    a, result[0] = (1, 2)              # refused: a later target is not a name (INCAN-P0001)
    point.z = 1                        # refused: Point declares no field z
    c: int = d = 1                     # refused: annotated chain (INCAN-P0001)
    return a
```
