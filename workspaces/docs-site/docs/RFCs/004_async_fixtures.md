# RFC 004: async fixtures

- **Status:** Draft
- **Created:** 2025-12-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 018 (testing), RFC 019 (runner testing)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/78
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC proposes async fixtures for Incan's test framework so `async def` fixtures can perform awaited setup and teardown while participating in ordinary fixture injection and scope management. The intended user experience stays close to the existing `@fixture` plus `yield` model, but the runtime contract becomes async-aware and guarantees awaited teardown ordering.

## Motivation

- Tests increasingly need async setup for HTTP servers, database pools, queues, and other service dependencies.
- Async tests and async fixtures should share one coherent model rather than forcing users into ad hoc setup helpers.
- Teardown must remain reliable even when async tests fail, panic, or are cancelled.

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

The runner must treat teardown as mandatory cleanup work. Async teardown is part of the fixture contract, not a best-effort callback.

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
- The RFC currently leans on a Tokio-backed implementation story, which may need tighter wording so the public contract remains Incan-owned rather than Tokio-shaped.

## Layers affected

- **Parser / AST**: must allow and validate yield-based fixture shape inside `async def`.
- **Typechecker / symbol resolution**: must validate legal async fixture declarations and fixture dependency usage.
- **Test runner**: must execute async setup and awaited teardown while preserving scope and dependency ordering.
- **Lowering / emission**: must preserve the async fixture contract without leaking backend runtime details into user-facing semantics.
- **Docs / examples**: must explain the async fixture model, teardown guarantees, and mixed sync/async composition clearly.

## Unresolved questions

- What should cancellation semantics be for long-running async fixtures when a test run is aborted?
- Should fixtures support per-fixture timeout configuration, or should timeout handling stay outside the fixture contract?
- Does the runner need an explicit public runtime-configuration story, or is one shared async runtime enough for the RFC's contract?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
