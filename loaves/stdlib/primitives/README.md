# Standard primitives

`std.primitives` is an ordinary ambient standard package. Its authored source imports no higher-level standard package. Required language/runtime support remains explicit in provider dependencies; source imports do not themselves prove an empty native closure.

The root `hexlify(data: bytes) -> str` operation returns lowercase hexadecimal text. `std.primitives.checksum` owns the `Crc32` class and `crc32` namespace for IEEE CRC-32, backed by the existing `crc32fast` native dependency. `std.checksum` reexports these declarations with the same type identity. Finalizers snapshot the checksum; subsequent updates continue the stream. CRC-32 detects accidental corruption and is not a security or typed-value identity mechanism.

`std.primitives.environ.read(key)` returns the present Unicode value or its exact `EnvironErrorKind`. Empty strings remain present values; missing variables, invalid keys and non-Unicode values remain distinct. `std.environ` reexports the enum and adds its existing error context, defaults and typed conversions. Observed values are never included in observation errors.

`std.primitives.hmac` owns the existing `HmacSha256Signer` class and `hmac_sha256` namespace. The old `std.hash` imports reexport those same declarations. Finalizing a signer resets its message state while retaining its key; verification uses the existing native verifier. The implementation depends on the `hmac` and `sha2` crates and the explicitly supplied native error runtime, without importing higher standard-package declarations.

Filesystem resources, binary I/O, other hashing algorithms, argument-vector observations and temporary directories remain in their existing standard packages. This package does not implement database storage, a Crumb format or ownership features merely by providing these algorithms.
