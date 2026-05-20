# std.encoding reference

`std.encoding` provides binary-to-text encoding and decoding helpers for interoperable representation formats.

```incan
from std.encoding import base32, base58, base64, base85, bech32, hex
```

The module is an ordinary Incan stdlib source surface. Public APIs should be authored in `.incn` modules, imported explicitly, and kept free of Rust-backed public shells. Implementations may use existing builtin byte and string operations, but the user-facing contract is the Incan `std.encoding` namespace.

Use `std.encoding` when bytes need a text-safe representation for APIs, identifiers, signatures, fixtures, transport payloads, or storage boundaries. It does not guess formats from payload shape and does not cover compression, archives, images, audio, or video.

## Error model

Decode failures return `Result[..., EncodingError]`. Strict decoding is the default for every format.

| Error kind | Meaning |
| --- | --- |
| `invalid_character` | Input contains a character that is not part of the selected alphabet. |
| `invalid_length` | Input length cannot represent complete encoded data. |
| `invalid_padding` | Padding is missing, misplaced, or present where the variant forbids it. |
| `invalid_checksum` | A checksum-bearing format, such as Bech32, failed validation. |

`EncodingError.message()` returns user-facing text suitable for diagnostics. Position-aware implementations should include the offending byte or character offset when it can be reported deterministically.

## Strict and lenient decoding

Decode functions are strict unless the function name says otherwise.

Lenient decoding uses separate names instead of boolean flags. Lenient mode is only available where the format has a clear interoperability convention, such as ignoring ASCII whitespace in copied base64 or hex.

Malformed alphabet characters, structurally impossible lengths, and bad checksums still fail in lenient mode.

## Hex

`std.encoding.hex` provides base16 text for byte buffers.

| API | Returns | Description |
| --- | --- | --- |
| `hex.encode(source, target, chunk_size: int = 65536)` | `Result[str, EncodingError]` | Lowercase hexadecimal text from `bytes`, `BytesIO`, or `Path`. |
| `hex.decode(source, target, chunk_size: int = 65536)` | `Result[bytes, EncodingError]` | Strict decode of even-length hex text from `str`, `BytesIO`, or `Path`. |
| `hex.decode_lenient(text: str)` | `Result[bytes, EncodingError]` | Decode while accepting ASCII whitespace. |

## Base32

`std.encoding.base32` provides RFC 4648 style base32 helpers. The standard alphabet is uppercase by default.

| API | Returns | Description |
| --- | --- | --- |
| `base32.b32encode(data: bytes)` | `str` | Standard padded base32 text. |
| `base32.b32decode(text: str)` | `Result[bytes, EncodingError]` | Strict standard base32 decode. |
| `base32.b32decode_lenient(text: str)` | `Result[bytes, EncodingError]` | Decode while accepting ASCII whitespace and lowercase input. |
| `base32.b32hexencode(data: bytes)` | `str` | Extended hex alphabet base32 text. |
| `base32.b32hexdecode(text: str)` | `Result[bytes, EncodingError]` | Strict extended hex alphabet decode. |
| `base32.encode(source, target, chunk_size: int = 65536)` | `Result[str, EncodingError]` | Standard Base32 from `bytes`, `BytesIO`, or `Path`. |
| `base32.decode(source, target, chunk_size: int = 65536)` | `Result[bytes, EncodingError]` | Strict standard Base32 from `str`, `BytesIO`, or `Path`. |
| `base32.b32hexencode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream Base32hex output. |
| `base32.b32hexdecode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream strict Base32hex input. |

Use the hex-alphabet functions when an external protocol requires that variant. Do not pass hidden alphabet flags to a generic decoder.

## Base64

`std.encoding.base64` provides standard and URL-safe base64 functions. The variant is part of the function name.

| API | Returns | Description |
| --- | --- | --- |
| `base64.b64encode(data: bytes)` | `str` | Standard alphabet with padding. |
| `base64.b64decode(text: str)` | `Result[bytes, EncodingError]` | Strict standard alphabet decode. |
| `base64.b64decode_lenient(text: str)` | `Result[bytes, EncodingError]` | Decode while accepting ASCII whitespace. |
| `base64.urlsafe_b64encode(data: bytes)` | `str` | URL-safe alphabet with padding. |
| `base64.urlsafe_b64decode(text: str)` | `Result[bytes, EncodingError]` | Strict URL-safe decode. |
| `base64.urlsafe_b64decode_lenient(text: str)` | `Result[bytes, EncodingError]` | Decode URL-safe text while accepting ASCII whitespace. |
| `base64.encode(source, target, chunk_size: int = 65536)` | `Result[str, EncodingError]` | Standard Base64 from `bytes`, `BytesIO`, or `Path`. |
| `base64.decode(source, target, chunk_size: int = 65536)` | `Result[bytes, EncodingError]` | Strict standard Base64 from `str`, `BytesIO`, or `Path`. |
| `base64.urlsafe_b64encode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream URL-safe base64 output. |
| `base64.urlsafe_b64decode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream strict URL-safe base64 input. |

## Base85

`std.encoding.base85` keeps the major base85 variants separate.

| API | Returns | Description |
| --- | --- | --- |
| `base85.a85encode(data: bytes)` | `str` | Ascii85 text. |
| `base85.a85decode(text: str)` | `Result[bytes, EncodingError]` | Strict Ascii85 decode. |
| `base85.b85encode(data: bytes)` | `str` | Git-style base85 text. |
| `base85.b85decode(text: str)` | `Result[bytes, EncodingError]` | Strict Git-style base85 decode. |
| `base85.z85encode(data: bytes)` | `Result[str, EncodingError]` | Z85 text; input length must be a multiple of 4. |
| `base85.z85decode(text: str)` | `Result[bytes, EncodingError]` | Strict Z85 decode; input length must be a multiple of 5. |
| `base85.a85encode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream ASCII85 output. |
| `base85.a85decode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream strict ASCII85 input. |
| `base85.encode(source, target, chunk_size: int = 65536)` | `Result[str, EncodingError]` | Git-style Base85 from `bytes`, `BytesIO`, or `Path`. |
| `base85.decode(source, target, chunk_size: int = 65536)` | `Result[bytes, EncodingError]` | Strict Git-style Base85 from `str`, `BytesIO`, or `Path`. |
| `base85.z85encode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream Z85 output. |
| `base85.z85decode_stream(source, target, chunk_size: int = 65536)` | `Result[None, EncodingError]` | Stream strict Z85 input. |

Do not treat base85 variants as interchangeable. They use different alphabets and framing rules.

## Base58

`std.encoding.base58` provides Bitcoin-alphabet base58.

| API | Returns | Description |
| --- | --- | --- |
| `base58.b58encode(data: bytes)` | `str` | Bitcoin alphabet base58 text. |
| `base58.b58decode(text: str)` | `Result[bytes, EncodingError]` | Strict Bitcoin alphabet decode. |
| `base58.encode(source, target, chunk_size: int = 65536)` | `Result[str, EncodingError]` | Bitcoin Base58 from `bytes`, `BytesIO`, or `Path`. |
| `base58.decode(source, target, chunk_size: int = 65536)` | `Result[bytes, EncodingError]` | Strict Bitcoin Base58 from `str`, `BytesIO`, or `Path`. |

Base58 is not a checksum format by itself. Protocols that add checksums should expose separately named helpers rather than weakening plain `b58decode`.

## Bech32 and Bech32m

`std.encoding.bech32` provides checksum-bearing Bech32 helpers for address-like text.

| API | Returns | Description |
| --- | --- | --- |
| `bech32.bech32_encode(hrp: str, data: list[int])` | `Result[str, EncodingError]` | Encode a human-readable prefix and five-bit data words using Bech32. |
| `bech32.bech32_decode(text: str)` | `Result[tuple[str, list[int]], EncodingError]` | Strict Bech32 decode with checksum validation. |
| `bech32.bech32m_encode(hrp: str, data: list[int])` | `Result[str, EncodingError]` | Encode five-bit data words using Bech32m checksum rules. |
| `bech32.bech32m_decode(text: str)` | `Result[tuple[str, list[int]], EncodingError]` | Strict Bech32m decode with checksum validation. |

Bech32 and Bech32m are distinct variants. Use the helper that matches the external protocol. Use `convertbits` when converting byte-oriented payloads into five-bit data words.

## Source/Sink I/O

Canonical `encode` and `decode` functions compose with `std.io.BytesIO` and `std.fs.Path`. A `Path` is treated as a finite binary source or sink: the function opens it and uses the same source/sink transform path as in-memory streams. Hex, Base32, Base64, Base85, and Base58 expose source/sink helpers; Bech32 remains a word-oriented checksum API instead of a byte stream codec.

Source/sink functions do not insert line wrapping by default. MIME-style wrapped output must use a clearly named helper or option when that surface exists.

## See also

- [Binary-text encoding how-to](../../how-to/binary_text_encoding.md)
- [std.io reference](io.md)
- [std.fs reference](fs.md)
