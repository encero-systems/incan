# RFC 094: Context managers

- **Status:** Planned
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
    - RFC 098 (`infer` members in trait headers)
    - RFC 122 (scoped validation policy)
    - RFC 129 (binding, borrowing, and ownership rules)
    - RFC 131 (typed error values and propagation)
    - RFC 134 (exiting pattern bindings inside resource scopes)
    - RFC 142 (async iteration, task ownership and cancellation)
    - RFC 147 (independent local Result matching)
    - RFC 154 (closures that capture entered views)
    - RFC 164 (explicit ownership and borrowing; entered views declared `from self`)
    - RFC 165 (`std.db` lazy scope descriptors)
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
3. **Cleanup is distinct from Result handling:** the manager must not suppress a failure or catch a panic. Local typed Result matching belongs to RFC 147; ScopeExit remains informational and cleanup-oriented.
4. **The protocol is explicit and typed:** context-manager support is ordinary Incan API surface, not hidden naming magic that only the compiler understands.
5. **Vocabulary blocks may desugar through context managers:** domain-specific soft syntax can use the same scoped cleanup contract instead of inventing one-off entry/exit behavior.
6. **Async safety is explicit:** context managers that interact with task-local state, guards, locks, spans, or transactions must not be accidentally held across suspension points without a type-level contract.

The purpose of with is a scoped resource lifetime or temporary runtime state that must be finished or restored. An ordinary fallible calculation or validation check needs no with statement merely because it returns Result; ordinary matching and RFC 070 combinators remain available. Local Result handling with `except` belongs to RFC 147 and is not a prerequisite for this lifetime contract.

## Motivation

Many useful APIs have paired lifecycle calls: open/close, acquire/release, begin/commit-or-rollback, enter/exit, capture/restore, start/end. Manual pairing is easy to get wrong, especially when a block has early returns or `?` propagation. Incan already has a Python-influenced surface, so users will expect a first-class scoped resource construct once the stdlib grows beyond simple values.

The language also needs a principled substrate for scoped vocabulary forms. Telemetry spans are the immediate example, but they are not the only one. A `span` block should not be a special cleanup snowflake in the parser; it should use a general scoped-lifetime hook. Tests, temporary directories, output capture, transactions, locks, and scoped runtime policy all want the same guarantee.

Rust gives useful prior art here without requiring Rust syntax. Rust uses lexical scope and the `Drop` trait to run cleanup when values leave scope. That design is not Python's `with`, but it proves the central idea: cleanup should be tied to scope and ownership, not remembered by convention. Incan should expose the idea in a Python-readable form while keeping Rust's refusal to treat cleanup as exception suppression.

## Goals

- Add with statement syntax for scoped context managers, and `async with` for managers whose entry or exit suspends.
- Define the typed `ContextManager` and `AsyncContextManager` protocols.
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

### Test drive: a database transaction

A transaction is the case where cleanup has a real outcome. Here `Pool`, `Connection`, `DbError` and their methods stand for an application's database client:

```incan
from std.scope import ContextManager, ScopeExit

class Transaction with ContextManager[Result[Borrowed[Connection], DbError], Result[None, DbError]]:
    connection: Connection

    def __enter__(mut self) -> Result[Borrowed[Connection], DbError] from self:
        self.connection.begin()?
        return Ok(self.connection)

    def __exit__(mut self, exit: ScopeExit) -> Result[None, DbError]:
        match exit:
            ScopeExit.Success | ScopeExit.Return => self.connection.commit()
            _ => self.connection.rollback()


def transfer(pool: Pool, source: int, target: int, amount: int) -> Result[None, DbError]:
    with Transaction(connection=pool.connect()?) as db:
        db.debit(account=source, amount=amount)?
        db.credit(account=target, amount=amount)?
    return Ok(None)
```

`pool.connect()?` acquires a connection before the context exists; a failure there returns before anything is entered. Entering then begins the transaction. If that fails, the block does not run and `transfer` returns the error, as `?` would. If the debit or credit fails, its `?` leaves the block, exit observes `ScopeExit.Error` and rolls back, and `transfer` returns that failure. If both succeed, exit commits. A rejected commit becomes `transfer`'s error, so the caller never sees `Ok(None)` for work that was not saved. If the credit and the rollback both fail, `transfer` returns both under `ScopeFailure.Multiple`. `db` is a view tied to the transaction, so it cannot be returned or stored past the block.

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

The current async mutex releases its guard when the guard is dropped. A proposed integration could make a short critical section visible, with the guard as the manager and its `__enter__` returning `Reference[Counter] from self`:

```incan
async def bump(shared_counter: Mutex[Counter]) -> Counter:
    with await shared_counter.lock() as counter:
        counter.increment()
        return counter      # refused: `counter` borrows the guard, which exits here
```

The lock acquisition suspends before context entry. `counter` is exclusive access tied to the guard, so the checker refuses returning it, storing it, capturing it in a task, or capturing it in a closure that outlives the suite; the guard's exit releases the lock with no live view left behind. Returning `counter.value()`, an owned copy of the count, is fine. An `await` inside the suite is permitted only when the guard and `counter` satisfy the ordinary checked suspension proof. The current async mutex supports guards held across await; the context protocol must not assume that every guard is unsafe or that every resource is safe to suspend.

### Test drive: cleanup that suspends

Some cleanup has to wait, such as committing over the network. Such a manager adopts `AsyncContextManager`, whose hooks are async, and is used with `async with` inside an async function:

```incan
async def save(client: Client, report: Report) -> Result[None, UploadError]:
    async with client.upload("reports/latest.json") as upload:
        await upload.write(report.to_json())?
    return Ok(None)
```

Entering opens the upload and the exit awaits its completion. A failed completion becomes `save`'s error under the same fallible-exit rules as `with`. A plain `with` over an async manager, or `async with` over a synchronous one, is refused with a diagnostic naming the other form.

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
import std.telemetry

def take_one(remaining: int) -> Result[int, str]:
    span "reserve":
        if remaining < 1:
            return Err("quota exhausted")
        return Ok(remaining - 1)
```

The `span` block is not defined here, but the important contract is that a vocabulary desugarer can target a context manager and receive the same cleanup guarantee as `with`.

### Result matching is independent

RFC 147 owns local typed Result matching, through a `?` followed by one `except:` block of ordinary match arms. This RFC does not require an except block, scope capture expression, new outcome container, or per-failure dispatch mechanism to establish a resource lifetime. Its fallible-finalization propagation contract remains an obligation of the resource construct itself.

## Reference-level explanation

### Syntax

A `with` statement has one context expression, an optional binding, and an indented suite:

```incan
with expression:
    suite

with expression as name:
    suite

async with expression as name:
    suite
```

Local Result-matching syntax is outside this RFC and belongs to RFC 147.

The `with` token is a statement introducer. It must not change the meaning of `with` in type-parameter bounds or type conformance positions that already use the spelling. `async with` is permitted only inside an async function.

The context expression of `with` must typecheck to `ContextManager[Entry, ExitOutcome]`, and that of `async with` to `AsyncContextManager[Entry, ExitOutcome]`, or to another type accepted by an explicitly defined context-manager adaptation rule. A `with` over a type that adopts only the async protocol, or an `async with` over a type that adopts only the synchronous one, must be refused with a diagnostic naming the form that applies. If an `as name` binding is present, `name` has the bound type defined under fallible entry below, only inside the suite. If no binding is present, no entered value is exposed by a binding; the manager must remain alive until exit. Omitting `as` must not release the manager early or skip its exit call.

### Protocol

The synchronous protocol is one trait in `std.scope` whose exit outcome is chosen by each implementation, using RFC 098's `infer` members:

```incan
pub trait ContextManager[Entry, infer ExitOutcome]:
    def __enter__(mut self) -> Entry
    def __exit__(mut self, exit: ScopeExit) -> Self.ExitOutcome
```

`__enter__` starts the scoped lifetime and returns what entering produces. `__exit__` ends the scoped lifetime and receives the exit outcome. The two type arguments therefore read as what entering gives and what exiting gives. A successful cleanup preserves the original control flow. A manager must not suppress the body's failure, but a declared typed finalization failure may prevent otherwise successful completion.

An adopting type supplies both arguments explicitly. For the exit outcome the checker must accept only `None` or `Result[None, E]`, where E satisfies RFC 131's common Error contract:

```incan
class CapturedOutput with ContextManager[CapturedOutput, None]:
    ...

class ReadScope with ContextManager[Result[Reader, DbError], None]:
    ...

class Transaction with ContextManager[Result[Borrowed[Connection], DbError], Result[None, DbError]]:
    ...
```

`None` declares infallible cleanup. For `Result[None, E]`, `Ok(None)` means cleanup succeeded and `Err(error)` reports its typed failure. Fallibility is therefore part of the checked public contract, known statically wherever the manager is used. Independent local Result matching belongs to RFC 147.

The entry argument declares fallible entry the same way. When `Entry` is `Result[T, E]`, with E satisfying the Error contract, successful entry binds T, not Result[T, E]; for any other `Entry`, `as` binds `Entry` itself. An entry failure does not execute the suite and must propagate into the enclosing function's Result, as `?` would. The checker must refuse a `with` over a manager with fallible entry in a function whose declared result cannot carry that failure; there is no implicit unwrap or panic conversion. This interpretation is specific to context entry and is not general Result method forwarding.

The `with` statement holds the manager exclusively for the whole scope, so both hooks take `mut self`. When the context expression is a temporary, the scope owns the manager. When it is an existing place, such as `with captured:`, the scope exclusively borrows that place and the place remains usable after the block; it is neither moved nor copied.

`Entry` declares what the binding may outlive, using RFC 164's ownership modes. An owned `T` is independent of the manager and may escape the suite under ordinary ownership rules, as a `Path` may. An entered value of `Borrowed[T]` or `Reference[T]`, alone or inside the Result of fallible entry, must be declared `from self` or from a field path of `self` on the adopting `__enter__`, and is then tied to the retained manager:

```incan
class Guard with ContextManager[Reference[Counter], None]:
    def __enter__(mut self) -> Reference[Counter] from self:
        ...
```

The checker must refuse returning such a binding, storing it, capturing it in a task, or capturing it in a closure that outlives the suite, under RFC 154, and the binding's borrow must end before `__exit__` runs. An implementation of `__enter__` must not declare a borrowed source other than its receiver.

A lazy scope descriptor can therefore be used directly in a header such as `with db.open(path).read() as reader:`. RFC 165 owns that library adaptation: header evaluation constructs the descriptor, while entry performs the database acquisition and starts the read. A fallible eager acquisition still uses its explicit Result handling before entry, as in the earlier file examples.

If composite entry acquires one resource and then fails acquiring another, the entry implementation owns cleanup of the already acquired resources. The compiler does not call the outer manager's exit hook after failed entry. Any cleanup failure must be retained alongside the entry failure, with the same typed failure and provenance guarantees as work/finalization failures. Successfully entered enclosing managers still exit normally on propagation.

`ScopeExit` must distinguish at least these outcomes:

```incan
pub enum ScopeExit:
    Success
    Return
    Break
    Continue
    Error
    Panic
    Cancelled
```

ScopeExit is a reason-only enum owned by std.scope. Error includes both question-mark error propagation and a return whose evaluated Result is Err. Return denotes other explicit returns, including an Ok result. The failure outcome takes precedence over the return spelling for cleanup classification. Cancelled denotes a synchronous manager exiting because its enclosing task was cancelled while the suite was suspended. Detailed originals, diagnostic provenance and the exit mechanism remain separate from these reason tags.

The reason tags must not carry a Python-style exception triple or acquire payload-dependent suppression behavior. Panic remains distinct from a recoverable Error value, and Cancelled remains distinct from both, as RFC 142 requires of cancellation and application failure.

### Entry and exit

The context expression must be evaluated exactly once before entering the suite.

If evaluating the context expression fails, `__enter__` is not called and `__exit__` is not called.

If `__enter__` fails, `__exit__` is not called because no scoped lifetime was successfully established.

If `__enter__` succeeds, `__exit__` must be called exactly once after the suite exits. This requirement applies to normal fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits.

Managers may declare typed fallible finalization using a Result error channel. If the body otherwise succeeds but finalization fails, the scope must report that failure rather than continue or return as though the complete operation succeeded. Fallibility must be visible in the checked manager contract, and the compiler must require valid handling or propagation. This does not require infallible managers to invent an error channel. The infallible `__exit__ -> None` form has no recoverable error channel; the fallible form returns `Result[None, E]`, with E bound by the manager's adopted exit outcome.

A fallible exit failure must propagate into the enclosing function's Result, as `?` would. The checker must refuse a `with` over a fallible manager in a function whose declared result cannot carry that failure. A failed cleanup overrides a pending success, `return`, `break` or `continue`: the scope exits with the failure instead, so a loop does not continue and a function does not return its pending value as though the work had been saved. Every failure that already existed is retained alongside it.

For example, a transaction-like manager may commit when its suite completes successfully. Inserts can succeed while the database subsequently rejects the commit. The caller must receive that finalization failure; successful execution of the insert statements is not proof that the transaction was saved. This scenario illustrates the context-manager contract and does not define a stdlib transaction API or select a universal commit-on-exit policy.

Every failure produced by the body or attempted finalization must be retained. If the body and cleanup both fail, neither error may replace, erase, or reduce the other to message text. Nested scopes must retain additional cleanup failures while preserving the required cleanup attempts for other successfully entered managers. Each retained typed error keeps its original identity, payload, diagnostic origin, upstream relationships, and any captured stack trace under RFC 131. Diagnostic presentation or propagation precedence must not determine which failures survive. Independent body and cleanup failures are aggregate members; retaining them together must not invent a caused_by relationship between them.

When only one work or finalization path fails, the original typed failure must propagate through a compatible enclosing error contract. When multiple paths fail, std.scope.ScopeFailure.Multiple must retain an immutable tuple of the original typed failures and per-member scope and phase provenance. The aggregate belongs to resource scopes, not to every Error. Reports must present work failure first, followed by attempted cleanup failures in actual inside-out order. An enclosing manager must still attempt cleanup and retain any additional failure. Independent members must not acquire a caused_by relationship merely because they are reported together. A single failure or the aggregate propagates under the exit-outcome rule above.

A panic always takes precedence over typed failures. When the body panics, each successfully entered manager's exit runs with `ScopeExit.Panic` and the scope exits by panicking. When a cleanup panics, the scope exits by panicking even if the body succeeded or a typed failure was pending. In both cases every typed failure that already existed, from the body or from an exit, must be attached to the panic's report with its original identity and provenance under RFC 131; a panic never becomes a Result, and no typed failure is discarded because a panic occurred. Enclosing managers still exit with `ScopeExit.Panic` and attach their own failures. A cleanup that panics while a panic is already unwinding aborts the process after both panics are reported, since the runtime cannot unwind twice; the cleanup guarantee does not cover enclosing managers in that case. Fallible cleanup must not be described as error suppression or as proof that cleanup always succeeds.

The cleanup guarantee applies to exits for which the runtime can execute cleanup, including panic/assert unwinding. It cannot promise execution of user cleanup during immediate process abort or external process termination. This boundary must not introduce a catchable exception model.

### Scoped bindings and resource escape

The manager must remain available for its required exit call. The compiler must not move it away, clone it implicitly, or let an entered value consume it in a way that prevents that call. An `as` binding must follow ordinary mutation permissions and must not acquire writable access solely because it appears in a context header.

Borrowed values that would be invalidated by context exit must not escape the suite through return values, storage, closures, or tasks. Independently owned results may escape when ordinary ownership rules permit it. An owned path string or `Path` value must not be treated as a lifetime guarantee for the resource it names.

An adaptation promising resource release at exit must establish that retained handles or aliases cannot silently keep that same resource active beyond the promised boundary. Such a manager must enter with a `from self` view, never an owned clone of a shared handle to the live resource. A resource-specific operation such as temporary-directory persistence may transfer ownership deliberately through an explicit method; exit must respect that operation without double cleanup.

### Control flow

The manager must not catch or suppress the body's failure or catch panics. When cleanup succeeds, the body's control-flow decision remains authoritative: a `return` still returns, a `break` still breaks, a `continue` still continues, `?` propagation still propagates, and panic/assert exits still exit according to the existing language rules. Declared fallible finalization must not report successful completion when it fails: a failed cleanup overrides a pending success, `return`, `break` or `continue` as failure, while retaining every existing failure. Successful cleanup must not change those exits. Interaction with panic is specified below. RFC 147 owns any independently selected local Result-handling boundary and must not bypass required cleanup.

`__exit__` receives `ScopeExit` only so it can perform correct cleanup and record metadata. For example, a transaction manager may roll back on `Error`, a telemetry manager may set span status on `Error` or `Panic`, and an output-capture manager may restore stdout/stderr regardless of outcome.

### Vocabulary desugaring

Descriptor-gated vocabulary blocks may declare that they lower to a context manager. Such a vocabulary block must preserve the same entry/exit guarantees as `with`.

A vocabulary block may provide additional compile-time information to the context manager, such as lexical symbol identity, source span, block name, static attributes, or descriptor identity. That additional information must not weaken the core cleanup guarantee.

Vocabulary blocks must remain explicitly activated through the project's vocabulary mechanism. A context-manager protocol implementation alone must not cause new statement syntax to appear in unrelated modules.

Diagnostics for a vocabulary block must use the authored block, its header or the responsible body expression as the primary span. The vocabulary or provider identity and the entry/exit lowering chain must be retained as structured secondary evidence, inspectable by tools. When both the work and cleanup fail, both must be shown. Diagnostics must not present the generated `with` as though the user had written it, and must not change secrecy rules.

### Async interaction

A synchronous context manager's suite may contain `await` only when the retained manager, the entered value and any state they borrow satisfy the ordinary checked suspension proof, the same proof that governs any value held across `await`. When the proof fails, the compiler must diagnose the resource and the `await`; it must not silently clone or move state to make suspension legal. There is no unchecked await-safe marker.

Awaited acquisition happens before entry, as in `with await shared_counter.lock() as counter:`. A synchronous `__exit__` must never hide an await. An entry or exit that itself needs to suspend uses the async protocol, also in `std.scope`:

```incan
pub trait AsyncContextManager[Entry, infer ExitOutcome]:
    async def __enter__(mut self) -> Entry
    async def __exit__(mut self, exit: ScopeExit) -> Self.ExitOutcome
```

`async with` awaits `__enter__` before the suite and awaits `__exit__` after it. Every rule above for `with` applies unchanged: the entry and exit outcomes, the bound type, borrowed views, failure retention, panic precedence and exit order. The ordinary checked suspension proof still governs the manager and entered value across any `await` in the suite and across both awaited hooks. A type may adopt both protocols.

A synchronous manager whose suite is suspended when its enclosing task is cancelled still exits, with `ScopeExit.Cancelled`. An async manager's `__exit__` cannot be awaited once its task has been cancelled, because nothing remains to drive it: it is not called, and the manager is released under its ordinary ownership contract. An async manager whose resource needs release on cancellation must therefore make that release part of dropping the manager, as a connection rolls back an uncommitted transaction when it is dropped. A fallible exit that fails during cancellation has no caller to receive the failure; it must be reported through runtime diagnostics rather than discarded silently. This does not introduce a recoverable cancellation model, which remains RFC 142's concern.

The same rules apply to vocabulary blocks that lower through context managers.

## Design details

### Syntax and grammar

The source form should intentionally be small. One manager per statement avoids the formatting and error-order complexity of Python's comma-separated manager list. Explicit nesting covers the same behavior while making exit order visible.

The `as` binding should support a plain name. Pattern bindings should be designed consistently across `let`, `for`, `match`, and `with` rather than invented only for context managers.

### Cleanup ordering

Nested context managers exit inside-out because the inner `with` suite exits before the outer one. Any multiple-manager header syntax must preserve the same order as explicit nesting.

### Interaction with destructors

Context managers are explicit source-level scoped lifetimes. They do not require Incan to expose Rust's `Drop` trait directly, and they should not rely on backend-specific destructor timing as their public contract.

Existing source resources already own drop-based runtime handles. Merely adding a no-op `__exit__` to such a wrapper does not establish scoped release if the live handle can survive elsewhere. Conversely, forcing a raw close through one alias while another valid owner still uses the handle would violate ordinary ownership expectations. The adaptation must connect the promised boundary to checked ownership or an explicit resource-state transition; that choice must be visible in its public contract.

The receiver and entry rules above supply that contract: the scope holds the manager exclusively, and an entered view declared `from self` cannot outlive it. A resource adaptation still has to choose which kind of entered value it returns, and that choice is visible in its adopted `Entry`.

### Interaction with `Result` and `?`

The `?` operator remains ordinary error propagation. A context manager observes `ScopeExit.Error` and then the original propagation continues. An explicit return whose evaluated Result is Err has the same failure classification, and its original return continues after cleanup. There is no `__exit__ -> bool` hook because suppression would blur cleanup with error handling.

### Why `std.scope`

`ContextManager`, `AsyncContextManager`, `ScopeExit` and `ScopeFailure` describe resource scopes in general: files, locks, transactions, captured output and spans all use them. Scoped validation policy (RFC 122) is one user of the module rather than its owner, so the protocol lives beside the scope vocabulary instead of under a validation or resource module.

### Interaction with vocabulary DSLs

Context managers are not a macro system. A vocabulary block that lowers to a context manager is still owned by its descriptor and must follow the scoped DSL rules for activation, parsing, formatting, diagnostics, and ambiguity. The context manager only supplies the scoped entry/exit runtime contract.

## Alternatives considered

1. Copy Python's context-manager protocol exactly. Rejected because `__exit__` returning a suppression boolean imports exception-handler semantics that Incan intentionally does not want.

2. Use only manual `close()`, `end()`, or `release()` calls. Rejected because manual pairing fails under early returns and `?` propagation, and it makes ordinary resource APIs unnecessarily error-prone.

3. Use only decorators for scoped behavior. Rejected because decorators cover declaration-shaped scopes well but do not cover arbitrary sub-operation blocks inside a function.

4. Use only vocabulary blocks without a general `with`. Rejected because files, locks, temporary directories, output capture, and transactions deserve a general construct instead of domain syntax for every resource.

5. Model context managers as destructors only. Rejected because destructors are not visible enough in Incan source, cannot receive an explicit `ScopeExit`, and would tie the public contract too closely to backend behavior.

6. Make with the host for general Result capture and local except matching. Not selected as this RFC's purpose: validation and calculations may fail without acquiring a resource, so RFC 147 owns the handling feature independently.

## Drawbacks

- `with` adds another statement form and must coexist cleanly with existing `with` usage in type positions.
- Context managers can hide meaningful side effects at block boundaries if APIs are poorly named.
- Cleanup failure precedence is subtle when a body is already exiting with another error.
- Two protocols and two statement forms exist because an exit that suspends cannot hide inside a synchronous hook.
- An async exit cannot run after its task is cancelled; an async manager's resource release on cancellation depends on its ordinary drop.
- Vocabulary desugaring through context managers adds power that must remain descriptor-gated and visible to tooling.

## Implementation architecture

This section is non-normative. A straightforward implementation can represent `with` as a scoped statement node whose lowering evaluates the manager once, calls the entry method, records whether entry succeeded, lowers the suite, and emits cleanup on every control-flow edge that leaves the suite. The emitted shape should preserve the existing meaning of returns, loop control, `?` propagation, and panic/assert exits while inserting the cleanup call before the original exit continues.

Vocabulary blocks that lower through context managers can use the same internal representation after their descriptor has parsed and validated the block-specific header.

## Layers affected

- **Parser / AST**: new `with` and `async with` statement syntax and a scoped statement representation are needed.
- **Typechecker / Symbol resolution**: context expressions must satisfy the context-manager protocol, `as` bindings must receive the entered type and suite scope, declared finalization fallibility must be checked, and ownership, escape, and async-safety constraints must be enforced.
- **IR Lowering**: all suite exit edges must carry cleanup, preserve original control flow on successful cleanup, and retain every body and finalization failure without skipping required outer cleanup.
- **Emission**: generated code must guarantee exactly-once cleanup after successful entry without relying on user-written cleanup calls.
- **Stdlib / Runtime**: source-authored standard resource APIs and their existing runtime carriers may adapt files, temporary directories, locks, capture helpers, scoped environment overrides, and telemetry span handles to the protocol.
- **Formatter**: `with` suites need stable formatting and vocabulary blocks must preserve their descriptor-owned layout.
- **LSP / Tooling**: hover, go-to-definition, diagnostics, and control-flow explanation should show the context-manager protocol and scoped binding.

## Design Decisions

- Retain `__enter__` and `__exit__` as the protocol hook names.
- Permit declared fallible entry through the adopted `Entry` argument: `ContextManager[Result[T, E], ExitOutcome]`. Successful entry binds T; failure skips the body, propagates into the enclosing Result as `?` would, is refused where that Result cannot carry it, and does not call that manager's exit hook. Composite acquisition cleans up its own partial resources and preserves all failures. Lazy descriptors support fluent library resource scopes without general Result unwrapping.
- The scope holds the manager exclusively, so both hooks take `mut self`; a place used as the context expression is borrowed for the scope, not moved. `Entry` uses RFC 164 modes to declare what the binding may outlive: an owned value may escape, while `Borrowed[T]` or `Reference[T]` declared `from self` is tied to the manager and cannot be returned, stored or captured past the block. A manager promising release must enter with such a view, never an owned clone of a shared live handle; deliberate transfers such as `persist()` stay explicit methods.
- A panic always takes precedence: the scope exits by panicking and every existing typed failure is attached to the panic's report. A cleanup that panics during unwinding aborts after both panics are reported.
- An entry or exit that suspends uses `AsyncContextManager` with async hooks, through `async with` inside async functions, under the same rules as `with`. A synchronous manager exits with `ScopeExit.Cancelled` when its task is cancelled; an async manager's exit cannot be awaited then, so its release on cancellation belongs to its ordinary drop.
- RFC 122's active validation policy belongs to the current execution context and survives suspension there. Cleanup restores the previous policy on every exit where cleanup executes, including return, propagation and cancellation. Independent execution contexts must not leak policy into one another. This rule does not choose inheritance for explicitly spawned tasks, which belongs to RFC 142, or permit every synchronous manager to suspend safely.
- `as` accepts ordinary irrefutable binding patterns. A refutable extraction requires an explicit failure path rather than an implicit panic; no new starred pattern syntax is introduced.
- Entered borrowed views invalidated by exit cannot escape through returns, storage, closures or tasks. Independently owned results may escape under ordinary ownership rules; explicit resource-specific checked transfer can preserve a resource beyond the scope. Retaining an owned path does not keep its resource alive. Receiver and protocol details must enforce these rules without changing them.

- Retain with for explicit scoped resource lifetimes and temporary runtime state with guaranteed finalization attempts or restoration. Temporary workspaces and scoped validation policy are motivating cases. Existing drop-based resource cleanup remains relevant; the context adaptation must establish the promised explicit boundary rather than claim to invent automatic cleanup. Ordinary Result handling does not require with. Local `except` handling belongs to RFC 147.

- Separate local except matching into RFC 147. Its block of match arms is an independent Result-handling feature, not part of the with grammar specified here. Resource lifetime and finalization must remain complete without that extension.

- Always retain failures from the body and every attempted cleanup, including nested cleanup failures. Preserve original typed identities and diagnostic provenance under RFC 131; reporting or propagation precedence must never discard another failure. Independent failures remain distinct aggregate members rather than invented upstream causes. ScopeFailure.Multiple and work-before-inside-out-cleanup presentation are selected. Propagation follows the protocol decision above; under a panic, the retained failures travel on the panic's report.

- Permit declared typed fallible finalization. If the body succeeds but finishing the resource fails, the caller must receive a failure rather than false success. The fallible `__exit__` returns `Result[None, E]`: Ok(None) completes cleanup, while Err retains its typed failure. Keep fallibility visible and checked without making every manager fallible. Propagation through pending returns and loop control follows the protocol decision above; simultaneous body/finalization failures must all be retained. Local Result matching is explored independently in RFC 147.

- One synchronous protocol, `ContextManager[Entry, infer ExitOutcome]`, with both arguments supplied explicitly at adoption. The checker accepts only `None` or `Result[None, E]` with E satisfying Error. A fallible exit propagates into a compatible enclosing Result; a failed cleanup overrides a pending success, return, break or continue as failure while retaining every existing failure; a function whose declared result cannot carry the failure is refused. `with` syntax is unchanged.
- `await` inside a synchronous manager's suite is allowed only when the manager, entered value and borrowed state satisfy the ordinary checked suspension proof; otherwise the resource and the `await` are diagnosed, without silent cloning. Awaited acquisition happens before entry. An exit that suspends uses the async protocol above.
- Diagnostics use the authored vocabulary block, header or responsible body expression as the primary span, with the vocabulary identity and entry/exit lowering chain as structured secondary evidence. Work and cleanup failures are both shown, and the generated `with` is not presented as handwritten.
- Cleanup must observe failure for both question-mark error propagation and an explicit return whose evaluated Result is Err. Diagnostic metadata may additionally preserve the exit mechanism, but the return spelling must not hide failure or change cleanup into error suppression. ScopeExit uses the reason-only cases Success, Return, Break, Continue, Error, Panic and Cancelled; detailed failure evidence remains separate.
