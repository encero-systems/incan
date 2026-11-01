# Standard primitives

`std.primitives` is an ordinary ambient standard package. Its authored source imports no higher-level standard package. Required language/runtime support remains explicit in provider dependencies; source imports do not themselves prove an empty native closure.

The root `hexlify(data: bytes) -> str` operation returns lowercase hexadecimal text. `std.primitives.checksum` owns the `Crc32` class and `crc32` namespace for IEEE CRC-32, backed by the existing `crc32fast` native dependency. `std.checksum` reexports these declarations with the same type identity. Finalizers snapshot the checksum; subsequent updates continue the stream. CRC-32 detects accidental corruption and is not a security or typed-value identity mechanism.

Filesystem resources, binary I/O, other hashing algorithms, environment observations and temporary directories remain in their existing standard packages. This package does not implement database storage, a Crumb format or ownership features merely by providing these algorithms.
