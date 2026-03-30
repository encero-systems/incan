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
cd /path/to/incan-programming-language
make build
```

This runs `cargo build --features lsp` and then links `~/.cargo/bin/incan` → `target/debug/incan` and `~/.cargo/bin/incan-lsp` → `target/debug/incan-lsp` when that binary exists. Set `INCAN_SKIP_CARGO_BIN_LINK=1` to skip linking, or rely on CI defaults (linking is off when `CI` is set).

After upgrading the compiler, **reload the editor window** (or restart the Incan language server) so the IDE spawns a fresh `incan-lsp` process.

### Alternative: release binary on `PATH`

```bash
cd /path/to/incan-programming-language
make lsp
```

Then add `target/release` to your `PATH`, or install into `~/.cargo/bin`:

```bash
cd /path/to/incan-programming-language
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

| Setting             | Default | Description                                   |
| ------------------- | ------- | --------------------------------------------- |
| `incan.lsp.enabled` | `true`  | Enable/disable the language server            |
| `incan.lsp.path`    | `""`    | Custom path to incan-lsp (uses PATH if empty) |

## Troubleshooting

### LSP Not Starting

1. **Check binary exists:**

      ```bash
      which incan-lsp
      # or
      incan-lsp --version
      ```

2. **Check VS Code output:**
      - View → Output → Select "Incan Language Server"

3. **Verify extension is active:**
      - Extensions panel → Search "Incan" → Check it's enabled

### No Diagnostics

- Ensure the file has `.incn` extension
- Check for syntax errors that prevent parsing
- Try reloading the window (Cmd/Ctrl + Shift + P → "Reload Window")

### Hover Not Working

- LSP must successfully parse the file first
- Check for diagnostics/errors in the file
- Ensure cursor is on a symbol (function name, type name, etc.)

## See also

- Architecture: [LSP architecture](../explanation/lsp_architecture.md)
- Reference: [LSP protocol support](../reference/lsp_protocol_support.md)
