# Explicit ownership annotations

Explicit ownership annotations state the access a parameter, local or result requires. Unannotated parameters and bindings keep their inferred passing and binding rules. An explicit mode is an obligation for the caller and the implementation: a program whose obligations cannot be proved is refused, and no mode is satisfied by an implicit copy, a new lease or weaker access.

## Annotation names

| Annotation | Access |
| --- | --- |
| `Borrowed[T]` | Shared access to existing storage; writes are refused, even through a `mut` binding |
| `BorrowedMut[T]` | Exclusive borrowed access to existing storage; `mut` permits writes |
| `Owned[T]` | Ownership of the value: a non-Copy value is transferred and a Copy value is copied |
| `Reference[T]` | Exclusive access returned to the caller for continued operations on its original place |

Each annotation takes exactly one type argument. These are canonical prelude declarations, not runtime wrapper constructors: `Owned(value)` is refused with `INCAN-T0123`. Ordinary lookup applies; a user declaration with the same name is an ordinary declaration. Aliases and generic substitutions preserve the mode. An annotation does not allocate a wrapper.

Owning a value does not erase the dependencies it holds: an owned value that retains a borrow keeps its source borrowed. Shared access follows the underlying type's checked interior-mutability operations; it is not a promise of deep immutability.

This applies both to declared result-source relationships and to compiler-defined lazy results. `list.iter()` and a generator expression over a list place retain a shared dependency on that place; iterator adapters, aggregates, closures and builtin result carriers preserve it. A moved or temporary list transfers its storage to the lazy result instead. No form implicitly clones the complete list.

## Inputs

A `Borrowed[T]` parameter receives shared access and a `BorrowedMut[T]` parameter exclusive access to the caller's storage; neither copies the argument. An `Owned[T]` parameter transfers a non-Copy argument, so the caller cannot use it afterward, and copies a Copy argument. A method's receiver is a borrowed input: `self` grants shared access and `mut self` exclusive, writable access. An explicit mode takes precedence over the parameter's inferred passing mode; the body cannot weaken it.

```incan
def replace(mut value: BorrowedMut[int]) -> None:
    value = 9


def main() -> int:
    mut value = 3
    replace(value)
    return value  # accepted: returns 9
```

A call borrows each borrowed argument from existing storage for the invocation. A fresh expression supplies temporary storage for that call. Borrowing a variable through parentheses preserves that variable's permissions; it does not create a fresh writable owner. Assigning to a writable borrowed parameter replaces the caller's value; assigning to a mutable owned parameter replaces the callee's own value. Immutable, static and interop access restrictions still apply: an annotation cannot grant permission to change a source that is otherwise read-only.

Each call takes its declared access for the invocation, whether the callee reads or writes the argument. Calls to ordinary functions from a body with explicit annotations keep the callee's inferred parameter contract.

## Locals

A local declared `Borrowed[T]` or `BorrowedMut[T]` is an alias of a checked local or stored-field place. Reads and writes use that original place; the alias does not allocate an independent value or acquire a separate drop obligation. Assigning through a writable alias writes through to its target; it does not retarget the alias. Reading a Copy referent produces a value. Reading a non-Copy referent retains the borrow and cannot transfer or return its ownership.

A local `Owned[T]` initializer creates independent destination storage. A whole non-Copy source transfers and cannot be used afterward; a Copy source remains usable. A fresh initializer supplies its new value directly. Initializing from a shared non-Copy source or from a source with a conflicting live borrow is refused.

```incan
def size() -> int:
    original = "payload"
    let held: Owned[str] = original
    return len(held)
```

## Loans and last use

A loan ends after its last dependent use, through every path that can reach that use, and the original owner is usable again afterward. An observer, comparison or formatted string takes shared read access to each non-Copy borrowed operand until the operation consumes it. For example, `print(view, replace(original))` conflicts if `view` borrows `original` and `replace` requests exclusive access. An exclusive alias cannot avoid the conflict by passing itself to `replace`. Using a disjoint source for `replace` is permitted. A completed inner operation ends its dependency: `print(f"{view}", replace(original))` first constructs an independent formatted string, then replaces the original. Copy operands are read by value.

A transfer applies on the path that performs it. A transfer inside one branch leaves the source usable on the other branches, and each path drops exactly the owners it still holds. A transfer inside a loop body applies on every iteration that reaches it: moving a non-Copy owner is refused with `INCAN-T0126` when another iteration can follow the transfer, through the end of the body or a `continue`; a `break` after the transfer leaves the loop first. A `for` loop over a checked place borrows it until the loop ends, so replacing the iterable inside the loop is refused with `INCAN-T0126`. Each path out of a loop, through its failed condition, its exhausted iterable or a `break`, drops exactly the owners it still holds.

```incan
def consume(text: Owned[str]) -> None:
    pass


def first_round(text: Owned[str], rounds: Borrowed[list[int]]) -> None:
    for _round in rounds:
        consume(text)
        break
```

A `?` consumes its `Result` or `Option` and continues with the success payload. On an error it leaves the body as a `return` does, and the body drops each owner it still holds. An error is not a rollback: changes made before the failure remain. A `match` runs exactly one arm; each arm starts from the evaluated scrutinee, an arm may transfer an owner that the other arms keep, and each continuing arm drops exactly the owners it still holds. An assertion message is evaluated only when the assertion fails, so a transfer inside the message applies only on that path.

```incan
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

## Result-source syntax

A function or method states the source of a borrowed result after its return annotation:

```incan
def view(value: Borrowed[str]) -> Borrowed[str] from value:
    return value
```

The grammar is `from parameter` followed by zero or more `.field` projections. Methods may use their receiver, and abstract trait methods accept the same clause. Calls, indexing, namespace separators and multiple source expressions are not accepted in the clause.

An explicit borrowed result requires a source even if there is only one parameter. This also applies to an explicit borrowed component inside an outcome or tuple. Missing sources and roots outside the callable's parameters or receiver produce `INCAN-T0124`. Every projected field must be a stored model or class field; generic fields use their declared type arguments. Computed properties are not valid source fields.

One clause names one source. Every borrowed component of the result must be justified by that source; disjoint projections of it are checked individually. A result whose borrowed components need independent sources produces `INCAN-T0124`, naming those sources.

An `Owned` result can declare a source relationship: the owned value retains a dependency on that source, and the caller's source stays borrowed while the result can use it. Neither the source clause nor `Owned` implicitly copies, pins, retains or acquires a lease on storage.

## Returned views

A `Borrowed[T]` or `Reference[T]` result with a source clause is a returned view: the caller receives shared or exclusive access to the source place itself.

```incan
class Customer:
    pub name: str
    pub address: Address

    def edit_address(mut self) -> Reference[Address] from self.address:
        return self.address
```

The source must be the receiver or a `Borrowed[T]` or `BorrowedMut[T]` parameter. `Reference[T]` requires exclusive access: a `mut self` receiver or a `BorrowedMut[T]` parameter. An `Owned[T]` parameter, an ordinary parameter and a shared source for `Reference[T]` produce `INCAN-T0124`. `T` must equal the type of the source place.

Every `return` in the body must return exactly the source place, an alias of it, or a call that returns a view of exactly that place. Returning another place or a value created in the body produces `INCAN-T0124`. The body can use the whole source while it runs; only the declared place stays borrowed after the call.

A call that returns a view borrows the source argument for as long as the view is used. The caller substitutes its argument's place into the declared path. Binding the result to a new local, using it as a receiver, passing it as an argument or discarding it in an expression statement uses that place directly; no copy or wrapper is made. A disjoint sibling field of the source can be edited while the view is used. Editing an overlapping place through its owner or replacing the owner before the view's last use produces `INCAN-T0126`. A view of a temporary ends with the statement that creates it; using it afterward is refused with `INCAN-T0123`.

```incan
def main() -> None:
    mut customer = Customer(name="Sam", address=Address(city="Utrecht"))
    mut address = customer.edit_address()
    customer.name = "Samuel"  # accepted: a disjoint sibling field
    address.city = "Rotterdam"  # accepted: writes customer.address.city
```

A returned view can itself be the source of a further view or a fluent call; every derived view keeps the original source dependency. A view placed in a `Result`, an `Option`, a tuple or another value keeps its dependency through that value.

### Carried views

A model or class type parameter can be instantiated with `Borrowed[T]` or `Reference[T]`. A result whose type contains that instantiation and whose `from` clause names the view source carries the source dependency through the user-defined value. A field declared as that type parameter stores shared or exclusive access according to the instantiated mode; constructing the value from the source place, an alias of it or a view of it does not copy the referent. Binding, moving, passing, returning, field access, method calls and storage inside another aggregate preserve the dependency through the carrying value's last use. A method can return another generic carrier `from self`, and that carrier also retains every source dependency already held by `self`.

```incan
class Cursor[S]:
    pub selection: S


class Selection[R]:
    pub reader: R
    pub key: int

    def cursor(self) -> Cursor[Borrowed[Selection[R]]] from self:
        return Cursor[Borrowed[Selection[R]]](selection=self)


def select(reader: Borrowed[GraphState], key: int) -> Selection[Borrowed[GraphState]] from reader:
    return Selection[Borrowed[GraphState]](reader=reader, key=key)
```

The result's one named source must justify every borrowed component. A field such as `reader: GraphState` has owned `GraphState` storage; passing a borrowed reader to it is refused. A field declared directly as `reader: Borrowed[GraphState]` is also refused. A carrying field must use a type parameter, and the enclosing type must instantiate that parameter with the required ownership mode.

A `Result[Borrowed[T], E]` or `Result[Reference[T], E]` result with a source clause carries a returned view in its success payload. The source rules of returned views apply to that payload.

```incan
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

Every `Ok(...)` return must carry exactly the source place, an alias of it, or a view of that place taken out of a carried call with `?`. A value created in the body produces `INCAN-T0124`. Returning a call that carries a view of the same place passes its carrier on.

At a call site, `?` takes the view out of the carrier. On success, the expression is a returned view of the caller's substituted place. On an error, the body leaves with the error, and the loan ends there; the caller's source is usable again after the call.

A `match` on a carried call also takes the view out. In an `Ok(name)` or `Some(name)` arm, `name` is a view of the caller's substituted place for the arm, and the call's loan lasts while the arm uses it; like any pattern binding it follows the place it binds, so a `Reference[T]` view writes through it. An `Err` or `None` arm holds no view, so it can use or change the source. A carried call written as a statement runs for its effects and drops its carrier.

A carried call can be bound to a local. The local holds the call's loan until its last use, and a `?` or `match` on the local takes the view out, exactly as on the call, and consumes the local. Each branch of an `if` or a `match` can take the view out of the same local; after the branches join, the view counts as taken out if any continuing branch took it. Taking the view out of a local bound before a loop inside that loop is refused: every iteration would take it again. A carried call bound inside the loop body gives a fresh view on each iteration.

An `Option[Borrowed[T]]` or `Option[Reference[T]]` result with a source clause is checked the same way, with `Some(...)` in place of `Ok(...)` and `None` carrying nothing.

A `Result[T, Borrowed[E]]` or `Result[T, Reference[E]]` result with a source clause carries the view in its error payload. Every `Err(...)` return must carry exactly the source place, an alias of it, or a call that carries an error view of the same place; a value created in the body produces `INCAN-T0124`.

```incan
class Account:
    pub name: str
    pub balance: int

    def withdraw(mut self, amount: int) -> Result[int, Borrowed[str]] from self.name:
        if amount > self.balance:
            return Err(self.name)
        self.balance -= amount
        return Ok(self.balance)
```

At a call site, a `match` on the call, or on a local bound to it, takes the view out: in an `Err(name)` arm, `name` is a view of the caller's substituted place, and an `Ok` arm holds no view. A `?` on such a call propagates the error with its view: the enclosing body's result must carry an error view of the same source.

```incan
def fill(mut buffer: BorrowedMut[SampleBuffer]) -> Result[None, str]:
    (buffer.append(10)?).append_all([20, 30])?
    return Ok(None)


def add_or_grow(mut buffer: BorrowedMut[SampleBuffer], sample: int) -> int:
    match buffer.append(sample):
        Ok(view) => return len(view.samples)
        Err(_) =>
            buffer.capacity += 1  # accepted: the error arm holds no view
            return 0
```

A tuple result with a source clause, such as `Tuple[int, Borrowed[str]]`, carries a view in each borrowed element. Every return builds the tuple with the source place, an alias of it, or a projection of it in each borrowed element, and owned values in the others, or returns a call that carries views of the same places. Exclusive views in one tuple must be of disjoint places. The elements are evaluated in order.

```incan
class Tag:
    pub name: str
    pub hits: int

    pub def label(self) -> Tuple[int, Borrowed[str]] from self.name:
        return (self.hits, self.name)


def score(tag: Borrowed[Tag]) -> int:
    hits, name = tag.label()
    return hits + len(name)
```

At a call site, unpacking the call into new names takes the views out: each name in a borrowed position is a view of the caller's substituted place, and the call's loan lasts while those names are used. Each other name owns its element. Binding the whole tuple to one local holds the loan while the local is used.

## Closures

A closure retains every dependency it captures. A captured Copy value can be read when the closure is built and held by value. Any other captured place, including the receiver, is borrowed: shared when the closure only reads it, and exclusive when the closure writes it. That place stays borrowed until the closure's last call, and changing it before then is refused with `INCAN-T0126`. Passing a closure to a callee lends its captures for that call.

```incan
def total(amount: Borrowed[int], extra: int) -> int:
    base = 10
    add: (int) -> int = (step) => amount + step + base
    return add(extra) + add(1)


def after_last_call(start: int) -> int:
    mut owned = start + 1
    view: Borrowed[int] = owned
    read: () -> int = () => view + 1
    seen = read()
    owned += 10  # accepted: after the closure's last call
    return seen + owned


def label_width(name: Borrowed[str], pad: int) -> int:
    width: (int) -> int = (extra) => len(name) + pad + extra  # accepted: holds a reference to name
    return width(1) + width(2)
```

A closure that would outlive a place it borrows, by being returned from the body, stored in a value that outlives that place or passed to a callee that keeps it, is refused with `INCAN-T0123`. A callable type preserves the explicit modes and result sources of the callables it describes. An explicitly written callable annotation that cannot express a dependency of the closure it receives is refused with `INCAN-T0123`, naming the missing source relationship.

## Native calls

A native method or function's Rust signature gives each input its access: a `&self` receiver or `&T` input is `Borrowed[T]` access from existing storage, a `&mut self` receiver or `&mut T` input is exclusive access, and a value input or receiver is owned, so a non-Copy argument passed by value moves. An exclusive receiver's edits reach the caller, and calling a `&mut self` method while a shared borrow of the same value is still used is refused with `INCAN-T0126`. An associated function written on its type, as in `HashMap.new()`, takes no receiver.

```incan
from rust::std::collections import HashMap


def add(mut counts: BorrowedMut[HashMap[str, int]], key: str) -> bool:
    counts.insert(key, 1)  # accepted: moves key into the map
    return counts.contains_key("b")


def hits(counts: Borrowed[HashMap[str, int]], key: str) -> int:
    match counts.get(key):
        Some(count) => return count
        None => return 0
```

A native result that borrows its receiver or an input is expressed with the same Incan modes as any other borrowed result: a shared result is `Borrowed[T]` and an exclusive result is `BorrowedMut[T]`. `HashMap.get`, for example, carries a shared view in its `Option` result. The Rust signature supplies the relationship: receiver lifetime elision selects the receiver, otherwise ordinary elision selects the sole reference input, and a named result lifetime retains every input carrying that lifetime. `Option` and `Result` payloads and native owner types with lifetime parameters keep those loans until their last dependent use. A static result retains no caller-owned source. If the signature gives no admitted Incan owner, the call is refused with `INCAN-T0123`; the compiler never guesses an owner or clones the value. An Incan callable called from native code keeps the same access and source contract.

## Async bodies

An async function or method checks its explicit inputs as a synchronous one does. An awaited call borrows its inputs until the await completes, and the caller's source is usable again afterward. A borrow the body holds stays live across each suspension, so awaiting a call that changes the borrowed place before the borrow's last use is refused with `INCAN-T0126`. An async body can return a view; the call keeps its source borrowed until the awaited result's last use.

```incan
import std.async


class Buffer:
    pub items: list[int]


async def measure(buffer: Borrowed[Buffer]) -> int:
    return len(buffer.items)


async def grow(mut buffer: BorrowedMut[Buffer], item: int) -> int:
    buffer.items.append(item)
    return len(buffer.items)


async def total(mut buffer: BorrowedMut[Buffer]) -> int:
    first = await measure(buffer)
    grown = await grow(buffer, 4)
    return first + grown
```

A task retains every dependency it uses. Spawning a task that would outlive a borrow it holds, or that requires a thread-safety property a borrowed value lacks, is refused with `INCAN-T0123`.

## Library boundaries

A Loaf's published functions, methods and callable types carry their explicit modes and result sources by parameter identity, so renaming a parameter does not change which input a result depends on. A consumer's calls are checked against the published contract without the provider's bodies, exactly as calls to local declarations with the same contract. Metadata that cannot represent a contract is refused rather than erased.

## Diagnostics

`INCAN-T0126` reports permission conflicts, overlapping live borrows and use after ownership transfer. It identifies the conflicting operation and its source declaration, borrow origin or earlier move. When a reachable dependent use keeps a borrow live, that use is included as a related location. A method's shared receiver cannot grant an exclusive local field loan; a mutable receiver can grant one when no overlapping dependency remains live.

```incan
def update(mut value: Owned[int]) -> int:
    let view: Borrowed[int] = value
    value = 2  # refused: INCAN-T0126, overlapping live borrow
    return view
```

`INCAN-T0124` reports a missing or invalid result source: no source clause on a borrowed result, a root outside the callable's parameters or receiver, a source that cannot grant the result's access, a return that does not come from the source, and borrowed components that need independent sources.

`INCAN-T0123` reports an ownership boundary that the contract cannot express or prove: a stored field declared with an ownership mode, an ownership annotation used as a constructor, a borrow that would outlive its source, a callable annotation or native lifetime relationship that cannot represent a dependency, and suspension whose lifetime or thread-safety requirements cannot be proved. The program is refused; no copy, lease or weaker mode is substituted.

```incan
model Page:
    title: Borrowed[str]  # refused: INCAN-T0123, a field cannot name the storage it borrows
```

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

`INCAN-T0125` is emitted for a new local initialized from an existing local, parameter, stored field or known list/dictionary element whose meaning changes when ordinary bindings borrow their initializer place. Bare, `let` and `mut` declarations are included. Ordinary value-type annotations such as `int` do not suppress the warning. An explicit ownership annotation states its own access request and does not receive this warning. Calls, constructor results, computed properties and explicit clone results do not receive this warning. The warning does not alter runtime behavior. An explicit clone states an independent value.

## Related explanation

[Ownership declarations](../explanation/ownership_declarations.md) explains why explicit modes exist alongside inferred borrowing, how views differ from retained readers and how contracts cross library boundaries.
