# Ownership declarations

Most Incan code never names an ownership mode. Parameters are duckborrowed, `mut` decides whether a callee may write through its parameter, and the compiler infers whether a local aliases a place or owns a value. Explicit ownership declarations exist for the boundaries where that inference is not enough: a public API that hands out a view of its own storage, a function that must take ownership of its argument, or a method that returns exclusive access for a fluent chain. The [annotation reference](../reference/explicit_ownership.md) states the rules; this page explains the model behind them.

## Mode and type are separate

A `Page` can be owned, borrowed read-only or borrowed exclusively; the mode does not change the type. `Borrowed[Page]` is not a wrapper object around a page. The body of `def title_length(page: Borrowed[Page]) -> int` uses `page.title` exactly as it would with an ordinary parameter, and no wrapper is allocated at the call. The mode only states which access the caller grants and the body may use.

A Rust `&T` is the same contract as `Borrowed[T]`, and `&mut T` the same as exclusive borrowed access. Native signatures therefore need no translation layer: their access and lifetime relationships are read as the ownership contracts they already are.

## An explicit mode is an obligation

Inference may choose how to satisfy an explicit mode, but it may not weaken it. A body that only reads an `Owned[T]` parameter still takes ownership, because the signature promised the caller that the value is transferred. A shared input cannot become writable by adding `mut`, and an exclusive input stays exclusive even if the body never writes it.

When an obligation cannot be met, the program is refused at its source. The compiler never repairs a conflict by copying a value, copying a whole container, acquiring a lease or switching to a runtime ownership check. An author who wants an independent value writes the clone; an author who wants a retained reader uses an API that owns what keeps the reader valid. Keeping those operations visible keeps their cost and retention visible too.

## Views and retained readers

A returned view is access to existing storage. `def edit_address(mut self) -> Reference[Address] from self.address` gives the caller the customer's own address, so the caller's edits reach the customer and the customer's address stays borrowed while the view is used. The `from` clause is the published relationship: a caller knows which input, and which field of it, the result depends on without reading the implementation, and the implementation must prove that it returns from that place.

A view is only valid while its source is. Some consumers need something else: a cursor that outlives the local binding of the snapshot it reads, or a batch whose buffers stay valid after the reader advances. Those are owned values that hold whatever keeps them valid, such as a lease or retained buffers, and acquiring that retention is an explicit library operation. The language keeps the two contracts distinct; it does not turn an escaping view into a retained one behind the author's back.

## Dependencies follow the value

A borrow does not disappear when its value is wrapped. A view inside a `Result`, an `Option`, a tuple or a closure keeps its source borrowed until the last use of whatever carries it, through branches, loops, matches and `?`. Exclusivity is decided by that last dependent use on each path, not by the end of a block, so a source becomes usable again as soon as nothing that depends on it can still run.

Failure is not rollback. When `?` propagates an error, the changes made before the failure remain. An error that carries no borrow releases the source, so the caller can repair it and retry through the original owner; an error that carries a view keeps that view's source borrowed until the error's last use.

## Library boundaries

A Loaf's published metadata carries each callable's modes and result sources by parameter identity, so a consumer checks its calls without the provider's bodies and a renamed parameter does not change what a result depends on. Providers that carry ownership contracts use library manifest format 7, which older compilers refuse rather than silently reading a contract without its source relationships; providers without explicit contracts keep format 6. Compiled providers record the executable representation they were built with, and a provider built for a different representation is rebuilt with matching compiler assets rather than reinterpreted.

## Preparing ordinary bindings for borrowing

A mutable local initialized directly from a stored string field receives an independent value: `mut title = page.title` followed by `title = "new"` leaves `page.title` unchanged. Under place bindings, a `mut` local initialized from a place is a write-through borrow of that place and a bare or `let` local borrows it read-only. `INCAN-T0125` identifies each declaration whose meaning differs between the two, so code that relies on the independent value can state it with an explicit clone. Explicit ownership annotations are not affected: they state their own access.
