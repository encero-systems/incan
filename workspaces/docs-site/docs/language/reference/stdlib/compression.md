# std.compression reference

`std.compression` provides codec-based compression and decompression for byte payloads, `BytesIO` streams, and
`std.fs.File` handles.

```incan
from std.compression import gzip, decompress_auto, Codec
from std.io import BytesIO
```

Use explicit codec modules when the data format is known. Use autodetection only when the input may be one of several
framed formats and the caller is willing to handle a no-match error.

## Codec Modules

`std.compression` exposes these codec namespaces:

| Namespace | Format |
| --------- | ------ |
| `gzip` | gzip-wrapped deflate |
| `zlib` | zlib-wrapped deflate |
| `deflate` | raw deflate |
| `zstd` | zstd frames |
| `bz2` | bzip2 streams |
| `lzma` | XZ/LZMA-family streams |
| `snappy` | framed Snappy |
| `snappy.raw` | raw Snappy blocks for advanced interop |

Each top-level codec namespace exposes one-shot helpers:

```incan
compressed = gzip.compress(payload, level=None)?
plain = gzip.decompress(compressed)?
```

The default `snappy` namespace uses framed Snappy. Raw Snappy is available under `std.compression.snappy.raw`, but it is
not part of autodetection because raw blocks do not carry a reliable frame signature.

## Streaming

Every required codec module exposes stream helpers over `std.io.BytesIO` and `std.fs.File`:

```incan
from std.compression import zstd
from std.fs import Path

source = Path("events.jsonl.zst").open("rb")?
target = Path("events.jsonl").open("wb")?
zstd.decompress_stream(source, target)?
target.flush()?
```

`compress_stream(source, target, level=None, chunk_size=65536)` reads plain bytes from `source` and writes compressed
bytes to `target`. `decompress_stream(source, target, chunk_size=65536)` reads compressed bytes and writes plain bytes.

`chunk_size` must be positive. A non-positive value returns a `CompressionError` with `kind == "invalid_chunk_size"`.

## Levels

`level=None` selects the codec default.

| Codec | Supported levels |
| ----- | ---------------- |
| `gzip`, `zlib`, `deflate`, `bz2`, `lzma` | `0` through `9` |
| `zstd` | `-7` through `22` |
| `snappy`, `snappy.raw` | no configurable level |

Codecs with numeric levels return `CompressionError(kind="invalid_level", ...)` for out-of-range values. Snappy returns
`CompressionError(kind="unsupported_option", ...)` when a level is supplied.

## Autodetection

Autodetection is explicit and decompression-only:

```incan
codec, plain = decompress_auto(blob, allowed=[Codec.Gzip, Codec.Zstd])?
```

`decompress_auto(data, allowed=Codec.all())` returns `(Codec, bytes)`. `decompress_auto_stream(source, target,
allowed=Codec.all(), chunk_size=65536)` writes the decoded stream to `target` and returns the detected `Codec`.

The `allowed` list is binding. An empty list or a payload whose signature does not match any allowed codec returns
`CompressionError(kind="unsupported_codec", ...)`.

Autodetection uses signatures and framing bytes only. It does not inspect file extensions, paths, or MIME types. Raw
deflate and raw Snappy are not guessed because they do not have reliable framing signatures.

## Errors

Fallible helpers return `Result[..., CompressionError]`.

| Field | Meaning |
| ----- | ------- |
| `kind` | Stable category such as `invalid_data`, `truncated_input`, `unsupported_codec`, `unsupported_option`, `invalid_level`, `invalid_chunk_size`, `io`, or `backend`. |
| `codec` | The codec involved in the failure, when known. |
| `operation` | The operation that failed, such as `compress`, `decompress_stream`, or `decompress_auto_stream`. |
| `detail` | Backend or stdlib validation detail for diagnostics. |

```incan
match gzip.decompress(blob):
    Ok(plain) => println(len(plain))
    Err(err) => println(err.kind)
```

## Boundaries

`std.compression` does not provide archive containers such as ZIP or TAR, dictionary training APIs, authenticated
encryption, checksums, or password hashing. Those require separate APIs because their compatibility and security
contracts are different from byte compression.
