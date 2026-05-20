# Binary-Text Encoding in Incan

Use `std.encoding` when binary data must cross a text boundary: API payloads, identifiers, fixtures, addresses, logs, or files that need an ASCII representation of bytes.

```incan
from std.encoding import base32, base58, base64, base85, bech32, hex
```

`std.encoding` is not text decoding. Use `std.fs.Path.read_text()` and `write_text()` for UTF-8 files. It is also not compression, encryption, hashing, or media decoding. It only changes how bytes are represented.

## Choose a Format

| Format | Use it for | Watch for |
| --- | --- | --- |
| Hex | Debug output, digests, binary fixtures, protocols that require base16. | Output is twice as long as the input. |
| Base32 | Copyable uppercase tokens, DNS-like or case-sensitive environments, RFC 4648 protocols. | Standard Base32 and Base32hex are different alphabets. |
| Base64 | General API payloads, compact binary fields, common wire formats. | Use URL-safe helpers for URLs; do not silently swap alphabets. |
| Base85 | Denser ASCII output for protocols that name a Base85 variant. | Ascii85, Git-style Base85, and Z85 are not interchangeable. |
| Base58 | Human-facing identifiers that avoid visually ambiguous characters. | Plain Base58 has no checksum. Add checksum behavior in a separately named protocol helper. |
| Bech32 / Bech32m | Address-like strings with a human-readable prefix and checksum. | Payload data is five-bit words, not raw bytes. |

## Encode Bytes

Value helpers are the clearest option when the payload is already in memory.

```incan
from std.encoding import base64, hex

payload = b"hello?"

token = base64.b64encode(payload)
assert token == "aGVsbG8/"

fingerprint = hex.hexlify(payload)
assert fingerprint == "68656c6c6f3f"
```

Pick the helper whose name matches the external format. For example, URL components should use URL-safe Base64:

```incan
from std.encoding import base64

token = base64.urlsafe_b64encode(b"hello?")
assert token == "aGVsbG8_"
```

## Decode Strictly at Boundaries

Decoders return `Result[..., EncodingError]`, and strict decoding is the default. That is the right behavior for API boundaries, protocol parsers, signature inputs, and stored data.

```incan
from std.encoding import EncodingError, base64

def parse_api_token(text: str) -> Result[bytes, EncodingError]:
    return base64.urlsafe_b64decode(text)
```

Use `?` when the caller should receive the same `EncodingError`:

```incan
from std.encoding import EncodingError, base64

def token_payload_size(text: str) -> Result[int, EncodingError]:
    payload = base64.urlsafe_b64decode(text)?
    return Ok(len(payload))
```

## Use Lenient Decoders Only for Copied Text

Lenient helpers exist for formats with a normal interoperability rule, such as ignoring ASCII whitespace in copied or wrapped text. They are separate functions so leniency is visible at the call site.

```incan
from std.encoding import base64, hex

raw = base64.b64decode_lenient("aGVs bG8/\n")?
fingerprint = hex.decode_lenient("68 65 6c 6c 6f 3f")?
```

Lenient decoding is still validation. It does not accept another alphabet, repair arbitrary input, or ignore bad checksums.

## Move Data Through Files and Streams

The canonical source/sink helpers accept bytes, `std.io.BytesIO`, and `std.fs.Path`. A `Path` is treated as a finite binary source or sink, so file and stream usage follow the same transform path.

```incan
from std.encoding import EncodingError, base64
from std.fs import Path
from std.io import BytesIO

def write_payload_token(source: Path, target: Path) -> Result[str, EncodingError]:
    return base64.encode(source, target)

def encode_in_memory(data: bytes) -> Result[bytes, EncodingError]:
    source = BytesIO(data)
    target = BytesIO()
    base64.encode(source, target, chunk_size=8192)?
    return Ok(target.into_bytes())
```

Prefer these helpers over open-coded file reads when the task is "encode this source into that sink." Use `Path.read_bytes()` or `BytesIO` directly when the caller needs to inspect or transform the bytes before encoding.

## Handle Bech32 Payloads as Five-Bit Words

Bech32 and Bech32m are address formats, not generic byte streams. The encoded text contains a human-readable prefix, a separator, five-bit payload words, and a checksum. Convert byte-oriented data into five-bit words before encoding.

```incan
from std.encoding import EncodingError, bech32

def encode_address(hrp: str, raw_words: list[int]) -> Result[str, EncodingError]:
    words = bech32.convertbits(raw_words, 8, 5)?
    return bech32.bech32_encode(hrp, words)

def decode_address(address: str) -> Result[list[int], EncodingError]:
    hrp, words = bech32.bech32_decode(address)?
    return bech32.convertbits(words, 5, 8, pad=false)
```

Use `bech32m_encode` and `bech32m_decode` when the external protocol requires Bech32m. A valid Bech32 checksum should not be accepted as Bech32m, or the other way around.

## Avoid Format Shortcuts

- Do not auto-detect the format from the text shape. Require the caller or protocol to name it.
- Do not use lenient decoders for security-sensitive boundaries.
- Do not treat Base85 variants as interchangeable.
- Do not describe plain Base58 as checksum-protected.
- Do not make hash APIs return encoded text by default. Hashing produces bytes; callers can choose `hex`, `base64`, or
another encoding at the presentation boundary.

## See Also

- [std.encoding reference](../reference/stdlib/encoding.md)
- [std.io reference](../reference/stdlib/io.md)
- [std.fs reference](../reference/stdlib/fs.md)
- [Error handling](../explanation/error_handling.md)
