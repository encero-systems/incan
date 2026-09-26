# Match patterns (reference)

This page specifies what a literal pattern matches and when the arms of a `match` cover its subject. Every refusal on this page is reported at check time with `INCAN-T0001`.

## Literal patterns

A literal pattern matches the values equal to the literal. The literal is checked against the type of the position it matches: the subject itself, or the payload, field, or tuple element the pattern sits in.

| Literal form | Positions it matches | Additional rule |
| --- | --- | --- |
| Integer, such as `0`, `-1`, `255` | An integer type: `int` and every exact-width integer | The value lies in the type's range. |
| Float, such as `1.5` | A float type: `float` or `f32` | For `f32`, the value is representable as a finite `f32`. |
| String, such as `"a"` | `str` | |
| `true`, `false` | `bool` | |
| `None` | `Option[T]` | |
| Decimal, such as `1.5d` | None | Refused in every pattern position. |
| Bytes, such as `b"ab"` | None | Refused in every pattern position. |

An integer literal is an `int` and never matches a float position.

A literal of a type its position does not admit, an integer or float literal outside its position's range, and a decimal or bytes literal are refused.

```incan
def classify(pair: tuple[int, int]) -> str:
    match pair:
        (0, "a") => return "x"      # refused: "a" is a str in an int position
        _ => return "y"

def small(byte: u8) -> str:
    match byte:
        255 => return "max"         # accepted
        300 => return "over"        # refused: 300 does not fit in u8
        _ => return "other"

def ratio(value: float) -> str:
    match value:
        1.5 => return "one and a half"   # accepted
        1 => return "one"                # refused: 1 is an int, value is a float
        _ => return "other"

def tagged(value: bytes) -> str:
    match value:
        b"ab" => return "ab"        # refused: bytes literals have no pattern form
        _ => return "other"
```

## Coverage

The unguarded arms of a `match` cover every value of its subject, or the `match` is refused. A guarded arm never counts toward coverage.

**Enums, `Option`, and `Result`.** Each variant is covered when the arms that name it cover every value of its payload. A generic enum's payload types are those of the subject's type arguments. The refusal names each variant left uncovered.

```incan
def first(value: Option[int]) -> int:
    match value:                    # refused: Some with a payload other than 0 is not covered
        Some(0) => return 0
        None => return -1

def describe(value: Result[Option[int], str]) -> str:
    match value:                    # accepted: Ok(Some(n)) and Ok(None) cover Ok
        Ok(Some(n)) => return f"{n}"
        Ok(None) => return "empty"
        Err(message) => return message

enum Shape[T]:
    Filled(T)
    Empty

def area(shape: Shape[int]) -> int:
    match shape:                    # refused: Empty is not covered
        Filled(n) => return n
```

**Every other subject.** A scalar, a string, a tuple, a model, or a class is covered when the arms together cover every value of it. A `bool` is covered by `true` and `false`, and an enum value by its variants. A tuple element or a field of a model or class is covered in the same way, position by position. A numeric, string, or bytes value, whether it is the subject or a position inside it, is covered only by an arm that matches any value there (`_` or a name), so literal arms over it need such an arm beside them.

```incan
def label(code: int) -> str:
    match code:                     # refused: codes other than 0 and 1 are not covered
        0 => return "zero"
        1 => return "one"

def label_all(code: int) -> str:
    match code:                     # accepted
        0 => return "zero"
        _ => return "other"

def corner(point: tuple[bool, int]) -> str:
    match point:                    # refused: (true, n) with n other than 0 is not covered
        (true, 0) => return "origin"
        (false, _) => return "left"

def corner_all(point: tuple[bool, int]) -> str:
    match point:                    # accepted
        (true, 0) => return "origin"
        (true, _) => return "right"
        (false, _) => return "left"
```

Union subjects follow [Union types](union_types.md): the arms cover every member type, or include `_`.
