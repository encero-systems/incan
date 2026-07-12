# Build an asynchronous worker pipeline

This tutorial runs several independent jobs concurrently, waits for their typed results, and applies a deadline to slow work. It uses the currently verified task APIs: `spawn`, `JoinHandle`, and `timeout`.

<ol class="inc-step-rail" style="--inc-step-count: 4" aria-label="Async worker tutorial steps">
  <li><strong>Work</strong>Define an async job</li>
  <li><strong>Spawn</strong>Start jobs concurrently</li>
  <li><strong>Join</strong>Handle task results</li>
  <li><strong>Bound</strong>Apply a timeout</li>
</ol>

## Step 1: Define a worker

Importing `std.async` activates Incan's async syntax and runtime support:

```incan
from std.async.task import spawn
from std.async.time import sleep, timeout

async def process_job(name: str, delay: float) -> str:
    println(f"starting {name}")
    await sleep(delay)
    return f"finished {name}"
```

Calling `process_job(...)` creates async work. `await` drives it directly; `spawn(...)` starts it as a concurrent task.

## Step 2: Spawn several jobs

```incan
async def main() -> None:
    first = spawn(process_job("orders", 0.20))
    second = spawn(process_job("customers", 0.10))
    third = spawn(process_job("inventory", 0.15))

    show_result("orders", await first)
    show_result("customers", await second)
    show_result("inventory", await third)
```

The jobs begin before the first handle is awaited, so their delays overlap. Awaiting a `JoinHandle[T]` returns `Result[T, TaskJoinError]`: task execution itself can fail independently of the value `T`.

## Step 3: Handle join results

```incan
from std.async.task import TaskJoinError

def show_result(name: str, result: Result[str, TaskJoinError]) -> None:
    match result:
        case Ok(value): println(value)
        case Err(error): println(f"{name} failed: {error.message()}")
```

Do not discard join failures in production paths. A spawned task is a separate failure boundary even when the worker's ordinary return value is infallible.

## Step 4: Add a deadline

Use `timeout(...)` when the underlying operation should be cancelled after the deadline:

```incan
async def bounded_job() -> None:
    match await timeout(0.05, process_job("slow", 0.50)):
        case Ok(value): println(value)
        case Err(error): println(f"deadline reached: {error.message()}")
```

Use `timeout_join(...)` instead when already-spawned work must remain live after the caller's deadline. Its timeout outcome returns the live handle rather than silently cancelling durable work.

Run the repository example:

```bash
incan run examples/advanced/async_worker_pipeline.incn
```

!!! note "Why this tutorial does not use channels yet"
    The intended bounded-channel surface is documented under `std.async.channel`, but its typed constructor and imported wrapper methods currently fail compiler verification. This tutorial stays on task APIs that pass end-to-end checks rather than presenting an unverified queue example.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You started independent work concurrently, preserved task failures as typed results, and bounded slow work with an explicit deadline.

</section>

## Continue

- [Async programming](../how-to/async_programming.md)
- [`std.async` reference](../reference/stdlib/async.md)
- [Error handling recipes](../how-to/error_handling_recipes.md)
