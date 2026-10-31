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

An explicit borrowed result needs a source even if there is only one parameter. This also applies to an explicit borrowed component inside an outcome or tuple. Missing sources and source roots outside the callable's parameters or receiver produce `INCAN-T0124`. Naming a valid root does not prove the returned value originates there: this stage still refuses execution until field relationships and ownership behavior are checked.

An owned wrapper may retain a borrowed dependency; its result-source syntax is preserved but remains unavailable in this stage. Neither the source clause nor `Owned` implicitly copies, pins, retains or acquires a lease on storage.

## Mutable field-copy migration warning

A mutable local initialized directly from a stored string field currently receives an independent value:

```incan
model Page:
    title: str


def main() -> None:
    page = Page(title="old")
    mut title = page.title
    title = "new"
```

The compiler warns that a future ownership release will make this form a write-through binding. Runtime behavior is unchanged in this stage. Use an explicit clone when an independent mutable snapshot is intended. This initial warning covers the demonstrated stored-string-field case; it is not a claim that every ordinary local binding has already migrated.
