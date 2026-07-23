# File I/O in Incan

Use `std.fs.Path` as the entry point for files and directories. File operations return `Result`, so handle recoverable failures with `match` and propagate boundary failures with `?` from functions that return `Result`.

```incan
from std.fs import IoError, Path
```

## Build Paths

```incan
from std.fs import Path

def config_path(root: Path) -> Path:
    return root / "config.toml"

def artifact_path(root: Path, name: str) -> Path:
    return root.joinpath(name)
```

Path construction and joins are lexical. They do not prove that anything exists on disk.

## Check Existence

Use bool predicates for quick branches:

```incan
from std.fs import Path

def has_cache(root: Path) -> bool:
    return root.joinpath("cache.bin").is_file()
```

Use `try_exists()` when the difference between "missing" and "the check failed" matters:

```incan
from std.fs import IoError, Path

def require_input(path: Path) -> Result[Path, IoError]:
    if path.try_exists()?:
        return Ok(path)
    return Err(IoError(path=path, kind="not_found", detail="input path does not exist"))
```

## Create Directories

```incan
from std.fs import IoError, Path

def prepare_output_dir(path: Path) -> Result[None, IoError]:
    path.mkdir(parents=True, exist_ok=True)?
    return Ok(None)
```

`parents=True` creates missing parent directories. `exist_ok=True` treats an existing directory as success.

## Read and Write Small Files

Whole-file helpers are convenient for configuration files, test fixtures, and payloads that comfortably fit in memory.

```incan
from std.fs import IoError, Path

def copy_small_file(source: Path, target: Path) -> Result[None, IoError]:
    data = source.read_bytes()?
    target.write_bytes(data)?
    return Ok(None)

def save_text(path: Path, text: str) -> Result[None, IoError]:
    path.write_text(text, "utf-8", "strict", None)?
    return Ok(None)
```

Use `read_text("utf-8", "strict")` and `write_text(..., "utf-8", "strict", None)` for normal UTF-8 text files.

## Stream Large Files

For large files, open a handle and read bounded chunks.

```incan
from std.fs import IoError, Path

def copy_in_chunks(source: Path, target: Path) -> Result[None, IoError]:
    input = source.open("rb", -1, None, None, None)?
    output = target.open("wb", -1, None, None, None)?

    for chunk in input.chunks(8192)?:
        output.write_bytes(chunk)?

    output.sync_data()?
    return Ok(None)
```

The chunk stream yields only non-empty byte blocks and treats EOF as ordinary loop exhaustion. `sync_data()` requests durable file data; use `sync()` when metadata durability matters too.

## Read a Fixed Header

```incan
from std.fs import IoError, Path

def read_magic(path: Path) -> Result[bytes, IoError]:
    file = path.open("rb", -1, None, None, None)?
    return file.read_exact(4)
```

`read_exact(size)` fails on short reads, which is useful for file headers and binary protocol frames.

## Seek Within a File

```incan
from std.fs import IoError, Path

def read_footer(path: Path, size: int) -> Result[bytes, IoError]:
    file = path.open("rb", -1, None, None, None)?
    file.seek(0 - size, 2)?
    return file.read_exact(size)
```

Use `tell()` when you need to save or report the current cursor.

## Copy, Move, and Clean Up

```incan
from std.fs import IoError, Path

def publish_tree(build_dir: Path, release_dir: Path) -> Result[Path, IoError]:
    copied = build_dir.copy_into(release_dir, follow_symlinks=True, preserve_metadata=False)?
    copied.joinpath("READY").touch(exist_ok=True)?
    return Ok(copied)

def replace_file(source: Path, target: Path) -> Result[Path, IoError]:
    return source.move(target)

def remove_workspace(path: Path) -> Result[None, IoError]:
    path.remove_tree()?
    return Ok(None)
```

`remove_tree()` is for directories. Use `unlink()` for files and symlinks.

## Publish a File Without Exposing Partial Contents

Use a sibling temporary file when readers must see either the previous complete value or the next complete value. Creating the temporary file inside `target.parent()` keeps the final replacement on the same filesystem, while `NamedTemporaryFile` reserves a unique name exclusively and cleans it up if writing or synchronization fails.

```incan
from std.fs import IoError, Path
from std.tempfile import NamedTemporaryFile

def publish_bytes(target: Path, contents: bytes) -> Result[None, IoError]:
    guard = target.lock_exclusive()?
    staging = NamedTemporaryFile.try_new_with(f".{target.name()}-", ".tmp", Some(target.parent()))?
    staged_path = staging.path()

    staged_path.write_bytes(contents)?
    staged_file = staged_path.open("rb")?
    staged_file.sync()?
    staged_path.replace(target)?
    target.parent().sync_directory()?
    return Ok(None)
```

Keep the exclusive `guard` live across reading the previous value, preparing the replacement, replacing it, and synchronizing the directory. Other cooperating writers acquire the same target lock, while readers that need a stable multi-step view use `target.lock_shared()`.

Each step provides a different guarantee:

1. `NamedTemporaryFile.try_new_with(...)` creates an exclusively reserved, unpredictable sibling path rather than sharing a predictable `.next` name with another process.
2. `staged_file.sync()` asks the host to persist the complete staged contents before publication.
3. `staged_path.replace(target)` atomically changes which complete file the target name identifies. It never deletes the old target first and returns `IoError(kind="cross_device")` rather than falling back to copy-and-delete.
4. `target.parent().sync_directory()` asks the host to persist the changed directory entry.

If writing, file synchronization, or replacement fails, `?` returns the error, the temporary wrapper cleans up its still-staged path, and `replace()` preserves the previous target instead of deleting it first. If directory synchronization fails, replacement has already happened: return the error and do not claim crash durability, even though readers may already observe the new complete file.

Do not put the staging file in the host's default temporary directory: that directory may be on another filesystem, where atomic replacement cannot be guaranteed. Do not use a fixed sibling name in a directory another user or process can write. The lock is advisory, so it coordinates only software that follows the same locking convention; it is not an authorization boundary. The initial durability and locking contract supports Linux and macOS, including macOS applications running on a Unix-like filesystem, and Windows through WSL. Native Windows filesystem semantics are not yet covered.

If atomic visibility is sufficient but crash durability is not required, the file and directory synchronization requests may be unnecessary for your application. Keep the same unique sibling and `replace()` pattern: atomic visibility, durability, and writer coordination are independent choices.

## Directory Listings

```incan
from std.fs import IoError, Path

def list_inputs(root: Path) -> Result[list[Path], IoError]:
    return root.glob("*.incn")

def list_all_inputs(root: Path) -> Result[list[Path], IoError]:
    return root.rglob("*.incn")
```

Use `scandir()` when you want directory entries that can answer `is_file()`, `is_dir()`, and `metadata()`.

## Temporary-File Layout

Temporary location creation belongs to `std.tempfile`; ordinary operations on those locations belong to `std.fs`.

```incan
from std.fs import IoError, Path
from std.tempfile import NamedTemporaryFile, SpooledTemporaryFile, TemporaryDirectory

def write_temp_payload(data: bytes) -> Result[Path, IoError]:
    temp = NamedTemporaryFile.try_new_with("payload-", ".bin", None)?
    path = temp.path()
    path.write_bytes(data)?
    return temp.persist()

def build_workspace() -> Result[Path, IoError]:
    workspace = TemporaryDirectory.try_new_with("incan-build-", "", None)?
    artifact = workspace.path() / "artifact.txt"
    artifact.write_text("ready", "utf-8", "strict", None)?
    return workspace.persist()

def collect_large_payload(chunks: list[bytes]) -> Result[Path, IoError]:
    spool = SpooledTemporaryFile(max_size=1024 * 1024)
    for chunk in chunks:
        spool.write(chunk)?
    return spool.persist()
```

Temporary wrappers delete their live paths when the wrapper is dropped. Call `persist()` when the caller should keep the path after the wrapper leaves scope. `NamedTemporaryFile.try_new()` and `TemporaryDirectory.try_new()` are fallible because they reserve real filesystem entries; use `try_new_with(prefix, suffix, dir)` when naming or parent placement matters.

`SpooledTemporaryFile(max_size=...)` starts in memory and rolls over to a named temporary file after the buffer grows beyond `max_size`. Use it when small payloads should avoid the filesystem but large payloads still need a path through `rollover()`, `path()`, or `persist()`.

## See Also

- [std.fs reference](../reference/stdlib/fs.md)
- [std.tempfile reference](../reference/stdlib/tempfile.md)
- [Error handling](../explanation/error_handling.md)
