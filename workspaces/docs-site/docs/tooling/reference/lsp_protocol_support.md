# LSP protocol support

This page lists the Language Server Protocol methods `incan-lsp` serves and what each returns. The server communicates over stdio and identifies itself as `incan-lsp` with its version.

## Methods

| Method | Contract |
| --- | --- |
| `initialize`, `initialized`, `shutdown` | Lifecycle. `initialize` advertises the capabilities on this page. |
| `textDocument/didOpen` | Analyzes the document and publishes its diagnostics through `textDocument/publishDiagnostics`. |
| `textDocument/didChange` | Full-document sync: each change carries the whole text, which is analyzed again. |
| `textDocument/didClose` | Drops the document and publishes an empty diagnostic list for it. |
| `textDocument/hover` | See [Hover](#hover). |
| `textDocument/signatureHelp` | The signature of a function declared in the same document, at a call to it inside a function or method body. Triggered by `(` and `,`, and retriggered by `,`. |
| `textDocument/definition` | For an `@describe` decorator, the subject it describes; for a raw C call, its binding declaration; for a standard-library module named in an import, or the module of a decorator, that module's source; for a symbol of a library vocabulary, the `pub::` import that brings it into scope; for any other checked name, such as a local partial or its target, the declaration of its canonical identity. A token inside an embedded fragment has no definition. |
| `textDocument/references` | The locations of the checked declaration under the cursor across the open documents, matched by canonical identity; the declaration itself is included when the request asks for it. |
| `textDocument/documentSymbol` | The document outline, including module-level partial declarations. |
| `textDocument/completion` | Basic completions, including local partial names as function-like items. Triggered by `.`, `:` and `[`. |
| `textDocument/semanticTokens/full` | See [Semantic tokens](#semantic-tokens). |
| `workspace/executeCommand` | The one command `incan.metadata.model.emit`, which emits contract-backed model source or bundle JSON for a project, a bundle JSON file or a `.incnlib` artifact. Its arguments are in [Checked contract metadata](contract_metadata.md#lsp-command). Any other command returns an object with an `error`. |

No other method is advertised: `textDocument/semanticTokens/range` is advertised as unsupported, and semantic-token deltas, `textDocument/rename` and `textDocument/formatting` are not advertised.

## Hover

After a document type-checks, hover shows:

- for a public declaration, a public partial callable preset, a checked public method, a field of a public model or class, or a public enum variant: the [checked API metadata](checked_api_metadata.md) preview, with its checked signature, raw docstring, field visibility, alias and description metadata, value-enum backing type and raw value, derives, trait adoptions and safe const values. A private field's preview labels it private.
- for a partial: the projected callable signature, with presets shown as default parameters, and the target and preset provenance.
- for a decorated function whose checked binding is callable-valued: the callable signature the checked API metadata exposes.
- for a computed property, in hover and completion: its owner and return type, such as `property Account.total -> int`.
- for a value enum and its variants, in hover and completion: the backing type and raw values where available.
- for a canonical or supported-alias `@describe` decorator: the checked registry, key, descriptor and described subject.
- for a checked C binding declaration: its header, logical system-library capability, resources and release operations, symbols with their checked C signatures, enum carriers and plain structures; for a direct raw C call: its checked declaration and its `unsafe:` acknowledgment.

A document with parse or type errors keeps its diagnostics, and hover shows syntax-level details instead of checked metadata. No command returns the checked API metadata package; `incan tools metadata api` prints it.

## Semantic tokens

`textDocument/semanticTokens/full` classifies every token of the document with this legend. Its order is the index each token refers to.

| Legend | Entries, in order |
| --- | --- |
| Token types | `keyword`, `operator`, `string`, `number`, `comment`, `decorator`, `function`, `method`, `class`, `struct`, `enum`, `interface`, `type`, `typeParameter`, `parameter`, `property`, `variable`, `namespace`, `enumMember`, `regexp`, `macro` |
| Token modifiers | `declaration`, `definition`, `readonly`, `defaultLibrary` |

- A declaration's name takes the kind of what it declares.
- Type positions come from the parsed program.
- A member access is a `method` when it is called and a `property` when it is read.
- The expressions interpolated in an f-string are classified as code; the text around them is `string`.
- An embedded fragment, where a library's vocabulary claims another language's syntax inside Incan source, is classified as that submode; the Incan expressions inside the fragment are classified as Incan.
- A document that does not parse is classified from its token stream alone, without type positions or fragment ownership.

## See also

- [Incan Language Server (LSP)](../how-to/lsp.md)
- [LSP architecture](../explanation/lsp_architecture.md)
- The [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) specification
