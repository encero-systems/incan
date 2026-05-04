# std.tempfile reference

`std.tempfile` creates temporary filesystem locations with automatic cleanup. Once a path exists, use `std.fs.Path` and `std.fs.File` for ordinary reads, writes, metadata, joins, and explicit cleanup.

```incan
from std.tempfile import NamedTemporaryFile, TemporaryDirectory
```

Creation is fallible because it reserves real host filesystem entries. Use `try_new()` for default acquisition and `try_new_with(prefix, suffix, dir)` for configured acquisition. Direct `NamedTemporaryFile(...)` and `TemporaryDirectory(...)` construction is ordinary infallible class construction and is not the resource-acquisition API.

## NamedTemporaryFile

| API | Returns | Description |
| --- | --- | --- |
| `NamedTemporaryFile.try_new()` | `Result[NamedTemporaryFile, IoError]` | Create a named temporary file with host defaults. |
| `NamedTemporaryFile.try_new_with(prefix: str, suffix: str, dir: Option[Path])` | `Result[NamedTemporaryFile, IoError]` | Create a named temporary file with configured naming or parent directory. |
| `file.path()` | `Path` | Current filesystem path for the temporary file. |
| `file.persist()` | `Result[Path, IoError]` | Keep the file at its current path and disable automatic deletion. |

```incan
from std.fs import IoError, Path
from std.tempfile import NamedTemporaryFile

def write_report(text: str) -> Result[Path, IoError]:
    temp = NamedTemporaryFile.try_new_with("report-", ".txt", None)?
    path = temp.path()
    path.write_text(text, "utf-8", "strict", None)?
    return temp.persist()
```

While the wrapper is live and not persisted, dropping it deletes the file. `path()` is useful for APIs that need a filename rather than an already-open handle.

## TemporaryDirectory

| API | Returns | Description |
| --- | --- | --- |
| `TemporaryDirectory.try_new()` | `Result[TemporaryDirectory, IoError]` | Create a temporary directory with host defaults. |
| `TemporaryDirectory.try_new_with(prefix: str, suffix: str, dir: Option[Path])` | `Result[TemporaryDirectory, IoError]` | Create a temporary directory with configured naming or parent directory. |
| `directory.path()` | `Path` | Current filesystem path for the temporary directory. |
| `directory.persist()` | `Result[Path, IoError]` | Keep the directory tree at its current path and disable automatic deletion. |

```incan
from std.fs import IoError, Path
from std.tempfile import TemporaryDirectory

def stage_artifact(name: str, data: bytes) -> Result[Path, IoError]:
    workspace = TemporaryDirectory.try_new_with("stage-", "", None)?
    artifact = workspace.path() / name
    artifact.write_bytes(data)?
    return workspace.persist()
```

Dropping an unpersisted `TemporaryDirectory` removes the whole temporary tree. Use `persist()` for outputs that intentionally survive the current scope.

## Parent Directory

Pass `dir` when temporary locations must live under a specific parent:

```incan
from std.fs import IoError, Path
from std.tempfile import NamedTemporaryFile

def scratch_under(root: Path) -> Result[Path, IoError]:
    temp = NamedTemporaryFile.try_new_with("scratch-", ".bin", Some(root))?
    return Ok(temp.path())
```

The `dir` argument accepts `Option[Path]`. Wrap string parents with `Path("...")` before passing them. Failure details are returned as `IoError` with the requested parent path when creation fails.

## Follow-up Surface

`SpooledTemporaryFile` is intentionally not part of this initial surface. It remains a follow-up RFC or issue so the in-memory-to-disk rollover contract can be designed against `std.fs.File` and streaming APIs without expanding the first tempfile slice.

## See Also

- [std.fs reference](fs.md)
- [File I/O how-to](../../how-to/file_io.md)
