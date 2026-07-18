# Incan Language Server (LSP)

The Incan Language Server provides IDE integration for real-time feedback while coding.

## Features

| Feature              | Description                                        |
| -------------------- | -------------------------------------------------- |
| **Diagnostics**      | Real-time errors, warnings, and hints as you type  |
| **Hover**            | View function signatures, types, and documentation |
| **Go-to-Definition** | Jump to symbol definitions (Cmd/Ctrl + Click)      |
| **Completions**      | Autocomplete for keywords and symbols              |

## Installation

### Recommended: build from a clone (CLI + LSP stay in sync)

From the Incan repository, a normal debug build updates both the compiler and the language server and, on local machines (not CI), symlinks them into `~/.cargo/bin` so your shell and editor see the same binaries:

```bash
cd /path/to/incan
make build
```

This runs `cargo build --features lsp` and then links `~/.cargo/bin/incan` → `target/debug/incan` and `~/.cargo/bin/incan-lsp` → `target/debug/incan-lsp` when that binary exists. Set `INCAN_SKIP_CARGO_BIN_LINK=1` to skip linking, or rely on CI defaults (linking is off when `CI` is set).

Use this path when you are developing the compiler itself or testing behavior from a checkout. It avoids the common split-brain state where your terminal runs one `incan` binary while VS Code or Cursor keeps launching an older `incan-lsp`.

Verify the shell side first:

```bash
command -v incan
command -v incan-lsp
ls -l ~/.cargo/bin/incan ~/.cargo/bin/incan-lsp
incan --version
```

The `command -v` output should resolve to `~/.cargo/bin/incan` and `~/.cargo/bin/incan-lsp` unless you deliberately configured absolute paths. The `ls -l` targets should point into the checkout you just built, usually `target/debug/incan` and `target/debug/incan-lsp`.

Then verify the editor side:

1. Open the Incan checkout or an Incan project in VS Code/Cursor.
2. Run **Incan: Doctor** from the command palette.
3. Open **View → Output** and select **Incan**.
4. Confirm the report shows the intended `incan` and `incan-lsp` paths, plus the source of each path (`setting`, `workspace`, or `path`).
5. Confirm the language server starts after opening a `.incn` file.
6. If diagnostics still look stale, run **Developer: Reload Window** from the command palette, or disable and re-enable the Incan extension so the editor starts a fresh `incan-lsp` process.

From a terminal, `incan tools doctor` prints the same local toolchain checks. Use `incan tools doctor --format json` when you need machine-readable output for an issue report or editor integration.

After upgrading the compiler or changing either binary path, reload the editor window or restart the Incan language server. Existing editor processes keep the executable they already launched; rebuilding on disk does not automatically replace a running language server.

### Alternative: release binary on `PATH`

```bash
cd /path/to/incan
make lsp
```

Then add `target/release` to your `PATH`, or install into `~/.cargo/bin`:

```bash
cd /path/to/incan
cargo install --path . --features lsp --bin incan-lsp --force
```

You can also use `make install-lsp` as a Makefile shortcut for the `cargo install` path.

### Install VS Code Extension

See [Editor Setup](editor_setup.md) for VS Code/Cursor extension installation.

## Usage

Once installed, the LSP activates automatically when you open `.incn` files.

### Real-time Diagnostics

Errors appear as you type with helpful hints:

```bash
type error: Type mismatch: expected 'Result[str, str]', found 'str'
  --> file.incn:8:5

note: In Incan, functions that can fail return Result[T, E]
hint: Wrap the value with Ok(...) to return success
```

### Hover Information

Hover over any symbol to see its type:

```incan
def process(data: List[str]) -> Result[int, Error]
```

When the file type-checks successfully, hover also previews checked public API metadata for public declarations, public partial callable presets, fields on public models/classes, checked public methods, and public enum variants. These previews use the same checked metadata extractor as `incan tools metadata api`, so they can include raw docstrings, checked signatures, partial target/preset provenance, field visibility and aliases/descriptions, enum backing values, derives, trait adoption, and safe const values. Private class fields are labeled private in their field preview without becoming accessible outside the declaring class. For value enums, enum and variant hover details show the backing type and variant raw value where available.

If the file has parse or type errors, diagnostics remain the source of truth and the LSP falls back to syntax-oriented hover details. The current LSP does not expose a workspace command for fetching the full checked API metadata JSON package from the editor; use `incan tools metadata api` for that.

### Contract model emit command

Editor integrations can call `workspace/executeCommand` command `incan.metadata.model.emit` to emit a contract-backed model from the same checked metadata used by the CLI:

```json
{
  "command": "incan.metadata.model.emit",
  "arguments": [
    {
      "uri": "file:///path/to/project/src/main.incn",
      "model": "OrderSummary",
      "format": "incan"
    }
  ]
}
```

The command accepts a source URI inside a project, a project path, a bundle JSON file, or a `.incnlib` artifact path. `model` may be the logical type name or stable model id. `format` is `incan` or `json`.

### Go-to-Definition

- **VS Code/Cursor**: Cmd+Click (macOS) or Ctrl+Click (Windows/Linux)
- **Keyboard**: F12 or Ctrl+Click

Works for:

- Functions
- Models
- Classes
- Traits
- Enums
- Newtypes

### Completions

Trigger completions with Ctrl+Space or by typing:

- `.` for field/method access
- `:` for type annotations

Suggestions include:

- Incan keywords (`def`, `model`, `class`, etc.)
- Symbols from current file
- Built-in types (`Result`, `Option`, etc.)

## Configuration

### VS Code Settings

```json
{
  "incan.lsp.enabled": true,
  "incan.lsp.path": "/path/to/incan-lsp"
}
```

| Setting             | Default | Description                                                                                      |
| ------------------- | ------- | ------------------------------------------------------------------------------------------------ |
| `incan.lsp.enabled` | `true`  | Enable/disable the language server                                                               |
| `incan.lsp.path`    | `""`    | Literal path to `incan-lsp`; when empty, the extension uses workspace binary discovery or `PATH` |

`incan.lsp.path` is not a shell command. It is passed directly to the editor's language-client process launcher, so it does not expand `$HOME`, `~`, command substitutions, or other shell syntax. Use a concrete executable path:

```json
{
  "incan.lsp.path": "/path/to/incan/target/debug/incan-lsp"
}
```

Avoid shell-style values:

```json
{
  "incan.lsp.path": "$HOME/dev/incan/target/debug/incan-lsp"
}
```

For most local development, leave `incan.lsp.path` empty and let `make build` keep `~/.cargo/bin/incan-lsp` pointed at the checkout. Set an explicit absolute path only when you intentionally want that workspace to use a specific binary.

The extension validates configured paths before starting the language server. If `incan.lsp.path` or `incan.compiler.path` contains shell syntax, points at a missing file, or points at a non-executable file, the extension writes the problem to the **Incan** output channel and shows a warning.

## Troubleshooting

### LSP Not Starting

1. **Check binary exists:**

      ```bash
      which incan-lsp
      ls -l "$(which incan-lsp)"
      ```

2. **Check configured path:**
      - If `incan.lsp.path` is set, make sure it is a literal executable path, not `$HOME/...`, `~/...`, or another shell expression.
      - If it is empty, the extension first tries a workspace-built `target/debug/incan-lsp` or `target/release/incan-lsp`, then falls back to `incan-lsp` from `PATH`.

3. **Run the doctor command:**
      - Command palette → "Incan: Doctor"
      - Or from a terminal: `incan tools doctor`

4. **Check VS Code output:**
      - View → Output → Select "Incan"

5. **Verify extension is active:**
      - Extensions panel → Search "Incan" → Check it's enabled

### No Diagnostics

- Ensure the file has `.incn` extension
- Check for syntax errors that prevent parsing
- Try reloading the window (Cmd/Ctrl + Shift + P → "Reload Window")

### Hover Not Working

- LSP must successfully parse the file first
- Check for diagnostics/errors in the file
- Ensure cursor is on a symbol (function name, type name, etc.)
- Checked API metadata hover requires a successful typecheck and only applies to public API declarations or selected public model/class members

## See also

- Architecture: [LSP architecture](../explanation/lsp_architecture.md)
- Reference: [LSP protocol support](../reference/lsp_protocol_support.md)
