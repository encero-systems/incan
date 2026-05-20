# `std.regex` reference

`std.regex` provides compiled regular expressions, match spans, capture results, splitting, and replacement for ordinary Incan text-processing code. For task-oriented examples, see [Regular expressions](../../how-to/regular_expressions.md).

## Imports

```incan
from std.regex import Captures, Match, Regex, RegexError
```

## Engine boundary

The stdlib regex engine is the safe default: it follows the predictable Rust-regex/RE2-style model rather than a fully backtracking Python/PCRE-style model.

| Surface | Contract |
| --- | --- |
| Supported patterns | Literals, character classes, quantifiers, alternation, grouping, anchors, indexed captures, named captures, inline flags, and Unicode-aware matching by default. |
| Supported constructor flags | `ignore_case`, `multiline`, `dotall`, and `verbose`. |
| Unsupported patterns | Lookaround such as `(?=...)` or `(?<=...)`, pattern backreferences such as `\1`, and engine-specific features beyond the documented safe surface. |
| Span model | Match offsets are byte positions in the input text. |

## `Regex`

`Regex` is a compiled, reusable pattern. Construction validates the pattern and returns `Result[Regex, RegexError]`.

| Constructor argument | Default | Meaning |
| --- | --- | --- |
| `pattern: str` | required | Regular-expression pattern text. |
| `ignore_case: bool` | `false` | Match letters case-insensitively. |
| `multiline: bool` | `false` | Make `^` and `$` match line boundaries inside the input. |
| `dotall: bool` | `false` | Make `.` match newlines. |
| `verbose: bool` | `false` | Allow whitespace and comments in patterns according to the safe engine's verbose syntax. |

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

## `Match`

`Match` represents one match span.

| Method | Returns | Description |
| --- | --- | --- |
| `match.as_str()` | `str` | Matched text. |
| `match.start()` | `int` | Start byte offset. |
| `match.end()` | `int` | End byte offset. |
| `match.span()` | `tuple[int, int]` | Start and end byte offsets. |

## `Captures`

`Captures` represents one successful match plus its capture groups. Group `0` is always the full match. Numbered groups start at `1`, and named groups are looked up by name.

| Method | Returns | Description |
| --- | --- | --- |
| `captures.full_match()` | `Option[Match]` | The full match span as group `0`. |
| `captures.group(key: int \| str)` | `Option[str]` | One captured value by group index or name. |
| `captures.span(key: int \| str)` | `Option[tuple[int, int]]` | One captured span by group index or name. |
| `captures.groups()` | `list[Option[str]]` | Indexed capture values, excluding group `0`. |
| `captures.groupdict()` | `dict[str, Option[str]]` | Named capture values by group name. |

Unmatched optional groups are explicit `None` values. They are not coerced to empty strings in `group(...)`, `groups()`, `groupdict()`, or replacement callbacks.

## Replacement strings

Replacement strings support capture interpolation with `$1` for numbered captures and `${name}` for named captures. The literal replacement methods insert replacement text exactly as written instead of interpreting capture references.

## Errors

`RegexError` reports pattern compilation failures and other regex-contract errors.

| Method | Returns | Description |
| --- | --- | --- |
| `error.kind()` | `str` | Stable category such as `"compile_error"`. |
| `error.message()` | `str` | Human-readable engine diagnostic. |

Rejected pattern syntax returns a `RegexError`. Error text is diagnostic text; program logic should branch on `kind()` values when it needs a stable category.

## See also

- [Regular expressions](../../how-to/regular_expressions.md)
- [Strings and bytes](../strings.md)
- [Callable objects](../stdlib_traits/callable.md)
- [RFC 059: std.regex](../../../RFCs/closed/implemented/059_std_regex.md)
