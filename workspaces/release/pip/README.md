# Incan SDK Python Installer

This package is a thin installer and command shim for the Incan SDK. It installs verified SDK archives from the shared Incan release manifest instead of building the compiler from Python packaging.

```bash
pipx install incan-sdk
incan --version
```

The command shims install the SDK into a package-local cache on first use. Set `INCAN_PIP_SDK_HOME`, `INCAN_PIP_BIN_DIR`, or `INCAN_SDK_MANIFEST` when you need a custom cache location or manifest.
