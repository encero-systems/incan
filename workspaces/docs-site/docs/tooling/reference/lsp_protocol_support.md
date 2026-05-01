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

`textDocument/hover` includes checked API metadata previews for public declarations, selected public model/class members, and public enum variants after successful typechecking. Value enum hovers and completions include backing type and raw-value details where available. `workspace/executeCommand` command `incan.metadata.model.emit` emits contract-backed model source or bundle JSON for a selected project, bundle JSON file, or `.incnlib` artifact. There is currently no LSP command that returns the full checked API metadata JSON package; call `incan tools metadata api` for that.

To learn more about LSP, see the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
specification. See also: [LSP architecture](../explanation/lsp_architecture.md).
