# RFC 129: Shared or copied bindings and the `mut` parameter contract

- **Status:** Draft
- **Created:** 2026-09-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (Python-style generators)
    - RFC 016 (`loop` and `break <value>`)
    - RFC 023 (compilable stdlib; §6 implicit ownership and borrowing)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 128 (one `Callable[(A, B) -> R]` trait; callable types carry the `mut` parameter marker)
    - #121 (implicit ownership inference)
    - #1609 and #1611 (duckborrowing across units, and liveness)
    - #1773 (`mut` parameter fixes, `INCAN-T0117`, and the transitional refusal)
    - #1790 (the `mut` marker in function types)
- **Issue:** [#1846](https://github.com/encero-systems/incan/issues/1846)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

This RFC specifies the `mut` parameter contract and chooses inferred borrowing for writable local aliases: `mut items = cart.items` borrows the existing field, `items = replacement` writes through that borrow, and `mut items = cart.items.clone()` creates a separate local value. Explicit declarations introduce bindings; assignment uses the binding already resolved. Shadowing that name resolution can disambiguate produces a warning, while ambiguous names and conflicting access produce errors. Duckborrowing must preserve these meanings and must never silently clone an alias to make a borrow conflict compile. Python and Scala provide prior art for naming and mutation, while Incan stays Rust-like in borrowing and lifetime safety, consistent with its dual surface with Rust. Bare and explicit-`let` declarations from places borrow read-only; writable access requires `mut`, including for scalar places. Explicit cloning creates independent owned contents. The remaining Draft work concerns storage and lifetime boundaries, not these settled binding rules.

## Core model

1. **A name resolves to one binding.** An explicit `let` or `mut` declaration introduces a binding. Its initializer resolves in the preceding environment; the new binding is visible afterward. A resolvable shadow produces a warning.
2. **A writable alias borrows an existing place.** For an existing place, including a scalar field, `mut local = source` gives `local` writable access to that place. An annotation does not request a copy. The source must already permit mutation.
3. **Assignment writes through the resolved binding.** Assigning through a writable alias replaces the borrowed value; it never retargets the alias or creates an independent local. Assigning to an owned local replaces that local's value.
4. **Construction and explicit cloning create local values.** `mut local = source.clone()` uses the clone result as an owned local. The clone owns independent mutable contents, including nested contents; it must not retain a borrow into the source. Duckborrowing chooses a representation that preserves that independence.
5. **Borrows have checked lifetimes.** A borrow remains active while a later use needs it. Conflicting access and replacement of a borrowed parent are errors. An alias does not follow a replaced parent.
6. **Duckborrowing preserves meaning.** Representation choices may change only when observable behavior does not. A clone that would detach intended mutations is never a legal conflict repair.
7. **Read-only access is the default.** Bare and explicit-`let` first declarations from a place borrow read-only. Mutation through a name requires `mut`; this includes nested mutation. `let` does not request a copy.
8. **The parameter contract remains explicit.** Caller-visible `mut` parameters share the caller's value. Scalar parameters and imported Rust values have the separate rules below. This RFC does not silently change parameter rebinding through a new alias spelling.

## Motivation

### A shorter name should keep naming the same place

A method often introduces a local name for a field so that subsequent operations are easier to read. If appending through that name changes the field, but assignment to the same name silently detaches it, the two operations have different targets without an explicit declaration. The proposed writable alias keeps one target for both operations.

### The generated representation must not choose the meaning

The aliasing defects tracked by #1773 and #1814 exposed spelling-dependent behavior for caller-visible parameters: an unannotated binding could inherit a reference, while an annotated binding or a reassignment could produce an owned copy. Mutation analysis could then miss changes, or a later parameter use could fail in generated code after checking succeeded. These are implementation defects, not a language contract. The transitional refusals in that work contain the affected parameter forms until a replacement contract is implemented.

RFC 023 §6 requires that ownership decisions preserve user-visible behavior. This RFC keeps that invariant: sharing is part of the source meaning, and a representation choice may not erase it.

### Safety belongs in the source diagnostic

Replacing an object while a later operation still needs a borrow of one of its fields is a conflict. Automatically copying the field would remove the conflict by redirecting the later mutation. A source diagnostic must explain the invalidating operation and later use instead. The author can finish using the alias first, shorten its lifetime, or explicitly request a clone.

## Goals

- Define writable local aliases, assignment through them, explicit copies, and declaration-based shadowing with realistic examples.
- Apply the same naming and assignment principles to collections and custom objects.
- Infer borrowing without requiring source-level reference or lifetime annotations.
- Keep borrowing, exclusive access, and lifetime safety Rust-like across Incan's dual surface with Rust; use Python and Scala as prior art for readability and naming, not as competing ownership contracts.
- Reject conflicting access at the Incan checker, with diagnostics that explain how long the borrow is needed.
- Specify the `mut` parameter contract, including caller-visible changes and conservative mutation analysis.
- Keep the remaining decisions explicit rather than presenting a partial borrowing contract as complete.

## Non-Goals

- Automatic alias retargeting when an owner is replaced.
- A silent clone fallback for a semantically shared writable alias.
- Reproducing Python assignment or Scala binding semantics exactly.
- Introducing runtime reference counting as the default identity model.
- Implementing the proposal in this documentation PR or claiming that the examples already compile.
- Deciding storage, whole-parameter replacement, suspension, or escape boundaries by implication.

## Guide-level explanation

The examples below describe the proposed behavior. Start with the cart examples for assignment, then read shadowing and borrow conflicts. The reference section defines the rules; Unresolved questions records where the Draft is still incomplete.

### Borrow the cart's field

```incan
class Cart:
    items: list[str]

    def add(mut self, item: str) -> None:
        mut items = self.items
        items.append(item)  # changes self.items

    def replace(mut self) -> None:
        mut items = self.items
        items = ["pear"]    # replaces self.items through the borrow
        items.append("orange")
        # self.items is now ["pear", "orange"]
```

The local alias belongs to the method's scope; `self.items` names the field. Matching their final name components is not a collision and needs no warning. Assignment through `items` keeps addressing the borrowed field. It does not turn `items` into an independent local.

### Request a separate value with clone

```incan
def prepare(cart: Cart) -> None:
    mut items = cart.items.clone()
    items.append("pear")
    items = ["orange"]
    # cart.items is unchanged
```

The initializer is a clone result, not a borrowed place. The new local owns that result. `.clone()` is the preferred spelling for an explicit copy; constructor-style copies such as `list(cart.items)` remain available where the type supports them. Cloning an enclosing object must not leave its mutable interior borrowed from the source. If a type cannot provide the required independent clone, the operation must be diagnosed rather than silently retain shared mutable contents.

### A new declaration can shadow an alias

```incan
def prepare(mut cart: Cart) -> None:
    mut items = cart.items
    items.append("apple")

    mut items = items.clone()  # warning: shadows the previous items
    items.append("pear")
    items = ["orange"]

    println(items)            # ["orange"]
    println(cart.items)       # includes "apple", not "pear" or "orange"
```

The clone initializer reads through the old alias. After the declaration, `items` resolves to the new local. The original field remains available as `cart.items`. Assuming there are no other uses or captures, evaluating the clone is the old alias's last use, so its borrow can end there.

An explicit `let` can likewise introduce an immutable local clone:

```incan
mut items = cart.items
let items = items.clone()     # warning: shadows the alias
```

This is different from assignment:

```incan
mut items = cart.items
items = items.clone()         # writes the clone back into cart.items
```

The assignment introduces no new binding. Omitting `let` when first introducing a name does not change its meaning: a place initializer is borrowed read-only, while an explicit clone or constructed value is owned. Explicit `let` introduces a new binding when shadowing is intended; plain assignment to an existing name does not.

### Read-only access and scalar places

```incan
items = cart.items        # read-only borrow
let same = cart.items     # equivalent explicit declaration
items.append("pear")     # error: mutation requires mut
```

Read-only access applies through nested fields too. A fixed local name does not grant permission to mutate an object merely because its type offers a mutating method.

```incan
mut quantity = order.quantity
quantity += 1            # increments order.quantity
quantity = 10            # replaces order.quantity
```

The scalar field follows the same writable-place rule as a list or custom-object field. This local borrowing rule is separate from the scalar parameter-passing contract.

```incan
mut copied = cart.clone()
copied.items.append("pear")
# cart.items is unchanged; the clone's interior is independent
```

### The same distinction applies to custom objects

```incan
class Basket:
    cart: Cart

    def replace(mut self) -> None:
        mut cart = self.cart
        cart = Cart(items=["pear"])  # replaces self.cart

    def prepare(mut self) -> None:
        mut cart = self.cart
        mut cart = Cart(items=["pear"])  # warning: shadows the alias
        cart.items.append("orange")
        # the separate local cart changes; self.cart is unchanged
```

The distinction is declaration versus assignment and place versus constructed value, not list versus custom object.

### A borrow cannot outlive replacement of its parent

```incan
mut cart = Cart(items=["apple"])
mut items = cart.items

cart = Cart(items=["pear"])  # error: items still borrows a field of cart
items.append("orange")      # this later use keeps the borrow active
```

The compiler must not redirect the alias to the new cart or silently clone the old list. Moving the replacement after the last alias use is accepted:

```incan
mut cart = Cart(items=["apple"])
mut items = cart.items

items.append("orange")      # last use of the borrow
cart = Cart(items=["pear"])  # accepted
```

### Conflicting access is separate from a name collision

```incan
mut items = cart.items
println(cart.items)         # error: separate access to an exclusively borrowed place
items.append("orange")
```

Reading through the alias is accepted:

```incan
mut items = cart.items
println(items)
items.append("orange")
```

Two different names can still conflict:

```incan
mut first = cart.items
mut second = cart.items    # error: conflicting writable borrow
first.append("apple")
second.append("pear")
```

The names are unambiguous, but the accesses conflict. Name shadowing may warn; borrow conflicts must error. Fields proven disjoint need not conflict, while an operation on their whole parent overlaps each of them.

### Parameters keep their own contract

```incan
def tag(mut tags: list[str], label: str) -> None:
    tags.append(label)      # reaches the caller


def countdown(mut remaining: int) -> int:
    remaining -= 1         # changes only the callee's scalar copy
    return remaining
```

A changed caller-visible parameter needs an argument the caller may change, except that an unshared temporary is accepted. Direct rebinding of a caller-visible parameter remains refused under `INCAN-T0001`; replacing a field through a local alias is a distinct operation. Whole-parameter replacement through a local alias must not become an accidental bypass of that refusal.

## Reference-level explanation

### Terms

- **Binding:** a declaration identity in a lexical scope. Shadowing creates another binding with the same spelling; it does not alter the earlier binding's identity.
- **Place:** an assignable location named by a binding or projected from it through a field or element access. Calls and constructed values are not places. The treatment of statics and other currently restricted sources must follow the applicable mutation permissions.
- **Writable alias:** a binding that borrows a place exclusively and writes through to that place. It is not an owned copy or a path that follows later replacement of its parent.
- **Owned local:** a binding containing its own value, such as a constructor or clone result. Assignment replaces that local's value.
- **Overlap:** two accesses overlap if they can address the same storage or one can invalidate the other's storage. A whole object overlaps its fields. Distinct fields may be proven disjoint; element accesses must be treated conservatively unless disjointness and stability are proved.
- **Caller-visible parameter:** an ordinary `mut` parameter, excluding `*args` and `**kwargs`, whose expanded type is neither `int`, `float`, nor `bool`, and is not an imported Rust type. `self` in a `mut self` method is caller-visible too.

### Declaration, resolution, and shadowing

1. An explicit `let` or `mut` declaration must introduce a new binding. The initializer must resolve names before that binding is introduced. Subsequent uses must resolve against the environment containing the new binding.
2. A declaration that shadows an existing accessible local binding or parameter must produce a warning when lexical rules determine the result. An unresolved ambiguity must produce an error rather than choose a target from the type needed by a later operation. Other invalid declarations, such as incompatible duplicate type members, are not legalized by this local-shadowing rule.
3. An unqualified local name and an explicitly qualified field such as `items` and `self.items` must not produce a collision warning merely because their final components match.
4. Plain assignment to a resolved existing binding must use that binding. It must not introduce a new binding, detach an alias, or retarget it. Explicit declarations are required for intentional shadowing.
5. A previously created alias must remain tied to its resolved binding and place; a later declaration with the same spelling must not redirect it. Shadowing alone does not end a borrow that is still used through another alias or closure.
6. A warning must identify the new declaration and the binding it shadows. An ambiguity error must identify the competing candidates and offer qualification where possible. Borrow and permission violations remain errors even when all names are unambiguous.

### Read-only local borrowing

1. A bare first declaration `name = place` and an explicit `let name = place` must both borrow the place read-only. Neither spelling requests a clone. A constructed or explicitly cloned initializer instead supplies an immutable owned local.
2. A read-only binding must not permit assignment or mutation through it, including mutation through nested fields or elements. Writable access requires `mut` and permission from the source; an interior mutable object does not bypass that rule.
3. Multiple read-only borrows may coexist. While one remains needed, overlapping mutation, exclusive borrowing, or invalidation must be refused. The last-use and provenance rules below apply to read-only as well as writable borrows.

### Writable local aliases

1. For an existing place `p`, `mut name = p` must create a writable borrow of `p`. This applies to scalar places as well as collections, models, and classes. An explicit compatible type annotation must not turn it into a copy. Imported types must respect their access and ownership contracts; inference cannot grant unsupported mutation rights.
2. The source must permit mutation. Borrowing must not grant mutation rights absent from the source or upgrade a read-only access into writable access. An immutable source must not be silently moved merely to make a writable alias legal.
3. In-place mutation and assignment through the alias must act on the borrowed place. For example, `items = replacement` must replace the borrowed field when `items` borrows `cart.items`. Compound assignment must likewise target the borrowed place where the operation is supported.
4. Assignment through an alias must leave its target unchanged. If the right-hand side is another place, duckborrowing must plan the value transfer without reborrowing or retargeting the left-hand alias implicitly. Any move or copy must preserve source-observable behavior and the destination's ownership contract; an impossible transfer must produce a diagnostic, not silently change sharing.
5. An initializer that constructs a new value or explicitly clones a value must create an owned local result, rather than borrow the initializer's receiver. An explicit clone must own independent mutable contents, including nested mutable contents; it must not retain source borrows. Unsupported cloning must be diagnosed. Generic bounds and representation choices must enforce this contract rather than weaken it.
6. A derived alias must preserve the original target identity and access permissions. Reborrowing through a writable alias may temporarily suspend use of the parent alias, subject to the same conflict checks. It must not manufacture two independently usable exclusive accesses.
7. These rules must apply consistently to field places in collections and custom objects. Borrowable element projections, selection expressions, and destructuring require the remaining rules listed under Unresolved questions; this Draft does not silently classify unsupported shapes as copies.

### Borrow duration and conflicts

1. A borrow must remain active for every use that depends on it. The analysis must include control-flow joins, repeated loop bodies, derived aliases, and closures that retain access; textual last occurrence within one block is insufficient.
2. While a writable borrow remains needed, separate overlapping access must be refused. Access through the borrow or a valid reborrow is permitted. Replacement or movement of an ancestor that would invalidate a later use must be refused.
3. Proven disjoint field access may be accepted. Collection operations that can invalidate an element borrow must be refused while it remains needed. Unknown overlap or lifetime safety must not be accepted by guessing or silently cloning.
4. After the last dependent use, the borrow may end before the lexical scope ends. Legal access after that point must not be rejected solely because the alias declaration is still in scope.
5. A conflict diagnostic must identify the borrow declaration, conflicting operation, and later use or capture that keeps the borrow active. Hints may suggest completing the alias's work first or explicitly cloning when independence is intended; they must not describe cloning as behavior-preserving in all cases.
6. The checker must report these errors as Incan diagnostics. A program accepted under the completed rules must not defer an alias-induced invalidation or overlap failure to the host compiler.

### Duckborrowing

The compiler may infer a borrow, move, or copy only when it preserves the source contract. An owned local's unobservable copy may be optimized away. A writable alias's observable sharing must not be optimized away. In particular, the compiler must not turn an alias into a copy to permit owner replacement, overlapping access, suspension, escape, or an unsupported expression shape. When the source requests conflicting access, the result must be a diagnostic.

### The `mut` parameter contract

1. A parameter declared `mut` is mutable within the body. A caller-visible parameter gives access to the caller's value; every permitted in-place change, field or element assignment, and forwarding to another mutating parameter must reach the caller.
2. A `mut` parameter that is not caller-visible is the function's own value. Scalar parameters are local copies. Imported Rust values follow their ownership contract rather than becoming caller-visible solely because their binding is `mut`.
3. Rebinding a caller-visible parameter directly, including compound assignment to its whole name, must remain refused under `INCAN-T0001` until an explicit design decision changes this contract. A local alias of the whole parameter must not bypass that restriction. Changes to a field or element are not whole-parameter rebinding.
4. For a caller-visible parameter a call changes or may change, the argument must be a permitted mutable place: a mutable binding, a caller-visible parameter, `self` in a `mut self` method, or an allowed field of one. Immutable arguments, collection elements, and statics retain the existing `INCAN-T0117` refusal; an unshared temporary must be accepted. Extending local element borrowing does not implicitly change this call-argument rule.
5. A method call counts as a change unless the method is known only to read its receiver. Unresolved methods count conservatively. A callee reached through a trait bound, trait default, trait-typed value, mut-marked callable type, unread compiled body, or reassigned function-valued local may change the marked parameter. A known unreassigned function value may use its body's effect information.
6. Mutation through a local alias rooted at a caller-visible parameter must count as mutation of that parameter. Derived aliases must not hide that effect. Passing it to Rust or C exclusive access must count too.
7. A closure retaining a caller-visible parameter must not outlive that call. A returned, stored, or yielded closure must not retain such a borrow without an explicit, checked escape contract; this RFC does not introduce one.

## Design details

### Syntax and the alias keyword

The selected writable spelling is `mut local = place`; an explicit clone is `mut local = place.clone()`. This proposal does not add `&`, `&mut`, or lifetime annotations to ordinary source. The existing `alias` marker keeps its declaration-level roles for top-level symbols, same-type methods, and enum variants. Model-field alias metadata remains a separate naming and wire-mapping feature. This RFC does not extend `alias` into function or method bodies: local writable borrowing is inferred from `mut local = place`, so no second explicit local alias spelling is introduced.

Explicit declarations and assignment are distinct even where `let` is optional for introducing an unused name. Bare and explicit-`let` first declarations have the same meaning: read-only borrowing from a place, or an immutable owned local from an explicit clone or constructed value. `let` does not implicitly request a clone.

### Spelling and behavior

| Spelling | Proposed meaning |
| --- | --- |
| `mut items = cart.items` | Writable borrow of the field |
| `mut items: list[str] = cart.items` | The same writable borrow |
| `items = ["pear"]`, where items is that alias | Replace the borrowed field; keep the alias target |
| `mut items = cart.items.clone()` | Owned mutable local containing the clone result |
| `let items = cart.items.clone()` | Owned immutable local containing the clone result |
| `mut items = items.clone()`, with an existing items | Clone the old binding, then shadow it; warn |
| `items = items.clone()`, where items is an alias | Write the clone back through the existing alias |
| `cart = Cart(...)`, followed by a use of an alias into cart | Borrow conflict error |
| Bare or explicit-let binding directly from cart.items | Read-only borrow; mutation through it is refused |
| `mut quantity = order.quantity` | Writable borrow of the scalar field |

### Prior art and the Rust-like semantic direction

Incan stays Rust-like in borrowing, exclusive access, and lifetime safety. Python and Scala are prior-art references for readable names, declaration, and mutation; their reference-sharing behavior is not the ownership contract selected here. This alignment supports Incan's dual surface with Rust: authors moving between Incan and Rust should encounter compatible rules for access and invalidation, even when Incan infers syntax that Rust writes explicitly. RFC 097 describes the Rust-hosted caller boundary; this RFC does not claim that every cross-surface feature is already implemented.

**Rust is the semantic reference for borrowing.** A writable alias corresponds conceptually to a mutable borrow. The compiler rejects overlapping access or owner replacement while a later alias use still needs that borrow. Incan infers the borrow and makes assignment through the alias implicit; Rust spells these operations with `&mut` and dereferencing. Incan's `mut local = place` is therefore not a literal translation of Rust's ordinary owned `let mut` binding. The different source spelling must preserve the relevant exclusivity and lifetime guarantees. See [Rust references and borrowing](https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html).

**Python is prior art for giving an existing object another readable name.** In Python, `items = cart.items; items.append("apple")` mutates the shared list, but `items = ["pear"]` rebinds only the local name. The proposed Incan writable alias deliberately differs on assignment: it writes to the borrowed place. Python's local rebinding behavior must not be cited as evidence for Incan's write-through rule. See [Python assignment statements](https://docs.python.org/3/reference/simple_stmts.html#assignment-statements).

**Scala is prior art for separating binding reassignment from object mutation.** `val` fixes a binding and `var` permits reassignment; the collection type determines whether its contents can change. Neither declaration requests an independent copy. This distinction helps state the questions separately, but does not make Scala's reference model the Incan borrowing contract. See [Scala bindings](https://docs.scala-lang.org/scala3/book/taste-vars-data-types.html) and [Scala collection mutability](https://docs.scala-lang.org/overviews/collections-2.13/overview.html).

**Explicit cloning must have an honest contract.** Rust's [Clone contract](https://doc.rust-lang.org/std/clone/trait.Clone.html) shows why copying a wrapper does not necessarily make all reachable state independent. Incan explicitly requires an independent clone here: its mutable interior must not remain borrowed from or shared with the source. An imported clone operation that only duplicates a shared handle cannot silently satisfy that contract. This is a deliberate semantic requirement, not an assumption that Rust's method name guarantees it.

### Relationship to RFC 023

The invariant that ownership decisions must not change user-visible behavior remains in force. This RFC narrows the clone fallback: it cannot apply to source-level writable aliases when a clone would change sharing. Such cases must produce a borrow diagnostic. For owned values, the planner retains its freedom to choose an equivalent representation.

RFC 023's suspension policy and this RFC's proposed last-use borrowing need a separate decision for `await` and `yield`. Rust-style borrowing does not by itself imply a blanket ban on every borrow across suspension. This Draft neither promises support for every such borrow nor permits cloning an alias to evade the problem. Closed RFCs remain unchanged; the eventual accepted contract must record any precise supersession here.

### Transition and migration

The transitional refusals tracked by #1773 and #1814 protect the affected caller-visible-parameter forms while the replacement contract is incomplete. Draft promotion alone must not remove those checks: they must be replaced only when equivalent source diagnostics and the selected semantics are implemented and verified.

The transition is not a compatibility guarantee for ordinary locals. Code that currently copies `mut other = original` can change meaning when that declaration becomes a writable borrow. Existing code may also become invalid because it overlaps accesses. Adoption requires a warning release identifying affected declarations before their meaning changes, with explicit cloning for independent values and direct mutation or a checked borrow for shared access. A warning cannot make an actual borrow conflict safe in the new semantics.

The current whole-parameter rebinding restriction remains separate. `clear()` on `list`, `dict`, and `set` remains a proposed in-place operation for callers that need to empty collections without rebinding a restricted parameter; it must remove all elements while preserving the collection as the mutation target. It does not determine alias semantics or add a new `.copy()` contract.

## Alternatives considered

- **Independent values for every local binding.** Predictable once taught, but a short mutable name for a field silently becomes a detached value unless every such use explicitly requests sharing. It is not the selected meaning for `mut local = place`.
- **Values with observable-copy refusals.** This protects some Python expectations by refusing implicit copies, but direct source-use checks miss sibling and transitive relationships. Two bindings copied from one source can diverge observably without a later use of that source. It also rejects ordinary snapshot and aliasing idioms. This is not the selected direction.
- **Writable views whose assignment retargets the view.** Rejected because mutation would address the original place while assignment silently changes what the local name addresses. A new declaration should be visible when a new local is intended.
- **Aliases that follow a replaced parent.** Rejected for writable borrowing. Replacing a borrowed parent while a later alias use needs it must be a conflict, not an instruction to follow a new object.
- **Explicit reference syntax.** Rust-style `&mut` can express the borrow, but the selected Incan surface infers it from the declaration and place. The absence of reference syntax must not remove safety checks.
- **Runtime shared identity everywhere.** A possible different language contract, but it requires decisions about storage identity, concurrency, and runtime checking that this local-borrowing proposal does not make.

## Drawbacks

- A reader must learn that `mut local = place` borrows, while `mut local = place.clone()` introduces an owned value. Familiar-looking syntax alone does not establish that distinction.
- Borrow conflicts become source-level errors even without reference syntax. The checker and diagnostics must explain them in terms of the user's names and operations.
- Correct last-use and overlap analysis must follow branches, loops, projections, and closures; per-block read counts are insufficient.
- Same-scope shadowing can hide a useful alias, even when deterministic. The warning must identify both declarations without warning on ordinary qualified-field aliases.
- Migration changes existing copy behavior and needs advance diagnostics. The transitional parameter checks do not eliminate this cost.
- Storage and lifetime boundaries still need specification. These remaining Draft questions must not reopen the settled read-only, scalar-borrowing, and independent-cloning rules.

## Implementation architecture

This section is non-normative.

The compiler should retain resolved binding identity, place projections, access permissions, and borrow provenance through semantic analysis. Shadowing changes name lookup; it does not rewrite previously resolved aliases. Last-use and overlap facts should support both source diagnostics and ownership planning, including derived aliases and captures.

A writable alias can lower to a checked reborrow; assignment through it lowers to a write to its target. The compiler need not hold a physical reference longer than required, but any shorter representation must preserve the source borrow contract. It must never recover from invalidation by recomputing a path against a replacement parent or cloning the aliased value. The liveness work tracked by #1611 is relevant evidence, not proof that this contract is already implemented.

## Layers affected

- **Typechecker / Symbol resolution:** declaration identity, initializer scope, same-scope shadowing warnings, ambiguity errors, alias classification, permissions, mutation effects, and conflict/lifetime diagnostics.
- **IR Lowering:** explicit alias provenance and write-through assignment rather than source-representation-dependent copying.
- **Ownership planning / Emission:** checked borrowing and lifetime-aware representation choices that preserve sharing; no conflict repair by cloning.
- **Diagnostics catalog:** stable codes and `incan explain` entries for shadowing, ambiguity, permission failures, and borrow conflicts alongside the parameter diagnostics.
- **LSP / Tooling:** hover and definition navigation distinguishing an owned local from a borrowed place; diagnostics connecting declarations, conflicting access, and later uses.
- **Stdlib:** proposed `clear()` operations and the eventual resolved cloning guarantees.
- **Docs:** binding and scope rules, mutation and parameter contracts, migration examples, and release notes when implemented.

## Acceptance criteria

The completed design and implementation must demonstrate:

- a list field and a custom-object field can be borrowed locally, mutated, and replaced through their aliases without retargeting;
- explicit cloning produces owned locals with independent mutable interiors; nested mutation of a cloned object does not reach the source, and unsupported independent cloning is diagnosed;
- bare and explicit-`let` declarations from a place borrow read-only, cannot mutate nested contents, and do not implicitly clone;
- writable scalar aliases update and replace their original fields just as writable collection or object aliases do;
- annotated and unannotated writable-place bindings agree;
- shadowing resolves its initializer through the old binding, warns, and resolves subsequent uses through the new binding;
- assignment never introduces a shadow or detaches an alias, and same-name qualified field access is not warned about merely for matching a local spelling;
- owner replacement before a later alias use is rejected, while replacement after the last dependent use is accepted;
- overlapping writable aliases, separate conflicting reads, invalid mutation permission, and invalidating element operations receive source diagnostics; proven disjoint fields remain usable;
- reborrows, branches, loops, and captures preserve borrow duration and mutation provenance;
- a source-level conflict never becomes accepted by an implicit clone or by following a replacement parent;
- caller-visible mutations reach callers, scalar parameters remain local copies, whole-parameter rebinding restrictions cannot be bypassed through aliases, and `INCAN-T0117` keeps its stated argument checks;
- `clear()` empties a caller-visible collection in place;
- each remaining decision has accepted and rejected examples, checker coverage, and native-build behavior coverage before the RFC is considered implemented;
- documentation and migration warnings explain the selected semantics, and generated code does not expose borrow failures for programs accepted by the completed checker.

## Design Decisions

- **Rust-like borrowing is the semantic direction.** Python and Scala are prior art for naming and mutation; borrowing, exclusivity, and lifetime safety remain aligned with Rust, supporting Incan's dual surface with Rust.
- **Inferred writable borrowing is the selected direction.** A mutable local initialized directly from an existing place borrows that place, including scalar fields; explicit cloning requests a separate local value.
- **Read-only borrowing is the default.** Bare and explicit-`let` declarations from a place borrow read-only; mutation, including nested mutation, requires `mut`. `let` does not request a clone.
- **Explicit clones own independent mutable interiors.** Duckborrowing must preserve this independence; a cloned object must not retain source borrows or silently share its mutable contents.
- **Scalar places use the same borrowing rule.** `mut quantity = order.quantity` writes through to the field.
- **Assignment writes through an alias.** It does not retarget the alias or detach it from the borrowed location.
- **Explicit declarations introduce bindings.** Deterministic shadowing warns, initializers resolve before the new binding is introduced, and subsequent uses resolve to the new binding.
- **Diagnostic severity follows the problem.** Resolvable name collisions warn; unresolved ambiguity, mutation-permission failures, and borrow conflicts error.
- **Aliases do not follow replacement parents.** Invalidating replacement is rejected while a later use still needs the borrow.
- **Duckborrowing must preserve observable sharing.** A conflict must not be repaired by silently cloning.
- **Lists and custom objects follow the same naming and assignment principles.** The proposal does not invent a special rebinding rule for lists.
- **No local `alias` extension.** The keyword keeps its existing declaration-level roles, and field alias metadata remains separate. Function and method bodies use inferred borrowing rather than a new `alias` form.
- **The RFC remains Draft.** The decisions below are not settled by choosing writable borrowing.

## Unresolved questions

The binding, mutation-permission, scalar-alias, and clone-independence rules above are settled. The following questions concern boundaries not resolved by the local test drives; implementation details of enforcing settled rules belong in the implementation work, not in this list.

- **Storage boundaries.** Define ownership transfer when an existing place is inserted into a field or collection: which uses move the value, retain a checked borrow, or require an explicit clone? In particular, specify whether a later use of the original remains valid. This must preserve the settled explicit-clone and no-silent-detachment guarantees.
- **Whole caller-visible parameter replacement.** Should assignment to a whole caller-visible parameter replace the caller's value, as assignment through a local writable alias does, or retain the existing rebinding refusal? Until resolved, a local alias must not bypass the existing restriction. This is separate from storing a value in a collection.
- **Mixed selection results.** Define a binding whose `match`, `if`, or `loop` result selects a borrowed place on one path and a constructed or cloned value on another. The rule must preserve each path's ownership meaning or diagnose an unsupported combination; it must not silently copy to make the shapes agree.
- **Projection and destructuring coverage.** Complete the rules for tuple unpacking, loop targets, and element projections, with accepted and rejected examples for overlap and invalidation. These are extensions of the chosen borrowing rules, not a new choice between copying and sharing for scalar or object fields.
- **Suspension.** Define which otherwise-valid borrows may cross `await` or `yield`, including task-safety requirements and any precise change to RFC 023's suspension policy. Cloning must not be used to evade an invalid borrow.
- **Escape and borrowed returns.** Define when a closure or returned value may retain a checked borrow, including how its lifetime relates to the original argument or owner. A borrow of a destroyed local must never escape.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
