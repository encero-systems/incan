# Match patterns (reference)

This page specifies the patterns of `match`, `if let`, and `while let`: each pattern form, what it matches and binds, the refusals with their codes, and when the arms of a `match` cover its subject.

## Where patterns appear

| Construct | Form | Guard | Alternation | Coverage |
| --- | --- | --- | --- | --- |
| `match` arm | `case p:` or `p =>` | `if condition` after `p` | Accepted | Required |
| `if let` | `if let p = value:` | None | Accepted | Not required |
| `while let` | `while let p = value:` | None | Refused (`INCAN-P0001`) | Not required |

- A guard runs only when its arm's pattern matched, and the arm is taken only when the guard is true. A false guard continues matching with the next arm.
- The names a pattern binds are in scope in its arm's guard and body, or in the body of its `if let` or `while let`.

## Pattern forms

| Form | Syntax | Matches | Binds |
| --- | --- | --- | --- |
| Wildcard | `_` | Any value | Nothing |
| Binding | `name` | Any value | `name`, to the value |
| Literal | `0`, `1.5`, `"a"`, `true`, `None` | The value equal to the literal | Nothing |
| Tuple | `(p,)`, `(p1, p2, …)` | A tuple with one element per sub-pattern, each matching its sub-pattern | The names its sub-patterns bind |
| Group | `(p)` | The values `p` matches | The names `p` binds |
| Variant | `Variant(p1, …)`, `Enum.Variant(p1, …)`, `Enum.Variant` | That variant, each payload value matching its sub-pattern | The names its sub-patterns bind |
| Record | `Type(field=p, …)` | A value of the model or class `Type` whose named fields match their sub-patterns | The names its sub-patterns bind |
| Type | `T(p)` | A union value of member type `T` that matches `p` | The names `p` binds |
| Alternation | <code>p1 &#124; p2 &#124; …</code> | The values any alternative matches | The names every alternative binds |

## Binding and tuple patterns

- A binding pattern binds its name to the matched value as an immutable binding.
- A tuple pattern has one sub-pattern per element of the subject's tuple type, in order.

Refused (`INCAN-T0001`): a binding pattern named `print` or `println`.

```incan
def first(pair: tuple[int, str]) -> str:
    match pair:
        (0, text) => return text        # accepted
        print => return "other"         # refused: print is a protected builtin name
        _ => return "none"
```

## Literal patterns

| Literal | Positions it matches | Additional rule |
| --- | --- | --- |
| Integer, such as `0`, `255` | An integer type: `int` and every exact-width integer | The value lies in the type's range. |
| Float, such as `1.5` | A float type: `float` (`f64`) or `f32` | For `f32`, the value is finite in `f32`. |
| String, such as `"a"` | `str` | |
| `true`, `false` | `bool` | |
| `None` | `Option[T]` | |
| Decimal, such as `1.5d` | None | |
| Bytes, such as `b"ab"` | None | |

A literal's position is the subject, or the payload, element, or field the literal sits in. An integer literal is an `int` and never matches a float position.

Refused (`INCAN-T0001`): a literal in a position its type does not admit, an integer or float literal outside its position's range, and every decimal and bytes literal.

```incan
def small(byte: u8) -> str:
    match byte:
        255 => return "max"         # accepted
        300 => return "over"        # refused: 300 does not fit in u8
        "a" => return "a"           # refused: "a" is a str in a u8 position
        _ => return "other"

def ratio(value: float) -> str:
    match value:
        1.5 => return "half"        # accepted
        1 => return "one"           # refused: 1 is an int in a float position
        _ => return "other"
```

## Variant patterns

- The subject is an enum declared in Incan, an `Option` (`Some(p)`; `None` is a literal), or a `Result` (`Ok(p)`, `Err(p)`).
- `Variant` is a variant of the subject's type or a variant alias. `Enum.Variant` qualifies it with the subject's enum, and a variant without a payload is written `Enum.Variant`.
- Sub-patterns are positional, one per payload value. `Some(p)`, `Ok(p)` and `Err(p)` have one sub-pattern.

Refused (`INCAN-T0001`): a variant the subject's enum does not declare, a qualifier other than the subject's enum, a named sub-pattern, more sub-patterns than the variant has payload values, and a `Some`, `Ok` or `Err` pattern without exactly one sub-pattern.

```incan
enum Shape:
    Circle(float)
    Empty

def area(shape: Shape) -> float:
    match shape:
        Circle(r) => return 3.0 * r * r      # accepted
        Shape.Empty => return 0.0            # accepted
        Square(side) => return 0.0           # refused: Shape declares no Square
        Circle(radius=r) => return 0.0       # refused: named sub-pattern
        Circle(r, s) => return 0.0           # refused: Circle has one payload value
```

## Record patterns

- `Type` is the subject's model or class.
- Sub-patterns are named: `field=p`. A field is named by its name or its alias.
- A private field is a field without `pub` on a `pub model` or on a class. A pattern names a private field only inside a method declared on the type that declares the field.
- The fields a pattern leaves unnamed, private ones included, match any value.

Refused (`INCAN-T0001`): a type other than the subject's, a positional sub-pattern, a field the type does not declare, a field named twice, and a private field named outside a method of its declaring type.

```incan
pub model Account:
    pub kind: str
    pub tier: int
    secret: str

    def is_open(self) -> bool:
        match self:
            Account(secret="") => return true        # accepted: a method of Account
            _ => return false

def kind_of(account: Account) -> str:
    match account:
        Account(kind="premium") => return "premium"  # accepted: tier and secret match any value
        Account(secret="x") => return "x"            # refused: secret is private to Account
        Account("basic") => return "basic"           # refused: positional sub-pattern
        _ => return "other"
```

## Type patterns

- The subject is a union (see [Union types](union_types.md)) or an `Option` of one.
- `T` is a member type of the subject, or a type alias whose members are all member types of the subject; `p` then matches a value of those member types.
- The sub-pattern is positional.

Refused (`INCAN-T0001`): a named sub-pattern.

```incan
def describe(value: int | str) -> str:
    match value:
        int(n) => return f"{n}"     # accepted
        str(s) => return s          # accepted
```

## Alternations

- An alternation is a whole pattern or a part of a larger pattern. Each alternative follows the rules of its own form.
- Every alternative binds the same names, each at the same type.
- Alternatives are tried in order, and the first one that matches binds the names.
- A guard after an alternation runs once, for the alternative that matched. A false guard continues matching with the next arm; the arm's later alternatives are not tried.

Refused (`INCAN-T0001`): alternatives that bind different names, and alternatives that bind one name at different types. Refused (`INCAN-P0001`): an alternation in a `while let` pattern.

```incan
def tag(pair: tuple[int, str]) -> str:
    match pair:
        (0, "a") | (1, "b") => return "known"      # accepted
        (0, name) | (1, name) => return name       # accepted
        (2, name) | (3, _) => return "partial"     # refused: only the first alternative binds name
        (n, _) | (_, n) => return "mixed"          # refused: n is an int in one alternative and a str in the other
        _ => return "other"
```

## Match values

A `match` has one type, which every arm produces.

- An arm whose body is an expression produces that expression's value. An arm whose body is a block produces no value, and a block whose last statement is `return`, `break` or `continue` takes no part.
- A literal arm of a `match` written to a place of a declared type (an annotated binding, a `return`, an argument) takes that type. When the type is numeric or an `Option` that holds no union, each arm is assignable to it and the `match` has it. Otherwise the arms unify: an arm assignable to another arm's type takes that type, so `i8` and `int` arms make an `int` match, and `Some(1)` and `None` arms an `Option[int]` one, and the `match` is then assignable to the place as any value is.
- A narrower numeric arm is widened, and a payload arm is wrapped in `Some`, in the arm itself (see [Assignment between numeric types](numeric_semantics.md#assignment-between-numeric-types)).

Refused (`INCAN-T0001`): arms that share no type, in any position, a `match` used as a statement included.

```incan
def pick(n: int, small: i8, wide: int) -> int:
    chosen = match n:               # accepted: an int
        0 => small
        _ => wide
    return chosen

def label(n: int) -> str:
    text = match n:                 # refused: a str arm and an int arm
        0 => "zero"
        _ => 5
    return text
```

## Coverage

The unguarded arms of a `match` cover its subject. A guarded arm never counts toward coverage, an alternation counts as one arm per alternative, and a group counts as the pattern it groups. An arm whose pattern is `_` or a binding covers any subject. Otherwise:

| Subject type | The unguarded arms cover it when |
| --- | --- |
| `bool` | They match `true` and `false`. |
| An enum, `Option[T]`, or `Result[T, E]` | For each variant, the arms that name it cover every value of its payload. A generic enum's payload types are those of the subject's type arguments. |
| A union | For each member type, the arms that name it cover every value of it. |
| A tuple | They cover every combination of element values, element by element. |
| A model or class | They cover every combination of field values, field by field. |
| A number, `str`, `FrozenStr`, `bytes`, or `FrozenBytes` | Never: literal arms do not cover it, and only an arm that is `_` or a binding does. |

The same rules apply to a payload, element, or field at any depth.

Refused (`INCAN-T0001`): a `match` whose unguarded arms do not cover its subject.

```incan
def first(value: Option[int]) -> int:
    match value:                    # refused: Some with a payload other than 0 is not covered
        Some(0) => return 0
        None => return -1

def show(value: Result[Option[int], str]) -> str:
    match value:                    # accepted
        Ok(Some(n)) => return f"{n}"
        Ok(None) => return "empty"
        Err(message) => return message

def label(code: int) -> str:
    match code:                     # refused: codes other than 0 and 1 are not covered
        0 => return "zero"
        1 => return "one"

def corner(point: tuple[bool, int]) -> str:
    match point:                    # refused: (true, n) with n other than 0 is not covered
        (true, 0) => return "origin"
        (false, _) => return "left"
```
