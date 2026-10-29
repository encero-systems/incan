# vocab_scriptkit

Conformance example for script/type-shaped descriptor-gated embedded fragments (RFC 081, `#1023`).

Read the Incan consumer first. Its Incan vocabulary entry registers the `RegexTemplate`, `TypePosition` and `RawText` submodes; it does not claim JavaScript, TypeScript or other script-language compatibility.

The consumer writes:

```incan
def greeting(name: str) -> None:
    pattern:
        `hello ${name}!`

def slug_pattern() -> None:
    pattern:
        /^[a-z0-9-]+$/i

def response_shape() -> None:
    shape:
        scriptkit.Response<str>? | scriptkit.Error

def deploy_note(owner: str) -> None:
    note:
        TODO({owner}): rotate the signing key before <<release>>
```

The important parts are:

- `from pub::scriptkit ...` activates the vocab metadata shipped by the producer library.
- `pattern:`, `shape:` and `note:` are library-defined block keywords, not core Incan keywords.
- `` `hello ${name}!` `` is a template string: `${name}` is an expression hole that re-enters ordinary Incan
  parsing and typechecks `name` as the real `str` parameter it is.
- `/^[a-z0-9-]+$/i` is a bare regex literal: pattern plus flags, nothing else accepted in that position.
- `scriptkit.Response<str>? | scriptkit.Error` exercises every construct the `TypePosition` submode's
  representative grammar enumerates: a namespace-qualified name, a generic argument, nullable, and a union.
- `TODO({owner}): rotate the signing key before <<release>>` is raw text, kept exactly as written. `<<release>>`
  has no meaning to the compiler and is not reinterpreted; `{owner}` is still an expression hole, which is what
  distinguishes the `RawText` submode from an opaque string.
- Outside these block bodies, none of this is ordinary Incan syntax -- a bare `` `template` `` or `/regex/`
  literal is not valid Incan expression syntax anywhere else.

## What this proves, and what it does not

This example proves the parser-to-typechecker-to-lowering contract: each fragment parses through its own dedicated
submode grammar, the template string's expression hole typechecks as a real Incan expression, and the resulting
typed `EmbeddedFragmentExpr` artifacts reach Body IR successfully. It does **not** register a desugarer or
lowering hook. `incan check` proves its parser/typechecker contract. Running or building the consumer is refused with `unsupported source construct: embedded DSL fragment expression`. The consumer is listed in `examples/check-only.txt`; the examples runner checks it.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/loaf.toml` - declares `[vocab] entry`.
- `producer/src/vocab.incn` - registers the `pattern:`, `shape:` and `note:` fragment descriptors through `std.vocab`.
