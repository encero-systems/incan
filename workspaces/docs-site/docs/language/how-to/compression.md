# Compress and Decompress Data

Use `std.compression` when a program needs to read or write compressed byte payloads, compressed files, or codec-framed data from another system.

```incan
from std.compression import Codec, CompressionError, decompress_auto, gzip, zstd
from std.io import BytesIO
```

Compression is not archive handling. Use `std.compression` for byte streams such as gzip, zstd, bzip2, XZ/LZMA, or Snappy. Archive containers such as ZIP and TAR have entry names, permissions, directory traversal risks, and extraction rules that belong in archive-specific APIs.

## Choose the Codec at the Boundary

Prefer an explicit codec whenever the format is known from a protocol, file extension, header, configuration value, or caller contract.

| Situation | Prefer |
| --- | --- |
| HTTP-style payloads or `.gz` files | `gzip` |
| zlib-wrapped deflate from older protocols | `zlib` |
| raw deflate blocks from a protocol that says "deflate" explicitly | `deflate` |
| data pipelines and log files that want high compression and fast decode | `zstd` |
| existing `.bz2` files | `bz2` |
| existing `.xz` / LZMA-family files | `lzma` |
| framed Snappy streams | `snappy` |
| raw Snappy blocks required by a storage format | `snappy.raw` |

Do not silently try codecs in a loop. If the format is ambiguous, use the explicit autodetection helpers so the policy is visible at the call site.

## Compress Bytes Already in Memory

Use one-shot helpers when the payload is already in memory and small enough to keep there.

```incan
from std.compression import CompressionError, gzip

def encode_payload(payload: bytes) -> Result[bytes, CompressionError]:
    return gzip.compress(payload, level=None)

def decode_payload(payload: bytes) -> Result[bytes, CompressionError]:
    return gzip.decompress(payload)
```

`level=None` uses the codec default. Pass a level only when the caller has a reason to trade compression speed for output size.

```incan
from std.compression import CompressionError, zstd

def archive_payload(payload: bytes) -> Result[bytes, CompressionError]:
    return zstd.compress(payload, level=Some(10))
```

Keep the compressed value typed as `bytes`. If it needs to cross a text-only boundary, encode it afterwards with `std.encoding`.

## Stream Files Instead of Loading Them

Use stream helpers for files and pipeline stages. They move bytes between `std.fs.File` and `std.io.BytesIO` without requiring the complete input to be materialized first.

```incan
from std.compression import zstd
from std.fs import Path

source = Path("events.jsonl").open("rb")?
target = Path("events.jsonl.zst").open("wb")?
zstd.compress_stream(source, target, level=Some(3), chunk_size=65536)?
target.flush()?
```

Decompression is the same shape:

```incan
from std.compression import zstd
from std.fs import Path

source = Path("events.jsonl.zst").open("rb")?
target = Path("events.jsonl").open("wb")?
zstd.decompress_stream(source, target, chunk_size=65536)?
target.flush()?
```

Choose a positive `chunk_size`. The default works for normal file workflows. Smaller chunks are useful in tests and latency-sensitive pipelines; larger chunks may reduce overhead for large local files.

## Use BytesIO for In-Memory Pipelines

`BytesIO` is useful when a pipeline step expects a stream but the caller starts with bytes.

```incan
from std.compression import CompressionError, gzip
from std.io import BytesIO

def stream_compress_for_response(payload: bytes) -> Result[bytes, CompressionError]:
    target = BytesIO()
    gzip.compress_stream(BytesIO(payload), target, level=None, chunk_size=8192)?
    return Ok(target.getvalue())
```

This keeps code shaped like the file-streaming path while still returning a byte payload to the caller.

## Autodetect Only When the Input Is Genuinely Mixed

Use `decompress_auto` when a boundary may receive several framed compression formats and the caller cannot know which one ahead of time.

```incan
from std.compression import Codec, CompressionError, decompress_auto

def decode_upload(payload: bytes) -> Result[bytes, CompressionError]:
    codec, plain = decompress_auto(payload, [Codec.Gzip, Codec.Zstd, Codec.Bz2])?
    return Ok(plain)
```

Keep the `allowed` list narrow. It documents the formats the boundary accepts and prevents unexpected codec behavior.

For streamed input, use `decompress_auto_stream`:

```incan
from std.compression import Codec, decompress_auto_stream
from std.fs import Path

source = Path("payload.bin").open("rb")?
target = Path("payload.out").open("wb")?
codec = decompress_auto_stream(source, target, [Codec.Gzip, Codec.Zstd], chunk_size=65536)?
target.flush()?
println(codec)
```

Autodetection uses signatures and framing bytes. It does not inspect file extensions, paths, or MIME types. Raw deflate and raw Snappy are not guessed because they do not have reliable frame signatures.

## Handle Compression Errors at the Same Boundary

Compression helpers return `Result[..., CompressionError]`. Match the error when the caller can recover or report a specific category.

```incan
from std.compression import gzip

match gzip.decompress(payload):
    Ok(plain) => println(len(plain))
    Err(err) => println(err.kind)
```

Common categories include `invalid_data`, `truncated_input`, `unsupported_codec`, `unsupported_option`, `invalid_level`, `invalid_chunk_size`, `io`, and `backend`.

## Keep Compression Separate from Related Work

- Use `std.encoding` after compression when bytes need a text-safe representation.
- Use hashing after compression only when the digest must cover the compressed bytes. Hash before compression when the
digest must cover the original payload.
- Keep password hashing and encryption separate from compression. They have different security contracts.
- Do not use raw Snappy unless another format specifically requires block-level Snappy behavior.

## See Also

- [`std.compression` reference](../reference/stdlib/compression.md)
- [Binary-text encoding](binary_text_encoding.md)
- [File I/O](file_io.md)
- [`std.io` reference](../reference/stdlib/io.md)
- [`std.fs` reference](../reference/stdlib/fs.md)
