# std.fs reference

`std.fs` is the standard-library module for filesystem paths, directory operations, metadata, and file handles.

```incan
from std.fs import DirEntry, DiskUsage, File, IoError, OpenOptions, Path, PathStat
```

The public surface is path-centric. Construct a `Path`, then call path or file-handle methods from there. Host failures return `Result[..., IoError]`; the error includes the affected `path`, a normalized `kind`, and a human-readable `message()`.

## Path

| API | Returns | Description |
| --- | --- | --- |
| `Path(path: str)` | `Path` | Construct a lexical path value. |
| `left / child` | `Path` | Join with one child segment. |
| `path.joinpath(child: str)` | `Path` | Join with one child segment. |
| `path.parent()` | `Path` | Parent path. |
| `path.name()` | `str` | Final component. |
| `path.suffix()` | `str` | Final suffix, including the dot. |
| `path.stem()` | `str` | Final component without its suffix. |
| `Path.cwd()` | `Result[Path, IoError]` | Current working directory. |
| `Path.home()` | `Result[Path, IoError]` | Current user's home directory. |

Construction and lexical helpers do not read the filesystem.

## Existence and Metadata

| API | Returns | Description |
| --- | --- | --- |
| `path.exists()` | `bool` | `true` when the path exists; inaccessible paths collapse to `false`. |
| `path.is_file()` | `bool` | `true` for regular files. |
| `path.is_dir()` | `bool` | `true` for directories. |
| `path.is_symlink()` | `bool` | `true` for symlinks. |
| `path.try_exists()` | `Result[bool, IoError]` | Honest existence check that preserves host errors. |
| `path.stat()` | `Result[PathStat, IoError]` | Metadata that follows symlinks. |
| `path.lstat()` | `Result[PathStat, IoError]` | Metadata for the path itself. |
| `path.samefile(other: Path \| str)` | `Result[bool, IoError]` | Whether two paths identify the same file. |
| `path.is_mount()` | `Result[bool, IoError]` | Whether the path is a mount point. |
| `path.disk_usage()` | `Result[DiskUsage, IoError]` | Total, used, and free bytes for the containing filesystem. |

Use `try_exists()` when "missing" and "could not check" lead to different behavior.

## Directory and Tree Operations

| API | Returns | Description |
| --- | --- | --- |
| `path.mkdir(parents: bool, exist_ok: bool)` | `Result[None, IoError]` | Create a directory. |
| `path.iterdir()` | `Result[list[Path], IoError]` | Immediate children. |
| `path.scandir()` | `Result[list[DirEntry], IoError]` | Immediate children as directory entries. |
| `path.glob(pattern: str)` | `Result[list[Path], IoError]` | Match children below the path. |
| `path.rglob(pattern: str)` | `Result[list[Path], IoError]` | Recursive glob. |
| `path.unlink()` | `Result[None, IoError]` | Remove a file or symlink. |
| `path.rmdir()` | `Result[None, IoError]` | Remove an empty directory. |
| `path.remove_tree()` | `Result[None, IoError]` | Remove a directory tree; files and symlinks are errors. |
| `path.touch(exist_ok: bool)` | `Result[None, IoError]` | Create the file if needed. |

`glob()` and `rglob()` support `*`, `?`, and bracket character classes such as `[abc]`, `[!abc]`, and `[a-z]`. `remove_tree()` is deliberately not "delete anything"; use `unlink()` for files.

## File Contents

| API | Returns | Description |
| --- | --- | --- |
| `path.read_bytes()` | `Result[bytes, IoError]` | Read an entire file into memory. |
| `path.write_bytes(data: bytes)` | `Result[None, IoError]` | Write a complete byte buffer. |
| `path.read_text(encoding: str, errors: str)` | `Result[str, IoError]` | Read and decode an entire file. |
| `path.write_text(data: str, encoding: str, errors: str, newline: Option[str])` | `Result[None, IoError]` | Encode and write text. |
| `path.open(mode: str, buffering: int, encoding: Option[str], errors: Option[str], newline: Option[str])` | `Result[File, IoError]` | Open a file handle. |

Whole-file helpers are for small payloads. Use `open(...)` when memory bounds or streaming matter.

`open(...)` supports `r`, `w`, `a`, `x`, their binary forms, and `+` read-write variants. Binary modes reject `encoding`, `errors`, and `newline`. Text handles currently support UTF-8 with strict error handling; unsupported encodings or error strategies return `IoError(kind="invalid_input")` rather than being silently ignored.

## Copy, Move, and Links

| API | Returns | Description |
| --- | --- | --- |
| `path.copy(target: Path \| str, follow_symlinks: bool, preserve_metadata: bool)` | `Result[Path, IoError]` | Copy a file or directory tree. |
| `path.copy_into(target_dir: Path \| str, follow_symlinks: bool, preserve_metadata: bool)` | `Result[Path, IoError]` | Copy into an existing directory. |
| `path.move(target: Path \| str)` | `Result[Path, IoError]` | Move or rename, with copy-delete fallback when required. |
| `path.move_into(target_dir: Path \| str)` | `Result[Path, IoError]` | Move into an existing directory. |
| `path.rename(target: Path \| str)` | `Result[Path, IoError]` | Rename and return the new path. |
| `path.replace(target: Path \| str)` | `Result[Path, IoError]` | Replace the target when supported. |
| `path.symlink_to(target: Path \| str)` | `Result[None, IoError]` | Create a symlink at this path. |
| `path.hardlink_to(target: Path \| str)` | `Result[None, IoError]` | Create a hard link at this path. |
| `path.chmod(readonly: bool)` | `Result[None, IoError]` | Set or clear readonly permissions. |
| `path.absolute()` | `Result[Path, IoError]` | Absolute path without canonicalization. |
| `path.resolve()` | `Result[Path, IoError]` | Canonical path. |
| `path.expanduser()` | `Result[Path, IoError]` | Expand a leading `~`. |

Metadata preservation during copy is best-effort where the host platform exposes it.

## File

| API | Returns | Description |
| --- | --- | --- |
| `file.read(size: int)` | `Result[str, IoError]` | Read text from the current cursor. |
| `file.read_bytes(size: int)` | `Result[bytes, IoError]` | Read at most `size` bytes, or the rest when `size` is negative. |
| `file.read_exact(size: int)` | `Result[bytes, IoError]` | Read exactly `size` bytes or fail. |
| `file.write(data: str)` | `Result[int, IoError]` | Write text and return bytes accepted. |
| `file.write_bytes(data: bytes)` | `Result[int, IoError]` | Write bytes and return bytes accepted. |
| `file.tell()` | `Result[int, IoError]` | Current cursor. |
| `file.seek(offset: int, whence: int)` | `Result[int, IoError]` | Move cursor; `0` start, `1` current, `2` end. |
| `file.flush()` | `Result[None, IoError]` | Flush user-space buffers. |
| `file.sync()` / `file.fsync()` | `Result[None, IoError]` | Request data and metadata persistence. |
| `file.sync_data()` | `Result[None, IoError]` | Request data persistence. |

Successful writes do not imply crash-safe persistence. Call `sync()` or `sync_data()` when durability matters.

## OpenOptions

`OpenOptions` provides explicit open flags:

```incan
from std.fs import OpenOptions, Path

file = OpenOptions.new().read(True).write(True).create(True).open(Path("data.bin"))?
```

Builder methods are `read`, `write`, `append`, `truncate`, `create`, and `create_new`.

## Temporary Files

`std.tempfile` owns temporary location creation and cleanup. Once a temporary path exists, use `std.fs` for path joins, reads, writes, opens, metadata, copy/move, and cleanup.

`std.tempfile.SpooledTempFile` remains a follow-up API for RFC 010 rather than part of `std.fs`.
