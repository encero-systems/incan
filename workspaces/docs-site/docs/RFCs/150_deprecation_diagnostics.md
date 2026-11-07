# RFC 150: Deprecation diagnostics

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (symbol aliases)
    - RFC 036 (declaration decorators)
    - RFC 082 (checked API documentation)
    - RFC 085 (field metadata)
    - RFC 096 (declaration metadata blocks; the nested `deprecated { ... }` value)
    - RFC 120 (canonical source identity)
- **Issue:** [#1288](https://github.com/encero-systems/incan/issues/1288)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose compiler-visible deprecation metadata for declarations and fields: optional since and planned retirement text, checked replacement identity, migration message and typed diagnostic severity. Editor guidance, compiler use diagnostics, bounded runtime warnings and generated documentation consume the same checked facts. Replacements remain guidance rather than an automatic rewrite, and version text does not remove code automatically. A field carries the same facts in a nested `deprecated { ... }` metadata block, and the compact `deprecated=true` and `deprecated="message"` spellings fill the same descriptor. A member that used to be public and has been made private carries the same facts as `made_private`, so the refusal at a former public use explains what replaces it.

## Motivation

A replacement API can exist for months while callers continue using an older name because the migration is recorded only in a changelog. A declaration-carried diagnostic connects the use directly to the reason and replacement guidance without requiring the compiler to guess a rewrite.

## Goals

- Warn at an Incan source location that refers to a deprecated declaration.
- Preserve the information through imports and compiled library metadata.
- Make editor and documentation explanations agree with compiler diagnostics.
- Distinguish human guidance from a checked, safe automatic replacement.

## Non-Goals

- Automatically removing declarations or changing their application semantics; the explicitly selected runtime warning is observability, not substitution.
- Treating version text as an automatic removal deadline.
- Applying unchecked textual replacement as an editor fix.
- Introducing a second field-metadata authority.

## Guide-level explanation

The decorator is proposed. Both functions below are application-defined and fully specified.

```incan
pub def checked_limit(value: int) -> Result[int, str]:
    if value < 0:
        return Err("limit must be non-negative")
    return Ok(value)


@deprecated(since="0.7", replacement=checked_limit, message="use checked_limit and handle its Result", by="cloud audit finding", issue="#1042")
pub def unchecked_limit(value: int) -> int:
    return value
```

A different module calling `unchecked_limit(-1)` receives a warning at that call, with the declaration location and message. The program still calls the old function. A compiler must not rewrite it to `checked_limit(-1)`: that would change the result type and introduce a new handling obligation.

Compiler use diagnostics inside the defining module remain quiet so the library can migrate its own implementation incrementally. This does not exempt an executed deprecated operation from the selected first-execution runtime warning. This exemption does not follow a re-export into a new module. An alias of the deprecated declaration still refers to its original identity unless a separately defined compatibility wrapper creates a new declaration.

A field is deprecated through its metadata. The compact RFC 085 spellings stay for the common case, and a nested block in the field's RFC 096 metadata block carries the full facts, with the decorator's names:

```incan
from std.diagnostics import DeprecationSeverity

model Customer:
    full_name: str
    nickname [deprecated="Use full_name"]: str = ""
    name: str = "" {
        deprecated {
            message = "Use full_name"
            since = "0.7"
            retire_in = "0.9"
            replacement = full_name
            severity = DeprecationSeverity.Warning
        }
    }

def greet(customer: Customer) -> str:
    return customer.name
```

`customer.name` in `greet` receives a warning that names `Customer.name`, the version it was deprecated in, its planned retirement, the message and `Customer.full_name` as the checked replacement. `nickname` produces the same kind of warning with only its message. `[deprecated=true]` deprecates a field with no message. All three spellings fill one checked descriptor, and authored field accesses, explicit constructor bindings and documentation consume it. Metadata traversal and serializer wire keys must not be treated as a new authored field reference for every record.

A member that used to be public can be made private when callers should go through other operations. It states what replaces it, so the refusal a caller now receives explains the move instead of only saying that the member is private:

```incan
pub model Inventory:
    items: List[str] = [] {
        made_private {
            since = "0.7"
            message = "add items with add() and check them with contains()"
            replacement = add
        }
    }

    pub def add(mut self, item: str) -> None:
        self.items.append(item)

    pub def contains(self, item: str) -> bool:
        return item in self.items
```

A different module writing `inventory.items.append("bolt")` is refused, as for any private field. The refusal names `Inventory.items`, the version it was made private in, the message and `Inventory.add` as the checked replacement. Code inside `Inventory` uses `items` with no diagnostic. A function or type that was made private takes the same facts as a decorator, `@made_private(since="0.7", replacement=checked_limit, message="use checked_limit")`.

## Reference-level explanation

- Optional since, retire_in, message, by and issue must be compile-time strings, and are the shared lifecycle facts that RFC 085's `renamed_from` also uses. `by` names who or what made the decision, such as a person, a team or a tool's finding, and `issue` points at where the decision is recorded. Both are unverified descriptive text and grant nothing. Optional replacement must be a checked declaration reference, retained as canonical identity rather than a string-dispatch key. retire_in is an advisory plan, not an automatic removal trigger.
- Editor hover and explicit deprecated imports/references must expose guidance, including a squiggle. Compiler diagnostics apply to authored consuming uses; an unused import does not by itself imply runtime execution.
- External consuming references must warn by default. Optional severity uses the compiler-recognized std.diagnostics.DeprecationSeverity cases Warning and Error. An author-declared Error rejects the use before execution. Project warning-as-error policy may strengthen Warning and must not silently weaken an author Error.
- The first executed deprecated canonical declaration/member use in one process must produce one runtime warning; later executed uses deduplicate by canonical identity. Import alone, dead code and compile-time-only metadata must not produce runtime warnings. Warnings identify symbols and safe guidance, never argument values or secrets.
- Diagnostics must identify canonical declaration identity, authored use and declaration locations, supplied message, version guidance and checked replacement when present. A local alias/re-export route must remain inspectable; generated forwarding must not duplicate one authored warning. A separately authored wrapper has its own explicit policy.
- Import aliases must not erase the warning. A re-export must not reset the defining module's identity.
- Version text must not cause automatic removal or alter callable resolution.
- Diagnostics and generated documentation must consume the same checked facts.
- An automatic replacement must not be offered solely because a message contains another symbol's spelling. A checked replacement reference must identify the target in diagnostics alongside the supplied migration message. It must not be treated as proof of equal arguments, effects or result types, and must not authorize a tool to invent error recovery.
- Field warnings must distinguish authored field access from unrelated runtime mapping keys and wire names.
- A field's `deprecated` metadata must accept `true`, a message string, or a nested `deprecated { ... }` block under RFC 096. The nested block may contain only `message`, `since`, `retire_in`, `by`, `issue`, `replacement` and `severity`, each at most once, with the same types and rules as the decorator's arguments. `replacement` must be a checked reference to a field of the same declaration or to another declaration. `deprecated=true` and `deprecated="text"` must be equivalent to a nested block with no entries and with `message = "text"`, respectively. A field must not combine spellings: `deprecated` remains one metadata key under RFC 096's duplicate-key rule.
- A declaration or field that used to be public may carry `made_private`: the `@made_private(...)` decorator on a declaration, or field metadata accepting `true`, a message string, or a nested `made_private { ... }` block. It may contain only `message`, `since`, `retire_in`, `by`, `issue` and `replacement`, each at most once, with the same types and rules as `deprecated`; it has no `severity`. `made_private` on a public declaration or field must be refused with a diagnostic naming the key, and a member must not carry both `made_private` and `deprecated`.
- A use outside a `made_private` member's visibility receives the ordinary visibility refusal, and that refusal is the only error at the use. It names the member, the version guidance, the message and the checked replacement. A use the visibility admits receives no diagnostic. A refused use never compiles, so no runtime warning applies. Without `made_private`, a refusal must not suggest making a member public when the member belongs to another package.
- `made_private` facts are preserved through compiled library metadata and generated documentation exactly as `deprecated` facts are, so a consumer of a compiled package receives the same refusal, and the `retire_in` lint applies to them.

## Design details

Rust's [deprecation attribute](https://doc.rust-lang.org/reference/attributes/diagnostics.html) is useful prior art for a compiler-visible warning with explanatory metadata. Incan should additionally expose this information as structured diagnostics so an agent can locate and assess the migration rather than parse prose alone.

Report the consuming authored reference once when elaboration creates several internal accesses. Editor import guidance, compiler use diagnostics and first-execution runtime warnings describe different events and must remain distinguishable in structured output. Reflection, generated adapters and schema metadata traversal must not manufacture source-use warnings.

## Alternatives considered

- Changelog-only deprecation: useful context, but disconnected from the actual caller.
- Free-form replacement strings with automatic application: unsafe when argument binding, effects, or return types differ.
- Immediate removal: sometimes deliberate, but offers no migration interval.
- Reusing `deprecated` with Error severity on a member made private: the use would receive a deprecation error beside the visibility refusal, and the member would read as going away when it remains its owner's implementation.
- A separate `replaced_by` key for members made private: a second name for the descriptor's `replacement`, without the lifecycle facts.

## Drawbacks

Warnings can become noisy across heavily used compatibility APIs. Defining-module suppression can hide internal migration debt. Field deprecation spans language references and data-schema expectations, which need an explicit shared contract.

## Layers affected

- **Parser / Formatter:** the nested `deprecated { ... }` and `made_private { ... }` field metadata values.
- **Declaration / field metadata:** stable deprecation and `made_private` facts, one descriptor for every spelling.
- **Name resolution / diagnostics:** external use checks and deduplication; the visibility refusal's `made_private` guidance.
- **LSP / documentation:** visible guidance and checked replacement affordances if adopted.

## Design Decisions

- Optional since, retire_in, message, by, issue and checked replacement metadata share one descriptor, and its lifecycle facts are shared with RFC 085's `renamed_from`, so parsing, checks, the `retire_in` lint and rendering exist once. Version guidance is advisory; neither a retirement version nor a replacement identity automatically changes or removes code.
- Existing field deprecated bool/text metadata drives authored source-use guidance and documentation. The descriptor carries richer facts without a second metadata authority.
- A field's full deprecation facts use a nested `deprecated { ... }` block with the decorator's names. `deprecated=true` and `deprecated="message"` remain as compact spellings of the same descriptor, so one key accepts three value shapes and every consumer reads the same facts.
- Editor import/reference guidance, compiler consuming-use warnings and one runtime warning per executed canonical identity per process are distinct. Imports alone, dead code and compile-time-only metadata do not execute deprecated code. Diagnostics must preserve secrecy.
- std.diagnostics.DeprecationSeverity.Warning is the default; Error rejects the use at compilation. Project policy may strengthen warnings without weakening an author-declared Error.
- Aliases and re-exports retain canonical policy and an inspectable authored route. Generated references deduplicate at their responsible source operation; a real wrapper has its own declaration policy.
- A checked replacement and migration message must be shown together; neither authorizes an automatic rewrite when call contracts differ.
- A member taken out of the public surface states its replacement with `made_private`, which shares the deprecation descriptor without `severity`. `deprecated` is not reused: a deprecated member still works for its callers, while a member made private is refused outside its owner, and that refusal stays the one error at the use.
