# RFC 168: `std.primitives`

- **Status:** Draft
- **Created:** 2026-10-31
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 056 (binary I/O)
    - RFC 064 (binary-text encoding)
    - RFC 065 (content hashing)
    - RFC 112 (crash-safe publication and file coordination)
    - RFC 114 (compiled providers and package features)
    - RFC 117 (Loaf project model)
    - RFC 124 (store unit identity and cross-plan artifact sharing)
    - RFC 164 (explicit ownership and borrowing contracts)
    - RFC 165 (typed embedded storage)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce `std.primitives`, an ordinary standard package for the low-level value and host facilities that both user programs and the platform storage substrate need. It depends on no other standard package. Existing standard modules retain their public APIs and delegate to the same primitive definitions, preserving type identity. Platform code may import `std.primitives` and no other standard package.

## Motivation

A reusable storage substrate needs paths, bounded byte cursors, content digests, locks and mapped immutable files. Giving each subsystem its own versions duplicates behavior and makes their values incompatible. Placing the common definitions beneath the higher-level standard modules lets the platform use them without importing those modules, while keeping the same supported facilities available to ordinary users.

## Goals

- Give each primitive one definition and one type identity.
- Preserve all existing standard import paths and observable behavior.
- Make low-level facilities a supported public surface, with explicit ownership, bounds and error contracts.
- Keep the package independent of all other standard packages.

## Non-Goals

- A database, query language, serialization format or build policy.
- New hashing, encoding, locking or filesystem semantics.
- Removing existing standard modules or requiring users to change imports.
- A guarantee of safe mappings while another writer mutates or truncates their backing inode.

## Guide-level explanation

Users may import primitive values directly:

```incan
from std.primitives import Path, BytesIO, Endian, sha256, hexlify
```

The familiar imports from `std.fs`, `std.io`, `std.hash` and `std.encoding.hex` remain valid. Values passed between the two surfaces have the same types; a path or mapped view does not need conversion. Higher-level standard modules remain the usual place to find their domain APIs.

## Reference-level explanation

### Inventory

Names below are proposed public names. Every old home must re-export the same declaration or wrap it without a second primitive implementation. Existing default values, error categories, byte representations and filesystem side effects must be preserved.

| Primitive | Old standard home | New name | Old home after the move | Why it is a primitive |
| --- | --- | --- | --- | --- |
| Path value and its existing methods | `std.fs.Path`, `std.fs.path.Path` | `std.primitives.Path` | Re-export | A common filesystem locator must have one type identity. |
| Open files and opening options | `std.fs.File`, `OpenFileMode`, `OpenOptions` | `std.primitives.File`, `OpenFileMode`, `OpenOptions` | Re-export | Path operations expose these types and bounded byte I/O. |
| Directory entries and filesystem metadata | `std.fs.DirEntry`, `PathStat`, `DiskUsage` | `std.primitives.DirEntry`, `PathStat`, `DiskUsage` | Re-export | Path operations return these values; they are host observations. |
| Read-only mapped file | `std.fs.MappedFile`, `std.fs.mapping.MappedFile` | `std.primitives.MappedFile` | Re-export | Retains an immutable inode's mapping. |
| Anchored mapped byte range and scalar reads | `std.fs.MappedView`, `std.fs.mapping.MappedView` | `std.primitives.MappedView` | Re-export | A bounded owner-anchored view avoids payload copies. |
| Open a mapping | `std.fs.mapping.map_read` | `std.primitives.map_read` | Re-export; `Path.map_read` retains its method | The mapping operation is shared with the platform. |
| Advisory lock guard | `std.fs.FileLock`, `std.fs.locking.FileLock` | `std.primitives.FileLock` | Re-export | The descriptor's lifetime owns a cooperative host lock. |
| Blocking and nonblocking shared/exclusive locks | `std.fs.locking.shared`, `exclusive`, `try_shared`, `try_exclusive` | `std.primitives.locking` with the same function names | Re-export; Path's lock methods retain their contracts | Publication needs host coordination without consumer policy. |
| Binary I/O error | `std.io.IoError`, `std.fs.IoError` | `std.primitives.IoError` | Re-export | Stream and filesystem boundaries share one error value. |
| Byte order | `std.io.Endian` | `std.primitives.Endian` | Re-export | Exact-width binary values need an explicit byte order. |
| Memory byte cursor and its concrete type | `std.io.BytesIO`, `_BytesIO` | `std.primitives.BytesIO`, `_BytesIO` | Re-export | Bounded reads, seeking and numeric I/O are format-independent. |
| Binary reader/writer capabilities and chunk stream | `std.io.BinaryRead`, `BinaryReader`, `BinaryWrite`, `ReaderChunks` | Same names in `std.primitives` | Re-export | File and cursor contracts expose these capabilities. |
| SHA-256 one-shot and incremental hashing | `std.hash.sha256`, `Sha256Hasher` | `std.primitives.sha256`, `Sha256Hasher` | Re-export; other hash dispatch delegates to them | Content identity and integrity need a common digest implementation. |
| SHA-256 path/file digest | `std.hash.file_digest` with algorithm `"sha256"` | `std.primitives.file_digest` | Wrap; SHA-256 delegates, other algorithms remain in `std.hash` | Bounded streaming content hashing needs neither a format nor domain policy. |
| Lowercase hexadecimal encoding | `std.encoding.hex.hexlify` | `std.primitives.hexlify` | Wrap or re-export; encoding helpers delegate to the same conversion | Content names need a deterministic byte-to-text representation. |
| Optional and defaulted environment reads | `std.environ.get_optional`, `get_or` | `std.primitives.environ.get_optional`, `get_or` | Re-export or wrap | Read-only process configuration does not require a higher-level module. |
| Scoped temporary directory | `std.tempfile.TemporaryDirectory` | `std.primitives.TemporaryDirectory` | Re-export | Temporary staging needs one host-backed owner and cleanup contract. |

The inventory includes the types reached by the existing `Path` and byte-cursor APIs. It does not imply that JSON, every hash algorithm, every encoding or every temporary-file helper belongs in the package. A shared helper required by those preserved contracts must have one home; its public status must be explicit.

### Dependencies

`std.primitives` must not import or depend on any other standard package, including through a private helper or a trait bound. Platform code must import no standard package except `std.primitives`. Higher-level standard packages may depend on primitive declarations through the ambient standard-library mechanism; users must not declare standard packages in their manifests. Native facilities used by the primitives remain ordinary native dependencies; package placement must not create another implementation of their algorithms or OS behavior.

### Compatibility and stability

`std.primitives` is a supported user-facing surface. Its low-level status must not be used to waive compatibility expectations. New and old import paths must name the same nominal types. A re-export must preserve constructors, methods and trait capabilities; a wrapper must preserve defaults, failures and observable results. Changes to names, ownership requirements, byte formats or errors must follow the standard library's versioned compatibility policy.

Mapped views must retain their owner, reject out-of-range reads and expose copying explicitly. Their backing inode must remain unmodified and untruncated while any mapping or view lives. File locks remain advisory and cooperative. File digest helpers must bound each read and must not impose a total-file-size ceiling. Environment reads must not include observed values in errors. Temporary directories must preserve their existing ownership and cleanup behavior.

## Design details

The dependency rule applies to the complete transitive Incan surface, including error and iteration traits. Existing public capabilities cannot disappear merely to remove a dependency. The final public inventory must settle those shared traits before adoption.

JSON remains a format in `std.json`. A sealed-set manifest using JSON belongs above the primitive storage substrate; moving that format is not required to give the substrate its own home.

## Alternatives considered

- Keep the platform dependent on higher-level standard modules: obscures the intended dependency direction and makes format or domain dependencies easy to introduce.
- Duplicate primitive types in the platform: creates conversions and multiple implementations of the same facility.
- Replace existing user APIs with native interop types: breaks compatibility and removes Incan's shared contracts.

## Drawbacks

The supported public surface grows. Keeping old import paths requires careful preservation of nominal identities and trait contracts. A path's existing convenience methods expose more supporting capabilities than a locator alone, so the boundary needs an explicit inventory.

## Layers affected

- **Standard library:** shared primitive declarations, compatibility exports and wrappers, public reference contracts.
- **Platform:** the sole allowed standard-package dependency and reusable storage interfaces.
- **Compiler and tooling:** ordinary standard-package namespace ownership, import lookup and completion.

## Unresolved questions

- Should primitive exports be flat, grouped by facility, or both? Which supporting declarations are public?
- What exact signature and typed error should the SHA-256-only file digest expose while the existing multi-algorithm helper keeps its contract?
- Which error and iterator capabilities move with the primitives so the package has no dependency on another standard package without dropping existing trait adoptions?
- Should the existing `_BytesIO` concrete type gain a public name while its old spelling remains compatible?
- Does the existing complete Path API belong in the primitive surface, or should extension methods preserve its higher-level operations without changing type identity?
- Which guarantees and failure categories should be portable beyond the currently supported Unix-like hosts?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
