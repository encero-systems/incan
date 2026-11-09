## Applying the boundary to the remaining families

This is a design test drive against the current stdlib source. All signatures below are proposed; they are not implemented APIs or claims of passing tests. The first implementation step remains moving canonical declarations without breaking existing callers. New ownership guarantees require RFC 164 enforcement before these entrypoints can ship.

The common pattern is **data contracts in models, failures in errors, opaque resources beside their operations, and ordinary functions for doing work**. This does not ban protocol methods required by an existing trait, such as an error's display behavior. It avoids making a convenience class the foundation every lower layer must depend on.

### Complete family map

| Family | Primitive responsibility | Stays in the higher-level facade |
| --- | --- | --- |
| Paths | Native path representation, construction and component operations through functions | Rich Path class, slash operator and convenience composition |
| Files and directories | Open/read/write/seek, explicit flush/sync/close, directory traversal resources, host metadata observation | Text decoding, recursive recipes, filtering and application policy |
| Mapping | Owned mappings, independently retained ranges, explicit copying and separately checked borrowed projections | File-oriented parsing and hashing |
| Locks | Shared/exclusive acquisition, contention, descriptor ownership and release | Retry policy, application scheduling and transaction policy |
| Temporary resources | Secure acquisition, cleanup ownership and explicit persistence | Spooling thresholds, defaults and multi-step convenience workflows |
| In-memory I/O | Byte cursors, buffers and endian-aware operations | Unified file/buffer convenience classes |
| Hashing | Byte algorithms and their incremental state, including keyed algorithms | File/reader loops, algorithm-name dispatch and workflow policy |
| Encoding | Byte/text transformations and exact malformed-input errors | File/stream adapters and convenience conversion recipes |
| Environment | Precise observation of host variables and arguments | Defaults, typed configuration conversion and ctx resolution |
| Async channels | Endpoints, capacity permits and send/receive mechanics | Application error policies and pipeline orchestration |
| Async synchronization | Locks, owned guards, semaphore permits and barrier generations | Worker coordination and recovery policy |
| Async execution/time | Task handles, runtime admission, abort/wait, clocks and timer mechanics | Task groups, application deadlines and higher-level workflows |
| Vocabulary | Pure syntax data and construction/inspection helpers; canonical vocab exposure | JSON codecs, worker transport and compiler execution |

An import of the primitive root must not activate async syntax or initialize a runtime. Importing the async family exposes the same canonical async vocabulary as the existing public entrypoint.

### Shared declarations: avoid a new dependency knot

| Proposed home | Examples | Boundary |
| --- | --- | --- |
| models | Endian and other genuinely shared values | Core dependencies only |
| models.vocab | Span, syntax nodes and vocabulary descriptors | Pure data; no JSON, serde, worker I/O or compiler execution |
| errors | IoError, EncodingError, HashError, EnvironError and payload-only async errors | May depend on shared values; must not import resource modules |
| Shared trait home | Existing Error/I/O contracts and native Send/Static facts where applicable | Preserve canonical identity and checked meaning; do not create lookalike traits |
| Resource family | File, MappedView, hash State, SenderPermit, MutexGuard and JoinHandle | Resource behavior and native representation stay with their owner |

The `std.primitives.traits` path used in the task sketch is a proposed grouping, not an extra settled namespace requirement. Extract only actual shared contracts. A result enum containing a live task handle stays with async, rather than forcing models to import async. A generic `SendError[T]` can carry T without importing T's defining resource module.

### Hashes: the algorithm only needs bytes

**Today:** the core hash module imports filesystem MappedView, and high-level hashing also includes file/reader loops. Existing incremental SHA-256 finalization resets the state.

**Proposed:** each algorithm has functional byte operations. SHA-256 is one algorithm family, not the only primitive. Keep SHA-2, SHA-3, SHAKE, BLAKE, xxHash, legacy compatibility algorithms and HMAC within the same ownership/dependency treatment. Moving them does not endorse every algorithm for every security use.


```python
"""Proposed std.primitives.hash.sha256 boundary; not compiled.
Other algorithm families use the same explicit state pattern.
"""
pub class State:
    """Private native SHA-256 state; no filesystem dependencies."""

pub def new() -> Owned[State]:
    """Create empty hashing state."""
    ...

pub def update(mut state: BorrowedMut[State], data: Borrowed[bytes]) -> None:
    """Absorb bytes without retaining a borrow of data."""
    ...

pub def finish(state: Owned[State]) -> Owned[bytes]:
    """Consume state and return an independent 32-byte digest."""
    ...

pub def finish_reset(mut state: BorrowedMut[State]) -> Owned[bytes]:
    """Return a digest and reset state, preserving today's finalize-reset behavior."""
    ...

pub def digest(data: Borrowed[bytes]) -> Owned[bytes]:
    """Hash one byte value without file, stream, or algorithm-selection policy."""
    ...
```


```python
"""Proposed accepted byte-algorithm consumer; all input is already bytes."""
from std.primitives.hash.sha256 import new, update, finish, finish_reset, digest

def hash_parts(parts: Borrowed[list[bytes]]) -> Owned[bytes]:
    mut state = new()
    for part in parts:
        update(state, part)
    return finish(state)

def reset_preserves_existing_behavior() -> None:
    mut state = new()
    update(state, b"AB")
    first = finish_reset(state)
    assert first == digest(b"AB")
    update(state, b"CD")
    assert finish(state) == digest(b"CD")
```


The two finalizers are deliberate: finish consumes state; finish_reset preserves today's reset behavior for the old facade. Mapping-aware hashing becomes composition above the byte algorithm, as shown below. No hash implementation needs to import a file or mapping type.

**Proof needed:** identical digest bytes and reset behavior for every moved algorithm; chunked and one-shot agreement; existing HMAC verification behavior; no secret key material in diagnostics; no hidden copies of incremental native state.

### Encoding: preserve exact decoding behavior

**Today:** the shared encoding module combines EncodingError with Path/BytesIO file helpers. That makes a simple value conversion depend on more than it needs.

**Proposed:** move the canonical error to errors, byte/text codecs to their primitive family, and leave file helpers above them. Apply this to hex, base32/base64/base58/base85, bech32 and their existing variants. Strict and permissive variants must retain their existing distinctions.


```python
"""Proposed std.primitives.encoding.hex boundary; not compiled."""
from std.primitives.errors import EncodingError

pub def hexlify(data: Borrowed[bytes]) -> Owned[str]:
    """Return independent lowercase hexadecimal text."""
    ...

pub def unhexlify(text: Borrowed[str]) -> Result[Owned[bytes], EncodingError]:
    """Strictly decode even-length hex; report malformed position without retaining input."""
    ...
```


```python
"""Proposed accepted value conversion, without file or stream dependencies."""
from std.primitives.encoding.hex import hexlify, unhexlify
from std.primitives.errors import EncodingError

def decode_payload(text: Borrowed[str]) -> Result[Owned[bytes], EncodingError]:
    return unhexlify(text)

def roundtrip() -> Result[None, EncodingError]:
    assert hexlify(b"AB") == "4142"
    assert decode_payload("4142")? == b"AB"
    match decode_payload("4z"):
        Ok(_) => assert false
        Err(error) =>
            assert error.position == 1
    return Ok(None)
```


The negative example checks a structured position, not a parsed error string. Existing `std.encoding.hex` imports continue to work. Its facade delegates to the same encoder/decoder, not a second implementation.

### Environment: observing absence is different from hiding failure

**Today:** the existing optional getter deliberately converts any read error to None. Typed conversion and defaults also live in std.environ.

**Proposed:** offer a precise primitive observation. Missing is Ok(None), an explicitly empty value is Ok(Some("")), and an invalid key or non-Unicode value is Err. The existing optional getter can preserve its documented lossy behavior by wrapping this operation.


```python
"""Proposed precise std.primitives.environ boundary; not compiled."""
from std.primitives.errors import EnvironError

pub def observe(key: Borrowed[str]) -> Result[Option[Owned[str]], EnvironError]:
    """Absent is Ok(None), empty is Some empty text, invalid key/non-Unicode is Err."""
    ...

pub def args() -> Result[Owned[list[str]], EnvironError]:
    """Read the current argument vector without lossy conversion or logging its values."""
    ...
```


```python
"""Proposed observation case; configuration defaults are deliberately outside primitives."""
from std.primitives.environ import observe
from std.primitives.errors import EnvironError

def optional_profile() -> Result[Option[Owned[str]], EnvironError]:
    # Missing, empty, and invalid Unicode remain distinct; never log the returned value.
    return observe("APP_PROFILE")
```


The application or ctx decides defaults and conversion. The primitive never logs returned values or includes secret values in diagnostics. Tests must restore the test environment and distinguish absent, empty, invalid and non-Unicode inputs where the host supports them.

### Filesystem resources: owning a range is different from borrowing bytes

**Today:** MappedView already retains its mapping independently. File locks own descriptors and coordinate cooperating processes through the existing sibling-lock convention.

**Proposed:** preserve these resource facts. A retained view can outlive the original mapping handle. A byte projection from that view cannot outlive the view itself.


```python
"""Proposed std.primitives.fs resource operations; not compiled.
Path is the primitive path. Mapping view retention is explicit, not a lexical borrow.
"""
from std.primitives.fs import Path
from std.primitives.errors import IoError

pub class MappedFile:
    """Owns a read-only mapping; caller must uphold the immutable-file contract."""

pub class MappedView:
    """Retains a mapping owner independently; its range is not a copied byte buffer."""

pub class FileLock:
    """Owns an advisory-lock descriptor; no implicit duplicate guard."""

pub def map_read(path: Borrowed[Path]) -> Result[Owned[MappedFile], IoError]:
    """Map an immutable backing file; path values are not retained borrows."""
    ...

pub def retain_view(mapping: Borrowed[MappedFile], offset: int, length: int) -> Option[Owned[MappedView]]:
    """Retain a bounded mapping range independently; invalid ranges return None."""
    ...

pub def view_bytes(view: Borrowed[MappedView]) -> Borrowed[bytes] from view:
    """Expose a compatible borrowed byte projection; no copy or extra retention."""
    ...

pub def to_bytes(view: Borrowed[MappedView]) -> Owned[bytes]:
    """Explicitly copy the view into independent bytes."""
    ...

pub def try_exclusive(path: Borrowed[Path]) -> Result[Option[Owned[FileLock]], IoError]:
    """Acquire the existing sibling advisory lock; contention is Ok(None)."""
    ...

pub def unlock(lock: Owned[FileLock]) -> Result[None, IoError]:
    """Consume the guard and expose explicit unlock failure; drop remains fallback cleanup."""
    ...
```


```python
"""Proposed mapping-to-hash composition; hash itself does not import filesystem types."""
from std.primitives.fs import Path, map_read, retain_view, view_bytes
from std.primitives.hash.sha256 import digest
from std.primitives.errors import IoError

def hash_prefix(path: Borrowed[Path]) -> Result[Option[Owned[bytes]], IoError]:
    mapping = map_read(path)?
    match retain_view(mapping, offset=0, length=64):
        Some(view) => return Ok(Some(digest(view_bytes(view))))
        None => return Ok(None)
```


The example hashes a 64-byte prefix. A file too short for that range returns Ok(None); opening/mapping failure returns Err. The hash consumes borrowed bytes synchronously and retains no view.

The new borrowed projection needs an actual native representation proof. If existing Incan bytes cannot borrow the mapping representation, this entrypoint remains unavailable until the compiler/native boundary supports it. An invisible copy would violate its contract; `to_bytes` is the explicitly copying alternative.

Read-only mapping still requires the backing-file immutability contract. A readonly handle or an advisory lock does not prevent an uncooperative process from truncating the file.

**Paths and metadata:** primitive operations can be ordinary `path(text)`, `join(parent, component)` and `metadata(path)` functions; the exact signatures follow native path encoding and error decisions during implementation. Do not require the high-level slash notation. The current PathStat methods re-read host metadata; a new snapshot-style FileInfo would be a different contract and must not silently replace that behavior. Directory iterators own traversal state; a directory entry containing a Path belongs in fs, not in the core-only shared models module.

**Writes and durability:** explicit write/flush/sync failures remain observable. Close consumes the handle; a successful close is not automatically a durability guarantee. These requirements apply to future writable-file declarations just as they do to the read-only boundary above.

### Temporary resources: a path is not cleanup ownership

**Today:** temporary resources wrap native cleanup objects; spooled files additionally decide when to spill from memory to disk.

**Proposed:** keep cleanup ownership in the primitive resource and spilling policy above it. The primitive accepts an explicit parent; host default-directory observation can be a separate operation.


```python
"""Proposed std.primitives.fs temporary-directory operations; not compiled."""
from std.primitives.fs import Path
from std.primitives.errors import IoError

pub class TemporaryDirectory:
    """Owns temporary cleanup responsibility; private state may become persisted."""

pub def temporary_directory(parent: Borrowed[Path], prefix: Borrowed[str]) -> Result[Owned[TemporaryDirectory], IoError]:
    """Acquire in an explicit parent; host default-directory policy is separate."""
    ...

pub def directory_path(temporary: Borrowed[TemporaryDirectory]) -> Borrowed[Path] from temporary:
    """Borrow the resource's path value without extending its filesystem lifetime."""
    ...

pub def persist_directory(mut temporary: BorrowedMut[TemporaryDirectory]) -> Result[Owned[Path], IoError]:
    """Disable cleanup only on success; preserve recoverable live state on failure."""
    ...

pub def cleanup_directory(temporary: Owned[TemporaryDirectory]) -> Result[None, IoError]:
    """Consume cleanup responsibility and report explicit removal failure."""
    ...
```


```python
"""Proposed explicit persistence; caller supplies an existing parent directory."""
from std.primitives.fs import Path, temporary_directory, persist_directory
from std.primitives.errors import IoError

def keep_scratch_directory(parent: Borrowed[Path]) -> Result[Owned[Path], IoError]:
    mut temporary = temporary_directory(parent, prefix="incan-")?
    # Failure leaves cleanup responsibility with temporary.
    # Success disables cleanup and returns an independent path value.
    return persist_directory(temporary)
```


Borrowing the directory's path does not transfer cleanup responsibility. Persist succeeds by disabling cleanup and returning an independent path value. On failure, the proposed contract retains recoverable cleanup state; implementation must prove this rather than discard a native error's retained resource. Named temporary files need the same success/failure ownership analysis. Spooling thresholds remain a higher-level decision.

### Async channels: transfer the payload only after capacity is available

**Today:** a pending send can own its payload, so cancelling that send can drop the payload. The existing reserve operation already has a native owned permit.

**Proposed:** expose both behaviors clearly. Reserve first when retaining the payload until capacity is available matters. Cancellation during reserve has no payload to lose.


```python
"""Proposed std.primitives.async.channel boundary; not compiled.
Importing this family activates the one canonical async vocabulary.
"""
import std.primitives.async
from std.primitives.errors import SendError

pub class Sender[T]:
    """Owns a sending endpoint; explicit duplication can retain another endpoint."""

pub class Receiver[T]:
    """Owns the receiving endpoint; receive requires exclusive access."""

pub class SenderPermit[T]:
    """Owns one reserved capacity slot and independently retains the channel."""

pub def channel[T](capacity: int) -> Tuple[Owned[Sender[T]], Owned[Receiver[T]]]:
    """Create a bounded channel with the existing capacity-validity contract."""
    ...

pub async def reserve[T](sender: Borrowed[Sender[T]]) -> Result[Owned[SenderPermit[T]], SendError[None]]:
    """Wait for capacity before transferring a payload; cancellation loses only queue position."""
    ...

pub def send_reserved[T](permit: Owned[SenderPermit[T]], value: Owned[T]) -> Result[None, SendError[T]]:
    """Synchronously consume permit and deliver value or return it in the error."""
    ...

pub async def receive[T](mut receiver: BorrowedMut[Receiver[T]]) -> Option[Owned[T]]:
    """Cancel-safe receive; None means closed and fully drained."""
    ...
```


```python
"""Proposed bounded-channel acceptance case; an async test harness runs this function."""
import std.primitives.async
from std.primitives.async.channel import Receiver, channel, reserve, send_reserved, receive
from std.primitives.errors import SendError

async def deliver_one() -> Result[None, SendError[None]]:
    sender, incoming = channel[int](1)
    mut receiver: Owned[Receiver[int]] = incoming
    permit = await reserve(sender)?
    match send_reserved(permit, 42):
        Ok(_) =>
            match await receive(receiver):
                Some(value) => assert value == 42
                None => assert false
        Err(error) =>
            assert error.value == 42
            assert false  # This test keeps its receiver alive; a send must succeed.
    return Ok(None)
```


This is an acceptance example, run by an async test harness. The receiver is alive throughout, so the assertion branches identify test failures. In an application the SendError payload is available for recovery.

The permit independently retains the channel; it is not declared as a borrow from the sender. Releasing an unused permit returns its capacity. Direct send remains valid with its documented cancellation semantics. Unbounded and oneshot channels retain their own closure, capacity and error contracts; they are not coerced into the bounded-channel example.

**Proof needed:** capacity returns after permit drop, no payload disappears before transfer, closed channels return the unsent payload, cancelled receive does not consume an item, and None means closed-and-drained for this receive operation. Keep the existing try-receive ambiguity and receiver-close behavior explicit rather than changing them accidentally during relocation.

### Async guards: this is a useful exclusive Reference

**Today:** native mutex guards independently retain their mutex storage, but the current public getter clones the protected value.

**Proposed:** keep owned guards and add a real exclusive projection. This is different from making every mutating function return a Reference for chaining.


```python
"""Proposed std.primitives.async.sync boundary; not compiled.
Guard acquisition explicitly retains the lock, matching the existing native owned guard.
"""
import std.primitives.async

pub class Mutex[T]:
    """Retained mutex storage; owning a handle does not itself grant access to its value."""

pub class MutexGuard[T]:
    """Owns lock release responsibility and independently retains the mutex storage."""

pub def mutex[T](value: Owned[T]) -> Owned[Mutex[T]]:
    """Transfer a value into lock-protected storage."""
    ...

pub async def lock[T](mutex: Borrowed[Mutex[T]]) -> Owned[MutexGuard[T]]:
    """Acquire an independently retained guard; cancellation before acquisition gives up queue position."""
    ...

pub def guard_value[T](mut guard: BorrowedMut[MutexGuard[T]]) -> Reference[T] from guard:
    """Borrow the protected place exclusively; no cloned get/set value."""
    ...

pub def unlock[T](guard: Owned[MutexGuard[T]]) -> None:
    """Release the guard; no dependent reference may remain live."""
    ...
```


```python
"""Proposed owned guard with explicit exclusive projection; requires native borrow proof."""
import std.primitives.async
from std.primitives.async.sync import mutex, lock, guard_value, unlock

async def increment() -> int:
    counter = mutex(0)
    mut guard = await lock(counter)
    mut value = guard_value(guard)
    value += 1
    observed = value
    # Observed is a checked scalar copy; no guard reference escapes.
    unlock(guard)
    return observed
```


```python
"""Proposed rejection: the exclusive projection still depends on the guard."""
import std.primitives.async
from std.primitives.async.sync import mutex, lock, guard_value, unlock

async def release_too_early() -> int:
    counter = mutex(0)
    mut guard = await lock(counter)
    mut value = guard_value(guard)
    unlock(guard)  # Reject: consuming guard conflicts with the later use below.
    value += 1
    return value
```


The rejection follows a concrete dependency: guard_value produced value from guard; unlock consumes guard; value is used afterwards. The diagnostic should identify those three sites.

This is not a rename of the current cloning getter. Its native wrapper includes an additional lock around the owned guard; returning a reference past a temporary native lock is unsafe. S5 must provide a valid representation and lifetime proof before this projection can be implemented. Until then, the existing get/set facade remains valid, and the new projection is gated.

Apply the same analysis to RwLock read/write guards, semaphore permits and barrier generations. Preserve cancellation and queue-position rules. A barrier participant cancelling before release is a different event from cancelling after its generation has been released.

### Tasks and time: ownership does not imply cancellation

**Today:** async has task handles, runtime future/callable contracts, Send/Static admission, timers and timeout/race outcomes. Some outcomes retain live handles.

**Proposed:** keep these mechanisms in their owning family. Moving them does not introduce a new cancellation policy.


```python
"""Proposed std.primitives.async.task boundary; not compiled.
Send/Static retain their canonical native facts. RuntimeFuture/RuntimeFnOnce
are the current checked runtime contracts, not new duck-typed wrapper traits.
"""
import std.primitives.async
from std.primitives.traits import Send, Static
from std.primitives.async import RuntimeFuture, RuntimeFnOnce
from std.primitives.errors import TaskJoinError

pub class JoinHandle[T]:
    """Retains a running task; dropping the handle detaches, not cancels."""

pub def spawn[T with (Send, Static), F with RuntimeFuture[T]](task: Owned[F]) -> Owned[JoinHandle[T]]:
    """Transfer an admitted future to the runtime; reject invalid borrowed captures."""
    ...

pub def spawn_blocking[T with (Send, Static), F with RuntimeFnOnce[T]](task: Owned[F]) -> Owned[JoinHandle[T]]:
    """Schedule admitted blocking work; abort cannot stop it after execution begins."""
    ...

pub async def wait[T with (Send, Static)](handle: Owned[JoinHandle[T]]) -> Result[Owned[T], TaskJoinError]:
    """Consume the handle and await its result; dropping this wait does not imply task cancellation."""
    ...

pub def abort[T](handle: Borrowed[JoinHandle[T]]) -> None:
    """Request task cancellation; completion must still be observed by waiting."""
    ...
```


```python
"""Proposed accepted task transfer with no borrowed capture."""
import std.primitives.async
from std.primitives.async.task import spawn, wait
from std.primitives.errors import TaskJoinError

async def compute() -> int:
    return 42

async def run_one() -> Result[int, TaskJoinError]:
    handle = spawn(compute())
    return await wait(handle)
```


The wait signature is a proposed explicit consuming boundary. Existing high-level method behavior must remain compatible. Dropping a task handle detaches; abort requests cancellation; waiting observes completion. Aborting blocking work cannot stop it once it has begun. Borrowed captures must satisfy the runtime admission contract, not be silently copied to pass it.

Keep timer precision, Duration validation, timeout precedence and retained-handle outcomes unchanged during migration. Pure shared duration values may live in models if genuinely shared; outcomes carrying handles stay in async. RFC 142's task-group/error-policy design is not being silently implemented through this move.

### Vocabulary: separate syntax data from transporting it

**Today:** std.vocab combines structured syntax models and helpers with JSON/serde, stdin/stdout worker transport and serialization methods.

**Proposed:** the primitive layer supplies pure syntax data and operations. Serialization and worker transport consume those declarations from above. Async's vocab can use this foundation without bringing JSON or a worker process into primitives.


```python
"""Proposed std.primitives.vocab construction helpers; not compiled.
Models are extracted from current std.vocab without JSON/serde or worker I/O.
"""
from std.primitives.models.vocab import Span, IncanExpr

pub def path_expression(name: Borrowed[str], span: Span) -> Owned[IncanExpr]:
    """Construct a spanned path expression with independently owned name components."""
    ...

pub def call_expression(callee: Owned[IncanExpr], arguments: Owned[list[IncanExpr]]) -> Owned[IncanExpr]:
    """Construct a call using the current callee-span rule and preserving child spans."""
    ...
```


```python
"""Proposed vocabulary construction without serialization or compiler execution."""
from std.primitives.models.vocab import Span, IncanExpr
from std.primitives.vocab import path_expression, call_expression

def log_value(span: Span, value: Owned[IncanExpr]) -> Owned[IncanExpr]:
    # Caller supplies the actual invocation span and checked structured argument.
    callee = path_expression("logger.info", span)
    return call_expression(callee, [value])
```


The caller supplies the actual source span and structured argument. The helper builds a call expression, preserving existing span rules; it does not execute logging or teach the compiler a new effect.

Extracting these types must preserve canonical identities, wire schema and source locations. Existing derive-generated codecs or to_json methods cannot simply be deleted: move codec implementation above the pure models and prove the old public worker protocol still round-trips. Compiler admission, checking and macro execution remain compiler responsibilities.

### Implementation acceptance: one boundary at a time

| Group | Acceptance evidence before activation |
| --- | --- |
| Shared substrate | Same nominal identities through old/new imports and published facades; no reverse imports into resource families |
| Hash/encoding | Existing vectors and malformed-input behavior; no fs/io convenience dependency in byte algorithms |
| Environment | Missing/empty/error distinctions; old wrapper compatibility; values absent from diagnostic output |
| Files/mapping/locks/temp | Resource release, explicit cleanup errors, contention, retained mapping lifetime and rejected escaping borrows |
| Async | Real awaited cases, cancellation/drop cases, payload recovery, guard projection rejection and runtime admission |
| Vocabulary | No JSON/serde/worker-I/O dependency in pure contracts; identical schema round-trips and origin/span preservation |
| All families | Provider-only consumers retain ownership/source facts; no implicit repair clones; old facades still behave as documented |

These are staged design and test obligations, not an expansion of the current passing census. Local contract files remain outside the compiled package. Existing APIs can move first where behavior is unchanged; new borrowed projections and resource contracts activate only when their compiler stages and native proofs are ready.

### Source baseline used for this pass

The baseline is the integration tree at commit `dfd9108232aa13693f247b9b06edab8b64c170d1`, before the new implementation slices.

- Hash: `loaves/stdlib/data/src/hash/_core.incn`, `_streaming.incn`, `_hmac.incn`.
- Encoding: `loaves/stdlib/codecs/src/encoding/_shared.incn` and algorithm modules.
- Files/resources: `loaves/stdlib/system/src/fs/mapping.incn`, `fs/locking.incn`, `fs/metadata.incn`, `tempfile.incn`, `environ.incn`.
- Async: `loaves/stdlib/async/src/async/` and native channel/synchronization adapters.
- Vocab: `loaves/stdlib/vocab/src/vocab.incn` and `scoped.incn`.
