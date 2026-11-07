# RFC 172: Async syntax without an import

- **Status:** In Progress
- **Created:** 2026-11-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (stdlib namespacing and compiler handoff)
    - RFC 039 (`race for`), whose import activation this RFC supersedes
    - RFC 142 (async consumers and structured concurrency)
    - RFC 168 (`std.primitives`)
- **Issue:** https://github.com/encero-systems/incan/issues/2683
- **RFC PR:** [#2685](https://github.com/encero-systems/incan/pull/2685)
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`async` and `await` become reserved words of the language, and `race for` becomes language syntax. None of them needs an import. Importing remains how a program reaches the async library: `spawn`, `sleep`, channels and the rest. A module named `async` stays importable and is used like any other module, so `std.async` keeps its name.

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
- `std.async` keeps its name and works like any other module.

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

A program can't declare either word: `async = 1`, `def async()` and a parameter named `await` are all refused. A module named `async` is different. Importing it binds it like any other module, so this works too:

```incan
from std import async


async def answer() -> int:
    await async.sleep(0.1)
    return 42
```

`race` stays an ordinary name, because only `race for <name>:` is syntax. `[race for race in races]` is still a comprehension.

## Reference-level explanation

- `async` and `await` must be reserved in every module, whatever it imports. A variable, parameter, field, function or other declaration named `async` or `await` must be refused with a diagnostic that says the word is reserved.
- `async` followed by `def` must parse as the async modifier in every module, and `await` must parse as the await expression.
- A reserved word may be a segment of a module path. A module named `async` may be bound by an import, and `async` followed by `.` must resolve through that binding as any other module binding does.
- `race for <name>:` must parse as the race expression in every module. `race` must remain an ordinary identifier, and `race for` followed by anything else must keep its ordinary meaning.
- Compiling a program that uses async syntax must select the async runtime from the checked use of that syntax, with no import required.
- A program that uses no async syntax and no async library, directly or through its dependencies, must not reach the async runtime.

## Design details

### Compatibility

Source that declares something named `async` or `await` no longer compiles. Python made the same change in 3.7. Incan has not reached 1.0, and renaming is mechanical. Existing async imports stay valid: an import that was only there to activate syntax becomes an unused import.

### Interaction with `std.primitives`

Whether the syntax needs a library is now separate from where the library lives, so neither `std.async` nor `std.primitives.async` is special to the parser.

## Alternatives considered

- **Contextual keywords, like `assert`.** `async` could stay usable as a declared name outside the positions where it has meaning. That keeps two meanings for one word in every program, for little benefit.
- **Keep import activation.** It preserves a rule whose reason has gone.
- **Refuse `async` as a module binding.** Nothing about parsing requires it, since `async` as the modifier is only ever followed by `def`. It would refuse `import std.async` and make `std.async` the one module that can't be called through its name.
- **Rename the module, as Python's `asyncio`.** That changes every existing import to avoid a conflict that doesn't exist.

## Drawbacks

- Two more words are unavailable as declared names.
- A reserved word that can still be a module binding needs one sentence to explain.

## Layers affected

- **Parser / AST**: `async` and `await` no longer check for an import, `async` is the modifier before `def` and a name before `.`, and `race for <name>:` no longer checks for an import.
- **Typechecker / Symbol resolution**: refuse the reserved words as declared names, and resolve a module named `async` like any other module binding.
- **Stdlib / Runtime**: runtime selection follows checked async use.
- **Formatter**: no new syntax.
- **LSP / Tooling**: completion offers the keywords in every module.

## Progress Checklist

- [x] Reserve `async` and `await` independently of imports and support `race for` without an activation import. The implementation is on `0.6.0-dev.9` ([#2685](https://github.com/encero-systems/incan/pull/2685)).
- [x] Verify reserved names, module access and existing programs. The implementation PR records its contract census, reserved-name refusal fixtures and example runs.
- [ ] Record the release containing these changes when it ships.

## Design Decisions

- `async` and `await` can't be declared, matching Python.
- A module named `async` is imported and called like any other module, so `std.async` keeps its name and `async.sleep(0.1)` works.
- `race` stays an identifier, and only `race for <name>:` is syntax.
