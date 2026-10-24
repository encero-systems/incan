# Declare library vocabulary metadata

Dev.8 accepts a vocabulary companion declaration in a library's `loaf.toml`:

```toml
[project]
name = "query_library"
version = "0.1.0"

[vocab]
crate = "vocab_companion"
```

Oven validates the table and its supported key. It does not build the companion, compile a Wasm desugarer or package one automatically. Do not add a companion build command to an application workflow.

## Check an existing library

Use an installed vocabulary-bearing artifact when checking source that imports its syntax. The formatter also needs that vocabulary to parse custom forms.

```bash
incan check src/lib.incn
incan build src/lib.incn
```

These commands check and build the Incan library; they do not provision a desugarer. Source-level DSL contracts and the library manifest format are defined by the checked library artifacts and the kernel vocabulary interfaces.
