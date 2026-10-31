# Standard primitives

`std.primitives` is an ordinary ambient standard package. Its authored source imports no other standard package and its manifest declares no dependencies. The current compiler still requires the implicit `incan_stdlib_core` provider, including for an integer identity function; a build with no other standard Loaf is not yet supported. Its first public operation is `hexlify(data: bytes) -> str`, which returns two lowercase hexadecimal characters per byte, preserving input order and returning an empty string for empty input. `std.encoding.hex` forwards to that implementation.

The package currently contains only hexadecimal encoding. Paths, binary I/O, hashing, environment reads and temporary directories remain in their existing standard packages until the toolchain seed can compile this package and its consumers.
