# Standard primitives

`std.primitives` is an ordinary ambient standard package. Its authored source imports no higher-level standard package. Required language/runtime support remains explicit in provider dependencies; source imports do not themselves prove an empty native closure.

`std.primitives.vocab` owns vocabulary descriptors and the structured request, response and error contracts used by authored transformations. Its transport derives use the package's declared native `serde` dependencies directly; naming a descriptor does not import `std.vocab`, the data standard package or compiler implementation. `std.vocab` reexports these declarations with the same nominal identity and adds metadata publication plus the executable worker loop.

The root `hexlify(data: bytes) -> str` operation returns lowercase hexadecimal text. `std.primitives.checksum` owns the `Crc32` class and `crc32` namespace for IEEE CRC-32, backed by the existing `crc32fast` native dependency. `std.checksum` reexports these declarations with the same type identity. Finalizers snapshot the checksum; subsequent updates continue the stream. CRC-32 detects accidental corruption and is not a security or typed-value identity mechanism.

`std.primitives.environ.read(key)` returns the present Unicode value or its exact `EnvironErrorKind`. Empty strings remain present values; missing variables, invalid keys and non-Unicode values remain distinct. `std.environ` reexports the enum and adds its existing error context, defaults and typed conversions. Observed values are never included in observation errors.

`std.primitives.hmac` owns the existing `HmacSha256Signer` class and `hmac_sha256` namespace. The old `std.hash` imports reexport those same declarations. Finalizing a signer resets its message state while retaining its key; verification uses the existing native verifier. The implementation depends on the `hmac` and `sha2` crates and the explicitly supplied native error runtime, without importing higher standard-package declarations.

`std.primitives.io` owns the shared `IoError` and `Endian` declarations and a growable `WriteBuffer`. The buffer owns its storage and exposes copying only through `snapshot()`. `std.io` reexports the two declarations so existing imports retain one error and byte-order identity.

`std.primitives.fs` owns simple paths, file handles, advisory lock guards and retained mapped views. Bounded and remaining-byte reads report host failures directly; higher-level filesystem mode policy and text-file conveniences stay in `std.fs`.

`std.primitives.text` copies borrowed UTF-8 text into owned bytes and decodes borrowed bytes into owned text with strict or replacement behavior. `std.primitives.native_strings` builds independently owned NUL-terminated UTF-8 storage only after rejecting every interior NUL. Builtin codecs retain their label ordering and structured error adaptation, while checked C lowering retains foreign-call admission and the buffer for the complete native call.

`std.primitives.collections` owns list, set and dictionary storage allocation, mutation, length observation and explicit copying. Its checked list-position operations accept nonnegative native positions and return ordinary errors; language index normalization, equality, display and canonical diagnostics remain in their existing adapters. Generated programs keep using the language runtime's own operations, because the runtime builds below the standard packages.

The `incan_std_primitives` facet owns the native carriers and operations that compiler-emitted MIR still expresses through Rust types: panic-hosted errors, Rust strings, numeric carriers, Rust collections and iterators, frozen program-lifetime values, compiler-emitted reflection metadata, and the builtin validation-error carrier. The reflection and validation modules remain Rust because the rustc MIR adapter must name and construct their concrete carrier layouts; their public contracts and control flow remain Incan-authored. The Incan runtime and higher standard packages call this one canonical facet.

`std.primitives.output` writes complete UTF-8 lines and returns actual host write failures as text without depending on higher-level I/O errors. Generated stdout and panic-hook stderr keep their own paths in the language runtime.

`std.primitives` owns the structured telemetry values, logger types and root `log` static used by ambient logging. `std.logging` reexports those declarations, and the standard prelude reexports the same static identity. The primitive logger uses the package's declared native serialization, clock and output dependencies directly; it imports no higher standard package. Merely loading the prelude does not make a program that never reads `log` reach the logger implementation.

Other hashing algorithms, argument-vector policy and temporary directories remain in their existing standard packages. This package does not implement database storage, a Crumb format or ownership features merely by providing these algorithms.
