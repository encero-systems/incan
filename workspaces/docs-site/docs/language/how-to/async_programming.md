# Async Programming in Incan

Incan provides full async/await support powered by the Tokio runtime.
This guide covers all async features available in Incan.

!!! important "Async is import-activated"
    `async` and `await` are **soft keywords**: they become reserved keywords only after importing `std.async`
    (for example `import std.async` or `from std.async.time import sleep`).

!!! note "Coming from Python?"
    Incan's async model mirrors Python's `asyncio` — you'll find `async def`, `await`, task spawning, and timeouts all
    work the same way.

    The key difference is under the hood: Incan compiles to Rust and uses Tokio, giving you the familiar Python syntax
    with Rust-level async performance.

## Quick Start

```incan
from std.async.time import sleep

async def fetch_data() -> str:
    await sleep(1.0)  # Wait 1 second
    return "data"

def main() -> None:
    # When async is used, main automatically gets #[tokio::main]
    println("Starting...")
```

## Core Concepts

### Async Functions

Declare async functions with `async def`:

```incan
from std.async.time import sleep

async def do_work() -> int:
    await sleep(0.5)
    return 42
```

### Await

Use `await` to wait for an async operation:

```incan
import std.async

async def process() -> str:
    data = await fetch_data()
    result = await transform(data)
    return result
```

`await` is only valid inside `async def` bodies and **async** methods Using `await` in an ordinary `def` or sync method is a type error.

!!! info "Coming from Python?"
    Incan's `await` follows the same rules as in Python: it is disallowed outside an `async` scope.

## Time Primitives

### sleep

Pause execution for a duration:

```incan
from std.async.time import sleep, sleep_ms

async def demo() -> None:
    await sleep(1.5)      # Sleep 1.5 seconds
    await sleep_ms(500)   # Sleep 500 milliseconds
```

### timeout

Run an operation with a time limit:

```incan
from std.async.time import timeout

async def demo() -> None:
    result = await timeout(5.0, slow_operation)
    match result:
        case Ok(value): println(f"Success: {value}")
        case Err(e): println("Operation timed out")
```

## Task Spawning

### spawn

Run a task concurrently:

```incan
from std.async.task import spawn
from std.async.time import sleep

async def background_work() -> int:
    await sleep(2.0)
    return 42

# Spawn returns immediately
handle = spawn(background_work)

# Do other work...
println("Working on other things...")

# Wait for the spawned task
result = await handle
println(f"Background task returned: {result}")
```

### spawn_blocking

Run CPU-intensive or blocking code on a dedicated thread:

```incan
from std.async.task import spawn_blocking

def heavy_computation() -> int:
    result = 0
    for i in range(1_000_000):
        result = result + i
    return result

# Won't block the async runtime
result = await spawn_blocking(heavy_computation)
```

### yield_now

Cooperatively yield to let other tasks run:

```incan
from std.async.task import yield_now

async def cooperative_loop() -> None:
    for i in range(10000):
        # Do some work...
        if i % 100 == 0:
            await yield_now()  # Let other tasks run
```

## Channels

Channels enable safe message passing between concurrent tasks.
They're the primary way to communicate between async tasks without shared mutable state.

### MPSC Channel (Multi-Producer, Single-Consumer)

**MPSC** stands for **M**ulti-**P**roducer, **S**ingle-**C**onsumer:

- **Multiple senders** can send messages (clone the sender to share it)
- **One receiver** processes all messages
- Messages arrive in order (FIFO)

#### When to use MPSC

| Use Case                       | Why MPSC                            |
| ------------------------------ | ----------------------------------- |
| Worker pool → result collector | Many workers, one aggregator        |
| Event bus                      | Multiple event sources, one handler |
| Logging                        | Multiple tasks → single log writer  |
| Request queue                  | Multiple clients → one server       |

**Bounded channel** (recommended):

```incan
from std.async.channel import channel

# Create channel with buffer size 32
tx, rx = channel[str](32)

# Sender - blocks if buffer is full (backpressure)
async def producer() -> None:
    await tx.send("hello")  # Waits if buffer full
    await tx.send("world")

# Receiver - blocks until message available
async def consumer() -> None:
    while True:
        match await rx.recv():
            case Some(msg): println(f"Got: {msg}")
            case None: break  # All senders dropped, channel closed
```

**Multiple producers** (clone the sender):

```incan
from std.async.channel import channel
from std.async.task import spawn

tx, rx = channel[int](100)

# Clone sender for each producer
tx1 = tx.clone()
tx2 = tx.clone()

spawn(async () -> None:
    await tx1.send(1)
    await tx1.send(2)
)

spawn(async () -> None:
    await tx2.send(100)
    await tx2.send(200)
)

# Single consumer receives from all producers
async def consume() -> None:
    while True:
        match await rx.recv():
            case Some(n): println(f"Received: {n}")
            case None: break
```

!!! note "Coming from Python?"
    Incan channels are inspired by Rust's `tokio::sync::mpsc`, not Python's `asyncio.Queue`. Key differences:

    - **Sender/Receiver split**: You get a `(tx, rx)` pair — clone `tx` for multiple producers
    - **Ownership semantics**: When all senders are dropped, the channel closes and `recv()` returns `None`
    - **No shared queue**: Unlike `asyncio.Queue`, you can't just pass the queue around — you pass senders or the receiver

    The Python equivalent would use `asyncio.Queue` with `put()`/`get()`, but lacks the automatic "channel closed" signal
    when producers finish.

**Bounded vs Unbounded:**

| Type                     | Backpressure                | Memory Safety    | Use When                |
| ------------------------ | --------------------------- | ---------------- | ----------------------- |
| `channel[T](n)`          | Yes - send blocks when full | Bounded memory   | Production code         |
| `unbounded_channel[T]()` | No - send never blocks      | Can grow forever | Prototyping, low-volume |

### Unbounded Channel

`send` never blocks, but use with caution — if producers outpace consumers, memory grows without limit:

```incan
from std.async.channel import unbounded_channel

# No capacity limit - send always succeeds immediately
tx, rx = unbounded_channel[int]()

# These never block
tx.send(1)
tx.send(2)
tx.send(3)

# Consumer still blocks waiting for messages
match await rx.recv():
    case Some(n): println(f"Got: {n}")
    case None: println("Channel closed")
```

**When unbounded is OK:**

- Low message volume
- Consumers always faster than producers
- Prototyping (switch to bounded before production)

### One-Shot Channel

A **oneshot channel** sends exactly one value.
After sending, the sender is consumed and can't be used again.
This is perfect for "request-response" patterns where you spawn a task and wait for its result.

**When to use oneshot vs regular channel:**

| Use Case                        | Channel Type |
| ------------------------------- | ------------ |
| Stream of messages              | `channel[T]` |
| Single result from spawned task | `oneshot[T]` |
| Producer-consumer queue         | `channel[T]` |
| Async function return value     | `oneshot[T]` |

**Basic usage:**

```incan
from std.async.channel import oneshot
from std.async.task import spawn

tx, rx = oneshot[int]()

spawn(async () -> None:
    result = expensive_computation()
    tx.send(result)  # Consumes sender - can only send once
)

# Wait for the result
match await rx.recv():
    case Ok(value): println(f"Got result: {value}")
    case Err(e): println("Sender dropped without sending")
```

**Common pattern - returning results from spawned tasks:**

```incan
from std.async.channel import oneshot
from std.async.task import spawn

async def compute_in_background(input: Data) -> Result[Output, ComputeError]:
    # Create oneshot for the result
    tx, rx = oneshot[Result[Output, ComputeError]]()
    
    # Spawn the computation
    spawn(async () -> None:
        result = heavy_computation(input)
        tx.send(result)
    )
    
    # Do other work while computation runs...
    do_other_stuff()
    
    # Wait for result
    match await rx.recv():
        case Ok(result): return result
        case Err(_): return Err(ComputeError.TaskFailed)
```

**Key differences from regular channels:**

- `send()` consumes the sender (can only call once)
- No buffering - it's either empty or has exactly one value
- Lighter weight than MPSC channels
- `recv()` returns `Err` if sender was dropped without sending

## Synchronization Primitives

Incan provides low-level synchronization primitives inspired by Rust's tokio runtime.
These give you fine-grained control over shared state and task coordination.

!!! note "Coming from Python?"
    Python's `asyncio` has some equivalents, but Incan's primitives work differently.

| Incan       | Python asyncio            | Key Difference                                       |
| ----------- | ------------------------- | ---------------------------------------------------- |
| `Mutex[T]`  | `asyncio.Lock`            | Incan wraps the value; Python protects external data |
| `RwLock[T]` | *(no equivalent)*         | Allows multiple readers OR single writer             |
| `Semaphore` | `asyncio.Semaphore`       | Similar behavior                                     |
| `Barrier`   | `asyncio.Barrier` (3.11+) | Similar behavior                                     |

The key difference: Incan's `Mutex[T]` and `RwLock[T]` **wrap a value** (like Rust),
while Python's Lock is just a lock you use alongside your data.

### Mutex

Mutual exclusion — ensures only **one task** can access the wrapped value at a time.

**When to use:** Multiple tasks need to both read AND write shared state.

**API:**

- `Mutex(value)` — Create a mutex wrapping a value
- `await mutex.lock()` — Acquire lock, returns a guard
- `guard.get()` — Read the value
- `guard.set(new_value)` — Write a new value
- Guard auto-releases when it goes out of scope

```incan
from std.async.sync import Mutex

shared_counter = Mutex(0)

async def increment() -> None:
    guard = await shared_counter.lock()  # Blocks until lock acquired
    current = guard.get()
    guard.set(current + 1)
    # Lock released when guard goes out of scope
```

!!! note "Python comparison"
    Python uses a lock separate from the data. Incan wraps the data (`Mutex[T]`), so the lock and value are kept together.

```python
# Python - lock is separate from data
lock = asyncio.Lock()
counter = 0

async def increment():
    async with lock:
        counter += 1
```

```incan
from std.async.sync import Mutex

# Incan - lock wraps the data
counter = Mutex(0)

async def increment() -> None:
    guard = await counter.lock()
    guard.set(guard.get() + 1)
```

### RwLock

Reader-writer lock — allows **multiple readers** OR a **single writer**, but not both simultaneously.
More efficient than Mutex when reads are frequent.

**When to use:** Many tasks read, few tasks write (e.g., configuration, caches).

**API:**

- `RwLock(value)` — Create an RwLock wrapping a value
- `await rwlock.read()` — Acquire read lock (shared with other readers)
- `await rwlock.write()` — Acquire write lock (exclusive)
- `guard.get()` — Read the value
- `guard.set(new_value)` — Write (only on write guard)

```incan
from std.async.sync import RwLock

config = RwLock(Config(debug=False))

async def read_config() -> bool:
    guard = await config.read()  # Multiple readers allowed simultaneously
    return guard.get().debug

async def update_config(debug: bool) -> None:
    guard = await config.write()  # Waits for all readers, then exclusive
    guard.set(Config(debug=debug))
```

!!! note "Coming from Python?"
    Python's `asyncio` doesn't have `RwLock`. This is a Rust/systems programming concept.

    If you need read-heavy access patterns in Python, you typically just use a `Lock` for everything.

### Semaphore

Counting semaphore — limits how many tasks can access a resource concurrently by managing a pool of "permits."

**When to use:** Connection pools, rate limiting, bounding concurrent operations.

**API:**

- `Semaphore(n)` — Create with n permits
- `await sem.acquire()` — Wait for and acquire a permit
- Permit auto-releases when it goes out of scope

```incan
from std.async.sync import Semaphore

# Allow max 3 concurrent connections
connection_limit = Semaphore(3)

async def make_request() -> Response:
    permit = await connection_limit.acquire()  # Blocks if no permits
    response = await http_get(url)
    # Permit released automatically when permit goes out of scope
    return response
```

!!! note "Python equivalent"
    `asyncio.Semaphore(n)` works the same way.

### Barrier

Synchronization point — makes N tasks wait until **all of them** reach the barrier before any can proceed.

**When to use:** Phased algorithms, coordinating startup, map-reduce patterns.

**API:**

- `Barrier(n)` — Create barrier for n tasks
- `await barrier.wait()` — Wait until all n tasks reach this point

```incan
from std.async.sync import Barrier

barrier = Barrier(3)  # Wait for 3 tasks

async def worker(id: int) -> None:
    println(f"Worker {id} starting phase 1")
    # ... do phase 1 work ...
    
    await barrier.wait()  # All 3 must reach here before anyone continues
    
    println(f"Worker {id} starting phase 2")  # All start phase 2 together
```

!!! note "Python equivalent"
    `asyncio.Barrier(n)` (added in Python 3.11) works the same way.

### Complete Example

See [examples/advanced/async_sync.incn](https://github.com/dannys-code-corner/incan/blob/main/examples/advanced/async_sync.incn)
for a runnable demo of all four primitives.

## Select (Racing Futures)

### select2

Race two futures, get whichever completes first:

```incan
from std.async.select import select2
from std.async.time import sleep

async def fast() -> str:
    await sleep(0.1)
    return "fast"

async def slow() -> str:
    await sleep(1.0)
    return "slow"

match await select2(fast, slow):
    case Either.Left(result): println(f"Fast won: {result}")
    case Either.Right(result): println(f"Slow won: {result}")
```

### select3

Race three futures:

```incan
from std.async.select import select3

result = await select3(op1, op2, op3)
match result:
    case Either3.First(v): println("op1 won")
    case Either3.Second(v): println("op2 won")
    case Either3.Third(v): println("op3 won")
```

### select_timeout

Simplified timeout returning Option:

```incan
from std.async.select import select_timeout

match await select_timeout(2.0, slow_operation):
    case Some(result): println(f"Got: {result}")
    case None: println("Timed out, using default")
```

## Runtime Integration

### Automatic Tokio Main

When your program uses async features, the `main()` function is automatically wrapped with `#[tokio::main]`:

Tokio is Rust's async runtime: see the [Tokio project](https://tokio.rs/) for a high-level overview.

```incan
import std.async

def main() -> None:
    # This becomes async main() under the hood
    # when async primitives are used
    result = await fetch_data()
    println(f"Result: {result}")
```

### Generated Dependencies

The compiler automatically adds tokio to your Cargo.toml:

```toml
[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
```

## Best Practices

### 1. Don't Block the Runtime

Avoid blocking operations in async code:

```incan
from std.async.time import sleep

# BAD: Blocks the async runtime
async def bad() -> None:
    std_thread_sleep(1.0)  # Don't do this!

# GOOD: Use async sleep
async def good() -> None:
    await sleep(1.0)
```

### 2. Use spawn_blocking for CPU Work

```incan
from std.async.task import spawn_blocking

# BAD: CPU work on async runtime
async def bad() -> int:
    return heavy_computation()

# GOOD: Offload to blocking pool
async def good() -> int:
    return await spawn_blocking(heavy_computation)
```

### 3. Use Bounded Channels

Prefer bounded channels to prevent memory issues:

```incan
from std.async.channel import channel, unbounded_channel

# Prefer this:
tx, rx = channel[Data](100)

# Over this (unbounded can grow forever):
tx, rx = unbounded_channel[Data]()
```

### 4. Handle Cancellation

Tasks can be cancelled when their handles are dropped:

```incan
from std.async.task import spawn
from std.async.time import sleep

async def cancellable_work() -> str:
    await sleep(10.0)
    return "done"

handle = spawn(cancellable_work)
# If we don't await handle, the task is cancelled!
# Always await or explicitly ignore:
_ = handle  # Explicit ignore (task continues)
```

## Error Handling

### Timeout Errors

```incan
from std.async.time import timeout

result = await timeout(1.0, slow_task)
match result:
    case Ok(value): process(value)
    case Err(e): println(f"Timed out: {e}")
```

### Channel Closed

When sending fails because the receiver was dropped, `SendError[T]` contains the value that couldn't be sent in its
`.value` field:

```incan
from std.async.channel import Sender

async def sender(tx: Sender[Message]) -> None:
    msg = Message(id=1, content="important data")
    
    match await tx.send(msg):
        case Ok(_): println("Sent successfully")
        case Err(e):
            # Recover the unsent value - it's not lost!
            println(f"Channel closed, saving for retry: {e.value:?}")
            save_for_later(e.value)
```

This pattern is important for reliability.
If a channel closes unexpectedly, you can recover and handle the data that failed to send.

## See Also

- [Error handling](../explanation/error_handling.md) - Concepts: `Result`, `Option`, `?`, `match`
- [Error handling recipes](../how-to/error_handling_recipes.md) - Patterns and best practices
- [Error trait](../reference/stdlib_traits/error.md) - Stdlib trait reference
- [Examples: Async Tasks](https://github.com/dannys-code-corner/incan/blob/main/examples/advanced/async_tasks.incn)
- [Examples: Channels](https://github.com/dannys-code-corner/incan/blob/main/examples/advanced/async_channels.incn)
- [Examples: Synchronization](https://github.com/dannys-code-corner/incan/blob/main/examples/advanced/async_sync.incn)
