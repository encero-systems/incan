# RFC 170: Loaf-visible declarations with `pub(loaf)`

- **Status:** Draft
- **Created:** 2026-11-02
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 031 (library system)
    - RFC 114 (compiled providers, SDK components and package features)
    - RFC 117 (`loaf.toml` and the Oven project model)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Add a third visibility level, `pub(loaf)`, between module-private and public. A `pub(loaf)` declaration is visible to every module of the Loaf that declares it and is never published to that Loaf's consumers. Modules inside one Loaf can then share helpers, types and fields without making them part of the Loaf's public API.

## Motivation

Incan has two visibility levels. A declaration without `pub` is private to its module, and a `pub` declaration is exported. A Loaf publishes its modules under `pub::package`, following its source files, so every `pub` declaration in every module is reachable by the Loaf's consumers. When two modules of one Loaf need to share a helper, the helper has to be `pub`, and it becomes public API:

- consumers can import it and depend on it, so changing it is a breaking change;
- it is published in the Loaf's library metadata, which every consumer reads;
- documentation and editor completion present it as part of the Loaf's surface.

Larger Loaves, the compiler's own among them, carry many helpers that are `pub` only so that a sibling module can call them. Rust solves the same problem with `pub(crate)`. Incan has no equivalent.

## Goals

- One visibility level for declarations that are shared within a Loaf and hidden from its consumers.
- The same spelling on every declaration that accepts `pub` today: functions, models, classes, traits, enums, newtypes, type aliases, constants, statics, fields and re-exports.
- A rule that public API never exposes a Loaf-visible type.

## Non-Goals

- Rust's `pub(super)` and `pub(in path)`. One additional level covers the common case; path-scoped visibility adds rules that see little use.
- Method visibility. Methods take no visibility modifier today; whether they gain one is a separate question.
- Visibility between Loaves, such as friend Loaves.
- Any change to what an unmarked or a `pub` declaration means.

## Guide-level explanation

Mark a declaration `pub(loaf)` when other modules of your Loaf need it and your consumers should not see it:

```incan
# src/format/number.incn
pub(loaf) def pad_left(text: str, width: int) -> str:
    return " " * (width - len(text)) + text
```

```incan
# src/report.incn
from format.number import pad_left      # accepted: the same Loaf

pub def render(total: int) -> str:
    return pad_left(str(total), 8)
```

A consumer of the Loaf sees `render`, but not `pad_left`:

```incan
from pub::reports import render                     # accepted
from pub::reports.format.number import pad_left     # refused: pad_left is visible only inside its Loaf
```

A field can be Loaf-visible on a public type. Code in the Loaf reads and writes it; consumers treat it like a field without `pub`:

```incan
pub model Invoice:
    pub number: str
    pub(loaf) ledger_key: str
```

A re-export can share a name within the Loaf without publishing it:

```incan
# src/text.incn
pub(loaf) from format.number import pad_left
```

A public declaration cannot expose a Loaf-visible type. If `Ledger` is `pub(loaf)`, then `pub def open_ledger() -> Ledger` is refused, because consumers could receive a value of a type they cannot name.

## Reference-level explanation

1. **Syntax.** `pub(loaf)` may appear wherever `pub` may appear. `loaf` is contextual inside the parentheses and is not a reserved word. Any other word inside the parentheses is a syntax error.
2. **Within the Loaf.** A module exports a `pub(loaf)` declaration to the other modules of its Loaf exactly as it exports a `pub` declaration: `from M import X` and `import M::X` bind it in any module of the same Loaf.
3. **Outside the Loaf.** A `pub(loaf)` declaration must not be published. It must not appear in the Loaf's `pub::` namespaces, in the public exports of its library metadata, or in its generated API documentation. A consumer's import of it must be refused with `INCAN-I0001`, and the diagnostic should say that the declaration is visible only inside its Loaf.
4. **No widening.** `pub from M import X` must be refused when `X` is `pub(loaf)`. `pub(loaf) from M import X` re-exports `X` within the Loaf, and may narrow a `pub` declaration.
5. **Public signatures.** The parameter types, result type, field types, type-parameter bounds and supertraits of a `pub` declaration, and the target of a `pub` type alias, must not name a `pub(loaf)` type. Such a declaration must be refused with a diagnostic that names the Loaf-visible type.
6. **Fields.** Within the declaring Loaf, a `pub(loaf)` field follows the rules for a `pub` field. Outside it, the field follows the rules for a field without `pub`, including construction, patterns and runtime field reflection.
7. **Bodies compiled by a consumer.** A function with type parameters and a published default expression may use `pub(loaf)` declarations of their own Loaf. Such a use must remain valid when the consumer compiles that body, even though the consumer's source cannot name the declaration.

## Design details

**Spelling.** `pub(loaf)` parallels Rust's `pub(crate)` and keeps one visibility syntax. `loaf` names Incan's unit of compilation and distribution, so it cannot be confused with the `pub::package` namespace a consumer imports through, which is exactly what a `pub(loaf)` declaration is hidden from.

**Inline tests.** An inline test block belongs to its module and sees that module's declarations, `pub(loaf)` included.

**Compatibility.** The change is additive; existing code keeps its meaning. Narrowing a published `pub` declaration to `pub(loaf)` is a breaking change for consumers, like removing it.

## Alternatives considered

- **`pub(package)`.** "Package" also names the `pub::package` namespace, which is the scope these declarations are hidden from.
- **An `internal` keyword**, as in Kotlin and Swift. It adds a reserved word; a Rust-shaped `pub(...)` keeps a single visibility syntax.
- **A leading-underscore convention**, as in Python. Nothing enforces it, so consumers and tools can still depend on the name.
- **Rust's full set, including `pub(super)` and `pub(in path)`.** See Non-Goals.
- **Listing exports in the manifest.** RFC 031 rejected this; visibility stays on the declaration.

## Drawbacks

- A third visibility level to learn.
- Narrowing existing `pub` declarations is a breaking change for consumers that used them, so adopting `pub(loaf)` in published Loaves needs a release decision per declaration.

## Layers affected

- **Parser / AST**: the `pub(loaf)` modifier on declarations, fields and re-exports.
- **Typechecker / Symbol resolution**: Loaf-scoped exports, refused consumer imports, the widening rule and the public-signature rule.
- **Library publication and metadata**: Loaf-visible declarations stay out of public exports, while bodies compiled by consumers keep resolving the declarations they use.
- **Emission**: Loaf-visible items stay reachable from consumer-compiled bodies that use them.
- **Formatter**: the new modifier.
- **LSP / Tooling**: completion, hover and API documentation respect Loaf visibility.

## Unresolved questions

- Do the test modules that `incan test` runs for a Loaf count as modules of that Loaf, so that they can use its `pub(loaf)` declarations?
- Should methods gain visibility modifiers, `pub(loaf)` among them, given that they take none today?
- Should a public signature that names a module-private type be refused as well, for consistency with rule 5?
- Should Loaves developed together in one workspace see each other's `pub(loaf)` declarations, or does visibility stop at each Loaf?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
