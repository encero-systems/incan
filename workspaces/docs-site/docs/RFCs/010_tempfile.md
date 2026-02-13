# RFC 010: Temporary Files and Directories

**Status:** Draft  
**Created:** 2024-12-11

## Summary

Add `TempFile` and `TempDir` types for creating temporary files and directories that are automatically cleaned up when
they go out of scope.

## Motivation

Temporary files are essential for:

- Testing (create fixtures, verify file operations)
- Safe file updates (write to temp, then rename)
- Processing pipelines (intermediate results)
- Caching and scratch space

Python provides `tempfile.NamedTemporaryFile` and `tempfile.TemporaryDirectory`.
Rust has the widely-used `tempfile` crate.
Incan should provide similar ergonomic temporary file handling.

### Python Comparison

```python
import tempfile

# Python - context manager pattern
with tempfile.NamedTemporaryFile(delete=True) as f:
    f.write(b"data")
    # use f.name to get path
# file deleted here

with tempfile.TemporaryDirectory() as d:
    path = Path(d) / "config.toml"
    path.write_text("...")
# directory deleted here
```

Incan should provide this with RAII instead of context managers.

## Design

### TempFile

```incan
# Create a temporary file
temp = TempFile.new()?

# Write to it
temp.write_text("some data")?

# Get the path (for passing to other functions)
path = temp.path()
process_file(path)

# File is automatically deleted when `temp` goes out of scope
```

### TempDir

```incan
# Create a temporary directory
temp_dir = TempDir.new()?

# Create files inside it
config = temp_dir.path() / "config.toml"
config.write_text(default_config)?

data_dir = temp_dir.path() / "data"
data_dir.mkdir()?

# Entire directory tree deleted when `temp_dir` goes out of scope
```

### Named Temporary Files

For cases where you need a specific suffix/prefix:

```incan
# With custom suffix
temp = TempFile.with_suffix(".json")?

# With custom prefix
temp = TempFile.with_prefix("download_")?

# With both
temp = TempFile.builder()
    .prefix("report_")
    .suffix(".pdf")
    .build()?
```

### Persistence (Keep on Drop)

Sometimes you want to create a temp file but keep it:

```incan
temp = TempFile.new()?
temp.write_text(data)?

# Keep the file - returns the Path and prevents deletion
final_path = temp.persist()?
println(f"Saved to: {final_path}")
```

### Explicit Location

Create temp files in a specific directory:

```incan
# In a specific directory
temp = TempFile.in_dir(Path("/var/cache/myapp"))?

# In system temp directory (default)
temp = TempFile.new()?  # Uses system temp dir
```

## API Reference

### TempFile Methods

| Method                    | Returns                     | Description                         |
| ------------------------- | --------------------------- | ----------------------------------- |
| `TempFile.new()`          | `Result[TempFile, IoError]` | Create temp file in system temp dir |
| `TempFile.with_suffix(s)` | `Result[TempFile, IoError]` | Create with file extension          |
| `TempFile.with_prefix(s)` | `Result[TempFile, IoError]` | Create with filename prefix         |
| `TempFile.in_dir(p)`      | `Result[TempFile, IoError]` | Create in specific directory        |
| `t.path()`                | `Path`                      | Get the file's path                 |
| `t.write_text(s)`         | `Result[None, IoError]`     | Write string                        |
| `t.write_bytes(b)`        | `Result[None, IoError]`     | Write bytes                         |
| `t.read_text()`           | `Result[str, IoError]`      | Read as string                      |
| `t.persist()`             | `Result[Path, IoError]`     | Keep file, return path              |

### TempDir Methods

| Method                   | Returns                    | Description                 |
| ------------------------ | -------------------------- | --------------------------- |
| `TempDir.new()`          | `Result[TempDir, IoError]` | Create temp directory       |
| `TempDir.with_prefix(s)` | `Result[TempDir, IoError]` | Create with name prefix     |
| `TempDir.in_dir(p)`      | `Result[TempDir, IoError]` | Create in specific parent   |
| `d.path()`               | `Path`                     | Get the directory's path    |
| `d.persist()`            | `Result[Path, IoError]`    | Keep directory, return path |

## Implementation

### Vocabulary / crate layout note

In the current workspace, user-facing vocabulary is centralized in `incan_core`.
This RFC introduces new surface types and methods, so the canonical spellings should be registered in:

- `crates/incan_core/src/lang/surface/types.rs` (add `TempFile`, `TempDir`)
- Add method names to `crates/incan_core/src/lang/surface/methods.rs` (consolidated surface method registries),
    e.g. a `tempfile_methods` registry for `TempFile`/`TempDir` methods like `new`, `with_suffix`, `persist`, etc.

### Backend

Map to the Rust `tempfile` crate:

- `TempFile` → `tempfile::NamedTempFile`
- `TempDir` → `tempfile::TempDir`

Add `tempfile` to generated `Cargo.toml` when these types are used.

### Generated Code

```incan
temp = TempFile.new()?
temp.write_text("data")?
```

Generates:

```rust
let temp = tempfile::NamedTempFile::new()?;
std::fs::write(temp.path(), "data")?;
```

## Use Cases

### Testing

```incan
def test_config_loading():
    temp = TempFile.with_suffix(".toml")?
    temp.write_text("""
        [server]
        host = "localhost"
        port = 8080
    """)?
    
    config = load_config(temp.path())?
    assert config.server.port == 8080
    # temp file cleaned up after test
```

### Safe File Updates

```incan
def safe_save(path: Path, content: str) -> Result[None, IoError]:
    # Write to temp file first
    temp = TempFile.in_dir(path.parent())?
    temp.write_text(content)?
    
    # Atomic rename (same filesystem)
    temp.persist()?.rename(path)?
    return Ok(None)
```

### Download with Cleanup

```incan
import std.async

async def process_download(url: str) -> Result[Data, AppError]:
    temp = TempFile.with_suffix(".zip")?
    
    # Download to temp file
    await download_to(url, temp.path())?
    
    # Process
    data = extract_and_parse(temp.path())?
    
    return Ok(data)
    # temp file deleted even if extraction fails
```

## Alternatives Considered

### 1. Functions Instead of Types

```incan
path = create_temp_file()?
# ... use path ...
remove(path)?  # Manual cleanup
```

Rejected: Easy to forget cleanup, especially on error paths.

### 2. Context Manager Syntax

```incan
with TempFile.new() as temp:
    temp.write_text("data")?
```

Rejected: Incan prefers RAII over explicit context managers.
The type-based approach is more Rust-like and ensures cleanup even without `with`.

## Open Questions

1. **Should TempFile implement File trait?** Could allow `temp.lines()`, `temp.read_all()`, etc.

2. **Secure temp files?** The `tempfile` crate provides secure creation (avoids race conditions).
    Should we expose options for this?

3. **Memory-backed temp files?** For small files, an in-memory option could be faster. Worth adding `TempFile.in_memory()`?

## References

- [Python tempfile module](https://docs.python.org/3/library/tempfile.html)
- [Rust tempfile crate](https://docs.rs/tempfile/latest/tempfile/)
- [RAII pattern](https://en.wikipedia.org/wiki/Resource_acquisition_is_initialization)
