# Build an asynchronous worker pipeline

This tutorial models several independent jobs, their typed results, and a deadline for slow work. It uses the task APIs `spawn`, `JoinHandle`, and `timeout` as a source-verified preview.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Application or workflow developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Functions, Result, match, and async basics</dd></div>
    <div><dt>Time</dt><dd>20–30 minutes</dd></div>
    <div><dt>Verified</dt><dd>Source and type contract verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code>; <code>std.async</code> execution needs a compatible Loaf beyond the 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Source-verified preview</dd></div>
    <div><dt>Outcome</dt><dd>A checked worker-pipeline design with typed joins and deadlines</dd></div>
    <div><dt>Artifacts</dt><dd>Checked worker-pipeline source</dd></div>
  </dl>
</aside>

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
    first = spawn(process_job("orders", 0.20))  # (1)
    second = spawn(process_job("customers", 0.10))
    third = spawn(process_job("inventory", 0.15))

    show_result("orders", await first)  # (2)
    show_result("customers", await second)
    show_result("inventory", await third)
```

1. `spawn(...)` starts each future immediately and returns a typed `JoinHandle`; the three jobs can therefore overlap.
2. Awaiting a handle returns the task boundary's `Result`, which is passed to one explicit handler rather than discarded.

The jobs begin before the first handle is awaited, so their delays overlap. Awaiting a `JoinHandle[T]` returns `Result[T, TaskJoinError]`: task execution itself can fail independently of the value `T`.

## Step 3: Handle join results

```incan
from std.async.task import TaskJoinError

def show_result(name: str, result: Result[str, TaskJoinError]) -> None:
    match result:  # (1)
        case Ok(value): println(value)
        case Err(error): println(f"{name} failed: {error.message()}")
```

1. This `match` is intentional terminal handling: both outcomes produce a visible side effect, so a combinator chain would hide rather than simplify the decision.

Do not discard join failures in production paths. A spawned task is a separate failure boundary even when the worker's ordinary return value is infallible.

## Step 4: Add a deadline

Use `timeout(...)` when the underlying operation should be cancelled after the deadline:

```incan
async def bounded_job() -> None:
    match await timeout(0.05, process_job("slow", 0.50)):  # (1)
        case Ok(value): println(value)
        case Err(error): println(f"deadline reached: {error.message()}")
```

1. `timeout(...)` owns the cancellation policy. The final `match` reports the outcome at the program boundary; it is not sequential plumbing that should be replaced with `map` or `and_then`.

Use `timeout_join(...)` instead when already-spawned work must remain live after the caller's deadline. Its timeout outcome returns the live handle rather than silently cancelling durable work.

Check the repository example:

```bash
incan check examples/advanced/async_worker_pipeline.incn
```

The finite 0.5 release envelope does not ship a compatible `std.async` Loaf. Source checking is the truthful completion boundary for this preview; do not substitute an implicit Cargo build or claim a native worker artifact.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You checked a design that starts independent work, preserves task failures as typed results, and bounds slow work with an explicit deadline. Native execution requires a toolchain with a compatible async Loaf.

</section>

## Continue

- [Async programming](../how-to/async_programming.md)
- [`std.async` reference](../reference/stdlib/async.md)
- [Error handling recipes](../how-to/error_handling_recipes.md)
