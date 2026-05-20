# Incan Code Style Guide

This page is the canonical style guide for human-written `.incn` source files. It describes how Incan code is laid out, what spacing is expected, and which readability gaps are allowed.

Use `incan fmt` to apply these formatting rules to source files. For formatter command usage and limits, see [Formatting with `incan fmt`](../../tooling/how-to/formatting.md).

!!! note "Historical note"
    RFC 053 is the design record behind the current vertical-spacing model.

## Scope

This guide is about source layout and readability:

- indentation
- blank lines
- comments and docstrings
- horizontal spacing
- quoting and commas
- naming conventions
- line breaking for common constructs

## Principles

Incan formatting optimizes for a few stable outcomes:

- code reads as intentional prose, not as minimally legal syntax
- the same construct looks the same across projects
- authors may use a small amount of vertical whitespace for readability
- formatting rules stay simple enough that humans can follow them without memorizing formatter internals

!!! note "The Zen of Incan"
    --8<-- "_snippets/language/zen_of_incan.md"

    This guide turns the Zen's style philosophy into concrete formatting and naming rules.  
    You can print it with `incan run -c "import this"`.

## Required Baseline

### Indentation

- Use `4` spaces per indentation level.
- Do not use tabs.
- Indentation is semantic in Incan, so accidental indentation drift is a real syntax problem, not just a style issue.

```incan
def calculate(x: int) -> int:
    if x > 0:
        return x * 2
    return 0
```

### Line length

- Treat `120` characters as the target line length.
- This is a best-effort target, not an absolute hard cap.
- When a constructor call or ordinary call overflows, rewrite it vertically with one argument per line.
- When a class adopts many traits, use a parenthesized `with (...)` list with one trait per line.
- Some strings and complex nested expressions may still require manual judgment.

```incan
result = build_report(
    source_uri,
    output_uri,
    include_lineage=true,
    include_statistics=true,
)
```

```incan
pub class Buffer with (
    BinaryReader,
    BinaryRead[u8],
    BinaryWrite[u8],
):
    handle: Cursor[bytes]
```

### Naming

Use naming that matches the kind of thing being declared:

- functions, methods, local variables, parameters, imported module aliases, and `static` bindings use `lower_snake_case`
- `const` bindings use `SCREAMING_SNAKE_CASE`
- classes, models, traits, enums, type aliases, newtypes, rusttypes, and enum variants use `UpperCamelCase`
- module file names use `lower_snake_case`

```incan
const DEFAULT_PORT: int = 8080
static active_clients: int = 0

type UserId = str

model UserProfile:
    display_name: str

    def is_internal_user(self) -> bool:
        return self.display_name.ends_with("_internal")

enum JobStatus:
    Pending
    Running
    Finished
```

## Vertical Layout

Vertical spacing is the most opinionated part of the current contract. The rule is deliberately simple:

- `2` blank lines are reserved for specific top-level declaration boundaries
- `1` blank line is allowed for readability inside ordinary code
- more than `2` blank lines are never allowed

### Top-level declarations

Use exactly `2` blank lines around top-level body-bearing type-like and callable declarations:

- `def`
- `class`
- `model`
- `trait`
- `enum`
- `type`
- `newtype`
- `rusttype`

Top-level aliases are declaration syntax, but they are not body-bearing declarations. Keep them grouped tightly with nearby imports, constants, statics, or related declarations unless they border one of the body-bearing declarations above.

```incan
def parse_user(raw: str) -> User:
    ...


def store_user(user: User) -> None:
    ...
```

This double-spacing is a root-level rule. It is not a general license to scatter double blank lines throughout the file.

### Type bodies

Inside `model`, `class`, `trait`, `enum`, and similar type bodies:

- keep adjacent fields, variants, and abstract signatures tight
- insert exactly `1` blank line before a following body-bearing member

```incan
model User:
    id: UserId
    email: str

    def is_internal(self) -> bool:
        return self.email.ends_with("@example.com")
```

### Ordinary statement blocks

Inside function bodies, loops, `if` blocks, `match` arms, and other indented suites:

- `1` authored blank line is allowed and is preserved
- `2` or more consecutive blank lines are not allowed

This is where "code prose" matters. A single readability gap between logic groups is valid Incan style and survives formatting.

```incan
def register_source(session: Session, source: Source) -> Result[None, SessionError]:
    validate_source(source)?

    logical_name = source.logical_name()
    physical_uri = source.uri

    return session.register(logical_name, source)
```

### Imports, constants, and statics

Keep import runs and grouped `const` / `static` declarations tight unless they border a top-level declaration that requires the two-blank-line rule.

```incan
from std.io import File
from std.path import Path

const DEFAULT_PORT: int = 8080
static active_clients: int = 0


def main() -> None:
    ...
```

## Comments And Docstrings

### Stand-alone comments

Stand-alone comments are attached by scope and structure. They do not create extra blank-line entitlement on their own.

- a comment with no blank line before the next construct is treated as a leading comment for that construct
- a comment separated from the next construct by a blank line stays with the previous construct instead

```incan
type UserId = str

# Validate before any network call.
def load_user(id: UserId) -> User:
    ...
```

### Docstrings

Docstrings are part of the readable source contract, but their interior spacing is normalized.

- single-line docstrings stay on one line when they fit
- multi-line docstrings use opening and closing quotes on their own lines
- repeated empty-line runs inside the docstring payload collapse to at most `1` blank line

```incan
"""
Load the project manifest.

This docstring may contain one intentional blank line.
"""
```

Literal text such as `\n` inside strings is still just text. Normalization applies to actual blank lines, not slash characters.

## Horizontal Spacing

Use ordinary spaces to make expressions readable.

- put spaces around binary operators: `a + b`
- put a space after commas: `foo(a, b)`
- put a space after the colon in type annotations: `x: int`
- do not put a space between a callable name and `(`
- do not put spaces around `=` in named arguments

```incan
value = left + right
user = User(name="Alice", age=30)
```

Avoid these forms:

```incan
value=left+right
user = User (name = "Alice", age = 30)
```

## Strings, Quotes, And Commas

- Prefer double quotes for strings.
- Existing single quotes may be preserved, but double quotes are the house style.
- Use trailing commas in multi-line constructs.

```incan
payload = {
    "kind": "event",
    "source": "cli",
}
```

## Match Arms And Short Forms

Short single-statement `match` arms are allowed on one line when they stay readable.

```incan
match node.kind:
    PrismNodeKind.ReadNamedTable => return str("ReadNamedTable")
    PrismNodeKind.Filter => return str("Filter")
```

When an arm needs more space or more than one statement, use a block body.

```incan
match result:
    Ok(value) =>
        log_success(value)
        return value
    Err(err) =>
        return report_error(err)
```

Do not insert extra blank lines immediately after `=>` or a suite header unless you genuinely intend one readability gap inside that block.

## What The Formatter Should Preserve

`incan fmt` preserves legitimate authored structure rather than flattening everything into one style-less block. Today that specifically includes:

- one blank line between logic groups inside indented code
- one blank line between sibling statements after nested suites
- short inline `match` arms when they are still readable
- actual string contents, including literal `\n` text

## What The Formatter Should Normalize

`incan fmt` normalizes mechanical drift:

- tabs or inconsistent indentation
- repeated blank-line runs beyond the allowed buckets
- trailing blank lines at end-of-file
- inconsistent wrapping of overflowing calls and constructors
- comment placement that would otherwise detach comments from the same-scope construct they describe

Formatted files must end with exactly one trailing newline.

## Tooling

Use [Formatting with `incan fmt`](../../tooling/how-to/formatting.md) for command-line usage, CI integration, and formatter limitations.

## See Also

- [CLI reference](../../tooling/reference/cli_reference.md)
- [Formatting with `incan fmt`](../../tooling/how-to/formatting.md)
- [RFC 053](../../RFCs/closed/implemented/053_formatter_vertical_spacing_buckets.md)
