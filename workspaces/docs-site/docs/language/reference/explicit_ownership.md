# Explicit ownership annotations

Ownership annotations are being introduced in stages. The checker and Body IR admit a limited direct-input profile described below; native qualification of that profile is still pending. Other ownership boundaries are refused with `INCAN-T0123`. Unannotated functions retain their ordinary parameter and return behavior.

## Annotation names

| Annotation | Requested access | Execution |
| --- | --- | --- |
| `Borrowed[T]` | Shared access to existing storage | Direct-input profile only |
| `BorrowedMut[T]` | Exclusive borrowed access; `mut` separately permits writes | Direct-input profile only |
| `Owned[T]` | Ownership transfer, including dependencies already held by the value | Direct-input profile only |
| `Reference[T]` | Exclusive borrowed result for continued operations on its original place | Recognized; execution refused |

Each annotation takes exactly one type argument. These are canonical prelude declarations, not runtime wrapper constructors: `Owned(value)` is refused. Ordinary lookup still applies; a user declaration with the same name remains an ordinary declaration.

Explicit ownership inside a tuple, outcome, field or callable annotation is also refused. Nesting an unsupported contract does not make it executable.

## Direct-input profile

Synchronous free functions can use direct `Borrowed[T]`, `BorrowedMut[T]` and `Owned[T]` parameters when the complete body retains checked access and storage facts. The initial profile requires fixed parameters without defaults, generic binders or decorators and straight-line direct access. It preserves existing storage for shared and exclusive inputs, including scalar values, and transfers an owned non-Copy root without adding a repair clone.

```incan
# Accepted by source checking and Body IR; native qualification is pending.
def replace(mut value: BorrowedMut[int]) -> None:
    value = 9


def main() -> int:
    mut value = 3
    replace(value)
    return value
```

The same source profile admits local `Borrowed[T]` and `BorrowedMut[T]` aliases when the referent is Copy and the source is a checked local or stored-field path. Reads and writes use that original place; the alias does not allocate an independent value or acquire a separate drop obligation. The checked loan ends after the alias's last use, allowing later access through the original owner. Non-Copy aliases and explicit owned locals remain refused.

Methods, returned dependencies, indirect calls, indexed access, conditional transfers and projected non-Copy transfers remain refused. Every calling body also needs its own checked access proof. A valid parameter annotation or a borrowed native signature alone cannot grant that proof. Native call resolution checks the caller independently of the selected callee, including when persisted caller data contains only ordinary parameters.

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

## Source conflict diagnostics

For checked function and method bodies with direct input contracts or explicit local loans, `INCAN-T0126` reports permission conflicts, overlapping live borrows and use after ownership transfer. It identifies the conflicting operation and its source declaration, borrow origin or earlier move. When a reachable dependent use keeps a borrow live, that use is included as a related location. A method's shared receiver cannot grant an exclusive local field loan; a mutable receiver can grant one when no overlapping dependency remains live.

```incan
# refused: INCAN-T0123; the replacement also produces INCAN-T0126
def update(mut value: Owned[int]) -> int:
    let view: Borrowed[int] = value  # borrow origin
    value = 2                      # conflicts with the live borrow
    return view                    # dependent use keeps it live
```

Selected free-function and method calls retain their declared input access through the invocation, independently of whether the callee reads or writes the argument. A fresh scalar or owned string/byte expression can supply temporary storage for that call. Borrowing an existing variable through parentheses preserves that variable’s permissions; it does not create a fresh writable owner. The analysis currently requires direct input contracts, complete fixed arguments, and scalar or owned string/byte results without retained dependencies.

Analysis alone does not authorize execution. Only the complete direct-input profile above can pass the current Body IR admission boundary. Returned dependencies, captures, suspension, unprojected calls and other source shapes whose complete dependency facts are unavailable still receive the execution refusal. The absence of `INCAN-T0126` is not proof that such a body has a complete ownership analysis.

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

`INCAN-T0125` is emitted for a new local initialized from an existing local, parameter, stored field or known list/dictionary element. Bare, `let` and `mut` declarations are included. Ordinary value-type annotations such as `int` do not suppress the warning. An explicit ownership annotation states its own access request and does not receive this migration warning; explicit local borrowing remains refused. Calls, constructor results, computed properties and explicit clone results do not receive this warning. The warning does not alter runtime behavior.

## Related explanation

[Why ownership declarations are introduced in stages](../explanation/ownership_declarations.md) explains the migration warning, provider compatibility and editor metadata.
