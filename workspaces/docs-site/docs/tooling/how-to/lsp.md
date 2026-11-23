# Configure the language server

Use an `incan-lsp` executable built for the same compiler line as your project. Toolchain archives managed by `incan toolchain` contain the native compiler closure; they do not install a language-server executable. The server selects `--profile debug` by default; start it with `--profile release` when the installation provides standard packages and native support for the release profile only.

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

## Review compile-time trust stops

The language server prepares projects in unattended mode. When a dependency would run a procedural macro, vocabulary registration, or a vocabulary worker without covering user or organization trust, the editor continues code-free analysis and shows `INCAN-T0134` as an informational diagnostic. Its vocabulary blocks remain not yet checked; no provider process starts for them, including from unsaved text.

Review the named provider before accepting the diagnostic's quick fix. The editor shows the exact publisher, Loaf, or project entry proposed by Oven and asks for confirmation before it runs `oven trust add`. The command changes user configuration, not the project. Reanalyze or reopen the document after adding trust so Oven can prepare the provider and the language server can expand the current buffer.

To inspect or reverse the resulting entry outside the editor, use `oven trust list` or `oven trust remove KIND VALUE`. An explicit `incan check`, build, run, or test remains an explicit request and is not widened by editor trust.

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
