# Explicit ownership annotations

This reference describes ownership annotation syntax and source-checking diagnostics. Its direct-input subset is specified below; other ownership boundaries are refused with `INCAN-T0123`. Source acceptance does not establish native execution support. Unannotated functions retain their ordinary parameter and return behavior.

## Annotation names

| Annotation | Requested access | Source acceptance |
| --- | --- | --- |
| `Borrowed[T]` | Shared access to existing storage | Direct-input profile; shared returned views |
| `BorrowedMut[T]` | Exclusive borrowed access; `mut` separately permits writes | Direct-input profile |
| `Owned[T]` | Ownership transfer, including dependencies already held by the value | Direct-input profile |
| `Reference[T]` | Exclusive borrowed result for continued operations on its original place | Exclusive returned views |

Each annotation takes exactly one type argument. These are canonical prelude declarations, not runtime wrapper constructors: `Owned(value)` is refused. Ordinary lookup still applies; a user declaration with the same name remains an ordinary declaration.

Explicit ownership inside a tuple, outcome, field or callable annotation is also refused. Nesting an unsupported contract does not make it executable.

## Direct-input profile

Synchronous free functions and concrete methods can use direct `Borrowed[T]`, `BorrowedMut[T]` and `Owned[T]` parameters within the following restrictions. They require fixed parameters without defaults, generic binders or decorators, and direct access in straight-line statements, `if`/`elif`/`else` branches, `for`, `while` and `loop` loops with `continue` and value-free `break`, statement `match` and `?`. It preserves existing storage for shared and exclusive inputs, including scalar values, and transfers an owned non-Copy root without adding a repair clone. A method's receiver is checked as a borrowed input: `self` grants shared access and `mut self` exclusive, writable access. A call to such a method borrows its receiver from existing storage for the invocation.

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

The checked loan ends after its last dependent operation, allowing later access through the original owner. An observer, comparison or formatted string takes shared read access to each non-Copy borrowed operand until the operation consumes it. This applies to direct borrowed parameters and to shared or exclusive aliases. For example, `print(view, replace(original))` conflicts if `view` borrows `original` and `replace` requests exclusive access. An exclusive alias cannot avoid the conflict by passing itself to `replace`. Using a disjoint source for `replace` is permitted. A completed inner operation can end its dependency earlier: `print(f"{view}", replace(original))` first constructs an independent formatted string, then replaces the original. Copy operands retain ordinary value-read behavior. Within this profile, `print` observes `str`, `int`, `float`, `bool` and unit values and lists of them; printing another value may run a display hook and receives `INCAN-T0123`.

A local `Owned[T]` initializer creates independent destination storage. A whole non-Copy source transfers and cannot be used afterward; a Copy source remains usable. A fresh initializer supplies its new value directly. Initializing from a non-Copy field, a shared non-Copy source or a source with a conflicting live borrow is refused. An initializer inside a branch transfers its source on that path only: the source stays usable on the other paths, and each path drops exactly the owners it still holds.

```incan
# accepted by source checking
def size() -> int:
    original = "payload"
    let held: Owned[str] = original
    return len(held)
```

A transfer inside a loop body applies on every iteration that reaches it. Moving a non-Copy owner inside a loop is refused with `INCAN-T0126` when another iteration can follow the transfer, through the end of the body or a `continue`; a `break` after the transfer leaves the loop before the next iteration. A `for` loop over a checked place borrows it until the loop ends, so replacing the iterable inside the loop is refused with `INCAN-T0126`. Each path out of a loop, through its failed condition, its exhausted iterable or a `break`, drops exactly the owners it still holds.

```incan
# accepted by source checking
def consume(text: Owned[str]) -> None:
    pass


def first_round(text: Owned[str], rounds: Borrowed[list[int]]) -> None:
    for _round in rounds:
        consume(text)
        break
```

A `?` consumes its `Result` or `Option` and continues with the success payload. On an error it leaves the body as a `return` does, and the body drops each owner it still holds. `Ok`, `Err` and `Some` build a fresh carrier from a fresh or Copy payload; moving a stored non-Copy owner into a carrier receives `INCAN-T0123`, as for model fields. A call whose result is a `Result` or `Option` of plain scalar, text or byte values is admitted like those values.

A statement `match` on a call's or constructor's result runs exactly one arm. `Ok`, `Err` and `Some` patterns bind their payload as a fresh owner, and `None` and `_` bind nothing. An arm may transfer an owner that the other arms keep, and each continuing arm drops exactly the owners it still holds. A match on a stored place, a guarded arm and other patterns receive `INCAN-T0123`.

```incan
# accepted by source checking
def size(text: Owned[str]) -> Result[int, str]:
    if len(text) == 0:
        return Err("empty")
    return Ok(len(text))


def both(text: Owned[str], other: Owned[str]) -> Result[int, str]:
    first = size(text)?
    second = size(other)?
    return Ok(first + second)


def describe(text: Owned[str]) -> str:
    match size(text):
        Ok(count) => return f"{count} characters"
        Err(message) => return message
```

Returned dependencies other than the returned views below, indirect calls, indexed access and projected non-Copy transfers remain refused. These restrictions apply to callers as well as callees, including callers whose own parameters have no explicit ownership annotations.

Assertion messages are evaluated only on failure. They may read borrowed values or use a checked exclusive input, but transferring a stored non-Copy owner from a message receives `INCAN-T0123`: unlike an `if` branch, the failure path does not yet carry its own cleanup. This also applies to ordinary locals passed to an `Owned` parameter. A fresh value constructed for the failure message and a Copy input do not transfer a surviving outer owner.

## Ordinary helpers within an input-checked body

A body that already requires explicit-input checking can call a plain synchronous free function with no arguments or only immutable Copy arguments and a scalar or owned text/byte result. The selected parameter types must match the canonical declaration without parameter substitution; missing or conflicting header facts receive `INCAN-T0123`. This does not change the helper's inferred parameter ABI or require ownership annotations in an otherwise ordinary caller.

```incan
# accepted by source checking; native qualification remains separate
def add(value: int) -> int:
    return value + 1


def use(value: Borrowed[int]) -> int:
    return add(value)
```

Named fixed arguments retain their existing declaration-order evaluation. Mutable ordinary inputs, non-Copy ordinary arguments, defaults, indirect calls and nominal result dependencies remain refused within this profile. A generic binder alone does not change input access: an instantiation that leaves the checked parameter inventory unchanged can satisfy this source rule, while a substituted parameter inventory needs further retained evidence.

## Result-source syntax


A function or method can state a result source after its return annotation:

```incan
# accepted by source checking
def view(value: Borrowed[str]) -> Borrowed[str] from value:
    return value
```

The grammar is `from parameter` followed by zero or more `.field` projections. Methods may use their receiver, and abstract trait methods accept the same clause. Calls, indexing, namespace separators and multiple source expressions are not accepted in the clause.

An explicit borrowed result requires a source even if there is only one parameter. This also applies to an explicit borrowed component inside an outcome or tuple. Missing sources and roots outside the callable's parameters or receiver produce `INCAN-T0124`. Every projected field must be a stored model or class field; generic fields use their declared type arguments. Computed properties are not valid source fields.

An `Owned` result can declare a source relationship. Neither the source clause nor `Owned` implicitly copies, pins, retains or acquires a lease on storage. An `Owned` result with a source, and a borrowed result nested in a tuple or another type, are refused for execution with `INCAN-T0123`. A borrowed success payload of a `Result` or `Option` is a [carried view](#carried-views).

## Returned views

A `Borrowed[T]` or `Reference[T]` result with a source clause is a returned view: the caller receives shared or exclusive access to the source place itself.

```incan
# accepted by source checking
class Customer:
    pub name: str
    pub address: Address

    def edit_address(mut self) -> Reference[Address] from self.address:
        return self.address
```

The source must be the receiver or a `Borrowed[T]` or `BorrowedMut[T]` parameter. `Reference[T]` requires exclusive access: a `mut self` receiver or a `BorrowedMut[T]` parameter. An `Owned[T]` parameter, an ordinary parameter and a shared source for `Reference[T]` produce `INCAN-T0124`. `T` must equal the type of the source place.

Every `return` in the body must return exactly the source place, an alias of it, or a call that returns a view of exactly that place. Returning another place or a value created in the body produces `INCAN-T0124`. The body can use the whole source while it runs; only the declared place stays borrowed after the call.

A call that returns a view borrows the source argument for the invocation. The caller substitutes its argument's place into the declared path. Binding the result to a new local, using it as a receiver, or discarding it in an expression statement uses that place directly; no copy or wrapper is made. A disjoint sibling field of the source can be edited while the view is used. Editing an overlapping place through its owner or replacing the owner before the view's last use produces `INCAN-T0126`. The source argument must be existing storage; a view of a temporary is refused.

```incan
# accepted by source checking
def main() -> None:
    mut customer = Customer(name="Sam", address=Address(city="Utrecht"))
    mut address = customer.edit_address()
    customer.name = "Samuel"  # a disjoint sibling field
    address.city = "Rotterdam"  # writes customer.address.city
```

Other uses of a returned view, such as passing it as an argument or storing it in a field, are refused with `INCAN-T0123`.

### Carried views

A `Result[Borrowed[T], E]` or `Result[Reference[T], E]` result with a source clause carries a returned view in its success payload. The source rules of returned views apply to that payload.

```incan
# accepted by source checking
class SampleBuffer:
    pub capacity: int
    pub samples: list[int]

    def append(mut self, sample: int) -> Result[Reference[Self], str] from self:
        if len(self.samples) >= self.capacity:
            return Err("full")
        self.samples.append(sample)
        return Ok(self)

    def append_all(mut self, samples: Borrowed[list[int]]) -> Result[Reference[Self], str] from self:
        for sample in samples:
            self.append(sample)?
        return Ok(self)
```

Every `Ok(...)` return must carry exactly the source place, an alias of it, or a view of that place taken out of a carried call with `?`. A value created in the body produces `INCAN-T0124`. `Err(...)` builds its error value under the carrier construction rules above. Returning a call that carries a view of the same place passes its carrier on.

At a call site, `?` takes the view out of the carrier. On success, the expression is a returned view of the caller's substituted place: it can be bound to a new local, used as a receiver or returned as a view, as described above. On an error, the body leaves with the error, and the loan ends there; the caller's source is usable again after the call. The call borrows its source argument as a returned-view call does.

A statement `match` on a carried call also takes the view out. In an `Ok(name)` or `Some(name)` arm, `name` is a view of the caller's substituted place for the arm, and the call's loan lasts while the arm uses it; like any pattern binding it follows the place it binds, so a `Reference[T]` view writes through it. An `Err` or `None` arm holds no view, so it can use or change the source. Binding the carrier itself to a local, or any other use of a carried call, is refused with `INCAN-T0123`.

An `Option[Borrowed[T]]` or `Option[Reference[T]]` result with a source clause is checked the same way, with `Some(...)` in place of `Ok(...)` and `None` carrying nothing. `?` does not apply to `Option`, so its callers take the view out with `match`.

```incan
# accepted by source checking
def fill(mut buffer: BorrowedMut[SampleBuffer]) -> Result[None, str]:
    (buffer.append(10)?).append_all([20, 30])?
    return Ok(None)


def add_or_grow(mut buffer: BorrowedMut[SampleBuffer], sample: int) -> int:
    match buffer.append(sample):
        Ok(view) => return len(view.samples)
        Err(_) =>
            buffer.capacity += 1  # the error arm holds no view
            return 0
```

## Source conflict diagnostics

For checked function and method bodies with direct input contracts or explicit local loans, `INCAN-T0126` reports permission conflicts, overlapping live borrows and use after ownership transfer. It identifies the conflicting operation and its source declaration, borrow origin or earlier move. When a reachable dependent use keeps a borrow live, that use is included as a related location. A method's shared receiver cannot grant an exclusive local field loan; a mutable receiver can grant one when no overlapping dependency remains live.

```incan
# refused: INCAN-T0123; the replacement also produces INCAN-T0126
def update(mut value: Owned[int]) -> int:
    let view: Borrowed[int] = value
    value = 2  # refused: INCAN-T0126, overlapping live borrow
    return view
```

Selected free-function and method calls retain their declared input access through the invocation, independently of whether the callee reads or writes the argument. A fresh scalar or owned string/byte expression can supply temporary storage for that call. Borrowing an existing variable through parentheses preserves that variable’s permissions; it does not create a fresh writable owner. This call-access rule requires direct input contracts, complete fixed arguments, and scalar or owned string/byte results without retained dependencies, or a returned view.

The absence of `INCAN-T0126` does not imply acceptance. Bodies outside the direct-input subset receive `INCAN-T0123`, including returned dependencies other than returned views, captures and suspension.

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
