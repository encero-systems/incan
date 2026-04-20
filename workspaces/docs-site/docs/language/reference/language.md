# Incan language reference

!!! warning "Generated file"
    Do not edit this page by hand.
    If it looks wrong/outdated, regenerate it from source and commit the result.

    Regenerate with: `cargo run -p incan_core --bin generate_lang_reference`

## Contents

- [Keywords](#keywords)
- [Soft keywords](#soft-keywords)
- [Standard library namespaces](#standard-library-namespaces)
- [Builtin exceptions](#builtin-exceptions)
- [Builtin functions](#builtin-functions)
- [Decorators](#decorators)
- [Derives](#derives)
- [Builtin traits](#builtin-traits)
- [Operators](#operators)
- [Punctuation](#punctuation)
- [Builtin types](#builtin-types)
- [Surface constructors](#surface-constructors)
- [Surface functions](#surface-functions)
- [Surface string methods](#surface-string-methods)
- [Surface types](#surface-types)
- [Surface methods](#surface-methods)

## Keywords

| Id | Canonical | Aliases | Reservation | Activation | Category | Usage | RFC | Since | Stability |
|----|---|---|---|---|---|---|---|---|---|
| If | `if` |  | Hard | - | ControlFlow | Statement, Expression | RFC 000 | 0.1 | Stable |
| Else | `else` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Elif | `elif` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Match | `match` |  | Hard | - | ControlFlow | Statement, Expression | RFC 000 | 0.1 | Stable |
| Case | `case` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| While | `while` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| For | `for` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Break | `break` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Continue | `continue` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Return | `return` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Yield | `yield` |  | Hard | - | ControlFlow | Statement, Expression | RFC 001 | 0.1 | Stable |
| Pass | `pass` |  | Hard | - | ControlFlow | Statement | RFC 000 | 0.1 | Stable |
| Assert | `assert` |  | Soft | `std.testing` | ControlFlow | Statement | RFC 018 | 0.2 | Draft |
| Def | `def` | `fn` | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Async | `async` |  | Soft | `std.async` | Definition | Modifier | RFC 000 | 0.1 | Stable |
| Await | `await` |  | Soft | `std.async` | Definition | Expression | RFC 000 | 0.1 | Stable |
| Class | `class` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Model | `model` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Trait | `trait` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Enum | `enum` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Type | `type` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| Newtype | `newtype` |  | Hard | - | Definition | Statement | RFC 000 | 0.1 | Stable |
| With | `with` |  | Hard | - | Definition | Modifier | RFC 000 | 0.1 | Stable |
| Extends | `extends` |  | Hard | - | Definition | Modifier | RFC 000 | 0.1 | Stable |
| Pub | `pub` |  | Hard | - | Definition | Modifier | RFC 000 | 0.1 | Stable |
| Import | `import` |  | Hard | - | Import | Statement | RFC 000 | 0.1 | Stable |
| From | `from` |  | Hard | - | Import | Statement | RFC 000 | 0.1 | Stable |
| As | `as` |  | Hard | - | Import | Modifier | RFC 000 | 0.1 | Stable |
| Rust | `rust` |  | Hard | - | Import | Modifier | RFC 005 | 0.1 | Stable |
| Python | `python` |  | Hard | - | Import | Modifier | RFC 000 | 0.1 | Stable |
| Super | `super` |  | Hard | - | Import | Expression | RFC 000 | 0.1 | Stable |
| Crate | `crate` |  | Hard | - | Import | Expression | RFC 005 | 0.1 | Stable |
| Const | `const` |  | Hard | - | Binding | Statement | RFC 008 | 0.1 | Stable |
| Static | `static` |  | Hard | - | Binding | Statement | RFC 052 | 0.2 | Stable |
| Let | `let` |  | Hard | - | Binding | Statement | RFC 000 | 0.1 | Stable |
| Mut | `mut` |  | Hard | - | Binding | Modifier | RFC 000 | 0.1 | Stable |
| SelfKw | `self` |  | Hard | - | Binding | ReceiverOnly | RFC 000 | 0.1 | Stable |
| True | `true` | `True` | Hard | - | Literal | Expression | RFC 000 | 0.1 | Stable |
| False | `false` | `False` | Hard | - | Literal | Expression | RFC 000 | 0.1 | Stable |
| None | `None` |  | Hard | - | Literal | Expression | RFC 000 | 0.1 | Stable |
| And | `and` |  | Hard | - | Operator | Operator | RFC 000 | 0.1 | Stable |
| Or | `or` |  | Hard | - | Operator | Operator | RFC 000 | 0.1 | Stable |
| Not | `not` |  | Hard | - | Operator | Operator | RFC 000 | 0.1 | Stable |
| In | `in` |  | Hard | - | Operator | Operator | RFC 000 | 0.1 | Stable |
| Is | `is` |  | Hard | - | Operator | Operator | RFC 000 | 0.1 | Stable |

### Examples

Only keywords with examples are listed here.

## Soft keywords

Soft keywords are only reserved when their activating `std.*` namespace is imported.

| Id | Canonical | Activated by | Category | Usage | RFC | Since | Stability |
|---|---|---|---|---|---|---|---|
| Assert | `assert` | `std.testing` | ControlFlow | Statement | RFC 018 | 0.2 | Draft |
| Async | `async` | `std.async` | Definition | Modifier | RFC 000 | 0.1 | Stable |
| Await | `await` | `std.async` | Definition | Expression | RFC 000 | 0.1 | Stable |

## Standard library namespaces

| Namespace | Feature gate | Submodules | Activates soft keywords |
|---|---|---|---|
| `std.web` | `web` | `std.web.app`, `std.web.routing`, `std.web.request`, `std.web.response`, `std.web.macros`, `std.web.prelude` | - |
| `std.testing` | - | - | `assert` |
| `std.async` | `async` | `std.async.time`, `std.async.task`, `std.async.channel`, `std.async.select`, `std.async.sync`, `std.async.prelude` | `async`, `await` |
| `std.serde` | `json` | `std.serde.json` | - |
| `std.reflection` | - | - | - |
| `std.derives` | - | `std.derives.string`, `std.derives.comparison`, `std.derives.copying`, `std.derives.collection` | - |
| `std.traits` | - | `std.traits.convert`, `std.traits.ops`, `std.traits.error`, `std.traits.indexing`, `std.traits.callable`, `std.traits.prelude` | - |
| `std.math` | - | - | - |
| `std.rust` | - | - | - |

## Builtin exceptions

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| ValueError | `ValueError` |  | Raised when an operation receives a value of the right type but an invalid value. | RFC 000 | 0.1 | Stable |
| TypeError | `TypeError` |  | Raised when an operation receives a value of an inappropriate type. | RFC 000 | 0.1 | Stable |
| ZeroDivisionError | `ZeroDivisionError` |  | Raised when dividing or taking modulo by zero (Python-like numeric semantics). | RFC 000 | 0.1 | Stable |
| IndexError | `IndexError` |  | Raised when an index is out of bounds (e.g. string/list indexing) or when calling `list.pop()` on an empty list. | RFC 000 | 0.1 | Stable |
| KeyError | `KeyError` |  | Raised when a dict key is missing. | RFC 000 | 0.1 | Stable |
| JsonDecodeError | `JSONDecodeError` |  | Raised when parsing JSON fails (Python-like). | RFC 000 | 0.1 | Stable |

### Examples

Only exceptions with examples are listed here.

#### `ValueError`

```incan
def main() -> None:
    print("abc"[0:3:0])  # step 0

```

Panics at runtime with `ValueError: slice step cannot be zero`.

```incan
def main() -> None:
    _ = int("abc")

```

Panics at runtime with `ValueError: cannot convert 'abc' to int`.

```incan
def main() -> None:
    _ = float("abc")

```

Panics at runtime with `ValueError: cannot convert 'abc' to float`.

```incan
def main() -> None:
    # range step cannot be zero (Python-like)
    for i in range(0, 5, 0):
        print(i)

```

Panics at runtime with `ValueError: range() arg 3 must not be zero`.

#### `TypeError`

```incan
def main() -> None:
    # Example: JSON serialization failures (e.g. NaN/Inf) raise TypeError
    _ = json_stringify(nan)

```

Panics at runtime with a `TypeError: ... is not JSON serializable` message.

#### `ZeroDivisionError`

```incan
def main() -> None:
    print(1 / 0)

```

Panics at runtime with `ZeroDivisionError: float division by zero`.

#### `IndexError`

```incan
def main() -> None:
    print("a"[99])

```

Panics at runtime with `IndexError: ...`.

```incan
def main() -> None:
    xs: list[int] = [1, 2, 3]
    print(xs[99])

```

Panics at runtime with `IndexError: index 99 out of range for list of length 3`.

```incan
def main() -> None:
    xs: list[int] = []
    _ = xs.pop()

```

Panics at runtime with `IndexError: pop from empty list`.

#### `KeyError`

```incan
def main() -> None:
    d: Dict[str, int] = {"a": 1}
    print(d["b"])

```

Panics at runtime with `KeyError: 'b' not found in dict`.

#### `JsonDecodeError`

```incan
@derive(Deserialize)
model User:
    name: str

def main() -> None:
    bad: str = "{"
    match User.from_json(bad):
        case Ok(u): print(u.name)
        case Err(e): print(e)

```

`from_json` returns `Result[T, str]`; on failure the error string is prefixed with `JSONDecodeError: ...`.

## Builtin functions

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Print | `print` | `println` | Print values to stdout. | RFC 000 | 0.1 | Stable |
| Len | `len` |  | Return the length of a collection/string. | RFC 000 | 0.1 | Stable |
| Sum | `sum` |  | Sum a numeric iterable/collection. | RFC 000 | 0.1 | Stable |
| Min | `min` |  | Return the minimum element of a collection. | RFC 000 | 0.1 | Stable |
| Max | `max` |  | Return the maximum element of a collection. | RFC 000 | 0.1 | Stable |
| Str | `str` |  | Convert a value to a string. | RFC 000 | 0.1 | Stable |
| Int | `int` |  | Convert a value to an integer. | RFC 000 | 0.1 | Stable |
| Float | `float` |  | Convert a value to a float. | RFC 000 | 0.1 | Stable |
| Bool | `bool` |  | Convert a value to a boolean. | RFC 000 | 0.1 | Stable |
| Abs | `abs` |  | Absolute value (numeric). | RFC 000 | 0.1 | Stable |
| Range | `range` |  | Create a range of integers. | RFC 000 | 0.1 | Stable |
| Enumerate | `enumerate` |  | Enumerate an iterable into (index, value) pairs. | RFC 000 | 0.1 | Stable |
| Zip | `zip` |  | Zip iterables element-wise into tuples. | RFC 000 | 0.1 | Stable |
| Sorted | `sorted` |  | Return a sorted copy of a collection. | RFC 000 | 0.1 | Stable |
| ReadFile | `read_file` |  | Read a file from disk into a string/bytes. | RFC 000 | 0.1 | Stable |
| WriteFile | `write_file` |  | Write a string/bytes to a file on disk. | RFC 000 | 0.1 | Stable |
| JsonStringify | `json_stringify` |  | Serialize a value to JSON. | RFC 000 | 0.1 | Stable |

## Decorators

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Derive | `@derive` |  | Derive common trait implementations. | RFC 000 | 0.1 | Stable |
| RustExtern | `@rust.extern` |  | Mark functions whose body is provided by a Rust module. | RFC 022 | 0.2 | Stable |
| StaticMethod | `@staticmethod` |  | Mark a method as static (no self receiver). | RFC 000 | 0.1 | Stable |
| ClassMethod | `@classmethod` |  | Mark a method as a class method (no implicit self receiver). | RFC 000 | 0.2 | Stable |
| Requires | `@requires` |  | Declare required fields for trait default methods. | RFC 000 | 0.1 | Stable |

## Derives

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Debug | `Debug` |  | Derive Rust-style debug formatting. | RFC 000 | 0.1 | Stable |
| Display | `Display` |  | Derive user-facing string formatting. | RFC 000 | 0.1 | Stable |
| Eq | `Eq` |  | Derive equality comparisons. | RFC 000 | 0.1 | Stable |
| PartialEq | `PartialEq` |  | Derive partial equality comparisons. | RFC 000 | 0.1 | Stable |
| Ord | `Ord` |  | Derive ordering comparisons. | RFC 000 | 0.1 | Stable |
| PartialOrd | `PartialOrd` |  | Derive partial ordering comparisons. | RFC 000 | 0.1 | Stable |
| Hash | `Hash` |  | Derive hashing support (for map/set keys). | RFC 000 | 0.1 | Stable |
| Clone | `Clone` |  | Derive deep cloning. | RFC 000 | 0.1 | Stable |
| Copy | `Copy` |  | Derive copy semantics for simple value types. | RFC 000 | 0.1 | Stable |
| Default | `Default` |  | Derive a default value constructor. | RFC 000 | 0.1 | Stable |
| Serialize | `Serialize` |  | Derive serialization support (e.g. JSON). | RFC 000 | 0.1 | Stable |
| Deserialize | `Deserialize` |  | Derive deserialization support (e.g. JSON). | RFC 000 | 0.1 | Stable |
| Validate | `Validate` |  | Enable validated construction via `TypeName.new(...)` and require a `validate(self) -> Result[Self, E]` method. | RFC 000 | 0.1 | Stable |

## Builtin traits

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Debug | `Debug` |  | Trait for debug formatting output. | RFC 000 | 0.1 | Stable |
| Display | `Display` |  | Trait for user-facing string formatting. | RFC 000 | 0.1 | Stable |
| Eq | `Eq` |  | Trait for equality comparisons. | RFC 000 | 0.1 | Stable |
| PartialEq | `PartialEq` |  | Trait for partial equality comparisons. | RFC 000 | 0.1 | Stable |
| Ord | `Ord` |  | Trait for ordering comparisons. | RFC 000 | 0.1 | Stable |
| PartialOrd | `PartialOrd` |  | Trait for partial ordering comparisons. | RFC 000 | 0.1 | Stable |
| Hash | `Hash` |  | Trait for hashing support. | RFC 000 | 0.1 | Stable |
| Clone | `Clone` |  | Trait for cloning values. | RFC 000 | 0.1 | Stable |
| Default | `Default` |  | Trait for default value construction. | RFC 000 | 0.1 | Stable |
| From | `From` |  | Trait for conversions. | RFC 000 | 0.1 | Stable |
| Into | `Into` |  | Trait for conversions. | RFC 000 | 0.1 | Stable |
| TryFrom | `TryFrom` |  | Trait for fallible conversions. | RFC 000 | 0.1 | Stable |
| TryInto | `TryInto` |  | Trait for fallible conversions. | RFC 000 | 0.1 | Stable |
| Iterator | `Iterator` |  | Trait for iterator behavior. | RFC 000 | 0.1 | Stable |
| IntoIterator | `IntoIterator` |  | Trait for conversion into iterators. | RFC 000 | 0.1 | Stable |
| Error | `Error` |  | Trait for error-like values. | RFC 000 | 0.1 | Stable |

## Operators

### Notes

- **Precedence**: Higher binds tighter (e.g. `*` > `+`). Values are relative and must be consistent with the parser.
- **Associativity**: How operators of the same precedence group (left-to-right vs right-to-left).
- **Fixity**: Whether the operator is used as a prefix unary operator or an infix binary operator.
- **KeywordSpelling**: Whether the operator token is spelled as a reserved word (e.g. `and`, `not`).

| Id | Spellings | Precedence | Associativity | Fixity | KeywordSpelling | RFC | Since | Stability |
|---|---|---:|---|---|---|---|---|---|
| Plus | `+` | 50 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Minus | `-` | 50 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Star | `*` | 60 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| StarStar | `**` | 70 | Right | Infix | false | RFC 000 | 0.1 | Stable |
| Slash | `/` | 60 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| SlashSlash | `//` | 60 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Percent | `%` | 60 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| EqEq | `==` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| NotEq | `!=` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Lt | `<` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| LtEq | `<=` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Gt | `>` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| GtEq | `>=` | 40 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| Eq | `=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| PlusEq | `+=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| MinusEq | `-=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| StarEq | `*=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| SlashEq | `/=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| SlashSlashEq | `//=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| PercentEq | `%=` | 10 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| DotDot | `..` | 30 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| DotDotEq | `..=` | 30 | Left | Infix | false | RFC 000 | 0.1 | Stable |
| And | `and` | 35 | Left | Infix | true | RFC 000 | 0.1 | Stable |
| Or | `or` | 35 | Left | Infix | true | RFC 000 | 0.1 | Stable |
| Not | `not` | 45 | Left | Prefix | true | RFC 000 | 0.1 | Stable |
| In | `in` | 35 | Left | Infix | true | RFC 000 | 0.1 | Stable |
| Is | `is` | 35 | Left | Infix | true | RFC 000 | 0.1 | Stable |

## Punctuation

| Id | Canonical | Aliases | Category | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Comma | `,` |  | Separator | RFC 000 | 0.1 | Stable |
| Colon | `:` |  | Separator | RFC 000 | 0.1 | Stable |
| Question | `?` |  | Marker | RFC 000 | 0.1 | Stable |
| At | `@` |  | Marker | RFC 000 | 0.1 | Stable |
| Dot | `.` |  | Access | RFC 000 | 0.1 | Stable |
| ColonColon | `::` |  | Access | RFC 000 | 0.1 | Stable |
| Arrow | `->` |  | Arrow | RFC 000 | 0.1 | Stable |
| FatArrow | `=>` |  | Arrow | RFC 000 | 0.1 | Stable |
| Ellipsis | `...` |  | Marker | RFC 000 | 0.1 | Stable |
| LParen | `(` |  | Delimiter | RFC 000 | 0.1 | Stable |
| RParen | `)` |  | Delimiter | RFC 000 | 0.1 | Stable |
| LBracket | `[` |  | Delimiter | RFC 000 | 0.1 | Stable |
| RBracket | `]` |  | Delimiter | RFC 000 | 0.1 | Stable |
| LBrace | `{` |  | Delimiter | RFC 000 | 0.1 | Stable |
| RBrace | `}` |  | Delimiter | RFC 000 | 0.1 | Stable |

## Builtin types

### String-like

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Str | `str` |  | Builtin UTF-8 string type. | RFC 000 | 0.1 | Stable |
| Bytes | `bytes` |  | Builtin byte buffer type. | RFC 000 | 0.1 | Stable |
| FrozenStr | `frozenstr` | `FrozenStr` | Immutable/const-friendly string type. | RFC 009 | 0.1 | Stable |
| FrozenBytes | `frozenbytes` | `FrozenBytes` | Immutable/const-friendly bytes type. | RFC 009 | 0.1 | Stable |
| FString | `fstring` | `FString` | Formatted string result type. | RFC 000 | 0.1 | Stable |


### Numerics

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Int | `int` | `i64`, `i32` | Builtin signed integer type. | RFC 000 | 0.1 | Stable |
| Float | `float` | `f64`, `f32` | Builtin floating-point type. | RFC 000 | 0.1 | Stable |
| Bool | `bool` |  | Builtin boolean type. | RFC 000 | 0.1 | Stable |


### Collections / generic bases

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| List | `List` | `list`, `Vec` | Growable list (generic sequence) type. | RFC 000 | 0.1 | Stable |
| Dict | `Dict` | `dict`, `HashMap` | Key/value map type. | RFC 000 | 0.1 | Stable |
| Set | `Set` | `set` | Unordered set type. | RFC 000 | 0.1 | Stable |
| Tuple | `Tuple` | `tuple` | Fixed-length heterogeneous tuple type. | RFC 000 | 0.1 | Stable |
| Option | `Option` | `option` | Optional value type (`Some`/`None`). | RFC 000 | 0.1 | Stable |
| Result | `Result` | `result` | Result type (`Ok`/`Err`). | RFC 000 | 0.1 | Stable |
| FrozenList | `FrozenList` | `frozenlist` | Immutable/const-friendly list type. | RFC 009 | 0.1 | Stable |
| FrozenDict | `FrozenDict` | `frozendict` | Immutable/const-friendly dict type. | RFC 009 | 0.1 | Stable |
| FrozenSet | `FrozenSet` | `frozenset` | Immutable/const-friendly set type. | RFC 009 | 0.1 | Stable |

## Surface constructors

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Ok | `Ok` |  | Construct an `Ok(T)` variant (Result success). | RFC 000 | 0.1 | Stable |
| Err | `Err` |  | Construct an `Err(E)` variant (Result failure). | RFC 000 | 0.1 | Stable |
| Some | `Some` |  | Construct a `Some(T)` variant (Option present). | RFC 000 | 0.1 | Stable |
| None | `None` |  | Construct a `None` variant (Option absent). | RFC 000 | 0.1 | Stable |

## Surface functions

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| SleepMs | `sleep_ms` |  | Sleep for N milliseconds. | RFC 000 | 0.1 | Stable |
| Timeout | `timeout` |  | Run an async operation with a timeout. | RFC 000 | 0.1 | Stable |
| TimeoutMs | `timeout_ms` |  | Run an async operation with a timeout in milliseconds. | RFC 000 | 0.1 | Stable |
| SelectTimeout | `select_timeout` |  | Select between futures with a timeout. | RFC 000 | 0.1 | Stable |
| YieldNow | `yield_now` |  | Yield execution back to the async scheduler. | RFC 000 | 0.1 | Stable |
| Spawn | `spawn` |  | Spawn an async task. | RFC 000 | 0.1 | Stable |
| SpawnBlocking | `spawn_blocking` |  | Spawn a blocking task on a dedicated thread pool. | RFC 004 | 0.1 | Stable |
| Channel | `channel` |  | Create a bounded channel (sender, receiver). | RFC 000 | 0.1 | Stable |
| UnboundedChannel | `unbounded_channel` |  | Create an unbounded channel (sender, receiver). | RFC 000 | 0.1 | Stable |
| Oneshot | `oneshot` |  | Create a oneshot channel (sender, receiver). | RFC 000 | 0.1 | Stable |

## Surface string methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Upper | `upper` |  | Convert to uppercase. | RFC 009 | 0.1 | Stable |
| Lower | `lower` |  | Convert to lowercase. | RFC 009 | 0.1 | Stable |
| Strip | `strip` |  | Strip leading and trailing whitespace. | RFC 009 | 0.1 | Stable |
| Replace | `replace` |  | Replace occurrences of a substring. | RFC 009 | 0.1 | Stable |
| Join | `join` |  | Join an iterable/list of strings with this separator. | RFC 009 | 0.1 | Stable |
| ToString | `to_string` |  | Return a string representation (identity for strings). | RFC 009 | 0.1 | Stable |
| SplitWhitespace | `split_whitespace` |  | Split on Unicode whitespace. | RFC 009 | 0.1 | Stable |
| Split | `split` |  | Split on a separator substring. | RFC 009 | 0.1 | Stable |
| Contains | `contains` |  | Return true if the substring occurs within the string. | RFC 009 | 0.1 | Stable |
| StartsWith | `startswith` | `starts_with` | Return true if the string starts with a prefix. | RFC 009 | 0.1 | Stable |
| EndsWith | `endswith` | `ends_with` | Return true if the string ends with a suffix. | RFC 009 | 0.1 | Stable |
| Len | `len` |  | Return the length (in Unicode scalars). | RFC 009 | 0.1 | Stable |
| IsEmpty | `is_empty` |  | Return true if the length is zero. | RFC 009 | 0.1 | Stable |

## Surface types

| Id | Canonical | Aliases | Kind | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|---|
| Mutex | `Mutex` |  | Generic | Async/runtime mutex. | RFC 000 | 0.1 | Stable |
| RwLock | `RwLock` |  | Generic | Async/runtime read-write lock. | RFC 000 | 0.1 | Stable |
| Semaphore | `Semaphore` |  | Named | Async/runtime semaphore. | RFC 000 | 0.1 | Stable |
| Barrier | `Barrier` |  | Named | Async/runtime barrier. | RFC 000 | 0.1 | Stable |
| JoinHandle | `JoinHandle` |  | Generic | Handle to a spawned task. | RFC 000 | 0.1 | Stable |
| Sender | `Sender` |  | Generic | Bounded channel sender. | RFC 000 | 0.1 | Stable |
| Receiver | `Receiver` |  | Generic | Bounded channel receiver. | RFC 000 | 0.1 | Stable |
| OneshotSender | `OneshotSender` |  | Generic | Oneshot channel sender. | RFC 000 | 0.1 | Stable |
| OneshotReceiver | `OneshotReceiver` |  | Generic | Oneshot channel receiver. | RFC 000 | 0.1 | Stable |
| Vec | `Vec` |  | Generic | Rust interop `Vec<T>`. | RFC 005 | 0.1 | Stable |
| HashMap | `HashMap` |  | Generic | Rust interop `HashMap<K, V>`. | RFC 005 | 0.1 | Stable |
| App | `App` |  | Named | Web application handle for running an HTTP server. | RFC 000 | 0.1 | Stable |
| Response | `Response` |  | Named | HTTP response builder for web handlers. | RFC 000 | 0.1 | Stable |
| Html | `Html` |  | Named | HTML response wrapper for web handlers. | RFC 000 | 0.1 | Stable |
| Json | `Json` |  | Generic | JSON response/extractor wrapper for web handlers. | RFC 000 | 0.1 | Stable |
| Query | `Query` |  | Generic | Query-string extractor wrapper for web handlers. | RFC 000 | 0.1 | Stable |
| Path | `Path` |  | Generic | Path-parameter extractor wrapper for web handlers. | RFC 000 | 0.1 | Stable |
| Body | `Body` |  | Generic | Request body extractor wrapper for web handlers. | RFC 000 | 0.1 | Stable |
| Request | `Request` |  | Named | Full HTTP request access for web handlers. | RFC 000 | 0.1 | Stable |
| FieldInfo | `FieldInfo` |  | Named | Field metadata record returned by __fields__(). | RFC 021 | 0.1 | Stable |

## Surface methods

### float methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Sqrt | `sqrt` |  | Square root. | RFC 009 | 0.1 | Stable |
| Abs | `abs` |  | Absolute value. | RFC 009 | 0.1 | Stable |
| Floor | `floor` |  | Round down to the nearest integer (as float). | RFC 009 | 0.1 | Stable |
| Ceil | `ceil` |  | Round up to the nearest integer (as float). | RFC 009 | 0.1 | Stable |
| Round | `round` |  | Round to the nearest integer (as float). | RFC 009 | 0.1 | Stable |
| Sin | `sin` |  | Sine. | RFC 009 | 0.1 | Stable |
| Cos | `cos` |  | Cosine. | RFC 009 | 0.1 | Stable |
| Tan | `tan` |  | Tangent. | RFC 009 | 0.1 | Stable |
| Exp | `exp` |  | Exponentiation (e^x). | RFC 009 | 0.1 | Stable |
| Ln | `ln` |  | Natural logarithm. | RFC 009 | 0.1 | Stable |
| Log2 | `log2` |  | Base-2 logarithm. | RFC 009 | 0.1 | Stable |
| Log10 | `log10` |  | Base-10 logarithm. | RFC 009 | 0.1 | Stable |
| IsNan | `is_nan` |  | Return true if this value is NaN. | RFC 009 | 0.1 | Stable |
| IsInfinite | `is_infinite` |  | Return true if this value is ±infinity. | RFC 009 | 0.1 | Stable |
| IsFinite | `is_finite` |  | Return true if this value is finite. | RFC 009 | 0.1 | Stable |
| Powi | `powi` |  | Raise to an integer power. | RFC 009 | 0.1 | Stable |
| Powf | `powf` |  | Raise to a float power. | RFC 009 | 0.1 | Stable |


### List methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Append | `append` |  | Append an element to the end of the list. | RFC 009 | 0.1 | Stable |
| Extend | `extend` |  | Append all elements from another list. | RFC 009 | 0.2 | Stable |
| Pop | `pop` |  | Remove and return the last element. On an empty list, panics with `IndexError: pop from empty list` (Python-compatible). | RFC 009 | 0.1 | Stable |
| Contains | `contains` |  | Return true if the list contains a value. | RFC 009 | 0.1 | Stable |
| Swap | `swap` |  | Swap two elements by index. | RFC 009 | 0.1 | Stable |
| Reserve | `reserve` |  | Reserve capacity for at least N more elements. | RFC 009 | 0.1 | Stable |
| ReserveExact | `reserve_exact` |  | Reserve capacity for exactly N more elements. | RFC 009 | 0.1 | Stable |
| Remove | `remove` |  | Remove and return the element at the given index. | RFC 009 | 0.1 | Stable |
| Count | `count` |  | Count occurrences of a value. | RFC 009 | 0.1 | Stable |
| Index | `index` |  | Return the index of a value (or error if not found). | RFC 009 | 0.1 | Stable |


### Dict methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Keys | `keys` |  | Return an iterable/list of keys. | RFC 009 | 0.1 | Stable |
| Values | `values` |  | Return an iterable/list of values. | RFC 009 | 0.1 | Stable |
| Get | `get` |  | Get a value by key, optionally with a default. | RFC 009 | 0.1 | Stable |
| Insert | `insert` |  | Insert or overwrite a key/value pair. | RFC 009 | 0.1 | Stable |


### Set methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Contains | `contains` |  | Return true if the set contains a value. | RFC 009 | 0.1 | Stable |


### Option methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Copied | `copied` |  | Copy from Option[&T] to Option[T] when T: Copy. | RFC 000 | 0.1 | Stable |
| UnwrapOr | `unwrap_or` |  | Return the contained value or a default. | RFC 000 | 0.1 | Stable |
| Unwrap | `unwrap` |  | Return the contained value or panic. | RFC 000 | 0.1 | Stable |


### FrozenList methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Len | `len` |  | Return the number of elements. | RFC 009 | 0.1 | Stable |
| IsEmpty | `is_empty` |  | Return true if the list is empty. | RFC 009 | 0.1 | Stable |


### FrozenDict methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Len | `len` |  | Return the number of entries. | RFC 009 | 0.1 | Stable |
| IsEmpty | `is_empty` |  | Return true if the dict is empty. | RFC 009 | 0.1 | Stable |
| ContainsKey | `contains_key` |  | Return true if the dict contains a key. | RFC 009 | 0.1 | Stable |


### FrozenSet methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Len | `len` |  | Return the number of elements. | RFC 009 | 0.1 | Stable |
| IsEmpty | `is_empty` |  | Return true if the set is empty. | RFC 009 | 0.1 | Stable |
| Contains | `contains` |  | Return true if the set contains a value. | RFC 009 | 0.1 | Stable |


### FrozenBytes methods

| Id | Canonical | Aliases | Description | RFC | Since | Stability |
|---|---|---|---|---|---|---|
| Len | `len` |  | Return the number of bytes. | RFC 009 | 0.1 | Stable |
| IsEmpty | `is_empty` |  | Return true if the byte string is empty. | RFC 009 | 0.1 | Stable |
