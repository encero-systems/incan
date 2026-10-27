# List iteration memory

Each measurement starts this executable in a fresh child process. Flat and nested lists contain the same 2,097,152 strings; each lazy form must stay within 25% plus 16 MiB of the corresponding direct-loop baseline. Reading the source after consuming it also guards the source's continued readability. The nested check permits one yielded row to be materialized, but excludes a copy of the whole nested source.

The test-local Rust bridge only reads `getrusage`'s resident high-water mark. Calling `libc::getrusage` or `std::mem::zeroed` from Incan is refused with “RFC 005 excludes unsafe Rust calls; `unsafe:` acknowledges checked C bindings only.” This is a limitation of [RFC 005](../../../../../docs-site/docs/RFCs/closed/implemented/005_rust_interop.md), tracked for this contract by #2426; the bridge is removable when Incan has a safe host resource API. The measured allocation and iteration are authored in Incan.
