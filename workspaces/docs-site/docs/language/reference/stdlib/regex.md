# `std.regex`

`std.regex` provides compiled regular expressions, match spans, capture results, splitting, and replacement for ordinary
Incan text-processing code.

The stdlib regex engine is intentionally the safe default: it follows the predictable Rust-regex/RE2-style model rather
than a fully backtracking Python/PCRE-style model. Use it for validation, extraction, cleanup, log processing, and other
large-text workflows where regex should not introduce catastrophic backtracking risk. Lookaround, backreferences inside
patterns, and other features that require backtracking semantics are not part of `std.regex`.

## Imports

```incan
from std.regex import Captures, Match, Regex, RegexError
```

## Engine Boundary

The core pattern surface supports literals, character classes, quantifiers, alternation, grouping, anchors, indexed
captures, named captures, inline flags, and Unicode-aware matching by default.

The safe-default boundary is part of the Incan contract, not an accidental backend detail:

- Supported: ordinary regular-expression matching over `str`, including named and indexed capture groups.
- Supported: inline flags and constructor flags for ignore-case, multiline, dotall, and verbose modes.
- Not supported: lookaround such as `(?=...)` or `(?<=...)`.
- Not supported: backreferences inside the pattern such as `\1` as a matching constraint.
- Not promised: engine-specific features beyond the documented safe surface.

Use literal string helpers such as `split`, `replace`, and `contains` for fixed text. Use `std.regex` when the pattern
itself is the program contract.

## Types

### `Regex`

`Regex` is a compiled, reusable pattern. Construction validates the pattern and returns a `Result`.

```incan
pattern = Regex("^v(?P<major>\\d+)\\.(?P<minor>\\d+)$")?
```

Constructor flags keep configuration separate from the pattern text:

```incan
case_insensitive = Regex("error: (?P<code>\\w+)", ignore_case=true)?
line_start = Regex("^warning:", multiline=true)?
```

Supported constructor flags:

| Flag | Default | Effect |
| --- | --- | --- |
| `ignore_case` | `false` | Match letters case-insensitively. |
| `multiline` | `false` | Make `^` and `$` match line boundaries inside the input. |
| `dotall` | `false` | Make `.` match newlines. |
| `verbose` | `false` | Allow whitespace and comments in patterns according to the safe engine's verbose syntax. |

Inline flags remain valid when the pattern needs to travel as a self-contained literal:

```incan
labels = Regex("(?im)^warning: (?P<body>.+)$")?
```

Methods:

| Method | Returns | Description |
| --- | --- | --- |
| `regex.is_match(text: str)` | `bool` | Whether the pattern matches anywhere in `text`. |
| `regex.find(text: str)` | `Option[Match]` | First non-overlapping match span. |
| `regex.find_iter(text: str)` | `Iterator[Match]` | All non-overlapping match spans, left to right. |
| `regex.captures(text: str)` | `Option[Captures]` | Captures for the first match. |
| `regex.captures_iter(text: str)` | `Iterator[Captures]` | Captures for each non-overlapping match, left to right. |
| `regex.full_match(text: str)` | `Option[Captures]` | Captures only when the entire input matches. |
| `regex.split(text: str)` | `Iterator[str]` | Split around all non-overlapping matches. |
| `regex.splitn(text: str, limit: int)` | `Iterator[str]` | Split around at most `limit` matches. |
| `regex.replace(text: str, repl: str \| Callable[Captures, str])` | `str` | Replace the first match. |
| `regex.replace_all(text: str, repl: str \| Callable[Captures, str])` | `str` | Replace every non-overlapping match. |
| `regex.replacen(text: str, limit: int, repl: str \| Callable[Captures, str])` | `str` | Replace at most `limit` matches. |
| `regex.replace_literal(text: str, repl: str)` | `str` | Replace the first match without capture interpolation. |
| `regex.replace_all_literal(text: str, repl: str)` | `str` | Replace every match without capture interpolation. |
| `regex.replacen_literal(text: str, limit: int, repl: str)` | `str` | Replace at most `limit` matches without capture interpolation. |

### `Match`

`Match` represents one match span.

| Method | Returns | Description |
| --- | --- | --- |
| `match.as_str()` | `str` | Matched text. |
| `match.start()` | `int` | Start offset. |
| `match.end()` | `int` | End offset. |
| `match.span()` | `tuple[int, int]` | Start and end offsets. |

Offsets are byte positions in the input text, matching the safe engine's span model. Use them with APIs that document the
same offset model; do not assume they can be reused unchanged against a separate encoded representation.

### `Captures`

`Captures` represents one successful match plus its capture groups. Group `0` is always the full match. Numbered groups
start at `1`, and named groups are looked up by name.

| Method | Returns | Description |
| --- | --- | --- |
| `captures.full_match()` | `Option[Match]` | The full match span as group `0`. |
| `captures.group(key: int \| str)` | `Option[str]` | One captured value by group index or name. |
| `captures.span(key: int \| str)` | `Option[tuple[int, int]]` | One captured span by group index or name. |
| `captures.groups()` | `list[Option[str]]` | Indexed capture values, excluding group `0`. |
| `captures.groupdict()` | `dict[str, Option[str]]` | Named capture values by group name. |

Unmatched optional groups are explicit `None` values. They are not coerced to empty strings in `group(...)`,
`groups()`, `groupdict()`, or replacement callbacks.

```incan
from std.regex import Regex, RegexError


def main() -> Result[None, RegexError]:
    release = Regex("^v(?P<major>\\d+)\\.(?P<minor>\\d+)(?:\\.(?P<patch>\\d+))?$")?
    caps = release.full_match("v0.3")

    match caps:
        Some(version) =>
            assert version.group("major") == Some("0")
            assert version.group("minor") == Some("3")
            assert version.group("patch") == None
        None =>
            println("not a release tag")

    return Ok(None)
```

## Searching And Scanning

Use `is_match(...)` when only the boolean matters, `find(...)` / `find_iter(...)` when spans matter, and
`captures(...)` / `captures_iter(...)` when capture groups matter.

```incan
from std.regex import Regex, RegexError


def main() -> Result[None, RegexError]:
    word = Regex("\\w+")?

    for item in word.find_iter("alpha beta"):
        println(f"{item.start()}:{item.end()} {item.as_str()}")

    return Ok(None)
```

`find_iter(...)` and `captures_iter(...)` scan left to right and return non-overlapping results.

## Splitting

Regex splitting is for pattern separators rather than fixed separators.

```incan
from std.regex import Regex, RegexError


def main() -> Result[None, RegexError]:
    separator = Regex("\\s*,\\s*")?
    fields = separator.split("name, email, active").collect()

    assert fields == ["name", "email", "active"]
    return Ok(None)
```

Use `splitn(...)` when the rest of the string should remain intact after a fixed number of separator matches:

```incan
header = Regex("\\s*:\\s*")?
parts = header.splitn("content-type: text/plain; charset=utf-8", 1).collect()
```

## Replacement

Replacement strings support capture interpolation with `$1` for numbered captures and `${name}` for named captures.

```incan
from std.regex import Regex, RegexError


def main() -> Result[None, RegexError]:
    slug = Regex("[^A-Za-z0-9]+")?
    clean = slug.replace_all("Incan 0.3 regex", "-")

    version = Regex("v(?P<major>\\d+)\\.(?P<minor>\\d+)")?
    normalized = version.replace_all("v0.3", "major=${major}, minor=$2")

    assert clean == "Incan-0-3-regex"
    assert normalized == "major=0, minor=3"
    return Ok(None)
```

Use a callable replacement when the replacement depends on code instead of interpolation text. The callable receives
`Captures` for the current match and returns the replacement string.

```incan
from std.regex import Captures, Regex, RegexError


def reverse_name(caps: Captures) -> str:
    first = caps.group("first").unwrap_or("")
    last = caps.group("last").unwrap_or("")
    return f"{last}, {first}"


def main() -> Result[None, RegexError]:
    name = Regex("(?P<first>\\w+)\\s+(?P<last>\\w+)")?
    out = name.replace_all("Ada Lovelace", reverse_name)

    assert out == "Lovelace, Ada"
    return Ok(None)
```

Use the `replace_literal(...)`, `replace_all_literal(...)`, and `replacen_literal(...)` methods when a replacement string
must be inserted exactly as written instead of interpreting `$1` or `${name}` as capture references.

## Errors

`RegexError` reports pattern compilation failures and other regex-contract errors.

| Method | Returns | Description |
| --- | --- | --- |
| `error.kind()` | `str` | Stable category such as `"compile_error"`. |
| `error.message()` | `str` | Human-readable engine diagnostic. |

Rejected pattern syntax returns a `RegexError`. Error text is diagnostic text; program logic should branch on
`kind()` values when it needs a stable category.

## See Also

- [Strings and bytes](../strings.md)
- [Callable objects](../stdlib_traits/callable.md)
- [RFC 059: std.regex](../../../RFCs/closed/implemented/059_std_regex.md)
