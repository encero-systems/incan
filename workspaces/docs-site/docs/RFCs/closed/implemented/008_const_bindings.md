# RFC 008: Const Bindings

- **Status:** Implemented
- **Created:** 2025-12-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 000 (core language)
- **Issue:** —
- **RFC PR:** [#12](https://github.com/encero-systems/incan/pull/12)
- **Written against:** v0.1
- **Shipped in:** v0.1

## Summary

Introduce explicit constant bindings in Incan using the industry-standard declaration form:

```incan
const PI: float = 3.14
const HOST = "127.0.0.1"  # type may be inferred when obvious
```

The goal is to let users declare compile-time constants with clear intent and to allow codegen to emit Rust `const` where possible.

## Goals

- Provide a clear, conventional syntax (`const NAME [: Type] = <const-expr>`).
- Enforce const-evaluable expressions at compile time (reject runtime-dependent values).
- Generate Rust `const` bindings when allowed; fall back to errors rather than silently downgrading.
- Keep scope simple initially (module-level constants).

## Non-Goals (initial)

- Block-level `const` semantics (can be added later).
- `static`/`static mut` support.
- Inline const expressions (e.g., `const_len(...)`) or const generics.
- Complex const-eval (function calls, IO, random, runtime-derived values).

## Syntax

Module-level bindings:

```incan
const PI: float = 3.14159
const APP_NAME: str = "incan"
const MAX_RETRIES: int = 5

# With type inference when the literal is unambiguous
const DEFAULT_PORT = 8080
```

Rules (initial):

- Required initializer.
- Optional type annotation; if omitted, the type must be inferrable from the literal.
- Only allowed at module scope (phase 1).

## Const-Evaluable Expressions (phase 1)

Allowed:

- Literals: int, float, bool, str, bytes.
- Simple unary/binary ops on const literals: `+`, `-`, `*`, `/`, `%`, `&&/||`, comparisons, string concatenation (`"a" + "b"`).
- Tuple/list/dict/set literals whose elements/keys/values are themselves const-evaluable.

Disallowed (initial):

- Function/method calls (including builtins).
- Comprehensions, ranges, f-strings.
- Accessing variables, environment, or runtime state.
- Imports resolved at runtime.

If an expression is not const-evaluable, the compiler should emit a clear error at the binding site.

## Codegen

- Emit Rust `const` for accepted const bindings.
- For string/bytes literals, emit as `&'static` where appropriate; otherwise use `const` with owned types that are `const`-constructible.
- If the expression cannot be represented as a Rust `const`, fail the build with an actionable error (no silent fallback to `let`).

## Typechecker

- Enforce const-evaluable expression rules.
- Ensure the inferred/annotated type matches the initializer.
- Restrict scope to module-level for phase 1.

## Future Work (not in scope here)

- Block-level `const`.
- `static` (single-instance data) and safety rules.
- Inline const expressions / const-eval helpers.
- Allowing a limited set of pure builtins in const context (e.g., length of const literals).

## Examples

```incan
const VERSION: str = "0.1.0"
const DEFAULT_TIMEOUT_SECS: float = 2.5
const GREETING = "hello" + " world"
const PORTS = [80, 443]
const HEADERS = {"User-Agent": "incan"}
```

Invalid (should error):

```incan
const NOW = time.now()            # function call
const HOST = env("HOST")          # runtime dependency
const STEP = 1 + x                # depends on non-const symbol
```

## Checklist

- [x] Lexer/parser support for `const` binding declarations.
- [x] AST node for const bindings (module scope).
- [x] Typechecker: const-eval rules and errors for non-const expressions.
- [x] Codegen: emit Rust `const`; error on non-representable expressions.
- [x] Docs: guide section + examples.
- [x] Tests: parser/typechecker/codegen coverage.
