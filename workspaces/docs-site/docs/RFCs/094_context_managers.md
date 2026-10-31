# RFC 094: Context managers

- **Status:** Draft
- **Created:** 2026-05-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language semantics)
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 036 (user-defined decorators)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 055 (`std.fs`)
    - RFC 070 (ordinary Result combinators)
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 093 (`std.telemetry`)
    - RFC 095 (`span` vocabulary blocks)
    - RFC 122 (scoped validation policy)
    - RFC 129 (binding, borrowing, and ownership rules)
    - RFC 131 (typed error values and propagation)
    - RFC 134 (exiting pattern bindings inside resource scopes)
    - RFC 147 (independent local Result matching)
- **Issue:** [#560](https://github.com/encero-systems/incan/issues/560)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC adds context managers to Incan: a general scoped-lifetime construct for resources that must be entered before a block and exited after the block regardless of how control leaves it. The user-facing syntax is Python-familiar, but the semantics are intentionally closer to Rust's scope-based cleanup model: context managers guarantee a cleanup attempt, report an exit outcome, and must not catch or suppress the body's failure. A manager may declare typed fallible finalization, whose failure must prevent an otherwise successful operation from being reported as successful. The same mechanism also becomes the substrate that vocabulary blocks can target when a domain-specific block needs compiler-guaranteed scope entry and exit.

## Core model

Read this RFC as six foundations:

1. **Scoped lifetime is a language primitive:** users should be able to express "this value is active for this block" without manual `close()`, `end()`, `release()`, or paired setup/cleanup calls.
2. **Cleanup is guaranteed after entry:** once `__enter__` succeeds, `__exit__` must run exactly once when the block exits, including fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits.
3. **Cleanup is distinct from Result handling:** the manager must not suppress a failure or catch a panic. Local typed Result matching is explored independently in RFC 147; ScopeExit remains informational and cleanup-oriented.
4. **The protocol is explicit and typed:** context-manager support is ordinary Incan API surface, not hidden naming magic that only the compiler understands.
5. **Vocabulary blocks may desugar through context managers:** domain-specific soft syntax can use the same scoped cleanup contract instead of inventing one-off entry/exit behavior.
6. **Async safety is explicit:** context managers that interact with task-local state, guards, locks, spans, or transactions must not be accidentally held across suspension points without a type-level contract.

The purpose of with is a scoped resource lifetime or temporary runtime state that must be finished or restored. An ordinary fallible calculation or validation check needs no with statement merely because it returns Result; ordinary matching and RFC 070 combinators remain available. Local Result capture and except handling are separate design work under RFC 147, not prerequisites for this lifetime contract.

## Motivation

Many useful APIs have paired lifecycle calls: open/close, acquire/release, begin/commit-or-rollback, enter/exit, capture/restore, start/end. Manual pairing is easy to get wrong, especially when a block has early returns or `?` propagation. Incan already has a Python-influenced surface, so users will expect a first-class scoped resource construct once the stdlib grows beyond simple values.

The language also needs a principled substrate for scoped vocabulary forms. Telemetry spans are the immediate example, but they are not the only one. A `span` block should not be a special cleanup snowflake in the parser; it should use a general scoped-lifetime hook. Tests, temporary directories, output capture, transactions, locks, and scoped runtime policy all want the same guarantee.

Rust gives useful prior art here without requiring Rust syntax. Rust uses lexical scope and the `Drop` trait to run cleanup when values leave scope. That design is not Python's `with`, but it proves the central idea: cleanup should be tied to scope and ownership, not remembered by convention. Incan should expose the idea in a Python-readable form while keeping Rust's refusal to treat cleanup as exception suppression.

## Goals

- Add with statement syntax for scoped context managers.
- Define a typed `ContextManager[T]` protocol.
- Define `ScopeExit` as the informational outcome passed to context-manager cleanup.
- Guarantee cleanup after successful entry on all block exit paths.
- Prohibit context managers from suppressing the body's failure or arbitrarily rewriting control flow, while allowing declared typed finalization failure to prevent successful completion.
- Expose fallible finalization in the checked context-manager contract without requiring every manager to be fallible.
- Allow context-manager lowering to serve as the substrate for descriptor-gated vocabulary blocks.
- Define async interaction rules conservatively enough that task-local guards and telemetry spans are not accidentally mis-scoped.
- Provide a clear model for stdlib resources such as files, locks, temporary directories, captured output, and scoped configuration.

## Non-Goals

- Adding general try blocks, exception throwing, panic-catching handlers, or a catchable exception runtime.
- Defining local except matching or a general scope-result capture API; RFC 147 owns independent Result matching.
- Copying Python's `__exit__` suppression behavior.
- Making all cleanup fallible or forcing cleanup errors into every function signature.
- Defining the `span` vocabulary block in this RFC.
- Defining transaction, lock, file, telemetry, or testing APIs beyond the context-manager contract they may implement.
- Making `with` a general macro system.
- Allowing imports to mutate global parsing outside descriptor-gated vocabulary positions.
- Leaving async context managers underspecified when the async safety contract affects correctness.

## Guide-level explanation

The examples describe proposed `with` behavior and proposed context-manager integration for existing resource types. The acquisition and operation spellings for `Path`, `File`, and `TemporaryDirectory` follow their current source APIs; those types do not thereby already implement this RFC. Helpers such as `capture_output`, `temporary_env`, `build_report`, and `publish_report` stand for application-defined operations.

The common shape is a scoped resource acquired through an explicit fallible call:

```incan
with Path("events.log").open("a")? as file:
    file.write("started\n")?
    file.flush()?
```

The `open` call returns a Result. The header's `?` propagates an acquisition failure before the context is entered. On success, the proposed manager integration enters the resource, binds the entered value to `file`, runs the block, and exits the context when the block completes. The explicit `flush()?` keeps a fallible flush visible; context cleanup does not silently convert it into an infallible operation.

A with statement establishes a resource lifetime, not a local error handler:

```incan
captured = capture_output()
with captured:
    run_command()?

assert captured.stderr == ""
```

If `run_command()?` propagates an error, cleanup still runs and then the original propagation continues. The context manager can observe that the block exited by error propagation, but it cannot swallow that propagation.

Some context managers do not need an `as` binding:

```incan
with temporary_env({"INCAN_LOG_LEVEL": "DEBUG"}):
    run_worker()
```

The block activates the manager for its duration, then restores the previous state.

Multiple resources can be nested explicitly:

```incan
with TemporaryDirectory.try_new()? as directory:
    with (directory.path() / "report.txt").open("w")? as file:
        file.write(report)?
        file.flush()?
```

This RFC does not need multiple-manager header syntax. Explicit nesting is unambiguous, easy to format, and enough to define the semantics.

### Test drive: existing drop-based resources

Temporary directories already remove their backing tree when their non-persisted owning wrapper is dropped. A context manager adds an explicit source boundary; it must not be presented as the first cleanup mechanism for these resources:

```incan
with TemporaryDirectory.try_new()? as workspace:
    build_report(workspace.path())?
    publish_report(workspace.path())?

continue_with_other_work()
```

The intended resource scope ends before the final helper runs. If building or publication propagates an error, the entered context still exits and the original propagation continues. An actual stdlib adaptation must establish how this boundary releases the resource's ownership, rather than merely emit an exit method while retaining the live resource elsewhere.

An owned path is not the resource itself:

```incan
def scratch_path() -> Result[Path, IoError]:
    with TemporaryDirectory.try_new()? as workspace:
        return Ok(workspace.path())
```

The returned `Path` may remain a valid value while referring to a directory that cleanup removed. Keeping a path value alive does not retain a temporary directory. This is distinct from returning a borrow or live handle that depends on the scoped resource and would become invalid at exit.

### Test drive: deliberate persistence

The existing temporary-directory API has an explicit `persist()` operation. A future context adaptation must preserve its meaning:

```incan
def retained_report() -> Result[Path, IoError]:
    with TemporaryDirectory.try_new()? as workspace:
        build_report(workspace.path())?
        retained = workspace.persist()?
        return Ok(retained)
```

Successful persistence disables automatic removal of that directory. Context exit must not delete it anyway or attempt to release the same consumed handle twice. This is a resource-specific ownership transfer, not a general permission for arbitrary `as` bindings to escape.

### Test drive: locks and suspension

The current async mutex releases its guard when the guard is dropped. A proposed integration could make a short critical section visible:

```incan
with await shared_counter.lock() as guard:
    guard.set(guard.get() + 1)

await publish_update()
```

The lock acquisition suspends before context entry. The intended guard scope ends before publication suspends. This requires an adaptation that actually releases the held guard at the boundary. Whether an `await` is also permitted inside the suite depends on the manager's selected async-safety contract. The current async mutex supports guards held across await; the context protocol must not assume that every guard is unsafe or that every resource is safe to suspend.

### Test drive: the body's exit reason

Inside an entered context, these are two ways to leave a Result-returning function with a failure. Here operation is an application-defined Result-returning function and failure is an application error value:

```incan
operation()?
```

```incan
return Err(failure)
```

Both exits must report failure to cleanup. A transaction-like manager must not roll back for question-mark propagation but treat an explicit error return as success. The evaluated Result outcome determines failure, including when an ordinary return expression evaluates to Err rather than spelling Err directly. Diagnostics may additionally retain whether the exit used return or question-mark propagation; that distinction must not replace its failure classification. A failed Result merely stored or inspected inside the suite does not itself constitute a failed scope exit.

Vocabulary blocks may use the same mechanism while exposing domain syntax:

```incan
import std.telemetry.vocab

span "checkout.charge", attributes={"order.id": order.id}:
    receipt = charge(order)?
```

The `span` block is not defined here, but the important contract is that a vocabulary desugarer can target a context manager and receive the same cleanup guarantee as `with`.

### Result matching is independent

RFC 147 explores local typed Result matching through one except block with ordinary match-style arms. Its attachment syntax, value binding, aggregate matching, and propagation boundaries remain under review there. This RFC does not require an except block, scope capture expression, new outcome container, or per-failure dispatch mechanism to establish a resource lifetime. Its fallible-finalization propagation contract remains an obligation of the resource construct itself.

## Reference-level explanation

### Syntax

A `with` statement has one context expression, an optional binding, and an indented suite:

```incan
with expression:
    suite

with expression as name:
    suite
```

Local Result-matching syntax is outside this RFC and belongs to RFC 147.

The `with` token is a statement introducer. It must not change the meaning of `with` in type-parameter bounds or type conformance positions that already use the spelling.

The context expression must typecheck to `ContextManager[T]` or another type accepted by an explicitly defined context-manager adaptation rule. If an `as name` binding is present, `name` has type `T` only inside the suite. If no binding is present, no entered value is exposed by a binding; the manager must remain alive until exit. Omitting `as` must not release the manager early or skip its exit call.

### Protocol

The candidate infallible protocol is:

```incan
pub trait ContextManager[T]:
    def __enter__(self) -> T
    def __exit__(self, exit: ScopeExit) -> None
```

`__enter__` starts the scoped lifetime and returns the value exposed to the block. `__exit__` ends the scoped lifetime and receives the exit outcome. A successful cleanup preserves the original control flow. A manager must not suppress the body's failure, but a declared typed finalization failure may prevent otherwise successful completion. The candidate signature above describes infallible cleanup only. A manager declaring fallible cleanup instead gives its exit method the following result shape, where E is its declared error type satisfying RFC 131's common Error contract:

```incan
def __exit__(self, exit: ScopeExit) -> Result[None, E]
```

`Ok(None)` means cleanup succeeded; `Err(error)` reports its typed failure. This selects the fallible exit method's return shape. The complete generic protocol declaration and propagation contract remain open. Independent local Result matching belongs to RFC 147.

`ScopeExit` must distinguish at least these outcomes:

```incan
pub enum ScopeExit:
    Success
    Return
    Break
    Continue
    Error
    Panic
```

ScopeExit is a reason-only enum owned by std.scope. Error includes both question-mark error propagation and a return whose evaluated Result is Err. Return denotes other explicit returns, including an Ok result. The failure outcome takes precedence over the return spelling for cleanup classification. Detailed originals, diagnostic provenance and the exit mechanism remain separate from these reason tags.

The reason tags must not carry a Python-style exception triple or acquire payload-dependent suppression behavior. Panic remains distinct from a recoverable Error value.

### Entry and exit

The context expression must be evaluated exactly once before entering the suite.

If evaluating the context expression fails, `__enter__` is not called and `__exit__` is not called.

If `__enter__` fails, `__exit__` is not called because no scoped lifetime was successfully established.

If `__enter__` succeeds, `__exit__` must be called exactly once after the suite exits. This requirement applies to normal fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits.

Managers may declare typed fallible finalization using a Result error channel. If the body otherwise succeeds but finalization fails, the scope must report that failure rather than continue or return as though the complete operation succeeded. Fallibility must be visible in the checked manager contract, and the compiler must require valid handling or propagation. This does not require infallible managers to invent an error channel. The infallible `__exit__ -> None` form has no recoverable error channel; the fallible form returns `Result[None, E]`. The complete protocol declaration, including how E is bound, and the exact caller-facing handling surface remain open.

For example, a transaction-like manager may commit when its suite completes successfully. Inserts can succeed while the database subsequently rejects the commit. The caller must receive that finalization failure; successful execution of the insert statements is not proof that the transaction was saved. This scenario illustrates the context-manager contract and does not define a stdlib transaction API or select a universal commit-on-exit policy.

Every failure produced by the body or attempted finalization must be retained. If the body and cleanup both fail, neither error may replace, erase, or reduce the other to message text. Nested scopes must retain additional cleanup failures while preserving the required cleanup attempts for other successfully entered managers. Each retained typed error keeps its original identity, payload, diagnostic origin, upstream relationships, and any captured stack trace under RFC 131. Diagnostic presentation or propagation precedence must not determine which failures survive. Independent body and cleanup failures are aggregate members; retaining them together must not invent a caused_by relationship between them.

When only one work or finalization path fails, the original typed failure must propagate through a compatible enclosing error contract. When multiple paths fail, std.scope.ScopeFailure.Multiple must retain an immutable tuple of the original typed failures and per-member scope and phase provenance. The aggregate belongs to resource scopes, not to every Error. Reports must present work failure first, followed by attempted cleanup failures in actual inside-out order. An enclosing manager must still attempt cleanup and retain any additional failure. Independent members must not acquire a caused_by relationship merely because they are reported together. The complete generic protocol and caller propagation mechanics remain open; retention and reporting order are settled.

A panic in cleanup retains its ordinary runtime significance. Panic precedence and its carrier remain unresolved, but must preserve available failure diagnostics where the runtime can execute cleanup and reporting. Retaining diagnostics does not make a panic recoverable through Result. Fallible cleanup must not be described as error suppression or as proof that cleanup always succeeds.

The cleanup guarantee applies to exits for which the runtime can execute cleanup, including panic/assert unwinding. It cannot promise execution of user cleanup during immediate process abort or external process termination. This boundary must not introduce a catchable exception model.

### Scoped bindings and resource escape

The manager must remain available for its required exit call. The compiler must not move it away, clone it implicitly, or let an entered value consume it in a way that prevents that call. An `as` binding must follow ordinary mutation permissions and must not acquire writable access solely because it appears in a context header.

Borrowed values that would be invalidated by context exit must not escape the suite through return values, storage, closures, or tasks. Independently owned results may escape when ordinary ownership rules permit it. An owned path string or `Path` value must not be treated as a lifetime guarantee for the resource it names.

An adaptation promising resource release at exit must establish that retained handles or aliases cannot silently keep that same resource active beyond the promised boundary. A resource-specific operation such as temporary-directory persistence may transfer ownership deliberately; exit must respect that operation without double cleanup. The exact receiver and borrowing contract for entered values remains open below.

### Control flow

The manager must not catch or suppress the body's failure or catch panics. When cleanup succeeds, the body's control-flow decision remains authoritative: a `return` still returns, a `break` still breaks, a `continue` still continues, `?` propagation still propagates, and panic/assert exits still exit according to the existing language rules. Declared fallible finalization must not report successful completion when it fails. The exact integration with pending returns, loop control, existing failures, and panic remains to be specified below; successful cleanup must not arbitrarily change those exits. RFC 147 owns any independently selected local Result-handling boundary and must not bypass required cleanup.

`__exit__` receives `ScopeExit` only so it can perform correct cleanup and record metadata. For example, a transaction manager may roll back on `Error`, a telemetry manager may set span status on `Error` or `Panic`, and an output-capture manager may restore stdout/stderr regardless of outcome.

### Vocabulary desugaring

Descriptor-gated vocabulary blocks may declare that they lower to a context manager. Such a vocabulary block must preserve the same entry/exit guarantees as `with`.

A vocabulary block may provide additional compile-time information to the context manager, such as lexical symbol identity, source span, block name, static attributes, or descriptor identity. That additional information must not weaken the core cleanup guarantee.

Vocabulary blocks must remain explicitly activated through the project's vocabulary mechanism. A context-manager protocol implementation alone must not cause new statement syntax to appear in unrelated modules.

### Async interaction

The RFC must choose a conservative async rule. A synchronous context manager whose scoped state is task-local, thread-local, guard-based, or otherwise sensitive to suspension should not be held across `await` by accident.

At minimum, the compiler must either reject `await` inside a `with` suite unless the manager type is marked await-safe, or require a distinct async context-manager protocol for suites that may suspend. The same rule must apply to vocabulary blocks that lower through context managers.

This RFC leaves the exact spelling of await-safe context managers unresolved, but the contract must be decided before the RFC advances beyond Draft.

## Design details

### Syntax and grammar

The source form should intentionally be small. One manager per statement avoids the formatting and error-order complexity of Python's comma-separated manager list. Explicit nesting covers the same behavior while making exit order visible.

The `as` binding should support a plain name. Pattern bindings should be designed consistently across `let`, `for`, `match`, and `with` rather than invented only for context managers.

### Cleanup ordering

Nested context managers exit inside-out because the inner `with` suite exits before the outer one. Any multiple-manager header syntax must preserve the same order as explicit nesting.

### Interaction with destructors

Context managers are explicit source-level scoped lifetimes. They do not require Incan to expose Rust's `Drop` trait directly, and they should not rely on backend-specific destructor timing as their public contract.

Existing source resources already own drop-based runtime handles. Merely adding a no-op `__exit__` to such a wrapper does not establish scoped release if the live handle can survive elsewhere. Conversely, forcing a raw close through one alias while another valid owner still uses the handle would violate ordinary ownership expectations. The adaptation must connect the promised boundary to checked ownership or an explicit resource-state transition; that choice must be visible in its public contract.

The candidate `__enter__(self) -> T` signature is not evidence that this integration is already solved. Returning a guard or resource view while retaining a manager for exit requires a defined receiver, borrowing, and escape contract. This RFC must resolve that contract consistently with RFC 129 before moving beyond Draft.

### Interaction with `Result` and `?`

The `?` operator remains ordinary error propagation. A context manager observes `ScopeExit.Error` and then the original propagation continues. An explicit return whose evaluated Result is Err has the same failure classification, and its original return continues after cleanup. There is no `__exit__ -> bool` hook because suppression would blur cleanup with error handling.

### Interaction with vocabulary DSLs

Context managers are not a macro system. A vocabulary block that lowers to a context manager is still owned by its descriptor and must follow the scoped DSL rules for activation, parsing, formatting, diagnostics, and ambiguity. The context manager only supplies the scoped entry/exit runtime contract.

## Alternatives considered

1. Copy Python's context-manager protocol exactly. Rejected because `__exit__` returning a suppression boolean imports exception-handler semantics that Incan intentionally does not want.

2. Use only manual `close()`, `end()`, or `release()` calls. Rejected because manual pairing fails under early returns and `?` propagation, and it makes ordinary resource APIs unnecessarily error-prone.

3. Use only decorators for scoped behavior. Rejected because decorators cover declaration-shaped scopes well but do not cover arbitrary sub-operation blocks inside a function.

4. Use only vocabulary blocks without a general `with`. Rejected because files, locks, temporary directories, output capture, and transactions deserve a general construct instead of domain syntax for every resource.

5. Model context managers as destructors only. Rejected because destructors are not visible enough in Incan source, cannot receive an explicit `ScopeExit`, and would tie the public contract too closely to backend behavior.

6. Make with the host for general Result capture and local except matching. Not selected as this RFC's purpose: validation and calculations may fail without acquiring a resource, so RFC 147 explores the handling feature independently.

## Drawbacks

- `with` adds another statement form and must coexist cleanly with existing `with` usage in type positions.
- Context managers can hide meaningful side effects at block boundaries if APIs are poorly named.
- Cleanup failure precedence is subtle when a body is already exiting with another error.
- Async safety requires real design work; copying synchronous guard behavior into async code would be a correctness bug.
- Vocabulary desugaring through context managers adds power that must remain descriptor-gated and visible to tooling.

## Implementation architecture

This section is non-normative. A straightforward implementation can represent `with` as a scoped statement node whose lowering evaluates the manager once, calls the entry method, records whether entry succeeded, lowers the suite, and emits cleanup on every control-flow edge that leaves the suite. The emitted shape should preserve the existing meaning of returns, loop control, `?` propagation, and panic/assert exits while inserting the cleanup call before the original exit continues.

Vocabulary blocks that lower through context managers can use the same internal representation after their descriptor has parsed and validated the block-specific header.

## Layers affected

- **Parser / AST**: new with statement syntax and a scoped statement representation are needed.
- **Typechecker / Symbol resolution**: context expressions must satisfy the context-manager protocol, `as` bindings must receive the entered type and suite scope, declared finalization fallibility must be checked, and ownership, escape, and async-safety constraints must be enforced.
- **IR Lowering**: all suite exit edges must carry cleanup, preserve original control flow on successful cleanup, and retain every body and finalization failure without skipping required outer cleanup.
- **Emission**: generated code must guarantee exactly-once cleanup after successful entry without relying on user-written cleanup calls.
- **Stdlib / Runtime**: source-authored standard resource APIs and their existing runtime carriers may adapt files, temporary directories, locks, capture helpers, scoped environment overrides, and telemetry span handles to the protocol.
- **Formatter**: `with` suites need stable formatting and vocabulary blocks must preserve their descriptor-owned layout.
- **LSP / Tooling**: hover, go-to-definition, diagnostics, and control-flow explanation should show the context-manager protocol and scoped binding.

## Design Decisions

- Retain `__enter__` and `__exit__` as the protocol hook names.
- RFC 122's active validation policy belongs to the current execution context and survives suspension there. Cleanup restores the previous policy on every exit where cleanup executes, including return, propagation and cancellation. Independent execution contexts must not leak policy into one another. This rule does not choose inheritance for explicitly spawned tasks, which belongs to RFC 142, or permit every synchronous manager to suspend safely.
- `as` accepts ordinary irrefutable binding patterns. A refutable extraction requires an explicit failure path rather than an implicit panic; no new starred pattern syntax is introduced.
- Entered borrowed views invalidated by exit cannot escape through returns, storage, closures or tasks. Independently owned results may escape under ordinary ownership rules; explicit resource-specific checked transfer can preserve a resource beyond the scope. Retaining an owned path does not keep its resource alive. Receiver and protocol details must enforce these rules without changing them.

- Retain with for explicit scoped resource lifetimes and temporary runtime state with guaranteed finalization attempts or restoration. Temporary workspaces and scoped validation policy are motivating cases. Existing drop-based resource cleanup remains relevant; the context adaptation must establish the promised explicit boundary rather than claim to invent automatic cleanup. Ordinary Result handling does not require with. Scope-result capture and local except handling remain separate design work under RFC 147.

- Separate local except matching into RFC 147. The preferred single match-arm block is retained there as an independent Result-handling direction; it is not part of the with grammar specified here. Resource lifetime and finalization must remain complete without that extension.

- Always retain failures from the body and every attempted cleanup, including nested cleanup failures. Preserve original typed identities and diagnostic provenance under RFC 131; reporting or propagation precedence must never discard another failure. Independent failures remain distinct aggregate members rather than invented upstream causes. ScopeFailure.Multiple and work-before-inside-out-cleanup presentation are selected. The complete static propagation mechanics and panic carrier remain open.

- Permit declared typed fallible finalization. If the body succeeds but finishing the resource fails, the caller must receive a failure rather than false success. The fallible `__exit__` returns `Result[None, E]`: Ok(None) completes cleanup, while Err retains its typed failure. Keep fallibility visible and checked without making every manager fallible. Complete protocol declarations and propagation through pending returns and loop control remain open; simultaneous body/finalization failures must all be retained. Local Result matching is explored independently in RFC 147.

- Cleanup must observe failure for both question-mark error propagation and an explicit return whose evaluated Result is Err. Diagnostic metadata may additionally preserve the exit mechanism, but the return spelling must not hide failure or change cleanup into error suppression. ScopeExit uses the six reason-only cases Success, Return, Break, Continue, Error and Panic; detailed failure evidence remains separate.

## Unresolved questions

- What complete static propagation mechanics and panic carrier compose with the selected ScopeFailure.Multiple aggregate? Original typed failures, immutable membership with scope/phase provenance, work-first then actual inside-out cleanup presentation, and distinct panic significance are settled. The generic protocol and receiver questions below remain open.
- What complete protocol declaration binds E for the selected `__exit__ -> Result[None, E]` form, and how do finalization failures propagate through an enclosing Result and interact with pending returns and loop control? Typed finalization failure, its Result return shape, checked fallibility, and rejection of false success are settled. Infallible cleanup retains its None-returning form.
- What receiver, borrowed-view, and ownership-transfer rules connect the retained manager to its entered value, especially for existing reference-counted handles and guards? The protocol must prevent invalid escape and make promised resource release enforceable.
- What is the exact async-safety rule: reject `await` in synchronous context-manager suites, require an await-safe marker, or introduce a separate async context-manager protocol?
- How should tooling display vocabulary blocks that lower through context managers without pretending the user wrote a `with` statement?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
