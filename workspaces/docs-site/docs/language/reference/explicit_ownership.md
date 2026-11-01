# Explicit ownership annotations

This reference describes ownership annotation syntax and source-checking diagnostics. Its direct-input subset is specified below; other ownership boundaries are refused with `INCAN-T0123`. Source acceptance does not establish native execution support. Unannotated functions retain their ordinary parameter and return behavior.

## Annotation names

| Annotation | Requested access | Source acceptance |
| --- | --- | --- |
| `Borrowed[T]` | Shared access to existing storage | Direct-input profile only |
| `BorrowedMut[T]` | Exclusive borrowed access; `mut` separately permits writes | Direct-input profile only |
| `Owned[T]` | Ownership transfer, including dependencies already held by the value | Direct-input profile only |
| `Reference[T]` | Exclusive borrowed result for continued operations on its original place | Recognized; execution refused |

Each annotation takes exactly one type argument. These are canonical prelude declarations, not runtime wrapper constructors: `Owned(value)` is refused. Ordinary lookup still applies; a user declaration with the same name remains an ordinary declaration.

Explicit ownership inside a tuple, outcome, field or callable annotation is also refused. Nesting an unsupported contract does not make it executable.

## Direct-input profile

Synchronous free functions can use direct `Borrowed[T]`, `BorrowedMut[T]` and `Owned[T]` parameters within the following restrictions. Functions require fixed parameters without defaults, generic binders or decorators and straight-line direct access. It preserves existing storage for shared and exclusive inputs, including scalar values, and transfers an owned non-Copy root without adding a repair clone.

```incan
# accepted by source checking
def replace(mut value: BorrowedMut[int]) -> None:
    value = 9


def main() -> int:
    mut value = 3
    replace(value)
    return value
```

The same source profile admits local `Borrowed[T]` and `BorrowedMut[T]` aliases of a checked local or stored-field path. Reads and writes use that original place; the alias does not allocate an independent value or acquire a separate drop obligation. Reading a Copy referent produces a value. Reading a non-Copy referent retains a borrow and cannot transfer or return its ownership.

The checked loan ends after its last dependent operation, allowing later access through the original owner. An observer, comparison or formatted string takes shared read access to each non-Copy borrowed operand until the operation consumes it. This applies to direct borrowed parameters and to shared or exclusive aliases. For example, `print(view, replace(original))` conflicts if `view` borrows `original` and `replace` requests exclusive access. An exclusive alias cannot avoid the conflict by passing itself to `replace`. Using a disjoint source for `replace` is permitted. A completed inner operation can end its dependency earlier: `print(f"{view}", replace(original))` first constructs an independent formatted string, then replaces the original. Copy operands retain ordinary value-read behavior.

A local `Owned[T]` initializer creates independent destination storage. A whole non-Copy source transfers and cannot be used afterward; a Copy source remains usable. A fresh initializer supplies its new value directly. Initializing from a non-Copy field, a shared non-Copy source or a source with a conflicting live borrow is refused. The straight-line restriction also applies to these local initializers.

```incan
# accepted by source checking
def size() -> int:
    original = "payload"
    let held: Owned[str] = original
    return len(held)
```


Methods, returned dependencies, indirect calls, indexed access, conditional transfers and projected non-Copy transfers remain refused. These restrictions apply to callers as well as callees, including callers whose own parameters have no explicit ownership annotations.

Assertion messages are evaluated only on failure. They may read borrowed values or use a checked exclusive input, but transferring a stored non-Copy owner from a message receives `INCAN-T0123` until conditional cleanup is supported. This also applies to ordinary locals passed to an `Owned` parameter. A fresh value constructed for the failure message and a Copy input do not transfer a surviving outer owner.

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
    let view: Borrowed[int] = value
    value = 2  # refused: INCAN-T0126, overlapping live borrow
    return view
```

Selected free-function and method calls retain their declared input access through the invocation, independently of whether the callee reads or writes the argument. A fresh scalar or owned string/byte expression can supply temporary storage for that call. Borrowing an existing variable through parentheses preserves that variable’s permissions; it does not create a fresh writable owner. This call-access rule requires direct input contracts, complete fixed arguments, and scalar or owned string/byte results without retained dependencies.

The absence of `INCAN-T0126` does not imply acceptance. Bodies outside the direct-input subset receive `INCAN-T0123`, including returned dependencies, captures and suspension.

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

`INCAN-T0125` is emitted for a new local initialized from an existing local, parameter, stored field or known list/dictionary element. Bare, `let` and `mut` declarations are included. Ordinary value-type annotations such as `int` do not suppress the warning. An explicit ownership annotation states its own access request and does not receive this migration warning. The direct-input subset defines which explicit local borrows are accepted. Calls, constructor results, computed properties and explicit clone results do not receive this warning. The warning does not alter runtime behavior.

## Related explanation

[Why ownership declarations are introduced in stages](../explanation/ownership_declarations.md) explains the migration warning, provider compatibility and editor metadata.
