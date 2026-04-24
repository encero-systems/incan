# Incan Code Formatting

The `incan fmt` command formats Incan source code following a consistent style inspired by
[Ruff](https://docs.astral.sh/ruff/) and [Black](https://black.readthedocs.io/).

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

!!! note "Repo formatting vs Incan formatting"
    - Use `incan fmt` to format Incan source files (`.incn`).
    - Use `make fmt` to format the Rust compiler/tooling code in this repository.

```bash
# Format a single file
incan fmt myfile.incn

# Format all .incn files in a directory
incan fmt src/

# Check if files need formatting (CI mode)
incan fmt --check .

# Show what would change without modifying
incan fmt --diff myfile.incn
```

!!! note "`--diff` output can look empty for EOF-only changes"
    If the only change is at end-of-file (like adding/removing a trailing newline), some diff viewers may not display an
    obvious change even though `incan fmt` would update the file.

## Style Guide

### Indentation

- **4 spaces** per indentation level
- No tabs

```incan
def calculate(x: int) -> int:
    if x > 0:
        return x * 2
    return 0
```

### Line Length

- **120 characters** target (best-effort, not strictly enforced)
- Long lines are wrapped after opening parentheses/brackets where possible
- Manual wrapping may be needed for very long expressions

### Blank Lines

- RFC 053 is the normative vertical-spacing contract for `incan fmt`: [RFC 053](../../RFCs/053_formatter_vertical_spacing_buckets.md)
- **Exactly 2 blank lines** around top-level `def`, `class`, `model`, `trait`, `enum`, `type`, `newtype`, and `rusttype` declarations
- **Exactly 1 blank line** before a following body-bearing member inside a type body
- **At most 1 blank line** everywhere else, including import runs, grouped constants/statics, statement blocks, and transitions involving module docstrings when no top-level spaced declaration is involved
- Stand-alone comments do **not** satisfy blank-line quotas by themselves; the formatter attaches them to nearby same-scope constructs as leading or trailing comment bundles

```incan
def first_function() -> None:
    pass


def second_function() -> None:
    pass


model User:
    name: str
    age: int

    def greet(self) -> str:
        return f"Hello, {self.name}"

    def is_adult(self) -> bool:
        return self.age >= 18
```

### Spacing

- **Spaces around binary operators**: `a + b`, not `a+b`
- **No space after function name**: `foo(x)`, not `foo (x)`
- **Space after comma**: `foo(a, b)`, not `foo(a,b)`
- **Space after colon in type annotations**: `x: int`, not `x:int`
- **No space around `=` in named arguments**: `User(name="Alice")`, not `User(name = "Alice")`

### Strings

- **Double quotes** preferred for strings
- Single quotes preserved if already used

### Trailing Commas

- Added in multi-line constructs for cleaner diffs

### Docstrings

- Single-line docstrings on one line: `"""Brief description"""`
- Multi-line docstrings with content on separate lines:
- Repeated empty lines inside the docstring payload collapse to **at most one** blank line

```incan
"""
This is a longer docstring.

It can span multiple lines.
"""
```

## CLI Options

| Option             | Description                                                 |
| ------------------ | ----------------------------------------------------------- |
| `incan fmt <path>` | Format file(s) in place                                     |
| `--check`          | Exit non-zero if files would be reformatted (useful for CI) |
| `--diff`           | Show what would change without modifying files              |

## Exit Codes

| Code | Meaning                                                   |
| ---- | --------------------------------------------------------- |
| 0    | Success (no changes needed, or formatting complete)       |
| 1    | Files need formatting (with `--check`) or errors occurred |

## CI Integration

Add to your CI pipeline to enforce consistent formatting:

```yaml
# GitHub Actions example
- name: Check formatting
  run: incan fmt --check .
```

## Configuration

Currently, formatting options use sensible defaults based on Ruff/Black conventions.
Configuration file support (e.g., `incan.toml`) is planned for future releases.

Default settings:

- Indent: 4 spaces
- Line length: 120 characters
- Quote style: Double quotes
- Trailing commas: Yes (in multi-line)

The formatter follows RFC 053's vertical-spacing buckets across both `incan fmt` and the library API. `FormatConfig` does not expose blank-line overrides.

## Limitations

### Parse-required

**The formatter requires valid syntax.**
Unlike some formatters that can handle partial/broken code, `incan fmt` operates on the parsed AST
and cannot format files with syntax errors.

If a file has errors, you'll see:

```bash
Error formatting myfile.incn: Parser error: [...]
```

Fix syntax errors before formatting.

### Line length is best-effort

The 120-character line length is a **target**, not a strict limit.
The formatter doesn't automatically wrap all long lines.
Some constructs (very long strings, complex expressions) may exceed the target and require manual formatting.

## Next Steps

- [Language Guide](../../language/index.md) - Learn Incan syntax and features
- [Examples](https://github.com/dannys-code-corner/incan/tree/main/examples) - Sample programs
- [testing](testing.md) - Test runner and fixtures
