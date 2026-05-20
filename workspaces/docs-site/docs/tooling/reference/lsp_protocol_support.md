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
| `workspace/executeCommand`    | Supported for `incan.metadata.model.emit` |
| `textDocument/references`     | Planned                                   |
| `textDocument/rename`         | Planned                                   |
| `textDocument/formatting`     | Planned                                   |

`textDocument/hover` includes checked API metadata previews for public declarations, public partial callable presets, selected public model/class members, and public enum variants after successful typechecking. Partial hover displays the projected callable signature using the same default-parameter visual model as ordinary callables, plus target and preset provenance. Computed property hovers and completions include the owner and return type, such as `property Account.total -> int`. Value enum hovers and completions include backing type and raw-value details where available. `textDocument/completion` includes local partial names as function-like completion items, `textDocument/definition` resolves local partial names and target identifiers, and `textDocument/documentSymbol` lists module-level partial declarations. `workspace/executeCommand` command `incan.metadata.model.emit` emits contract-backed model source or bundle JSON for a selected project, bundle JSON file, or `.incnlib` artifact. There is currently no LSP command that returns the full checked API metadata JSON package; call `incan tools metadata api` for that.

`textDocument/rename` remains planned. The current server does not advertise rename support, so partial rename behavior is not exposed through LSP yet.

To learn more about LSP, see the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) specification. See also: [LSP architecture](../explanation/lsp_architecture.md).
