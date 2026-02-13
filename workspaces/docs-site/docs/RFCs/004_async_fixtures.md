# RFC 004: Async Fixtures

**Status:** Draft
**Created:** 2025-12-10

## Summary

Define async fixtures for the Incan test framework, integrating with Tokio so `async def` fixtures can perform async
setup/teardown and participate in fixture dependency injection.

## Motivation

- Enable async setup (e.g., HTTP servers, DB pools, queues).
- Align with async tests and existing `@fixture` ergonomics.
- Ensure teardown always runs, even on panic/cancellation.

## Design Goals

- Preserve pytest-like ergonomics: `@fixture` + `yield`, but async-capable.
- Support the same scopes: function / module / session.
- Integrate with Tokio (single runtime per test run).
- Ensure deterministic teardown ordering for async fixtures.
- Work with parametrized tests and regular fixtures.

## Proposed Design

### Syntax

```incan
import std.async
from std.testing import fixture

@fixture(scope="function")
async def http_server() -> ServerHandle:
    server = await start_server(port=0)
    yield server
    await server.shutdown()

```

#### Teardown guarantee

The test runner ensures fixture teardowns run even if the test panics or is cancelled.
For async fixtures, teardown is awaited before proceeding to dependent teardowns.

### Semantics

- Async fixtures must be `async def` and use `yield` exactly once.
- The value yielded is injected into dependent tests/fixtures.
- Teardown is awaited even if the test panics or is cancelled.
- Scoping rules mirror sync fixtures; session/module fixtures are awaited once per scope.

### Tokio Integration

- A single Tokio runtime is created per test run (multi-threaded).
- Fixtures run on that runtime; no nested runtimes.
- Blocking operations inside async fixtures must use `tokio::task::spawn_blocking`.

### Fixture Graph with Mixed Sync/Async

- Dependency resolution builds a DAG of fixtures.
- If any dependency is async, dependents await it before running.
- Teardown order is reverse topological; async teardowns are awaited.

### Parametrize Interop

- Parametrized tests expand first, then fixtures are resolved per test case.
- Async fixtures can be used alongside `@parametrize` without changes to test syntax.

### Error Handling

- Setup errors fail the test (or all tests in the scope for module/session fixtures).
- Teardown errors are reported but do not mask setup failures; aggregate errors if multiple teardowns fail.

## Implementation Phases

1) Parser/AST

    - Add `Yield` expression support inside `async def`.
    - Validate single `yield` per fixture function.

2) Test Runner

    - Create shared Tokio runtime for the run.
    - Resolve fixture graph; support async setup/teardown with scopes.
    - Ensure teardown runs on panic using `catch_unwind` + `DropGuard`-like patterns.

3) Codegen

    - Emit Rust async fixtures as async functions returning the yielded value.
    - Generate await points for setup/teardown.
    - Enforce scoping caches for module/session fixtures.

4) Parametrize Integration

    - Expand parametrized tests first, then resolve fixtures per case.
    - Ensure async fixtures are awaited per case (function scope) or cached (module/session).

5) Documentation & Examples

    - Add examples for async HTTP server fixture, DB pool fixture, and queue client fixture.

## Open Questions

1. Cancellation semantics: how do we handle long-running fixtures on test abort?
2. Timeouts: should fixtures support per-fixture timeout configuration?
3. Runtime configuration: allow opting into current-thread runtime?

## Checklist

- [ ] Parser: `yield` inside `async def` fixtures
- [ ] Validator: single `yield` per fixture
- [ ] Runner: shared Tokio runtime per test run
- [ ] Runner: async fixture setup/teardown with scopes
- [ ] Teardown: always awaited, even on panic
- [ ] Parametrize interop with async fixtures
- [ ] Error reporting: setup vs teardown aggregation
- [ ] Docs/examples for async fixtures (HTTP server, DB pool, queue)
