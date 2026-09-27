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

Use `if let` when you want to try a pattern match and do something only on success.

```incan
def print_primary_email(user: Option[User]) -> None:
    if let Some(u) = user:
        println(u.email)
```

This is the concise form of “match a successful shape, otherwise do nothing.” It is most useful with `Option`, `Result`, and enum payloads.

```incan
def log_port(raw: str) -> None:
    if let Ok(port) = parse_port(raw):
        println(f"listening on {port}")
```

When several successful patterns share the same body, join them with `|`:

```incan
enum JobStatus:
    Running
    Completed
    Cancelled

def log_terminal(status: JobStatus) -> None:
    if let Completed | Cancelled = status:
        println("done")
```

Prefer `if let` when:

- one success branch matters, whether it uses one pattern or a pattern alternation;
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

Use pattern alternation in a `match` arm when several patterns should run the same body:

```incan
enum PortLookup:
    Cached(int)
    Fresh(int)
    Failed(str)

match lookup_port(raw):
    case Cached(port) | Fresh(port): println(f"port={port}")
    case Failed(e): println(f"error: {e}")
```

Alternatives that bind names must bind the same names with the same types. `Cached(port) | Fresh(port)` is valid because both payloads have the same type; `Some(value) | None` is rejected because only one alternative binds `value`.

An alternative is any pattern the arm could use on its own, so literals nest inside alternatives the same way they nest inside a single pattern, and an alternation can sit inside a larger pattern:

```incan
def classify(pair: tuple[int, str]) -> str:
    match pair:
        (0, "a") | (1, "b") => return "known"    # (0, "a") and (1, "b") only; (0, "b") is "other"
        _ => return "other"

def describe_tag(pair: tuple[int, Option[str]]) -> str:
    match pair:
        (n, Some("a") | None) => return f"{n}: a or nothing"
        (n, _) => return f"{n}: something else"
```

Alternatives are tried in order, and the first one that matches binds the names. A guard after an alternation runs once, for that alternative: when it is false, the arm is skipped and matching continues with the next arm. Later alternatives of the same arm are not tried.

```incan
def first_match(pair: tuple[str, str]) -> str:
    match pair:
        (x, "a") | ("b", x) if x == "a" => return "a"   # ("b", "a"): the first alternative binds x = "b", the guard is false, the arm is skipped
        _ => return "other"                             # ("b", "a") returns "other"
```

### Guards, and the two ways to write an arm

Add `if <condition>` after a pattern when the pattern alone does not decide the arm. The guard runs only if the pattern matched, and the arm is taken only if the guard is also true; when it is false, matching continues with the next arm.

```incan
match lookup_port(raw):
    case Cached(port) if port > 1024: println(f"cached user port={port}")
    case Cached(port): println(f"cached reserved port={port}")
    case Fresh(port): println(f"fresh port={port}")
    case Failed(e): println(f"error: {e}")
```

An arm can be written two ways. `case <pattern>:` introduces a body as an indented suite or on the same line, and `<pattern> => <expression>` is the shorter form for an arm whose body is a single expression:

```incan
def classify(n: int) -> str:
    return match n:
        x if x < 0 => "negative"
        0 => "zero"
        _ => "positive"
```

These are two spellings of one arm, not two kinds of arm. Anything you can write in one you can write in the other, guards included; pick whichever reads better for the arm at hand. A guard sits between the pattern and the arm's `:` or `=>` in both.

A `match` over a number or a string needs an arm that matches any value, such as `_` or a name, because literal arms never cover every value there; the `_` arm in `classify` is that arm. The full coverage rules are in [Match patterns](../reference/match_patterns.md#coverage).

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

### Loops that take their items

A `for` loop over a list normally reads each item where it stays, and the list keeps its items. A task handle can be neither copied nor cloned, and awaiting it uses it up, so a loop that awaits the handles of a list has to take them out of it:

```incan
async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    for handle in handles:
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
```

The compiler takes a list's items when three things hold. The list is a local binding, the binding of an enclosing `for` loop that itself takes its list's items, or a parameter not marked `mut`. Each item is a handle, or a tuple or collection that contains one. And the loop body uses by value a loop binding that holds a handle: it awaits it, passes it as an argument to a function, method or constructor other than a builtin function, assigns it to another name, returns or yields it, breaks with it, or puts it in a new tuple, list or set, or as a value in a new dict. A binding that holds a list of handles, as in a list of lists of handles, is also used by value when a nested `for` loop over it takes its items, so each group is emptied in turn:

```incan
async def main() -> None:
    groups = [[spawn(work())], [spawn(work())]]
    for group in groups:
        for handle in group:
            match await handle:
                Ok(value) => println(value)
                Err(_) => println("join failed")
```

Once the loop has taken the items, the list is empty, so the compiler refuses whatever could still see it (`INCAN-T0119`):

- a read of the list inside or after the loop, until an assignment gives the name a new list on every path after the loop, with no branch, `break` or `continue` able to skip it;
- a closure that captured the list before the loop, since the closure could read it later;
- an enclosing loop that repeats the loop over a list defined outside it, unless each pass assigns the list a new one before the loop and before any read of the list in that pass.

Loops over any other item type keep reading their items in place.

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
- Pattern rules and coverage: [Match patterns](../reference/match_patterns.md)
- Error-driven control flow (`Result`/`Option`): [Error Handling](error_handling.md)
