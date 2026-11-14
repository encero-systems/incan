# std.db reference

`std.db` is embedded, transactional storage over the platform's store substrate. A store holds named views; a snapshot reads one immutable selection of a view; a transaction stages changes to one view and publishes them together or not at all. Records are addressed by a `RecordKey` and hold encoded values as `bytes`.

```incan
from std import db
from std.db import DbError, DbErrorKind, RecordKey, RecordState
```

Every fallible operation returns `Result[..., DbError]`. A refusal is identified by `DbError.kind` and `DbError.operation`; the detail text is not part of the contract.

## Opening and closing

| API | Returns | Description |
| --- | --- | --- |
| `db.memory()` | `Result[Store, DbError]` | Open an isolated memory store. Its contents stay in process memory, never reach a file, and are visible to no other store. |
| `db.open(path: Path)` | `Result[Store, DbError]` | Open the disk store at `path`, creating it when `path` is missing or an empty directory. |

`db.open` refuses a path that holds anything but a store:

| Refusal | `kind` | `operation` |
| --- | --- | --- |
| `path` is a file | `Scope` | `Open` |
| `path` is a directory with entries and no store identity | `Scope` | `Open` |
| The store is written in a format `std.db` does not read | `Unsupported` | `Open` |
| The placement cannot be read or written | `Io` | `Open` |

A path refused with `Scope` is left unchanged.

## Store

| API | Returns | Description |
| --- | --- | --- |
| `store.placement()` | `Placement` | Where the store keeps its contents. |
| `store.identity()` | `str` | The store's persistent identity: 32 lowercase hexadecimal characters. Reopening a disk store returns the same identity. |
| `store.is_open()` | `bool` | Whether the store accepts new work. |
| `store.close()` | `Result[None, DbError]` | Stop new work through the store and every view taken from it. Closing a closed store returns `Ok(None)`. |
| `store.view(name: str)` | `Result[View, DbError]` | The view `name`, created empty in one publication when the store has no view of that name. |
| `store.views()` | `Result[list[str], DbError]` | The names of the store's views, in name order. |
| `store.fork(snapshot: Snapshot, name: str)` | `Result[View, DbError]` | A new view `name` whose selection is exactly `snapshot`'s selection. |

A forked view shares its records with its source; later changes to either view are not visible in the other. Two concurrent `store.view(name)` calls for one new name return the same view.

| Refusal | `kind` | `operation` |
| --- | --- | --- |
| The store is closed | `Closed` | `View` or `Fork` |
| `name` breaks the [name rule](#names) | `InvalidName` | `View` or `Fork` |
| `store.fork` names an existing view | `Exists` | `Fork` |
| `store.fork` receives a snapshot of another store | `Scope` | `Fork` |
| `store.fork` receives a released snapshot | `Closed` | `Fork` |

## Placement

| Value | Spelling | Meaning |
| --- | --- | --- |
| `Placement.Disk` | `"disk"` | Records live in a directory on disk and survive the process. |
| `Placement.Memory` | `"memory"` | Records live in process memory and never reach a file. |

## View

| API | Returns | Description |
| --- | --- | --- |
| `view.name` | `str` | The view's name. |
| `view.snapshot()` | `Result[Snapshot, DbError]` | The view's current selection, leased. |
| `view.begin()` | `Result[Transaction, DbError]` | An open transaction staged against the view's current selection. |

Both operations are refused with `Closed` once the store is closed, with `operation` `Snapshot` or `Begin`.

## Snapshot

A snapshot is immutable: publications after it is taken are not visible through it. It holds a lease on its selection, and reads through it succeed after its store is closed, until it is released.

| API | Returns | Description |
| --- | --- | --- |
| `snapshot.store()` | `str` | The identity of the store it was taken from. |
| `snapshot.view()` | `str` | The name of the view it selects. |
| `snapshot.revision()` | `str` | The view revision it selects. Equal revisions select equal contents; an empty view's revision is `""`. |
| `snapshot.generation()` | `int` | The store generation it was taken from. |
| `snapshot.is_released()` | `bool` | Whether `release()` was called. |
| `snapshot.release()` | `None` | Give up the lease. Releasing twice has no further effect. |
| `snapshot.read(key: RecordKey)` | `Result[RecordState[bytes], DbError]` | What the snapshot holds under `key`, as an owned copy. |

| Refusal | `kind` | `operation` |
| --- | --- | --- |
| The snapshot is released | `Closed` | `Read` |
| `key` breaks the [name rule](#names) | `InvalidName` | `Read` |
| The stored record is damaged | `Corruption` | `Read` |

A damaged record is never read as `Absent`, and never as an older value.

## Records

| API | Description |
| --- | --- |
| `RecordKey(family: str, id: str)` | The address of one logical record: its family's storage name and its identity within that family. Keys with different families address different records. |
| `key.store_key()` | `str`: the store key holding the record. |
| `RecordState.Present(RecordVersion[T])` | A present value. |
| `RecordState.Deleted(Tombstone)` | A deletion. |
| `RecordState.Absent` | No value and no deletion under the key. |
| `RecordVersion[T](value: T, revision: Option[str])` | A value and the revision holding it. |
| `Tombstone(revision: Option[str])` | A deletion and the revision recording it. |

A committed value or tombstone has `Some(revision)`. A value or tombstone staged in a transaction and read through that transaction has `None`.

## Transaction

A transaction stages changes to one view against the snapshot it began from. Its reads see its own staged changes first, then that snapshot. No snapshot sees a staged change before `commit()` publishes it.

| API | Returns | Description |
| --- | --- | --- |
| `transaction.view()` | `str` | The view it publishes to. |
| `transaction.base_revision()` | `str` | The view revision it began from. |
| `transaction.state()` | `TransactionState` | Where the transaction is in its life. |
| `transaction.read(key: RecordKey)` | `Result[RecordState[bytes], DbError]` | Its own staged value or tombstone under `key`, otherwise its base snapshot's state. |
| `transaction.write(key: RecordKey, value: bytes)` | `Result[None, DbError]` | Stage `value` under `key`, replacing anything staged there. |
| `transaction.delete(key: RecordKey)` | `Result[None, DbError]` | Stage a tombstone under `key`, replacing anything staged there. |
| `transaction.abort()` | `Result[None, DbError]` | Discard every staged change. The view is unchanged. |
| `transaction.commit()` | `Result[CommitReceipt, DbError]` | Publish every staged change in one generation, or none of them. |

`commit()` publishes only while the view still has `base_revision()`. A refused `read`, `write` or `delete` leaves the transaction's state unchanged.

| Refusal | `kind` | `operation` | State after |
| --- | --- | --- | --- |
| Any operation on a committed or aborted transaction | `Closed` | the operation | unchanged |
| `read`, `write`, `delete` or `commit` on a failed transaction | `Closed` | the operation | `Failed` |
| `key` breaks the [name rule](#names) | `InvalidName` | the operation | `Open` |
| `commit()` after the store is closed | `Closed` | `Commit` | `Failed` |
| `commit()` after another publication moved the view | `Conflict` | `Commit` | `Failed` |
| `commit()` fails while the view still has `base_revision()` | the failure's kind | `Commit` | `Failed` |
| `commit()` fails after the view moved from `base_revision()` | `Indeterminate` | `Commit` | `Failed` |

`Conflict` and every failure reported with the view still at `base_revision()` publish nothing. `Indeterminate` means the commit may or may not have been published. `abort()` ends an open or failed transaction.

## TransactionState

| Value | Spelling | Meaning |
| --- | --- | --- |
| `TransactionState.Open` | `"open"` | Reads, writes, deletes, commit and abort are accepted. |
| `TransactionState.Committed` | `"committed"` | `commit()` published it. |
| `TransactionState.Aborted` | `"aborted"` | `abort()` discarded it. |
| `TransactionState.Failed` | `"failed"` | `commit()` was refused or failed; only `abort()` is accepted. |

## CommitReceipt

| API | Returns | Description |
| --- | --- | --- |
| `receipt.transaction` | `str` | The committed transaction's identity. Two commits have different identities. |
| `receipt.view` | `str` | The view it published to. |
| `receipt.prior` | `str` | The view revision the transaction began from. |
| `receipt.successor` | `str` | The view revision after the commit. |
| `receipt.durability` | `Durability` | The durability the commit achieved. |
| `receipt.changed()` | `bool` | Whether `successor` differs from `prior`. A commit with no changes, or with only values already present, has `successor == prior`. |
| `receipt.snapshot()` | `Result[Snapshot, DbError]` | A snapshot of exactly the selection the commit published. |

## Durability

| Value | Spelling | Meaning |
| --- | --- | --- |
| `Durability.Memory` | `"memory"` | A commit to a memory store. It does not survive the process. |
| `Durability.Unqualified` | `"unqualified"` | A commit to a disk store, written and synchronized in publication order. Survival of a crash or power loss is not claimed. |

## DbError

| API | Returns | Description |
| --- | --- | --- |
| `error.kind` | `DbErrorKind` | What the operation met. |
| `error.operation` | `DbOperation` | The operation that failed. |
| `error.object` | `str` | The store object, view, name or key the failure concerns. |
| `error.detail` | `str` | What was found. |
| `error.message()` | `str` | `"<operation> <kind> in <object>: <detail>"`, using the enum spellings. |
| `DbError.refused(kind: DbErrorKind, operation: DbOperation, object: str, detail: str)` | `DbError` | Construct an error. |

| `DbErrorKind` | Spelling | Meaning |
| --- | --- | --- |
| `Corruption` | `"corruption"` | Stored bytes or metadata are damaged. |
| `Conflict` | `"conflict"` | The view moved on since the transaction began. |
| `Busy` | `"busy"` | Coordination the operation needs is held elsewhere. |
| `UnavailableSnapshot` | `"unavailable-snapshot"` | The selected revision is not retained. |
| `Unsupported` | `"unsupported"` | The store's format or a required capability is not supported, or the store reported a failure outside this set. |
| `Io` | `"io"` | Reading or writing the store's placement failed, and nothing was published. |
| `Indeterminate` | `"indeterminate"` | A commit failed after its publication may have become visible. |
| `Closed` | `"closed"` | A closed store, a released snapshot, or an ended transaction. |
| `InvalidName` | `"invalid-name"` | A view name, family name or record identity breaks the name rule. |
| `Exists` | `"exists"` | The operation must create a name that already exists. |
| `Scope` | `"scope"` | A handle of another store, or a path that is not a store. |

`DbOperation` values and spellings: `Open` (`"open"`), `Close` (`"close"`), `View` (`"view"`), `Fork` (`"fork"`), `Snapshot` (`"snapshot"`), `Read` (`"read"`), `Begin` (`"begin"`), `Write` (`"write"`), `Delete` (`"delete"`), `Commit` (`"commit"`), `Abort` (`"abort"`).

## Names

A view name, a family name and a record identity are each nonempty and hold no whitespace or control character. A name that breaks this rule is refused with `InvalidName` before anything is read or published.

## See also

- [Store records with std.db](../../how-to/embedded_storage.md)
