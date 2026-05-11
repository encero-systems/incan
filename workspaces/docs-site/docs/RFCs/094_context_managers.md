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
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 093 (`std.telemetry`)
    - RFC 095 (`span` vocabulary blocks)
- **Issue:** #560
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC adds context managers to Incan: a general scoped-lifetime construct for resources that must be entered before a block and exited after the block regardless of how control leaves it. The user-facing syntax is Python-familiar, but the semantics are intentionally closer to Rust's scope-based cleanup model: context managers guarantee cleanup, report an exit outcome, and must not catch, suppress, or transform control flow. The same mechanism also becomes the substrate that vocabulary blocks can target when a domain-specific block needs compiler-guaranteed scope entry and exit.

## Core model

Read this RFC as six foundations:

1. **Scoped lifetime is a language primitive:** users should be able to express "this value is active for this block" without manual `close()`, `end()`, `release()`, or paired setup/cleanup calls.
2. **Cleanup is guaranteed after entry:** once `__enter__` succeeds, `__exit__` must run exactly once when the block exits, including fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits.
3. **No exception model is introduced:** `with` does not add `try`, `except`, `catch`, or suppression semantics. `ScopeExit` is informational and cleanup-oriented.
4. **The protocol is explicit and typed:** context-manager support is ordinary Incan API surface, not hidden naming magic that only the compiler understands.
5. **Vocabulary blocks may desugar through context managers:** domain-specific soft syntax can use the same scoped cleanup contract instead of inventing one-off entry/exit behavior.
6. **Async safety is explicit:** context managers that interact with task-local state, guards, locks, spans, or transactions must not be accidentally held across suspension points without a type-level contract.

## Motivation

Many useful APIs have paired lifecycle calls: open/close, acquire/release, begin/commit-or-rollback, enter/exit, capture/restore, start/end. Manual pairing is easy to get wrong, especially when a block has early returns or `?` propagation. Incan already has a Python-influenced surface, so users will expect a first-class scoped resource construct once the stdlib grows beyond simple values.

The language also needs a principled substrate for scoped vocabulary forms. Telemetry spans are the immediate example, but they are not the only one. A `span` block should not be a special cleanup snowflake in the parser; it should use a general scoped-lifetime hook. Tests, temporary directories, output capture, transactions, locks, and scoped runtime policy all want the same guarantee.

Rust gives useful prior art here without requiring Rust syntax. Rust uses lexical scope and the `Drop` trait to run cleanup when values leave scope. That design is not Python's `with`, but it proves the central idea: cleanup should be tied to scope and ownership, not remembered by convention. Incan should expose the idea in a Python-readable form while keeping Rust's refusal to treat cleanup as exception suppression.

## Goals

- Add `with` statement syntax for scoped context managers.
- Define a typed `ContextManager[T]` protocol.
- Define `ScopeExit` as the informational outcome passed to context-manager cleanup.
- Guarantee cleanup after successful entry on all block exit paths.
- Explicitly prohibit context managers from suppressing or rewriting control flow.
- Allow context-manager lowering to serve as the substrate for descriptor-gated vocabulary blocks.
- Define async interaction rules conservatively enough that task-local guards and telemetry spans are not accidentally mis-scoped.
- Provide a clear model for stdlib resources such as files, locks, temporary directories, captured output, and scoped configuration.

## Non-Goals

- Adding `try`, `except`, `catch`, or exception handlers.
- Copying Python's `__exit__` suppression behavior.
- Making all cleanup fallible or forcing cleanup errors into every function signature.
- Defining the `span` vocabulary block in this RFC.
- Defining transaction, lock, file, telemetry, or testing APIs beyond the context-manager contract they may implement.
- Making `with` a general macro system.
- Allowing imports to mutate global parsing outside descriptor-gated vocabulary positions.
- Leaving async context managers underspecified when the async safety contract affects correctness.

## Guide-level explanation

The common shape is a scoped resource:

```incan
with File.open(path) as file:
    data = file.read_text()
```

`File.open(path)` produces a context manager. The manager enters the resource, binds the yielded value to `file`, runs the block, and exits the resource no matter how the block completes.

The construct is not an error handler:

```incan
with capture_output() as captured:
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
with TemporaryDirectory() as dir:
    with File.open(dir / "report.txt") as file:
        file.write_text(report)
```

This RFC does not need multiple-manager header syntax. Explicit nesting is unambiguous, easy to format, and enough to define the semantics.

Vocabulary blocks may use the same mechanism while exposing domain syntax:

```incan
import std.telemetry.vocab

span "checkout.charge", attributes={"order.id": order.id}:
    receipt = charge(order)?
```

The `span` block is not defined here, but the important contract is that a vocabulary desugarer can target a context manager and receive the same cleanup guarantee as `with`.

## Reference-level explanation

### Syntax

A `with` statement has one context expression, an optional binding, and an indented suite:

```incan
with expression:
    suite

with expression as name:
    suite
```

The `with` token is a statement introducer. It must not change the meaning of `with` in type-parameter bounds or type conformance positions that already use the spelling.

The context expression must typecheck to `ContextManager[T]` or another type accepted by an explicitly defined context-manager adaptation rule. If an `as name` binding is present, `name` has type `T` inside the suite. If no binding is present, the entered value is discarded after entry.

### Protocol

The core protocol is:

```incan
pub trait ContextManager[T]:
    def __enter__(self) -> T
    def __exit__(self, exit: ScopeExit) -> None
```

`__enter__` starts the scoped lifetime and returns the value exposed to the block. `__exit__` ends the scoped lifetime and receives the exit outcome. `__exit__` must not suppress, replace, or redirect the original control flow.

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

An implementation may attach structured payloads to these variants if the language's public error and panic representations support that cleanly. Payloads must not be required for the cleanup guarantee.

### Entry and exit

The context expression must be evaluated exactly once before entering the suite.

If evaluating the context expression fails, `__enter__` is not called and `__exit__` is not called.

If `__enter__` fails, `__exit__` is not called because no scoped lifetime was successfully established.

If `__enter__` succeeds, `__exit__` must be called exactly once after the suite exits. This requirement applies to normal fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits.

If `__exit__` itself fails or panics during normal fallthrough, that failure becomes the block's exit. If `__exit__` fails while another exit is already in progress, the implementation must define a deterministic precedence rule and diagnostics should make the double-failure situation visible. This RFC does not require cleanup failures to be silently ignored.

### Control flow

`with` must not introduce a catchable exception model. The body's control-flow decision remains authoritative after cleanup. A `return` still returns, a `break` still breaks, a `continue` still continues, `?` propagation still propagates, and panic/assert exits still exit according to the existing language rules.

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

### Interaction with `Result` and `?`

The `?` operator remains ordinary error propagation. A context manager observes `ScopeExit.Error` and then the original propagation continues. There is no `__exit__ -> bool` hook because suppression would blur cleanup with error handling.

### Interaction with vocabulary DSLs

Context managers are not a macro system. A vocabulary block that lowers to a context manager is still owned by its descriptor and must follow the scoped DSL rules for activation, parsing, formatting, diagnostics, and ambiguity. The context manager only supplies the scoped entry/exit runtime contract.

## Alternatives considered

1. Copy Python's context-manager protocol exactly. Rejected because `__exit__` returning a suppression boolean imports exception-handler semantics that Incan intentionally does not want.

2. Use only manual `close()`, `end()`, or `release()` calls. Rejected because manual pairing fails under early returns and `?` propagation, and it makes ordinary resource APIs unnecessarily error-prone.

3. Use only decorators for scoped behavior. Rejected because decorators cover declaration-shaped scopes well but do not cover arbitrary sub-operation blocks inside a function.

4. Use only vocabulary blocks without a general `with`. Rejected because files, locks, temporary directories, output capture, and transactions deserve a general construct instead of domain syntax for every resource.

5. Model context managers as destructors only. Rejected because destructors are not visible enough in Incan source, cannot receive an explicit `ScopeExit`, and would tie the public contract too closely to backend behavior.

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

- **Parser / AST**: new `with` statement syntax and a scoped statement representation are needed.
- **Typechecker / Symbol resolution**: context expressions must satisfy the context-manager protocol, `as` bindings must receive the entered type, and async-safety constraints must be enforced.
- **IR Lowering**: all suite exit edges must carry cleanup while preserving original control flow.
- **Emission**: generated code must guarantee exactly-once cleanup after successful entry without relying on user-written cleanup calls.
- **Stdlib / Runtime (`incan_stdlib`)**: standard resources such as files, temporary directories, locks, capture helpers, scoped environment overrides, and telemetry span handles may implement the protocol.
- **Formatter**: `with` suites need stable formatting and vocabulary blocks must preserve their descriptor-owned layout.
- **LSP / Tooling**: hover, go-to-definition, diagnostics, and control-flow explanation should show the context-manager protocol and scoped binding.

## Unresolved questions

- What is the exact public spelling and payload shape of `ScopeExit`?
- Should `__enter__` and `__exit__` use double-underscore names, or should Incan prefer explicit protocol names such as `enter_context` and `exit_context`?
- What deterministic precedence rule should apply when `__exit__` fails while the suite is already exiting with an error or panic?
- Should `as` allow pattern bindings, or only a single identifier?
- What is the exact async-safety rule: reject `await` in synchronous context-manager suites, require an await-safe marker, or introduce a separate async context-manager protocol?
- How should tooling display vocabulary blocks that lower through context managers without pretending the user wrote a `with` statement?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
