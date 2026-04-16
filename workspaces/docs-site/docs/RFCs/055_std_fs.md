# RFC 055: `std.fs` — pathlib-shaped filesystem APIs with chunked file I/O

- **Status:** Planned
- **Created:** 2026-04-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language builtins, including whole-file helpers)
    - RFC 005 (Rust interop)
    - RFC 010 (temporary filesystem objects)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 056 (`std.io` in-memory byte cursors)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/286
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces `std.fs` as Incan's path-centric filesystem module. The compatibility target for the core path surface is CPython 3.14 `pathlib.Path`, while Incan also exposes a small set of explicit filesystem extensions inspired by Rust's `std::fs` where `pathlib` is intentionally incomplete. The module should be broad enough that users can stay inside `std.fs.Path` for ordinary filesystem work, including copy and move operations, without needing a separate `os`- or `shutil`-shaped layer. The initial contract is not limited to whole-file helpers: `std.fs` must also provide chunked binary file I/O through `Path.open(...)` and a `File` surface with bounded reads, writes, seeking, and durability operations so large files do not require loading the entire payload into memory.

## Motivation

Incan already exposes minimal whole-file builtins for the common text case, but that is not a complete filesystem story. Programs need ordinary path operations, metadata, directory lifecycle operations, and binary-safe reads and writes. They also need a truthful large-file path: authors must be able to open a file and process it incrementally in chunks rather than pretending `read_bytes()` is sufficient for every workload. Today, authors fall through to `rust::std::fs` and `rust::std::io`, which works but fragments documentation, discoverability, and examples. A namespaced stdlib surface matches RFC 022's direction and gives the project one place to define contracts for path behavior, binary I/O, durability, and errors while letting the runtime stay Rust-native underneath.

## Goals

- Provide `std.fs.Path` as Incan's primary filesystem entry point, with method names and grouping that should match CPython 3.14 `pathlib.Path` wherever semantics and language grammar allow.
- Make large files chunkable from day one: the `std.fs` contract must include `open(...)`, bounded `read(size)`, exact `read_exact(size)`, `write(...)`, `tell()`, and `seek(...)` on a file type so streaming workloads do not depend on `rust::`.
- Keep the API path-centric for ordinary tasks: tutorials should center `Path`, not ad hoc compiler builtins or direct `rust::std::fs` usage.
- Expose a small set of explicit Incan extensions where `pathlib` is not enough for everyday work, including honest existence checks, recursive delete, path-centric copy and move operations, structured open flags, and durability primitives.
- Keep Rust interop (RFC 005) as the escape hatch for advanced or host-specific behavior such as memory mapping, ACL-specific operations, or exotic platform knobs.

## Non-Goals

- Defining async filesystem APIs in this RFC.
- Delivering every committed `std.fs` capability in a single PR or release. The spec is end-to-end; implementation may still land in phases.
- Standardizing in-memory byte cursors here; that belongs to RFC 056 (`std.io`).
- Mirroring Rust's `std::fs` names one-to-one; Rust remains the implementation substrate, not the tutorial vocabulary.
- Introducing a parallel `std.os` or `std.shutil` module for common path-owned chores unless a later RFC shows that `std.fs` is the wrong home.
- Importing every newer `pathlib` addition mechanically. CPython 3.14 remains the baseline reference, but Incan should still choose a coherent path-centric surface rather than mirroring every adjacent Python helper without judgment.

## Guide-level explanation

Authors work with `std.fs.Path` the way they would with `pathlib.Path`: they join components, inspect lexical parts, create directories, and read or write files using path methods.

```incan
from std.fs import Path

model = Path("model.bin")
data = model.read_bytes()?

out = Path("out") / "copy.bin"
out.write_bytes(data)?

cfg_dir = Path("config")
if not cfg_dir.exists():
    cfg_dir.mkdir(parents=True)?
```

When the file is too large to load all at once, authors open it and process bounded chunks.

```incan
from std.fs import Path

fh = Path("video.bin").open("rb")?
header = fh.read_exact(16)?
chunk = fh.read(8192)?
offset = fh.tell()?
```

The mental model is simple: `Path` owns path-based operations; `File` owns open-file streaming; whole-file helpers are convenience operations, not the only supported route.

## Reference-level explanation

### Compatibility target and extensions

- The standard library must expose `std.fs` for path-based filesystem work.
- `std.fs.Path` should follow CPython 3.14 `pathlib.Path` for method spelling and behavior when that surface already exists there.
- When Incan exposes capabilities that are outside `pathlib` proper, the docs must describe them as explicit Incan extensions rather than implying they are standard `pathlib` behavior.
- The initial extension set includes `try_exists`, `copy`, `copy_into`, `move`, `move_into`, `remove_tree`, `scandir`, `disk_usage`, `OpenOptions`, `File.sync`, and `File.sync_data`.
- CPython 3.14 compatibility is a baseline, not a wholesale import rule: Incan should adopt the pieces that strengthen a coherent path-centric filesystem module, not every adjacent helper automatically.
- Implementations may use Rust `std::fs`, `std::io`, and related crates internally, but user-visible semantics are defined by this RFC and stdlib docs, not by Rust's type names.

### Required capabilities (initial contract)

The first `std.fs` release must provide the following baseline:

- `Path(path: str | Path) -> Path`.
- Path joining via `/` and/or `joinpath(...)`.
- Lexical properties `parent`, `name`, `suffix`, and `stem`.
- Predicates `exists()`, `is_file()`, `is_dir()`, and `is_symlink()` where the host OS supports symlink inspection.
- `try_exists() -> Result[bool, E]` as the honest existence probe when callers must distinguish "missing" from "could not determine".
- `mkdir(...)` with `parents` / `exist_ok` style options.
- Whole-file binary helpers `read_bytes()` and `write_bytes(...)`.
- `open(...)` with the full Python-style mode family: `"r"`, `"w"`, `"a"`, `"x"`, `"rb"`, `"wb"`, `"ab"`, `"xb"`, `"r+"`, `"w+"`, `"a+"`, `"x+"`, `"rb+"`, `"wb+"`, `"ab+"`, and `"xb+"`.
- A `File` surface that must support `read(size)`, `read_exact(size)`, `write(data)`, `tell()`, `seek(offset, whence=0)`, `sync()`, and `sync_data()`.

Large-file behavior is normative:

- `File.read(size)` must return at most `size` bytes and must not require loading the remainder of the file into memory.
- `File.read(size)` must return an empty `bytes` value at EOF rather than failing.
- `File.read_exact(size)` must fail if fewer than `size` bytes remain.
- `Path.read_bytes()` remains valid convenience API, but it must be documented as a whole-file helper rather than the preferred route for large inputs.

Durability semantics are also normative:

- Successful `write(...)`, `write_bytes(...)`, or object drop must not by themselves imply crash-safe persistence.
- `sync()` is the explicit durability operation and must request persistence of file content and associated metadata.
- `fsync()` must exist as an alias of `sync()` with identical semantics.
- `sync_data()` is the lighter durability operation and may omit metadata that is not required for data visibility, subject to host-platform behavior documented by the stdlib.

`open(...)` mode semantics are also normative:

- `"r"` / `"rb"` must open an existing file for reading and fail if the path does not exist.
- `"w"` / `"wb"` must open for writing, creating the file if needed and truncating it if it already exists.
- `"a"` / `"ab"` must open for append, creating the file if needed and writing new data at the end.
- `"x"` / `"xb"` must open for exclusive creation and fail if the target already exists.
- Modes containing `"+"` must permit both reading and writing.
- Modes containing `"b"` must use binary I/O.
- Modes without `"b"` must use text I/O and therefore participate in the encoding / newline rules defined elsewhere in this RFC.

Text I/O defaults are also normative:

- Text mode must default to `encoding="utf-8"` and `errors="strict"`.
- `read_text(...)` and text-mode `open(...)` must use universal newline handling on read, accepting `"\n"`, `"\r\n"`, and `"\r"` as line boundaries and normalizing them in the returned text.
- `write_text(...)` and text-mode `open(...)` must default to writing `"\n"` line endings unless `newline=...` is explicitly provided.
- Callers may override `encoding`, `errors`, and `newline` when interoperating with legacy systems or external formats that require different text conventions.

Existence-query behavior is also normative:

- `exists()`, `is_file()`, `is_dir()`, and `is_symlink()` may follow CPython 3.14's ergonomic bool style, where callers get `false` for missing or inaccessible paths instead of a raised OS error.
- `try_exists()` exists specifically because the bool style is lossy: it must return `Ok(false)` only when absence is known, and `Err(...)` when the runtime cannot determine existence without ambiguity.

Copy, move, and tree-removal behavior is also normative:

- `copy(...)` and `copy_into(...)` must work for both regular files and directory trees.
- If `follow_symlinks` is `true`, copying a symlink must copy the symlink target's contents. If `follow_symlinks` is `false`, the symlink itself must be recreated at the destination rather than dereferenced.
- If `preserve_metadata` is `false`, the implementation must guarantee copied file data and directory structure, but it must not promise preservation of ownership, timestamps, ACLs, extended attributes, or platform-specific metadata.
- If `preserve_metadata` is `true`, the implementation should preserve permissions, modification/access times, flags, and extended attributes where the host platform can do so. The docs must state clearly which metadata classes are best-effort rather than guaranteed.
- `move(...)` and `move_into(...)` must behave like ordinary renames when the source and destination are on the same filesystem and must fall back to copy-then-delete semantics when they are not.
- `remove_tree()` must delete a directory tree rooted at `self`.
- `remove_tree()` must fail if `self` names a regular file; callers must use `unlink()` for files.
- `remove_tree()` must fail if `self` is a symbolic link, including a symlink to a directory; it must never recurse into a symlink target.
- `remove_tree()` must remove entries bottom-up so non-empty directories are not removed before their children.

### Pathlib alignment roadmap

The path-centric API is approved as a staged roadmap under this one RFC so follow-on `std.fs` work does not need a new design record for every additional `pathlib` method.

| Tier | Intent | Capabilities |
| --- | --- | --- |
| **A — Initial contract** | Must ship with the first `std.fs` release. | Construction from `str`; joining; `parent` / `name` / `suffix` / `stem`; `exists` / `is_file` / `is_dir` / `is_symlink`; `try_exists`; `mkdir`; `read_bytes` / `write_bytes`; `open(...)`; `File.read` / `read_exact` / `write` / `tell` / `seek` / `sync` / `sync_data`. |
| **B — Everyday pathlib** | Should land in near-term releases without a new RFC. | `read_text` / `write_text`; `unlink`; `rmdir`; `rename`; `replace`; `iterdir`; `glob`; `rglob`; `resolve`; `absolute`; `stat`; `lstat`; `Path.cwd()`; `Path.home()`; `copy`; `copy_into`; `move`; `move_into`; `disk_usage`; `OpenOptions`; `remove_tree`; `scandir`; `DirEntry`; `flush`; `fsync`. |
| **C — Host-sensitive edges** | May trail later or require a follow-on RFC if semantics become too platform-specific. | `chmod`; `chown`; `touch`; `symlink_to`; `hardlink_to`; `samefile`; `is_mount`; `expanduser`; `walk`; `owner`; `group`; other permission or host-sensitive edges. |

### Expected API shape (skeletal)

This subsection names the user-visible spellings. It is not an implementation plan.

#### `Path`

- `Path(path: str | Path) -> Path` — **[A]**.
- `/` and `joinpath(...)` for path composition — **[A]**.
- `parent() -> Path`, `name() -> str`, `suffix() -> str`, `stem() -> str` — **[A]**.
- `exists() -> bool`, `is_file() -> bool`, `is_dir() -> bool`, `is_symlink() -> bool` — **[A]**.
- `try_exists() -> Result[bool, E]` — **[A]** explicit Incan extension.
- `mkdir(...) -> Result[(), E]` — **[A]**.
- `read_bytes() -> Result[bytes, E]`, `write_bytes(data: bytes) -> Result[(), E]` — **[A]**.
- `open(mode: str = "r", buffering: int = -1, encoding: str | None = None, errors: str | None = None, newline: str | None = None) -> Result[File, E]` — **[A]**. Supports the full Python-style mode family defined above.
- `read_text(encoding: str = "utf-8", errors: str = "strict") -> Result[str, E]`, `write_text(data: str, encoding: str = "utf-8", errors: str = "strict", newline: str | None = "\n") -> Result[(), E]` — **[B]**.
- `iterdir() -> Result[Iterator[Path], E]`, `glob(pattern: str) -> Result[Iterator[Path], E]`, `rglob(pattern: str) -> Result[Iterator[Path], E]` — **[B]**.
- `stat(...) -> Result[PathStat, E]`, `lstat() -> Result[PathStat, E]` — **[B]**.
- `copy(target: Path | str, follow_symlinks: bool = True, preserve_metadata: bool = False) -> Result[Path, E]` — **[B]**. Copies a file or directory tree to `target` and returns the new path. With `preserve_metadata=False`, only file data and directory structure are guaranteed; with `preserve_metadata=True`, metadata preservation is attempted where supported and documented.
- `copy_into(target_dir: Path | str, follow_symlinks: bool = True, preserve_metadata: bool = False) -> Result[Path, E]` — **[B]**. Copies this path into an existing target directory and returns the copied path.
- `move(target: Path | str) -> Result[Path, E]`, `move_into(target_dir: Path | str) -> Result[Path, E]` — **[B]**. Path-centric move operations following CPython 3.14 naming. Same-filesystem moves should use rename/replace-class semantics; cross-filesystem moves must behave as copy-then-delete.
- `disk_usage() -> Result[DiskUsage, E]` — **[B]** explicit Incan extension returning at least `total`, `used`, and `free` in bytes for the filesystem containing this path.
- `remove_tree() -> Result[(), E]` — **[B]** explicit Incan extension for directory trees only. It must fail on regular files and symlinks rather than silently treating them as trees.
- `scandir() -> Result[Iterator[DirEntry], E]` — **[B]** explicit Incan extension.
- `chown(user: str | int | None = None, group: str | int | None = None, follow_symlinks: bool = True) -> Result[(), E]` — **[C]** host-sensitive ownership change where the platform supports it.

#### `OpenOptions`

- `OpenOptions.new() -> OpenOptions` — **[B]**.
- `read(v: bool) -> OpenOptions`, `write(v: bool) -> OpenOptions`, `append(v: bool) -> OpenOptions`, `truncate(v: bool) -> OpenOptions` — **[B]**.
- `create(v: bool) -> OpenOptions`, `create_new(v: bool) -> OpenOptions` — **[B]**.
- `open(path: Path | str) -> Result[File, E]` — **[B]**.

#### `File`

- `read(size: int = -1) -> Result[bytes | str, E]` — **[A]**. Return type depends on whether the file was opened in binary or text mode.
- `read_exact(size: int) -> Result[bytes, E]` — **[A]**.
- `write(data: bytes | str) -> Result[int, E]` — **[A]**. Accepted data type depends on whether the file was opened in binary or text mode.
- `tell() -> Result[int, E]`, `seek(offset: int, whence: int = 0) -> Result[int, E]` — **[A]**.
- `sync() -> Result[(), E]` — **[A]** explicit durability primitive.
- `fsync() -> Result[(), E]` — **[B]** alias of `sync()` with identical semantics.
- `sync_data() -> Result[(), E]` — **[A]** explicit durability primitive that may omit non-essential metadata writes.
- `flush() -> Result[(), E]` — **[B]**. Flushes user-space buffers but does not imply durable persistence.

#### `DirEntry`

- `path() -> Path`, `file_name() -> str` — **[B]**.
- `is_file(...) -> bool`, `is_dir(...) -> bool`, `is_symlink() -> bool` — **[B]**.
- `metadata() -> Result[PathStat, E]` — **[B]**.

### Errors and compatibility

- Operations must surface failure through ordinary `Result` returns unless a helper is explicitly documented otherwise.
- Error payloads should be actionable, including at minimum the relevant path and the underlying OS message for filesystem failures.
- This RFC is additive. Existing programs and builtins keep compiling while `std.fs` becomes the documented default.

## Design details

### Why chunked file I/O belongs in `std.fs`

Large-file chunking is a filesystem concern before it is an in-memory parsing concern. A program that wants to hash, upload, transcode, or scan a multi-gigabyte file must be able to open that file and consume bounded reads without routing through `read_bytes()`. That is why `Path.open(...)` and a minimal `File` contract are part of the initial `std.fs` approval unit rather than a deferred convenience.

### CPython baseline plus explicit Incan extensions

The core design rule is: if CPython 3.14 `pathlib.Path` already has the operation, Incan should prefer the same spelling and broadly compatible behavior. When Incan needs more, the RFC should say so plainly. `try_exists`, path-centric copy and move helpers, `remove_tree`, `OpenOptions`, and durability methods are not "secret pathlib"; they are deliberate Incan extensions informed by Rust's `std::fs` because they solve real filesystem tasks cleanly.

CPython 3.14 also made `exists()`-style queries more aggressively bool-shaped by returning `false` rather than surfacing OS errors for inaccessible paths. That is a reasonable default for quick predicates, but it is not sufficient for correctness-sensitive code. Incan therefore keeps the ergonomic bool predicates and also standardizes `try_exists()` for callers that need to preserve the distinction between "missing" and "unknown because the probe failed."

CPython 3.14 introduced path copy and move helpers as well. Those belong in Incan's `std.fs` story for the exact reason you called out: users should be able to stay on `Path` for ordinary filesystem work instead of reaching for a second filesystem module. This RFC adopts the CPython 3.14 spellings directly: `copy`, `copy_into`, `move`, and `move_into`.

Some `shutil` ideas do fit naturally in a path-centric module. Disk-usage queries are a good example because they are path-owned and filesystem-facing: a `Path.disk_usage()` method keeps the operation in `std.fs` without introducing a second high-level file-operations namespace. By contrast, shell/environment helpers such as path-variable expansion or executable lookup should not be pulled into `std.fs` just because Python happens to expose them elsewhere.

This same principle applies to whole-file helpers. The RFC does not expose parallel module-level `read_bytes(...)` / `write_bytes(...)` / `read_text(...)` / `write_text(...)` shortcuts, because keeping a second "string path first" style alive would weaken the intended `Path`-first model. Incan should teach: construct a `Path`, then operate on that path.

Existing compiler builtins such as `read_file` and `write_file` may remain for compatibility. This RFC does not deprecate or remove them. The design claim is narrower: `std.fs.Path` is the canonical filesystem model for new APIs, documentation, and examples, and any builtin overlap should converge on the same observable semantics where practical.

The text defaults follow the same philosophy. Incan should choose explicit, modern defaults rather than inheriting ambient host conventions, so UTF-8 with strict error handling and `"\n"` output are the normative defaults. At the same time, the API must stay parameterized because migration and interoperability work are real use cases; callers need to be able to opt into ASCII, Latin-1, Windows line endings, or other legacy conventions when the target system requires them.

### Copy and tree semantics that should not stay implicit

The weak point in many filesystem APIs is not naming but hand-wavy behavior around metadata, symlinks, and recursive deletion. This RFC should be explicit there.

`copy(..., preserve_metadata=False)` is intentionally the lower-guarantee path: callers get copied bytes and the expected directory structure, but not a portability promise about timestamps, ownership, ACLs, or extended attributes. `preserve_metadata=True` is the opt-in request for richer preservation, with the understanding that some metadata classes are inherently host-sensitive and must remain best-effort unless the platform can guarantee them.

`remove_tree()` also needs a hard boundary: it is for real directory trees, not a polymorphic "delete whatever I point at" shortcut. Python's `rmtree()` explicitly says the path "must point to a directory (but not a symbolic link to a directory)" ([shutil 3.14](https://docs.python.org/3.14/library/shutil.html)), and that is the right safety posture here too.

Durability should also stay explicit. Python normally requires `flush()` plus `os.fsync(...)` for a strong persistence request ([os 3.14](https://docs.python.org/3.14/library/os.html)), and Rust exposes `sync_all()` / `sync_data()` rather than treating ordinary writes or drop as durability boundaries ([Rust `std::fs::File`](https://doc.rust-lang.org/std/fs/struct.File.html)). Incan should follow that model: successful writes mean normal write success, while `sync()` / `sync_data()` express persistence intent. Because the runtime is Rust-backed, the RFC does not require an explicit `close()` method as part of the public contract.

### Interaction with Rust interop

Authors may still use `rust::std::fs` and `rust::std::io` for capabilities this RFC does not standardize. The stdlib module should remain the documented default for portable baseline filesystem work.

## Alternatives considered

1. **Whole-file helpers only** — smaller initial surface, but it does not solve large-file chunking and would leave serious workloads on `rust::`.
2. **Keep `std.fs` and `std.io` in one RFC** — rejected because filesystem review and in-memory byte-cursor review are separable approval units.
3. **Rust-shaped public API** — accurate to the backing implementation, but a worse tutorial and documentation story for Incan users.
4. **Separate `std.os` / `std.shutil` style modules** — rejected for now because ordinary path-owned chores should stay path-centric in Incan.

## Drawbacks

- `std.fs` is still a broad surface area even after splitting out `std.io`.
- Promise drift is a risk: docs must not claim "pathlib parity" while quietly changing semantics or inventing extensions without labeling them as such.
- Durability, recursive deletion, and cross-platform metadata have semantic corners that require careful stdlib documentation and tests.
- Whole-file helpers remain easy to misuse on large inputs, so docs must explicitly teach when to choose chunked `open(...)` instead.

## Implementation architecture

*(Non-normative.)* A practical first delivery implements `Path`, `File`, and related helpers as normal Incan stdlib code that uses `rust::` interop to reach `std::fs`, `std::io`, and associated path and file primitives. Large Rust-only wrappers should be reserved for narrow host boundaries rather than used as the default substitute for writing ordinary stdlib logic in Incan.

## Layers affected

- **Stdlib / runtime (`incan_stdlib`)**: new `std.fs` module and supporting types such as `Path`, `File`, `OpenOptions`, and `DirEntry`.
- **Language surface**: imports, constructors, methods, and helpers must be available without ad hoc special cases.
- **Reference docs**: documentation must explain the difference between whole-file helpers and chunked file-handle APIs.
- **LSP / tooling**: completions and hovers for `std.fs` members.
- **Tests / docs-site**: API docs and examples must cover both whole-file and chunked large-file workflows.

## Design Decisions

- `std.fs.Path` is the canonical filesystem model. Existing builtins such as `read_file` and `write_file` may remain for compatibility, but new APIs, documentation, and examples should prefer `Path`.
- Construction uses direct calls such as `Path("config.toml")`, not `Path.new(...)`.
- `Path.open(...)` commits to the full Python-style mode-string family. Delivery may still be phased, but the spec is not intentionally partial.
- `Path` remains the single path object for both files and directories. Opening a path yields `File`; there is no parallel `Folder` abstraction.
- Durability is explicit. Successful writes and object drop do not imply crash-safe persistence; callers use `sync()` or `sync_data()` when they need persistence guarantees. `fsync()` exists as an alias of `sync()`.
- Text I/O defaults are `utf-8`, `strict`, and `"\n"` output, with override parameters for interoperability and migration work.
- `std.fs` stays path-centric: copy, move, recursive deletion, scanning, and disk-usage queries belong on `Path` rather than being pushed into separate `os`- or `shutil`-style modules.
