# Explicit ownership annotations

Ownership annotation syntax is recognized, but executing a declaration containing an explicit ownership annotation is refused with `INCAN-T0123`. Unannotated functions retain their ordinary parameter and return behavior.

## Annotation names

| Annotation | Requested access | Execution |
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
# refused: INCAN-T0123
def view(value: Borrowed[str]) -> Borrowed[str] from value:
    return value
```

The grammar is `from parameter` followed by zero or more `.field` projections. Methods may use their receiver, and abstract trait methods accept the same clause. Calls, indexing, namespace separators and multiple source expressions are not accepted in the clause.

An explicit borrowed result requires a source even if there is only one parameter. This also applies to an explicit borrowed component inside an outcome or tuple. Missing sources and roots outside the callable's parameters or receiver produce `INCAN-T0124`. Every projected field must be a stored model or class field; generic fields use their declared type arguments. Computed properties are not valid source fields. A declaration with a result-source clause is refused for execution with `INCAN-T0123`.

An `Owned` result can declare a source relationship. Neither the source clause nor `Owned` implicitly copies, pins, retains or acquires a lease on storage.

## Place-binding migration warning

A mutable local initialized directly from a stored string field receives an independent value. Assigning to `title` in this example leaves `page.title` unchanged:

```incan
model Page:
    title: str


def main() -> None:
    page = Page(title="old")
    mut title = page.title
    title = "new"
```

`INCAN-T0125` is emitted for a new local initialized from an existing local, parameter, stored field or known list/dictionary element. Bare, `let` and `mut` declarations are included. Compatible type annotations do not suppress the warning. Calls, constructor results, computed properties and explicit clone results do not receive this warning. The warning does not alter runtime behavior.

## Related explanation

[Why ownership declarations are introduced in stages](../explanation/ownership_declarations.md) explains the migration warning, provider compatibility and editor metadata.
