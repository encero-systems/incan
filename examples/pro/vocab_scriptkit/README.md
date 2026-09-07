# vocab_scriptkit

Conformance example for script/type-shaped descriptor-gated embedded fragments (RFC 081, `#1023`).

This example is meant to be read from the Incan consumer surface first. The Rust companion crate exists to
describe the accepted `RegexTemplate` and `TypePosition` submode grammars to the compiler; it does not claim
JavaScript, TypeScript, or any other script-language compatibility.

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
```

The important parts are:

- `from pub::scriptkit ...` activates the vocab metadata shipped by the producer library.
- `pattern:` and `shape:` are library-defined block keywords, not core Incan keywords.
- `` `hello ${name}!` `` is a template string: `${name}` is an expression hole that re-enters ordinary Incan
  parsing and typechecks `name` as the real `str` parameter it is.
- `/^[a-z0-9-]+$/i` is a bare regex literal: pattern plus flags, nothing else accepted in that position.
- `scriptkit.Response<str>? | scriptkit.Error` exercises every construct the `TypePosition` submode's
  representative grammar enumerates: a namespace-qualified name, a generic argument, nullable, and a union.
- Outside these two block bodies, none of this is ordinary Incan syntax -- a bare `` `template` `` or `/regex/`
  literal is not valid Incan expression syntax anywhere else.

## What this proves, and what it does not

This example proves the parser-to-typechecker-to-lowering contract: each fragment parses through its own dedicated
submode grammar, the template string's expression hole typechecks as a real Incan expression, and the resulting
typed `EmbeddedFragmentExpr` artifacts reach Body IR successfully. It does **not** register a desugarer or
lowering hook, so it has no runtime meaning yet -- RFC 081 explicitly assigns that responsibility to the owning
DSL's own desugarer or lowering hook (`#1023`'s scope is the mechanism, not a concrete script runtime). Building
this consumer with `incan build` will therefore stop at Rust emission with a clear, explicit refusal (`cannot emit
Rust code for a descriptor-gated embedded fragment: no owning DSL lowering hook is registered for it yet`) rather
than silently emitting nothing or guessing at semantics. `incan check` against the consumer proves the
parser/typechecker contract without needing a lowering hook.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/incan.toml` - points the producer library at its vocab companion crate.
- `producer/vocab_companion/src/lib.rs` - registers the `pattern:`/`shape:` blocks and their embedded-fragment
  descriptors.
