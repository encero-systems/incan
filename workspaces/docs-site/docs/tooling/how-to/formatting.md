# Formatting With `incan fmt`

`incan fmt` is the formatter for Incan source files. It is the reference implementation of the canonical [Incan Code Style Guide](../../language/reference/code_style.md).

If you want the actual style rules, start with the style guide. This page is about the tool: how to run it, what it guarantees, and where its current limits are.

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
    If the only change is at end-of-file (like adding/removing a trailing newline), some diff viewers may not display an obvious change even though `incan fmt` would update the file.

## Relationship To The Style Guide

The style guide is the canonical source for what Incan code should look like:

- [Incan Code Style Guide](../../language/reference/code_style.md)

`incan fmt` should make valid source conform to that guide. In particular, the current formatter contract includes:

- `4`-space indentation
- a `120`-character line-length target
- wrapping for long class trait adoption headers into parenthesized one-trait-per-line `with (...)` lists
- best-effort wrapping for long parenthesized logical expression chains at `and` / `or` breakpoints
- leading-dot wrapping for overflowing fluent method-call chains
- top-level double-blank-line spacing only where the style guide permits it
- preservation of one authored readability gap inside ordinary code blocks
- comment placement that remains same-scope and structure-aware
- one trailing newline at end-of-file

RFC 053 remains the historical design record for the vertical-spacing portion of that contract.

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

Currently, formatting options use sensible defaults. Configuration file support (for example `incan.toml`) is planned for a future release.

Default settings:

- Indent: 4 spaces
- Line length: 120 characters
- Quote style: Double quotes
- Trailing commas: Yes (in multi-line)

The same formatting behavior applies through both `incan fmt` and the library formatter API. `FormatConfig` controls ordinary options such as indentation, line length, quote style, and trailing commas, but it does not currently expose blank-line or comment-placement overrides.

## Logical Expression Chains

When a parenthesized logical expression chain exceeds the configured line-length target, `incan fmt` may split the chain onto multiple lines with the `and` / `or` operators leading each continuation line:

```incan
return (
    item.kind_name == "filter"
    and item.predicate_kind_name == "bool_literal"
    and item.source_name == "rewritten_prism_node"
)
```

Short parenthesized logical expressions remain inline.

## Fluent Method Chains

When a method-call chain exceeds the configured line-length target, `incan fmt` may split the chain after the receiver and keep each continuation line led by the next method call. The same leading-dot layout is valid source syntax, so users can author fluent interfaces in the shape the formatter emits.

```incan
enriched = orders
    .with_column("region_norm", upper(trim(col("region"))))
    .with_column("status_norm", lower(trim(col("status"))))
```

Short method-call chains remain inline when they fit.

## Limitations

### Parse-required

**The formatter requires valid syntax.** Unlike some formatters that can tolerate partial or broken code, `incan fmt` works on parsed syntax and cannot format files with syntax errors.

If a file has errors, you'll see:

```bash
Error formatting myfile.incn: Parser error: [...]
```

Fix syntax errors before formatting.

### Line length is best-effort

The `120`-character line length is a target, not a strict hard limit. The formatter does not yet rewrite every possible overflowing construct. Very long strings and some nested expressions may still require manual judgment.

## Next Steps

- [Incan Code Style Guide](../../language/reference/code_style.md)
- [Language Guide](../../language/index.md)
- [Examples](https://github.com/dannys-code-corner/incan/tree/main/examples)
- [Testing](testing.md)
