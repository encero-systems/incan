# Union Types

Incan supports anonymous closed union types for values that may be one of several unrelated types.

```incan
def parse_value(flag: bool) -> int | str:
    if flag:
        return 42
    return "fallback"
```

`Union[A, B, ...]` is the canonical spelling for ordinary unions. `A | B` is equivalent syntax in type positions, and nested unions, duplicate members, and member ordering normalize to the same semantic type.

When `None` appears in a union, the type canonicalizes through `Option[...]`:

```incan
str | None          # Option[str]
int | str | None    # Option[Union[int, str]]
```

Concrete member values are assignable to a union that contains that member. A source union is assignable to a target union when every source member is accepted by some target member.

Union values do not expose member-specific methods or operators until narrowed. Use `isinstance(value, T)` or a type pattern in `match`:

```incan
def normalize(value: int | str) -> str:
    if isinstance(value, str):
        return value.upper()
    return "number"

def describe(value: int | str) -> str:
    match value:
        int(n) =>
            return str(n)
        str(s) =>
            return s.upper()
```

`match` over a union must cover every member type or include `_`.

Pattern alternation can group union type patterns when the branch does not need differently typed bindings:

```incan
def classify(value: int | str | None) -> str:
    match value:
        int(_) | str(_) => return "present"
        None => return "missing"
```

Do not reuse one binding name across alternatives that infer different types. `int(item) | str(item)` is rejected because `item` would be both `int` and `str` in the same branch.

For unions that canonicalize through `Option[...]`, use `is None` or `is not None` to narrow the optional value:

```incan
def label(value: str | None) -> str:
    if value is not None:
        return value.upper()
    return "missing"
```

Current implementation note: ordinary unions support return/assignment/call-argument wrapping, `isinstance` narrowing for true branches, else branches, wider unions, and chained `elif` branches, and exhaustive `match` type patterns. Unions containing `None` continue to use the existing `Option[...]` representation and narrow through `is None`, `is not None`, `isinstance`, and `match`.
