The supported release channels install the same Incan toolchain payload. Choose the command manager that already fits your environment:

=== "Direct installer"

    ```bash
    --8<-- "_snippets/commands/direct_install.sh"
    export PATH="$HOME/.local/bin:$PATH"
    incan --version
    incan-lsp --version
    ```

    This path verifies the release manifest and checksum and can provision the pinned Rust 1.98.0 backend through `rustup`.

=== "Homebrew"

    ```bash
    brew tap encero-systems/tap
    brew install incan
    incan --version
    ```

    Homebrew installs the prebuilt command binaries. Manage Rust separately.

=== "npm"

    ```bash
    npm install -g @incan/toolchain
    incan --version
    ```

    npm installs command shims without a lifecycle script; the first `incan` run provisions the checksum-verified toolchain for your host. Manage Rust and the `wasm32-wasip1` target separately.

=== "pipx"

    ```bash
    pipx install incan
    incan --version
    ```

    `pipx` keeps the command package isolated and routes installation through the shared release installer.

Native Windows and Linux arm64 are not supported by the current binary installer. Use WSL2 or a source build on those hosts.
