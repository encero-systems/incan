# Why ownership declarations are introduced in stages

Explicit ownership declarations are being introduced in stages. The [annotation reference](../reference/explicit_ownership.md) lists recognized syntax and diagnostics; recognition does not enable executable ownership behavior. Existing unannotated functions retain their current semantics.

The parser preserves each result-source token for diagnostics and formatting. Static field paths are resolved through checked model and class declarations, including generic field substitution. Naming a valid path does not prove the returned value originates there: execution remains refused until ownership behavior is checked. An owned wrapper may retain a borrowed dependency, but recording that request does not prove the wrapper preserves its source.

Direct input annotations describe access to the underlying value. For example, the body of `def add(value: Borrowed[int]) -> int` can type-check `value + 1`; there is no generic wrapper object to unwrap. A `mut` binding cannot grant write permission to `Borrowed[T]`. Declaration and value-type checks alone do not enable execution: caller loans, transfers and native storage must agree. The first direct-input profile now admits complete straight-line source proofs through Body IR; its native qualification remains pending.

## Preparing ordinary bindings for borrowing

A mutable local initialized directly from a stored string field currently receives an independent value. The migration warning identifies bindings whose meaning will change: a mutable declaration will become a write-through borrow; bare and `let` declarations will borrow read-only. Compatible annotations do not request a copy. Use an explicit clone when an independent snapshot is intended. The warning release must be available before a subsequent release changes ordinary binding semantics; shipping both changes together does not provide a migration interval.

## Metadata and tooling

Checked contracts retain parameter slots, access modes, underlying types and canonical stored-field identities. Parameter renaming does not change which input a result depends on. Provider metadata, aliases and editor signatures preserve this relationship; imported signatures render `from` using the currently exported parameter name. Incomplete or unsupported ownership metadata is refused. Providers carrying these contracts use manifest format 7 so older compilers cannot silently ignore a source-only relationship; ordinary providers retain format 6. A provider containing explicit ownership contracts cannot be admitted for execution by this stage.

Valid free-function contracts are available from completed headers before any function body is checked. A later declaration and each overload retain their own contract; their meaning does not depend on the order in which bodies are checked. Parameter bindings cannot reinterpret an already resolved mode by shadowing its name.

The free-function checker-record profile is version 2. Its Incan-owned codec records access modes and source relationships in structured fields; old profiles cannot silently omit them. The profile remains limited to its supported scalar, list and callable type grammar with explicit ownership wrappers. Nominal, generic-owner and other unsupported record shapes are refused; provider metadata preserves those richer declaration contracts separately.

The developing input analysis also retains body-owned control-flow results. Version 2 cannot encode those results, so it refuses bodies containing them while continuing to preserve declaration contracts. A cached body must not silently lose ownership facts that a fresh check would produce. This limitation remains separate from the execution refusal.

Executable representation version 39 carries an explicit input-access tag separately from binding mutability and native collection ownership, plus a declaration-owned checked input profile. Its positional records are incompatible with older versions: providers must be rebuilt with matching compiler assets. Source-signature profile 2 and native callable symbols include this tag so shared, exclusive and owned inputs cannot reuse an inferred-access identity. The tag alone never authorizes execution. An admitted body also needs a supported profile version, its exact canonical owner and a complete matching slot/access inventory. Missing or contradictory profiles refuse, including after publication. Closure and default frames do not inherit their parent's authority. The native acceptance tests for this newly admitted source profile remain pending.

A caller with ordinary parameters can still need that profile because it invokes a function with explicit inputs. Native source-call selection therefore validates both declarations after resolving their canonical identities. Removing the caller's profile from persisted data does not turn the call into an ordinary inferred-access operation.
