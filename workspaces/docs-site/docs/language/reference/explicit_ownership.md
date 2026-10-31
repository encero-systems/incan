# Explicit ownership annotations

Explicit ownership declarations are being introduced in stages. This compiler recognizes the annotation names and result-source syntax below, but refuses their execution with `INCAN-T0123`. Recognition does not enable copying, borrowing or transfer behavior. Existing unannotated functions retain their current semantics.

## Annotation names

| Annotation | Intended contract | Current support |
| --- | --- | --- |
| `Borrowed[T]` | Shared access to existing storage | Recognized; execution refused |
| `BorrowedMut[T]` | Exclusive borrowed access; `mut` separately permits writes | Recognized; execution refused |
| `Owned[T]` | Ownership transfer, including dependencies already held by the value | Recognized; execution refused |
| `Reference[T]` | Exclusive borrowed result for continued operations on its original place | Recognized; execution refused |

Each annotation takes exactly one type argument. These are canonical prelude declarations, not runtime wrapper constructors: `Owned(value)` is refused. Ordinary lookup still applies; a user declaration with the same name remains an ordinary declaration.

Explicit ownership inside a tuple, outcome, field or callable annotation is also refused. Nesting an unsupported contract does not make it executable.

## Result-source syntax

A function or method can state a result source after its return annotation:

```incan
# Parsed and formatted, but not executable in this stage.
def view(value: Borrowed[str]) -> Borrowed[str] from value:
    return value
```

The grammar is `from parameter` followed by zero or more `.field` projections. Methods may use their receiver, and abstract trait methods retain the same clause. The parser preserves each source token for diagnostics and formatting. Calls, indexing, namespace separators and multiple source expressions are not accepted in the clause.

An explicit borrowed result needs a source even if there is only one parameter. This also applies to an explicit borrowed component inside an outcome or tuple. Missing sources and source roots outside the callable's parameters or receiver produce `INCAN-T0124`. Static field paths are resolved through checked model and class declarations, including generic field substitution. Naming a valid path does not prove the returned value originates there: execution remains refused until ownership behavior is checked.

An owned wrapper may retain a borrowed dependency; its result-source syntax is preserved but remains unavailable in this stage. Neither the source clause nor `Owned` implicitly copies, pins, retains or acquires a lease on storage.

## Place-binding migration warning

A mutable local initialized directly from a stored string field currently receives an independent value:

```incan
model Page:
    title: str


def main() -> None:
    page = Page(title="old")
    mut title = page.title
    title = "new"
```

The compiler emits `INCAN-T0125` for a new local initialized from an existing local, parameter, stored field or known list/dictionary element. A mutable declaration will become a write-through borrow; bare and `let` declarations will borrow read-only. Compatible annotations do not request a copy. Calls, constructor results, computed properties and explicit clone results do not receive this place-binding warning. Runtime behavior is unchanged; use an explicit clone when an independent snapshot is intended. The warning release must be available before a subsequent release changes ordinary binding semantics.

## Metadata and tooling

Checked contracts retain parameter slots, access modes, underlying types and canonical stored-field identities. Parameter renaming does not change which input a result depends on. Provider metadata, aliases and editor signatures preserve this relationship; imported signatures render `from` using the currently exported parameter name. Incomplete or unsupported ownership metadata is refused. Providers carrying these contracts use manifest format 7 so older compilers cannot silently ignore a source-only relationship; ordinary providers retain format 6. A provider containing explicit ownership contracts cannot be admitted for execution by this stage.

The free-function checker-record profile is version 2. Its Incan-owned codec records access modes and source relationships in structured fields; old profiles cannot silently omit them. The profile remains limited to its supported scalar, list and callable type grammar with explicit ownership wrappers. Nominal, generic-owner and other unsupported record shapes are refused; provider metadata preserves those richer declaration contracts separately.
