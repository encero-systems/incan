# LSP protocol support

This page lists which LSP methods are currently implemented.

## Supported methods

| Method                        | Status                                    |
| ----------------------------- | ----------------------------------------- |
| `textDocument/didOpen`        | Supported                                 |
| `textDocument/didChange`      | Supported                                 |
| `textDocument/didClose`       | Supported                                 |
| `textDocument/hover`          | Supported                                 |
| `textDocument/definition`     | Supported                                 |
| `textDocument/documentSymbol` | Supported                                 |
| `textDocument/completion`     | Supported (basic)                         |
| `textDocument/references`     | Supported                                 |
| `textDocument/semanticTokens/full` | Supported                            |
| `workspace/executeCommand`    | Supported for `incan.metadata.model.emit` |
| `textDocument/semanticTokens/range` | Planned                             |
| `textDocument/rename`         | Planned                                   |
| `textDocument/formatting`     | Planned                                   |

`textDocument/hover` includes checked API metadata previews for public declarations, public partial callable presets, selected public model/class members, and public enum variants after successful typechecking. Hovering a canonical or supported-alias `@describe` decorator also shows the compiler-checked registry, key, descriptor, and described subject; go-to-definition on that decorator navigates to the described declaration. Partial hover displays the projected callable signature using the same default-parameter visual model as ordinary callables, plus target and preset provenance. Computed property hovers and completions include the owner and return type, such as `property Account.total -> int`. Value enum hovers and completions include backing type and raw-value details where available. `textDocument/completion` includes local partial names as function-like completion items, `textDocument/definition` resolves local partial names and target identifiers, and `textDocument/documentSymbol` lists module-level partial declarations. `workspace/executeCommand` command `incan.metadata.model.emit` emits contract-backed model source or bundle JSON for a selected project, bundle JSON file, or `.incnlib` artifact. There is currently no LSP command that returns the full checked API metadata JSON package; call `incan tools metadata api` for that.

`textDocument/semanticTokens/full` classifies every token in a file, which is what the protocol requires: a server publishes a legend of token types and answers with one entry per token, so there is no way to classify part of a file and leave the rest to the editor's own grammar. Declarations take the kind of the thing they declare, type positions are read from the parsed program rather than guessed from capitalisation, member access distinguishes a called method from a read property, and an f-string's interpolated expressions are classified as ordinary code rather than as string content. Embedded fragments (RFC 081) are highlighted as their own submode, so a markup tag or a style selector never renders as if it were an Incan local; the expression holes inside a fragment are ordinary Incan and are highlighted as such. A file that does not currently parse still receives highlighting from the token stream alone, losing only type positions and fragment ownership, so colour does not disappear while you type. Only the `full` request is served today; `textDocument/semanticTokens/range` and delta requests are not advertised.

`textDocument/rename` remains planned. The current server does not advertise rename support, so partial rename behavior is not exposed through LSP yet.

To learn more about LSP, see the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) specification. See also: [LSP architecture](../explanation/lsp_architecture.md).
