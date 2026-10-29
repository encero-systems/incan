# Configure the language server

Use an `incan-lsp` executable built for the same compiler line as your project. Toolchain archives managed by `incan toolchain` contain the native compiler closure; they do not install a language-server executable.

## Select an executable in the editor

For the VS Code or Cursor extension, set a literal executable path:

```json
{
  "incan.lsp.enabled": true,
  "incan.lsp.path": "/absolute/path/to/incan-lsp"
}
```

The path is passed directly to process creation. It does not expand shell variables, `~` or command substitutions. Leave it empty only when editor discovery or `PATH` selects the intended binary.

Open the editor's Incan output channel to verify the selected executable. Restart the language server after changing its path.

## Check source independently

```bash
incan check src/main.incn --format json
```

Compare these diagnostics with the editor's selected compiler line. Dev.8's `incan` has no `lsp` or `tools doctor` command.

Graph MCP is a separate stdio service:

```bash
incan mcp .
```

Closing its stdin ends the session. Its graph tools, resources and initialization protocol are specified in [Agent context CLI and MCP](../reference/agent_context_cli.md).
