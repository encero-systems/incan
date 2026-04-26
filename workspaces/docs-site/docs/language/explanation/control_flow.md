# Control flow

This page explains branching and looping constructs in Incan.

## Branching with `if`

```incan
def classify(n: int) -> str:
    if n < 0:
        return "negative"
    elif n == 0:
        return "zero"
    else:
        return "positive"
```

Use ordinary `if` when the condition is a boolean expression and both the true/false shape of the branch matters.

## Pattern-oriented branching with `if let`

Use `if let` when you want to try one pattern match and do something only on success.

```incan
def print_primary_email(user: Option[User]) -> None:
    if let Some(u) = user:
        println(u.email)
```

This is the concise form of “match one successful shape, otherwise do nothing.” It is most useful with `Option`, `Result`, and enum payloads.

```incan
def log_port(raw: str) -> None:
    if let Ok(port) = parse_port(raw):
        println(f"listening on {port}")
```

Prefer `if let` when:

- exactly one successful pattern matters;
- the non-match path should do nothing;
- the code reads more naturally as opportunistic extraction than as full branching.

Use `match` instead when the non-match path matters, when you need more than one arm, or when you want exhaustiveness to stay explicit.

```incan
match parse_port(raw):
    case Ok(port): println(f"listening on {port}")
    case Err(e): println(f"invalid port: {e}")
```

`if let` bindings exist only inside the body. In v1, `if let` is intentionally single-arm only and does not accept `elif` or `else`.

## Pattern matching with `match`

Use `match` to branch on enum values like `Result` and `Option`.

```incan
def main() -> None:
    result = parse_port("8080")

    match result:
        case Ok(port): println(f"port={port}")
        case Err(e): println(f"error: {e}")
```

Use `match` when:

- both success and failure paths matter;
- more than one variant needs its own behavior;
- you want the full branching structure to stay visible.

## Looping while a pattern keeps matching with `while let`

Use `while let` when a loop should continue only while one pattern keeps matching.

```incan
async def drain(rx: Receiver[str]) -> None:
    while let Some(msg) = await rx.recv():
        println(f"Got: {msg}")
```

This replaces the more repetitive shape:

```incan
while True:
    match await rx.recv():
        case Some(msg): println(f"Got: {msg}")
        case None: break
```

Prefer `while let` when:

- each iteration destructures the same success case;
- the loop naturally ends on the first non-match;
- `while True` plus `match` plus `break` adds noise rather than meaning.

Like `if let`, names bound by the pattern exist only inside the successful body of that iteration.

## Looping with `for`

Incan supports Python-like `for` loops:

```incan
def main() -> None:
    items = ["Alice", "Bob", "Cara"]

    for name in items:
        println(name)
```

Break early when needed:

```incan
for name in items:
    if name == "Bob":
        break
```

## Looping with `while`

Use `while` when the loop condition should be checked before each iteration:

```incan
def countdown(start: int) -> None:
    mut current = start

    while current > 0:
        println(current)
        current -= 1
```

## Looping with `loop`

Use `loop:` for explicit infinite loops and for loops that produce a value with `break <expr>`.

```incan
def find_value(flag: bool) -> int:
    return loop:
        if flag:
            break 42
        break 7
```

`break <expr>` is only valid for `loop:`. Plain `break` remains valid for `for`, `while`, and `loop:`.

## See also

- Book chapter: [4. Control flow](../tutorials/book/04_control_flow.md)
- Enums and `match`: [Enums](enums.md)
- Error-driven control flow (`Result`/`Option`): [Error Handling](error_handling.md)
