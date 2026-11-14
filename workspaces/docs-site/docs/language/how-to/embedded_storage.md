# Store records with std.db

Use `std.db` when a program needs records that survive a restart, or several changes that must land together. This guide opens a store, writes and reads one record, handles a conflicting writer, and starts a preview view. The [std.db reference](../reference/stdlib/db.md) lists every operation and refusal.

## Open a store and a view

Open a disk store with `db.open`, or an isolated memory store with `db.memory()` when nothing has to outlive the process. Select the view your program publishes to with `store.view`; the first call creates it.

```incan
from std import db
from std.db import DbError, RecordKey, RecordState
from std.fs import Path


def open_site() -> Result[db.View, DbError]:
    store = db.open(Path("site.db"))?
    return store.view("published")
```

`db.open` refuses a file, or a directory holding other files, instead of turning it into a store, so point it at a new path or an existing store.

## Write records in a transaction

Begin a transaction on the view, stage your changes, and commit them. Nothing you stage is visible to any snapshot until `commit()` succeeds, and then all of it is.

```incan
def publish_guide(published: db.View) -> Result[None, DbError]:
    mut transaction = published.begin()?
    transaction.write(RecordKey(family="pages", id="guide"), b"Guide")?
    transaction.delete(RecordKey(family="pages", id="draft"))?
    receipt = transaction.commit()?
    println(f"published {receipt.successor} with {receipt.durability.value()} durability")
    return Ok(None)
```

Check `receipt.durability` before telling a user that data is safe: a memory store reports `memory`, and a disk store reports `unqualified`, which claims no survival of a crash or power loss.

## Read a record

Take a snapshot and match on the state it returns. A snapshot keeps its selection while other writers publish, so take a new one when you want their changes.

```incan
def guide_text(published: db.View) -> Result[Option[bytes], DbError]:
    snapshot = published.snapshot()?
    match snapshot.read(RecordKey(family="pages", id="guide"))?:
        RecordState.Present(version) => return Ok(Some(version.value))
        RecordState.Deleted(_) => return Ok(None)
        RecordState.Absent => return Ok(None)
```

A damaged record is returned as a `Corruption` error, not as `Absent`, so let it propagate rather than treating it as missing.

## Handle a conflicting writer

A commit is refused with `DbErrorKind.Conflict` when another writer published to the view after your transaction began. Nothing of yours was published: begin a fresh transaction, read again, and decide again, rather than replaying the old changes blindly.

```incan
from std.db import DbErrorKind


def publish_once_more(published: db.View) -> Result[None, DbError]:
    match publish_guide(published):
        Ok(_) => return Ok(None)
        Err(error) =>
            if error.kind == DbErrorKind.Conflict:
                return publish_guide(published)
            return Err(error)
```

An `Indeterminate` error means the commit may already be visible. Read the view again before deciding whether to repeat the change.

## Start a preview view

Fork a snapshot into a new view to edit a copy without touching the published one. The fork starts at exactly that snapshot's selection and copies no records.

```incan
def open_preview(store: db.Store, published: db.View) -> Result[db.View, DbError]:
    base = published.snapshot()?
    return store.fork(base, "preview")
```

`store.fork` refuses a name that already exists, so pick a new name for each preview or reuse the existing view with `store.view`.

## Close the store

Call `store.close()` when your program stops using the store. Snapshots you still hold stay readable until you call `release()` on them.
