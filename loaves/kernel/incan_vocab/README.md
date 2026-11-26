# incan_vocab

This kernel Loaf holds vocabulary descriptor DTOs and the compiler's compatibility bindings. Vocabulary authors use the Incan standard package `std.vocab`; they do not write a Rust companion crate or depend on compiler-private syntax types.

`VOCAB_METADATA_VERSION` versions published registrations. `VOCAB_SYNTAX_VERSION` versions the public spanned syntax and native worker protocol. The frontend translates that published schema into compiler AST explicitly.

The `WasmModule` metadata tag remains readable only to issue a rebuild diagnostic. No WebAssembly runtime, export macro, memory ABI or execution helper is provided. Legacy Rust registration types remain descriptor compatibility APIs; they do not build executable vocabulary providers.
