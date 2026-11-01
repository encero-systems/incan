# Why ownership declarations are introduced in stages

Explicit ownership declarations are being introduced in stages. The [annotation reference](../reference/explicit_ownership.md) lists recognized syntax and diagnostics; recognition does not enable executable ownership behavior. Existing unannotated functions retain their current semantics.

The parser preserves each result-source token for diagnostics and formatting. Static field paths are resolved through checked model and class declarations, including generic field substitution. Naming a valid path does not prove the returned value originates there: execution remains refused until ownership behavior is checked. An owned wrapper may retain a borrowed dependency, but recording that request does not prove the wrapper preserves its source.

Direct input annotations describe access to the underlying value. For example, the body of `def add(value: Borrowed[int]) -> int` can type-check `value + 1`; there is no generic wrapper object to unwrap. A `mut` binding cannot grant write permission to `Borrowed[T]`. Declaration and value-type checks alone do not enable execution: caller loans, transfers and native storage must agree. The first direct-input profile now admits complete straight-line source proofs through Body IR; its native qualification remains pending.

## Preparing ordinary bindings for borrowing

A mutable local initialized directly from a stored string field currently receives an independent value. The migration warning identifies bindings whose meaning will change: a mutable declaration will become a write-through borrow; bare and `let` declarations will borrow read-only. Compatible annotations do not request a copy. Use an explicit clone when an independent snapshot is intended. The warning release must be available before a subsequent release changes ordinary binding semantics; shipping both changes together does not provide a migration interval.

## Metadata and tooling

Checked contracts retain parameter slots, access modes, underlying types and canonical stored-field identities. Parameter renaming does not change which input a result depends on. Provider metadata, aliases and editor signatures preserve this relationship; imported signatures render `from` using the currently exported parameter name. Incomplete or unsupported ownership metadata is refused. Providers carrying these contracts use manifest format 7 so older compilers cannot silently ignore a source-only relationship; ordinary providers retain format 6. A provider containing explicit ownership contracts cannot be admitted for execution by this stage.

Valid free-function contracts are available from completed headers before any function body is checked. A later declaration and each overload retain their own contract; their meaning does not depend on the order in which bodies are checked. Parameter bindings cannot reinterpret an already resolved mode by shadowing its name.

The free-function checker-record profile is version 4. Its Incan-owned codec records declaration access modes and source relationships, canonical binding types, direct input calls, local loans, explicit owned initializer relationships, completed mutation facts and body-owned control-flow results. Native restoration analyzes the decoded flow graph and checks its retained success or failure state rather than copying current checker results into the restored record. Invalid infrastructure graphs retain their exact failure; unavailable source analysis retains its source anchor and reason. Old profiles cannot silently omit these facts. Unsupported types, pending mutation capabilities and other unrepresented source shapes are refused; provider metadata preserves richer declaration contracts separately.

Executable representation version 39 carries an explicit input-access tag separately from binding mutability and native collection ownership, plus a declaration-owned checked input profile. Its positional records are incompatible with older versions: providers must be rebuilt with matching compiler assets. Source-signature profile 2 and native callable symbols include this tag so shared, exclusive and owned inputs cannot reuse an inferred-access identity. The tag alone never authorizes execution. An admitted body also needs a supported profile version, its exact canonical owner and a complete matching slot/access inventory. Missing or contradictory profiles refuse, including after publication. Closure and default frames do not inherit their parent's authority. The native acceptance tests for this newly admitted source profile remain pending.

A caller with ordinary parameters can still need that profile because it invokes a function with explicit inputs. Native source-call selection therefore validates both declarations after resolving their canonical identities. Removing the caller's profile from persisted data does not turn the call into an ordinary inferred-access operation.

## Source checking and native qualification

The checker and Body IR admit a bounded direct-input profile. Native qualification remains a separate acceptance gate: source acceptance alone does not prove every scalar, aggregate or local-access path executes correctly. A valid parameter annotation or borrowed native signature cannot grant a calling body its own checked access proof. Native call resolution checks the caller independently of the selected callee, including persisted callers with ordinary parameters.

In the reference's `update` example, `view` establishes the borrow origin, replacing `value` conflicts with that live borrow, and the later return of `view` is the dependent use keeping it live. Source diagnostics can describe that conflict even when another restriction prevents execution of the body.
