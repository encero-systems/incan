# RFC 172: Async syntax without an import

- **Status:** Draft
- **Created:** 2026-11-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (stdlib namespacing and compiler handoff)
    - RFC 039 (`race for`), whose import activation this RFC supersedes
    - RFC 142 (async consumers and structured concurrency)
    - RFC 168 (`std.primitives`)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`async` and `await` become reserved words of the language, and `race for` becomes language syntax. None of them needs an import. Importing remains how a program reaches the async library: `spawn`, `sleep`, channels and the rest.

## Motivation

Async syntax activates when a module imports `std.async` or `std.primitives.async`. That rule came from a Cargo feature: importing the module was how a generated project got its async runtime. Native compilation has no generated project, and a program's runtime dependencies follow from what it uses, so the reason for the rule has gone.

What's left is a surprise for authors. This module defines a coroutine and awaits nothing from any library, yet it fails until an unrelated import appears:

```incan
async def answer() -> int:
    return 42
```

Python separates these concerns. `async def` and `await` are reserved words since Python 3.7 and need no import. `asyncio` is imported only for the library that runs and coordinates coroutines.

## Goals

- `async` and `await` are always reserved.
- `race for` parses without an import.
- A program that uses async syntax gets the runtime it needs because it uses it, not because of an import.

## Non-Goals

- Changing what `async`, `await` or `race for` mean.
- Making `std.async` names ambient. `spawn`, `sleep` and channels are still imported.
- Changing how library vocabulary activates. A package's own keywords still activate on import.

## Guide-level explanation

```incan
from std.async import sleep


async def answer() -> int:
    await sleep(0.1)
    return 42
```

The import is there for `sleep`. Without the `sleep` call, the module needs no import at all.

`async` and `await` can't name a variable, a parameter or any other declaration. `race` still can, because only the two words `race for` form the syntax.

## Reference-level explanation

- `async` and `await` must be hard keywords in every module, whatever it imports.
- `race for` must parse as the race expression in every module. `race` alone must remain an ordinary identifier.
- Using `async` or `await` as a name must be refused with a diagnostic that says the word is reserved.
- Compiling a module that uses async syntax must select the async runtime from the checked use of that syntax, with no import required.
- A module that uses no async syntax and imports no async library must not reach the async runtime.

## Design details

### Compatibility

Source that names something `async` or `await` no longer compiles. Python made the same change in 3.7. Incan has not reached 1.0, and renaming is mechanical. Existing async imports stay valid: an import that was only there to activate syntax becomes an unused import.

### Interaction with `std.primitives`

Whether the syntax needs a library is now separate from where the library lives, so neither `std.async` nor `std.primitives.async` is special to the parser.

## Alternatives considered

- **Contextual keywords, like `assert`.** `async` could stay usable as a name outside the positions where it has meaning. That keeps two meanings for one word and a more complicated parser, for little benefit.
- **Keep import activation.** It preserves a rule whose reason has gone.

## Drawbacks

- Two more words are unavailable as names.

## Layers affected

- **Parser / AST**: `async` and `await` become hard keywords, and `race for` no longer checks for an import.
- **Typechecker / Symbol resolution**: refuse the reserved words as names.
- **Stdlib / Runtime**: runtime selection follows checked async use.
- **Formatter**: no new syntax.
- **LSP / Tooling**: completion offers the keywords in every module.

## Design Decisions

- `async` and `await` are reserved rather than contextual, matching Python.
- `race` stays an identifier, and only `race for` is syntax.
