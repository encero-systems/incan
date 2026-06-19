# RFC 004: async fixtures

- **Status:** Implemented
- **Created:** 2025-12-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 018 (testing), RFC 019 (runner testing)
- **Issue:** [#78](https://github.com/encero-systems/incan/issues/78)
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** v0.3.0-dev.31

## Summary

This RFC proposes async fixtures for Incan's test framework so `async def` fixtures can perform awaited setup and teardown while participating in ordinary fixture injection and scope management. The intended user experience stays close to the existing `@fixture` plus `yield` model, but the runtime contract becomes async-aware and guarantees awaited teardown ordering.

## Motivation

- Tests increasingly need async setup for HTTP servers, database pools, queues, and other service dependencies.
- Async tests and async fixtures should share one coherent model rather than forcing users into ad hoc setup helpers.
- Teardown must remain reliable while the generated worker remains alive, including when async tests fail or panic.

## Goals

- Preserve pytest-like fixture ergonomics while allowing async setup and teardown.
- Support the same fixture scopes as synchronous fixtures.
- Define deterministic teardown ordering for async fixtures and mixed sync/async graphs.
- Make async fixtures compose with parametrized tests.
- Keep the public test authoring model Incan-first even if the runtime is backed by Tokio underneath.

## Non-Goals

- Introducing a second, unrelated async-fixture decorator surface.
- Replacing the general fixture model with a distinct async-only testing subsystem.
- Settling every runtime-configuration choice in this RFC.

## Guide-level explanation

Async fixtures follow the same shape as normal yield-based fixtures, except the fixture function is declared with `async def` and can await during both setup and teardown.

```incan
import std.async
from std.testing import fixture

@fixture(scope="function")
async def http_server() -> ServerHandle:
    server = await start_server(port=0)
    yield server
    await server.shutdown()
```

The yielded value is injected into dependent tests and fixtures as usual. The important difference is that teardown is awaited before the runner proceeds to dependent teardowns.

## Reference-level explanation

- Async fixtures must be declared with `async def`.
- Async fixtures must use `yield` exactly once.
- The yielded value is the fixture value injected into dependent tests or fixtures.
- Fixture scopes mirror the synchronous fixture story: function, module, and session fixtures remain valid.
- If a dependency in the fixture graph is async, dependents must await its setup before running.
- Teardown order must remain reverse-topological across the fixture graph, and async teardowns must be awaited before the runner continues.
- Setup failures fail the dependent test or scope as appropriate.
- Teardown failures must be reported and must not silently disappear; when multiple teardowns fail, the runner should preserve aggregate error reporting semantics.
- Parametrized tests expand first, and fixture resolution then happens per expanded test case under the normal scope rules.

## Design details

### Runtime model

The current design assumes a shared async runtime per test run rather than nested runtimes per fixture or per test. The RFC is motivated by Tokio-backed execution, but the public contract is that async fixtures run on the test runner's async runtime and may await normally.

### Mixed sync and async fixture graphs

Synchronous and asynchronous fixtures must compose in one dependency graph. Async boundaries must be handled by the runner rather than leaked into user-facing fixture syntax beyond `async def`.

### Failure and teardown behavior

The runner must treat teardown as mandatory cleanup work while the generated worker remains alive. Async teardown is part of the fixture contract, but hard runner-level timeout enforcement can terminate the worker process before remaining teardown has a chance to run.

## Alternatives considered

1. **Keep fixtures synchronous and force async setup into helper functions inside tests**
   - Rejected because it duplicates setup logic, weakens reuse, and breaks the fixture model exactly where async resources are most useful.

2. **Introduce a separate async-fixture API unrelated to `@fixture`**
   - Rejected because it creates two mental models for the same concept and weakens the existing fixture ergonomics.

3. **Run a fresh async runtime per fixture**
   - Rejected because it complicates scope sharing, increases overhead, and makes composed async fixture graphs harder to reason about.

## Drawbacks

- Async fixture teardown and failure aggregation add complexity to the test runner.
- Cancellation and timeout semantics become materially more important once fixture setup can await external resources.
- The implementation must preserve an Incan-owned public contract even though the current runtime backing is Tokio.

## Layers affected

- **Parser / AST**: must allow and validate yield-based fixture shape inside `async def`.
- **Typechecker / symbol resolution**: must validate legal async fixture declarations and fixture dependency usage.
- **Test runner**: must execute async setup and awaited teardown while preserving scope and dependency ordering.
- **Lowering / emission**: must preserve the async fixture contract without leaking backend runtime details into user-facing semantics.
- **Docs / examples**: must explain the async fixture model, teardown guarantees, and mixed sync/async composition clearly.

## Implementation Plan

### Phase 1: Fixture shape, metadata, and diagnostics

- Accept `@fixture` on `async def` declarations without introducing a separate async fixture decorator.
- Preserve enough fixture metadata to distinguish synchronous setup, asynchronous setup, yielded fixture values, and teardown bodies.
- Validate that async fixtures use the accepted yield-based shape and emit clear diagnostics for missing yields, multiple yields, invalid teardown placement, and unsupported declaration forms.
- Keep ordinary synchronous fixture declarations source-compatible.

### Phase 2: Fixture graph execution and runtime ownership

- Resolve mixed synchronous and asynchronous fixture dependency graphs under the existing function, module, and session scope model.
- Execute all async fixture setup and teardown work on one runner-owned async runtime for the test run.
- Await async setup before dependent tests or fixtures run.
- Preserve reverse-topological teardown ordering across mixed fixture graphs, with each async teardown awaited before the next dependent teardown proceeds.

### Phase 3: Failure, timeout, and cancellation behavior

- Report setup failures against every dependent test case or fixture scope affected by the failed setup.
- Always run teardown for fixtures whose setup reached the yielded-value point when the test body fails or panics and the generated worker remains alive.
- Preserve aggregate teardown failure reporting when more than one teardown fails.
- Reuse the runner-level timeout and cancellation controls rather than adding per-fixture timeout configuration in this RFC.
- Document that hard worker termination for timeout or cancellation can bypass remaining teardown.

### Phase 4: Parametrization, docs, and release readiness

- Ensure parametrized tests expand before fixture resolution so each expanded test case receives fixture values under the normal scope rules.
- Add parser, typechecker, runner, codegen, and integration coverage for valid async fixtures, invalid shapes, teardown ordering, mixed sync/async graphs, failure aggregation, timeout/cancellation reporting, and parametrized tests.
- Update user-facing testing docs with async fixture authoring, teardown guarantees, mixed fixture composition, and timeout/cancellation behavior.
- Update release notes and bump the active development version when implementation lands.

## Implementation Log

### Spec / RFC lifecycle

- [x] RFC 004 moved from Draft to In Progress with settled design decisions.
- [x] Keep RFC 004 progress checklist current as implementation slices land.

### Fixture declaration surface

- [x] Accept `@fixture` on `async def` without adding an async-only decorator.
- [x] Preserve async fixture setup/yield/teardown metadata for downstream runner behavior.
- [x] Reject missing yields, multiple yields, invalid teardown placement, and unsupported async fixture declarations with span-precise diagnostics.
- [x] Preserve existing synchronous fixture behavior.

### Fixture graph / runtime

- [x] Resolve mixed synchronous and asynchronous fixture dependency graphs under function, module, and session scopes.
- [x] Execute async fixture setup and teardown on one runner-owned async runtime per test run.
- [x] Await async setup before dependent fixtures or tests run.
- [x] Await async teardown in reverse-topological dependency order.

### Failure / cancellation / timeout behavior

- [x] Report async fixture setup failures against affected dependent tests or scopes.
- [x] Always run teardown for yielded fixtures after test failure or panic while the generated worker remains alive.
- [x] Preserve aggregate teardown failure reporting for multiple teardown failures.
- [x] Reuse runner-level timeout and cancellation controls; do not add per-fixture timeout configuration in this RFC.
- [x] Document that hard worker timeout or cancellation can bypass remaining teardown.

### Parametrization

- [x] Expand parametrized tests before fixture resolution.
- [x] Resolve async fixtures per expanded test case under normal scope rules.
- [x] Preserve parametrized fixture caching semantics for broader scopes.

### Tests

- [x] Parser/AST tests cover accepted and rejected async fixture declarations.
- [x] Typechecker tests cover valid async fixture usage and invalid declaration/dependency forms.
- [x] Runner tests cover async setup, awaited teardown, mixed sync/async graphs, scopes, failure aggregation, timeout/cancellation reporting, and parametrization.
- [x] Codegen or snapshot tests cover emitted async fixture setup/teardown structure where applicable.
- [x] Integration tests compile and run representative async fixture suites.

### Docs / release

- [x] Update authored testing docs for async fixture authoring and teardown guarantees.
- [x] Update tooling/CLI docs for timeout and cancellation interaction where user-visible.
- [x] Add release notes for RFC 004.
- [x] Bump the active `0.3.0-dev.N` version by one before closeout.

## Design Decisions

- Async fixtures use the existing `@fixture` decorator on `async def`; this RFC does not add a separate async-only decorator.
- The test runner owns one shared async runtime per test run. Fixtures and tests must not create nested fixture-local runtimes as part of the language contract.
- Async fixture teardown is mandatory cleanup while the generated worker remains alive. If setup reaches the `yield`, teardown must be awaited when the dependent test fails or panics.
- Timeout and cancellation remain runner-level controls in this RFC. Per-fixture timeout configuration is deferred to a follow-up RFC if real use cases require it.
- Hard timeout or cancellation enforcement may terminate the worker process, so remaining teardown is best-effort in that path.
- Parametrized tests expand before fixture resolution; fixture setup and teardown then follow the normal scope rules for each expanded test case.
