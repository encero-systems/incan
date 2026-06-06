=== "Recommended: SDK install"

    ```bash
    curl -fsSL https://incan.pub/install.sh | sh
    export PATH="$HOME/.local/bin:$PATH"
    incan --version
    ```

    Notes:

    - The SDK installer links `incan` and `incan-lsp` into `~/.local/bin` by default.
    - If `incan` is not found, make sure `~/.local/bin` is on your `PATH`.

=== "Contributor: source checkout"

    ```bash
    make release
    ./target/release/incan --version
    ```
