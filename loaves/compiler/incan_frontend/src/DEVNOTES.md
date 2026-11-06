# Frontend developer notes

The frontend checks parsed Incan modules, records semantic facts and constructs checked Body IR. The native driver lowers that representation through `incan_mir_lowering` and compiles it through rustc MIR. It does not generate Rust source.

## Source owners

- `loaves/kernel/incan_syntax/src/` owns the lexer, parser, AST and diagnostic catalog.
- `symbols.rs` owns lexical scopes and canonical declaration identities.
- `typechecker/` collects declarations, checks bodies and retains accepted semantic facts.
- `body_ir.rs` and `body_ir/` construct Body IR from those checked facts.
- `library_manifest/` defines the published Loaf interface.
- `provider/` owns the vocabulary provider contract; `incan_driver` loads providers.

Oven reads project manifests and passes unit descriptions to the native driver. Rust interop facts come from rustc metadata through ABI caches; see DD-0009 in `workspaces/docs-site/docs/design_records/0009_rust_facts_from_rustc_metadata.md`.

## Verification

Run the Rust facet's unit and integration tests through Oven:

```sh
incan test loaves/compiler/incan_frontend --profile release
```

Executable and refused-source contracts live in `workspaces/contracts/fixtures/`; their families and selectors are documented in `workspaces/contracts/README.md`. Extend the existing case that owns the behavior before adding one. Check a source project with `incan check PATH` and execute it with `incan run PATH`.

Read the contributor architecture and style guides before changing a pipeline boundary. Checked semantics belong in the frontend or Body IR; native lowering consumes those facts rather than reconstructing them.
