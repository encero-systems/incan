# RFC 112: Crash-safe local publication and file coordination

- **Status:** In Progress
- **Created:** 2026-07-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 055 (`std.fs` path-centric filesystem APIs with chunked file I/O)
    - RFC 077 (workspace and multi-package projects)
- **Issue:** #829
- **RFC PR:** —
- **Written against:** v0.4
- **Shipped in:** —

## Summary

This RFC defines a small `std.fs` contract for safely publishing locally generated files and coordinating concurrent publishers. It makes `Path.replace(...)` a same-filesystem atomic replacement operation rather than a delete-then-move helper, adds explicit parent-directory synchronization for callers that need crash durability, and defines shared and exclusive advisory file locks. The contract deliberately distinguishes atomic visibility, durability after a crash, and mutual exclusion: no one operation is permitted to imply guarantees it cannot provide.

## Core model

1. **Publication has three separate properties:** replacement atomicity controls what concurrent readers observe, synchronization requests durable persistence, and locks coordinate cooperating writers. Applications must select each property explicitly.
2. **A replacement is never delete-first:** `Path.replace(...)` must either make the new entry visible in place of the old entry or report an error; it must not unlink the destination as a preparatory step.
3. **The containing directory owns publication durability:** synchronizing the replacement file alone is insufficient to request persistence of a renamed directory entry, so directory synchronization is an explicit operation.
4. **Locks are advisory and scoped:** shared and exclusive guards coordinate processes that use this contract; they do not prevent an unrelated process from modifying the path.
5. **Portability is truthful:** the initial supported hosts are Linux and macOS. A host that cannot honor a requested guarantee must return a typed filesystem error rather than silently weakening the operation.

## Motivation

Generated lockfiles, cached metadata, local library artifacts, and other derived project state are normally read much more often than they are written. A publisher that first removes an old file and then moves a new file into place creates an avoidable failure window: a reader can observe no file at all, and a crash can turn a recoverable update into data loss. A copy-delete fallback has the same problem and is not an atomic publication primitive.

Even a successful rename does not answer every reliability question. A process may need to request that written contents and the directory entry survive power loss, while two concurrently running commands need a clear rule for who may publish next. Existing general filesystem helpers are useful for ordinary moves and copies, but they should not be presented as a durable, coordinated publication protocol when they cannot make that promise.

## Goals

- Define same-filesystem atomic replacement for an already-staged file using the existing `Path.replace(...)` spelling.
- Define an explicit parent-directory synchronization operation for callers that require a crash-durability request after replacement.
- Define blocking and non-blocking shared and exclusive advisory lock acquisition for local paths.
- Provide one documented, testable publication recipe suitable for lockfiles and locally persisted generated artifacts.
- Preserve ordinary copy and cross-filesystem move behavior as separate operations rather than disguising either as atomic replacement.
- Require macOS and Linux integration tests for visibility, contention, cleanup, and failure behavior.

## Non-Goals

- Atomic publication of directory trees, multiple files, or an entire workspace transaction.
- A distributed lock service, lease protocol, lock recovery daemon, or network filesystem consistency model.
- Mandatory durability for every `write`, `move`, or `replace` operation.
- Locking non-cooperating processes, preventing administrative changes, or providing authorization control.
- A new context-manager language feature; lock guards follow existing scope-based resource behavior and may later compose with RFC 094.
- Defining Windows support in the initial release.

## Guide-level explanation

An application writes a complete replacement beside the current file, asks the file handle to persist its contents when crash durability matters, atomically replaces the destination, and then synchronizes the destination directory. The replacement is intentionally a separate final step, so readers see either the prior complete file or the new complete file, never the publisher's partial write.

```incan
from std.fs import Path

target = Path("incan.lock")
staged = Path(".incan.lock.next")

staged.write_bytes(rendered_lockfile)?
staged_file = staged.open("rb")?
staged_file.sync()?

staged.replace(target)?
target.parent.sync_directory()?
```

When more than one command can publish the same logical state, cooperating publishers acquire an exclusive lock before preparing and replacing it. Readers that need a stable multi-step view may acquire a shared lock. The guard is released when it leaves scope; programs must keep the guard live for the complete critical section.

```incan
from std.fs import Path

target = Path("incan.lock")
guard = target.lock_exclusive()?

# Read the current state, produce a sibling staged file, replace it, and sync
# the parent directory while `guard` remains live.
publish_updated_lockfile(target)?
```

The file handle and guard follow ordinary scope-based resource lifetime. Applications must keep a guard live for the complete critical section and must not hold a blocking file lock across unrelated work, interactive waits, or asynchronous suspension.

## Reference-level explanation

### Atomic replacement

- `Path.replace(target: Path | str) -> Result[Path, IoError]` must publish the receiver at `target` by a single same-filesystem replacement operation.
- `target` may be absent; a successful replacement then publishes a new entry. A symbolic-link target is replaced as an entry and must not be followed.
- On success, a path lookup of `target` must observe the complete prior entry or the complete replacement entry; it must not observe an intentionally unlinked destination caused by this operation.
- The receiver and target must be on the same filesystem. If that condition cannot be established or the host reports a cross-device replacement, the method must return `IoError(kind="cross_device")` without deleting the target and without falling back to copy-delete behavior.
- The initial contract applies only to non-directory replacement entries. Replacing a directory, replacing an incompatible destination type, or attempting to replace a path that the host cannot atomically replace must return an error without a delete-first fallback.
- On success, the receiver no longer names the staged entry and the method returns `target`. On failure, the implementation must preserve the prior target entry whenever the host primitive makes that possible; it must never explicitly unlink it to retry the operation.
- `Path.move(...)` and `Path.rename(...)` retain their ordinary transport semantics. They must not be documented as equivalent to publication because cross-filesystem movement may require copy-delete behavior.

### Durability requests

- `Path.sync_directory() -> Result[None, IoError]` must request synchronization of the directory represented by the path. It must fail with `IoError(kind="invalid_input")` when the receiver is not a directory.
- A publisher that requires the strongest local crash-durability request must write and synchronize the staged file, call `replace`, then call `target.parent.sync_directory()` in that order.
- A successful `replace` guarantees atomic visibility only. It must not claim that replacement contents or the directory entry have survived a power loss unless the caller has requested the relevant synchronization operations and the host reports success.
- A successful synchronization is a request to the host filesystem, not a guarantee against defective hardware, kernel bugs, remote filesystem semantics, or power-loss behavior outside the host's documented durability model.

### Advisory file locks

- `Path.lock_shared() -> Result[FileLock, IoError]` must acquire a blocking shared advisory lock associated with the path.
- `Path.lock_exclusive() -> Result[FileLock, IoError]` must acquire a blocking exclusive advisory lock associated with the path.
- `Path.try_lock_shared() -> Result[Option[FileLock], IoError]` and `Path.try_lock_exclusive() -> Result[Option[FileLock], IoError]` must return `Ok(None)` when a conflicting lock would block. A contention result is not an error.
- A `FileLock` must release its host lock when the guard is dropped and may expose an idempotent explicit release operation. Releasing an already released guard must not unlock a lock acquired by another guard.
- Shared locks may coexist with other shared locks; an exclusive lock conflicts with every shared or exclusive lock using the same lock identity. The initial contract does not support lock upgrade, downgrade, reentrancy, or fairness guarantees.
- Locks are advisory. They coordinate only processes that acquire the same lock identity through this API; they do not make path mutation impossible for other software.
- A lock implementation must not follow a symbolic link to select a lock identity. The documented identity is a dedicated sibling lock entry derived from the protected path, so replacing the protected file does not invalidate a still-held lock.
- Blocking acquisition must not be held across asynchronous suspension. The API remains synchronous in this release.

### Errors and security

- Errors must use the existing `IoError` shape and identify the operation and affected path without exposing file contents or unrelated filesystem data.
- Unsupported host facilities must return `IoError(kind="unsupported")`; they must not silently use an in-process mutex, an unlocked operation, or a delete-first replacement.
- Permission, read-only filesystem, malformed path, busy path, cross-device, and synchronization failures must remain distinguishable where the host exposes that distinction.
- Library code that publishes a file must use a unique sibling staging name created with exclusive creation. Predictable shared temporary names must not be used for untrusted directories.

## Design details

### API and semantics

This RFC changes the safety contract of the existing `Path.replace(...)` surface. The current delete-first behavior is incompatible with its role as a publication primitive and must be removed. This is a compatibility tightening: callers that relied on recursive directory removal or cross-device copy-delete must select an explicit directory operation or `move(...)` instead.

Directory synchronization belongs on `Path` because it names the directory entry whose persistence is being requested. File content synchronization remains on `File`, as defined by RFC 055. The resulting sequence makes the boundary visible in source: staged file content, namespace replacement, then parent-directory durability.

The lock identity is deliberately separate from the published file. Renaming a staged artifact over the target changes the target inode or equivalent host object, so locking that mutable object directly would create a coordination gap. A stable sibling lock entry gives cooperating publishers one identity before, during, and after replacement.

### Interaction with existing features

- **Error propagation:** all publication and lock operations return `Result`; `?` remains the ordinary way to stop a failed publication before a later step claims success.
- **Temporary files:** temporary-file helpers may provide staging locations, but a staged publisher must ensure the final replacement stays on the target filesystem.
- **Workspaces:** workspace lock publication uses the recipe in this RFC. Workspace selection and dependency semantics remain owned by RFC 077.
- **Generated artifacts:** compiled local artifacts may use the same recipe, but this RFC does not define artifact format, discovery, or validity rules.
- **Async:** this RFC supplies no async locking API and does not permit a lock guard to span suspension.

### Compatibility and migration

Existing code using `Path.replace(...)` for file publication gains a stronger guarantee and needs no source rewrite. Code that used `replace` to remove destination directories recursively or to move files between filesystems must migrate to explicit directory removal or `move(...)`, respectively. Reference documentation must call out that `replace` is a same-filesystem file-publication operation and that it does not imply durability without synchronization.

## Alternatives considered

### Add a new `atomic_replace` method and leave `replace` unsafe

This would preserve every historical behavior, but it leaves the unsurprising `replace` spelling as a footgun and creates two nearly identical operations with materially different failure semantics. Tightening `replace` gives one clear publication primitive; explicit move and directory APIs retain the broader behaviors.

### Copy the staged file over the destination

Copying cannot provide atomic visibility for the destination contents and may expose a truncated or mixed file to readers. It is suitable for ordinary transfer, not publication.

### Synchronize every write and replacement automatically

Automatic synchronization would hide potentially substantial cost, still could not establish a multi-file transaction, and would make callers unable to distinguish atomic visibility from an explicit durability request. The ordered recipe remains visible instead.

### Use an in-process mutex

An in-process mutex does not coordinate separate commands and does not survive process boundaries. It may remain an internal optimization, but it cannot satisfy this contract.

### Lock the target file itself

Replacing the target changes the object that a file-backed lock would protect. A stable sibling lock entry keeps the coordination identity independent of the published artifact.

## Drawbacks

The contract adds concepts that small scripts may not otherwise need: staging, file synchronization, parent-directory synchronization, and advisory lock scope. Correct durability testing is host-sensitive and cannot prove behavior on every filesystem or hardware configuration. Tightening `replace` also removes convenience behavior that some callers may have used for directory replacement or cross-device movement, but retaining that behavior would make the publication guarantee misleading.

## Implementation architecture

Implementations should map the public contract to the host's native replace, synchronization, and advisory-lock facilities, preserving structured error categories at the `std.fs` boundary. Test fixtures should run independent writer and reader processes, inject failures between each publication step, and assert that a reader observes only the old or new complete payload. This architecture is non-normative; the requirements above, rather than a specific host API or runtime library, define conformance.

## Layers affected

- **Stdlib / Runtime (`incan_stdlib`)**: must provide safe same-filesystem replacement, directory synchronization, and advisory lock guards with the specified result and lifetime behavior.
- **Rust interop boundary**: must preserve host error categories and must not translate unsupported operations into weaker behavior.
- **CLI / tooling**: workspace and artifact publishers must use the documented ordered recipe and report publication failures without claiming success.
- **Documentation**: filesystem reference material must distinguish replacement atomicity, durability requests, and advisory coordination; migration guidance must cover the tightened `replace` semantics.
- **Tests**: macOS and Linux integration tests must exercise concurrent readers and writers, lock contention, cross-device rejection where available, failure preservation, and staged-file cleanup.

## Implementation Plan

### Phase 1: Public filesystem contract

- Tighten `Path.replace(...)` to its same-filesystem atomic publication contract and document the compatibility boundary.
- Add directory synchronization and advisory-lock declarations to `std.fs` with typed error behavior.

### Phase 2: Native runtime support

- Connect the public API to native replacement, directory synchronization, and shared/exclusive advisory-lock facilities on supported hosts.
- Preserve the documented error categories and guard lifetime behavior at the standard-library boundary.

### Phase 3: Conformance tests and documentation

- Add replacement visibility, failure-preservation, directory-synchronization, and multi-process contention tests for Linux and macOS.
- Publish the crash-safe recipe and migration guidance in the filesystem reference and release notes.

## Progress Checklist

### Spec / design

- [x] Define separate atomicity, durability, and coordination guarantees in RFC 112.
- [x] Settle the existing `Path.replace(...)` compatibility decision.

### Stdlib / Runtime

- [ ] Provide the public directory-synchronization surface.
- [ ] Provide shared, exclusive, and non-blocking advisory file-lock guards.
- [ ] Implement non-destructive same-filesystem replacement.
- [ ] Preserve typed unsupported and cross-device failures without destructive fallback.

### Tests

- [ ] Cover complete old-or-new reader visibility during replacement.
- [ ] Cover target preservation when replacement fails.
- [ ] Cover directory synchronization errors and supported-host behavior.
- [ ] Cover multi-process shared/exclusive lock contention on Linux and macOS.

### Docs

- [ ] Update the user-facing `std.fs` reference with the publication recipe and migration guidance.
- [ ] Add a 0.5 release-note entry.

## Design Decisions

- The initial lock identity is derived only from the protected path. Applications that need one lock for multiple related paths must use one explicit coordination path; configurable lock identities remain a possible follow-up extension.
