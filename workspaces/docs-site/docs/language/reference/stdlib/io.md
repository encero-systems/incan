# std.io reference

`std.io` provides in-memory binary streams for byte buffers that are already in memory.

```incan
from std.io import BinaryReader, BytesIO, Endian, IoError, ReaderChunks
```

Use `BytesIO` for parser fixtures, protocol payloads, generated binary blobs, and format readers that should not depend on filesystem paths. `std.fs` gets bytes into or out of files; `std.io` moves a cursor through bytes that already exist.

## BytesIO

| API | Returns | Description |
| --- | --- | --- |
| `BytesIO(initial: bytes = b"")` | BytesIO stream | Construct a stream with its cursor at `0`. |
| `buf.read(size: int = -1)` | `Result[bytes, IoError]` | Read at most `size` bytes, or the rest when `size` is negative. |
| `buf.read_bytes(size: int)` | `Result[bytes, IoError]` | Trait-backed bounded byte read for consumers such as `std.hash.reader_digest`. |
| `buf.chunks(chunk_size: int)` | `ReaderChunks[BytesIO]` | Return a fallible stream of non-empty bounded chunks. |
| `buf.read_exact(size: int)` | `Result[bytes, IoError]` | Read exactly `size` bytes or return `unexpected_eof`. |
| `buf.read_until(byte: u8)` | `Result[bytes, IoError]` | Read through a delimiter byte or EOF. |
| `buf.skip_until(byte: u8)` | `Result[int, IoError]` | Discard through a delimiter byte or EOF and return the skipped byte count. |
| `buf.tell()` | `int` | Current cursor position. |
| `buf.seek(offset: int, whence: int = 0)` | `Result[int, IoError]` | Move the cursor; `0` start, `1` current, `2` end. |
| `buf.rewind()` | `Result[None, IoError]` | Move to the start. |
| `buf.seek_relative(offset: int)` | `Result[None, IoError]` | Move relative to the current cursor. |
| `buf.write(data: bytes)` | `Result[int, IoError]` | Write bytes at the current cursor. |
| `buf.write_bytes(data: bytes)` | `Result[int, IoError]` | Compatibility spelling for `write`. |
| `buf.truncate(size: Option[int] = None)` | `Result[int, IoError]` | Resize to `size`, or to the cursor when omitted. |
| `buf.getvalue()` | `bytes` | Return a snapshot of the buffer. |
| `buf.into_bytes()` | `bytes` | Return the buffer bytes without changing the cursor. |
| `buf.remaining()` | `int` | Unread bytes from the cursor to the end. |

## Binary Numeric I/O

`BytesIO` uses trait-backed overloads for exact-width numeric reads and writes. Callers use `read(endian)` and `write(value, endian)` directly on the stream.

Reads are selected by the expected result type, so provide static type context. Writes are selected by the value type.

| Trait | API |
| --- | --- |
| `Endian` | `Endian.Little`, `Endian.Big` |
| `BinaryReader` | `read_bytes(size: int) -> Result[bytes, IoError]`; `chunks(size: int) -> ReaderChunks[Self]` |
| `BinaryRead[T]` | `read(endian: Endian) -> Result[T, IoError]` |
| `BinaryWrite[T]` | `write(value: T, endian: Endian) -> Result[None, IoError]` |

Supported `T` values are `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `u64`, `i64`, `u128`, `i128`, `f32`, and `f64`. Endianness is ignored for one-byte values.

## Fallible chunk streams

`BinaryReader.chunks(size)` owns a reader's advancing state and yields only non-empty `bytes` values. Consume it with the error-aware loop form: the trailing `?` propagates an `IoError` from each read, while EOF ends the loop normally.

```incan
from std.io import BytesIO, IoError

def print_chunk_lengths() -> Result[None, IoError]:
    for chunk in BytesIO(b"abcde").chunks(2)?:
        println(len(chunk))
    return Ok(None)
```

The example prints `2`, `2`, and `1`. An empty successful read is EOF; a size less than or equal to zero returns `IoError(kind="invalid_input")` on the first poll.

`ReaderChunks` implements `FallibleIterator[bytes, IoError]`, so it supports the generic fallible adapter and terminal surface. For example, this maps only read failures into an application error before collecting all chunks:

```incan
from std.io import BytesIO, IoError

enum LoadError:
    Read(str)


def to_load_error(error: IoError) -> LoadError:
    return LoadError.Read(error.detail)


def load_chunks(data: bytes) -> Result[list[bytes], LoadError]:
    return BytesIO(data).chunks(65536).map_err(to_load_error).collect()
```

Use lazy `map`, `filter`, list-returning `flat_map`, `take`, `inspect`, `map_err`, and `inspect_err` when a loop pipeline should transform the stream. Use `collect` or `fold` when the whole stream should become one `Result`. See [Collection protocols](../stdlib_traits/collection_protocols.md#fallibleiterator-fallible-iteration) for exact adapter, terminal, setup-error, and retry behavior.

`BytesIO` is in-memory only. Use `std.tempfile.SpooledTemporaryFile` when a stream should start in memory and roll over to a temporary file after a size threshold.
