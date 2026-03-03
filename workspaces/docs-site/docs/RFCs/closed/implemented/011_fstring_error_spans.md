# RFC 011: Precise Error Spans in F-Strings

- **Status**: Implemented
- **Author(s)**: Danny Meijer (@dannymeijer)
- **Issue**: #71
- **RFC PR**: —
- **Created**: 2021-12-17
- **Target version**: 0.2

## Summary

Improve error messages for f-string interpolation expressions to point to the specific `{expr}` that caused the error,
rather than the entire f-string.

## Motivation

Currently, when an error occurs in an f-string expression like:

```incan
println(f"[Producer {id}] Sending message {i}")
```

The error points to the entire f-string:

```bash
type error: Unknown symbol 'i'
  --> file.incn:16:17
     |
  16 |         println(f"[Producer {id}] Sending message {i}")
     |                 ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

Ideally, it should point directly to `{i}`:

```bash
type error: Unknown symbol 'i'
  --> file.incn:16:49
     |
  16 |         println(f"[Producer {id}] Sending message {i}")
     |                                                   ^^^
```

## Current Implementation

The lexer stores f-string parts without position information:

```rust
pub enum FStringPart {
    Literal(String),
    Expr(String),  // Just the expression text, no offset
}
```

The parser uses the f-string token's span for all expression parts.

## Proposed Changes

### 1. Lexer Changes

Add offset tracking to `FStringPart`:

```rust
pub enum FStringPart {
    Literal(String),
    Expr {
        text: String,
        offset: usize,  // Absolute byte offset of the opening `{` in source
    },
}
```

Update `scan_fstring()` to track the absolute opening-brace offset for each interpolation expression.

### 2. Parser Changes

Update `convert_fstring_parts()` to compute precise spans:

```rust
fn convert_fstring_parts(&self, parts: &[LexFStringPart]) -> Vec<FStringPart> {
    parts.iter().map(|p| match p {
        LexFStringPart::Expr { text, offset } => {
            let start = *offset;
            let end = start + text.len() + 2; // +2 for braces
            FStringPart::Expr(Spanned::new(expr, Span::new(start, end)))
        }
        // ...
    })
}
```

## Complexity

~40 lines of changes across:

- `crates/incan_syntax/src/lexer/strings.rs` - Track offsets in `scan_fstring()`
- `crates/incan_syntax/src/ast.rs` - Update `FStringPart` (if needed)
- `crates/incan_syntax/src/parser.rs` - Compute precise spans

## Priority

Low - The current implementation (pointing to the f-string) is already usable. This is a polish improvement.

## References

- Issue discovered while testing various examples
- Current fix: Use f-string token span instead of `Span::default()`
