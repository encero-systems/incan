# Incan Language Support for VS Code / Cursor

Full syntax highlighting and language support for the Incan programming language (`.incn` files).

## Features

### Language Server (LSP)

The extension includes support for the Incan Language Server, providing:

- **Real-time diagnostics** - See errors and warnings as you type
- **Hover information** - View function signatures and type information
- **Go-to-definition** - Jump to function, model, class, and trait definitions
- **Autocomplete** - Keywords and symbols from your code

**Requirements:** The `incan-lsp` binary must be installed and available in your PATH, or you can configure the path in settings.

**Tip (development):** When your workspace is the Incan compiler repo, the extension will automatically prefer `target/debug/incan-lsp` (or `target/release/incan-lsp`) if present. This keeps diagnostics in sync with the syntax supported by your current checkout (e.g. `pub const`). Running `make build` at the repo root also symlinks `~/.cargo/bin/incan-lsp` (and `incan`) to `target/debug/...` on local machines, so other workspaces pick up the same binaries via PATH after you reload the window.

Run **Incan: Doctor** from the command palette to see which `incan` and `incan-lsp` binaries the extension resolved, whether configured paths exist and are executable, and whether `~/.cargo/bin/incan*` symlinks point at the expected checkout. The CLI counterpart is `incan tools doctor`.

### Syntax Highlighting

- **Function definitions** with parameters and return types

  ```incan
  def calculate(x: int, y: int) -> int:
  async def fetch_data(url: str) -> Result[str, Error]:
  ```

- **Class, Model, Trait, Enum declarations**

  ```incan
  class Animal extends Base with Debug:
  model User:
  trait Comparable:
  enum Shape:
  ```

- **Import statements** (both Python and Rust styles)

  ```incan
  from models import User, Product
  import utils::format_currency as fmt
  ```

- **F-string interpolation** with nested expressions

  ```incan
  f"Hello {user.name}, total: {format_currency(amount)}"
  ```

- **Decorators** with arguments

  ```incan
  @derive(Debug, Clone)
  @validate
  ```

- **Type annotations** throughout

  ```incan
  name: str
  items: List[Product]
  -> Result[User, Error]
  ```

- **Built-in functions** highlighted distinctly

  ```incan
  println, len, range, enumerate, zip, read_file, write_file
  ```

- **Method calls vs field access**

  ```incan
  user.name        # field access
  user.validate()  # method call
  ```

- **Constants** (ALL_CAPS)

  ```incan
  MAX_RETRIES = 3
  ```

### Editing Support

- **Auto-closing** pairs: `()`, `[]`, `{}`, `""`, `''`, `""""""`
- **Bracket matching**
- **Indentation-based folding** (Python-style)
- **Smart indentation** after `:` lines
- **Comment toggling** with `Ctrl+/` or `Cmd+/`

## Installation

### Option 1: Install from VSIX (Recommended for VS Code)

1. Build the VSIX package:

    ```bash
    cd workspaces/ide/vscode
    npm install
    npm run compile
    npm install -g @vscode/vsce
    vsce package
    ```

    This will create a file like `incan-0.1.0.vsix` in the `workspaces/ide/vscode` directory.

2. Open VS Code, go to the Extensions sidebar (Cmd+Shift+X), click the three-dot menu (…), and choose **Install from VSIX…**. Select your `.vsix` file.

3. Fully restart VS Code after installing.

4. Open a `.incn` file to verify highlighting and language features.

---

### Option 2: Symlink (Development, for Cursor or advanced users)

```bash
# For Cursor
ln -sf /path/to/incan/workspaces/ide/vscode ~/.cursor/extensions/incan-language

# For VS Code (only if you know the extension folder naming rules)
ln -sf /path/to/incan/workspaces/ide/vscode ~/.vscode/extensions/<name-from-package.json>
```

Make sure the symlink name matches the `name` field in `package.json`. Then restart your editor.

---

### Option 3: Copy Extension Folder (Development)

Copy the `workspaces/ide/vscode` folder to:

- **Cursor**: `~/.cursor/extensions/incan-language`
- **VS Code**: `~/.vscode/extensions/<name-from-package.json>`

Then restart your editor.

## LSP Setup

To enable language server features (diagnostics, hover, go-to-definition):

### 1. Build the LSP Server

```bash
cd /path/to/incan
cargo build --release --bin incan-lsp
```

### 2. Add to PATH

```bash
# Add to your shell profile (.bashrc, .zshrc, etc.)
export PATH="$PATH:/path/to/incan/target/release"
```

Or configure the path directly in VS Code/Cursor settings:

```json
{
  "incan.lsp.path": "/path/to/incan/target/release/incan-lsp"
}
```

### 3. Restart Editor

Restart VS Code/Cursor to activate the language server.

## Configuration

|       Setting       | Default |             Description             |
| ------------------- | ------- | ----------------------------------- |
| `incan.lsp.enabled` | `true`  | Enable/disable the language server  |
| `incan.lsp.path`    | `""`    | Literal path to the incan-lsp binary |

Path settings are literal executable paths. The extension does not expand `$HOME`, `~`, command substitutions, or other shell syntax in `incan.lsp.path` or `incan.compiler.path`.

## Scopes Reference

For theme authors, here are the TextMate scopes used:

|    Element    |                              Scope                              |
| ------------- | --------------------------------------------------------------- |
| Keywords      | `keyword.control.flow.incan`, `keyword.declaration.incan`       |
| Functions     | `entity.name.function.incan`, `entity.name.function.call.incan` |
| Methods       | `entity.name.function.method.incan`                             |
| Types         | `entity.name.type.incan`, `support.type.primitive.incan`        |
| Parameters    | `variable.parameter.function.incan`                             |
| Properties    | `variable.other.property.incan`                                 |
| Strings       | `string.quoted.double.incan`, `string.interpolated.incan`       |
| F-string expr | `meta.template.expression.incan`                                |
| Comments      | `comment.line.number-sign.incan`                                |
| Decorators    | `entity.name.function.decorator.incan`                          |
| Modules       | `entity.name.module.incan`                                      |
| Constants     | `constant.numeric.*.incan`, `constant.language.*.incan`         |
| Operators     | `keyword.operator.*.incan`                                      |

## Example

```incan
"""User management module"""

from models import User, Role
import utils::validate_email

@derive(Debug, Clone)
model UserProfile:
    user: User
    roles: List[Role]
    active: bool = true

def create_user(name: str, email: str) -> Result[User, str]:
    if not validate_email(email):
        return Err("Invalid email")
    
    user = User(name = name, email = email)
    println(f"Created user: {user.name}")
    return Ok(user)

async def fetch_users(limit: int = 10) -> List[User]:
    # Fetch from database
    pass
```

## Contributing

To modify keyword highlighting:

1. Update the registry-backed helpers in `incan_core::lang`
2. Run `cargo run --bin generate_vscode_grammar_keywords`
3. Reload VS Code/Cursor and test with files in `examples/`

For non-keyword grammar structure, update `incan.tmLanguage.json` directly and keep the generated keyword regexes in sync via the command above.

## License

Apache 2.0
