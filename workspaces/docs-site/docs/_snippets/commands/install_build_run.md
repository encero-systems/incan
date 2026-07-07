=== "Recommended: toolchain install"

    ```bash
    --8<-- "_snippets/commands/direct_install.sh"
    export PATH="$HOME/.local/bin:$PATH"
    incan --version
    ```

    Notes:

    - The toolchain installer links `incan` and `incan-lsp` into `~/.local/bin` by default.
    - Homebrew, npm, and pipx install the same toolchain binaries through package-manager adapters.
    - If `incan` is not found, make sure `~/.local/bin` is on your `PATH`.

=== "Contributor: source checkout"

    ```bash
    make release
    ./target/release/incan --version
    ```
