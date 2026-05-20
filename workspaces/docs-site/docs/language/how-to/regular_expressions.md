# Regular expressions

Use `std.regex` when the pattern itself is the program contract. Use literal string helpers such as `split`, `replace`, and `contains` for fixed text.

## Match a whole input

Use `full_match(...)` when partial matches should not be accepted:

```incan
from std.regex import Regex, RegexError

def parse_release_tag(text: str) -> Result[Option[str], RegexError]:
    release = Regex("^v(?P<major>\\d+)\\.(?P<minor>\\d+)(?:\\.(?P<patch>\\d+))?$")?
    caps = release.full_match(text)

    match caps:
        Some(version) =>
            major = version.group("major").unwrap_or("")
            minor = version.group("minor").unwrap_or("")
            patch = version.group("patch").unwrap_or("0")
            return Ok(Some(f"{major}.{minor}.{patch}"))
        None => return Ok(None)
```

Unmatched optional groups are `None`, so callers can distinguish missing captures from captures that matched an empty string.

## Scan for repeated matches

Use `is_match(...)` when only the boolean matters, `find(...)` / `find_iter(...)` when spans matter, and `captures(...)` / `captures_iter(...)` when capture groups matter.

```incan
from std.regex import Regex, RegexError

def print_words(text: str) -> Result[None, RegexError]:
    word = Regex("\\w+")?

    for item in word.find_iter(text):
        println(f"{item.start()}:{item.end()} {item.as_str()}")

    return Ok(None)
```

`find_iter(...)` and `captures_iter(...)` scan left to right and return non-overlapping results.

## Split on pattern separators

Regex splitting is for separators described by a pattern rather than a fixed literal:

```incan
from std.regex import Regex, RegexError

def parse_csv_like_header(text: str) -> Result[list[str], RegexError]:
    separator = Regex("\\s*,\\s*")?
    return Ok(separator.split(text).collect())
```

Use `splitn(...)` when the rest of the string should remain intact after a fixed number of separator matches:

```incan
from std.regex import Regex, RegexError

def split_header(text: str) -> Result[list[str], RegexError]:
    header = Regex("\\s*:\\s*")?
    return Ok(header.splitn(text, 1).collect())
```

## Replace with captures

Replacement strings support capture interpolation with `$1` for numbered captures and `${name}` for named captures:

```incan
from std.regex import Regex, RegexError

def normalize_version(text: str) -> Result[str, RegexError]:
    version = Regex("v(?P<major>\\d+)\\.(?P<minor>\\d+)")?
    return Ok(version.replace_all(text, "major=${major}, minor=$2"))
```

Use the literal replacement methods when `$1` or `${name}` must be inserted exactly as written.

## Replace with code

Use a callable replacement when the replacement depends on code instead of interpolation text. The callable receives `Captures` for the current match and returns the replacement string.

```incan
from std.regex import Captures, Regex, RegexError

def reverse_name(caps: Captures) -> str:
    first = caps.group("first").unwrap_or("")
    last = caps.group("last").unwrap_or("")
    return f"{last}, {first}"

def normalize_names(text: str) -> Result[str, RegexError]:
    name = Regex("(?P<first>\\w+)\\s+(?P<last>\\w+)")?
    return Ok(name.replace_all(text, reverse_name))
```

## Keep the safe-engine boundary visible

`std.regex` does not support lookaround or pattern backreferences. If a pattern depends on those features, choose a different parsing strategy or a package that explicitly opts into backtracking semantics. Do not hide that choice behind a pattern that happens to work in one engine.

## See also

- [`std.regex` reference](../reference/stdlib/regex.md)
- [Strings and bytes](../reference/strings.md)
- [Callable objects](../reference/stdlib_traits/callable.md)
