# Coming from Python (apps)

This page routes Python developers who are evaluating Incan for application code, services, typed domain packages, and deployment-oriented tooling.

## Install first

If you use Python tooling day to day, `pipx` is the cleanest package-manager entrypoint because it keeps the command package isolated from project environments while still installing the verified Incan toolchain archive:

```bash
pipx install incan
incan --version
```

The direct installer is the same toolchain release path and is useful in shell scripts, CI images, and environments where you do not want another package manager involved:

```bash
curl -fsSL https://github.com/encero-systems/incan/releases/latest/download/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
incan --version
```

After installation, create a project and run the normal first-contact loop:

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

## What you should do next

- Install the toolchain and create a starter project: [Getting Started](../tooling/tutorials/getting_started.md)
- If anything fails: [Troubleshooting](../tooling/how-to/troubleshooting.md)
- Learn the basics: [The Incan Book (Basics)](../language/tutorials/book/index.md)
- Build your first API: [Build your first API](../language/tutorials/build_your_first_api.md)
- Multi-file apps: [Imports and modules (how-to)](../language/how-to/imports_and_modules.md)
- Write tests: [Testing in Incan](../language/how-to/testing_stdlib.md)
- Set up your workflow:
    - [Formatting](../tooling/how-to/formatting.md)
    - [Testing CLI](../tooling/how-to/testing.md)
    - [Editor setup](../tooling/how-to/editor_setup.md) (LSP, syntax highlighting)

## Explanation

- [Why Incan?](../language/explanation/why_incan.md)
- [How Incan works](../language/explanation/how_incan_works.md)

## Mental model translations (high level)

- **errors**: exceptions vs Result/Option (see: [Error Handling](../language/explanation/error_handling.md))
- **models**: dataclasses/Pydantic vs models/derives (see: [Models & Classes](../language/explanation/models_and_classes/index.md))
- **interfaces**: Protocols/ABCs vs traits/derives (see: [Derives & Traits](../language/reference/derives_and_traits.md))
- **async**: asyncio mental model mapping (see: [Async Programming](../language/how-to/async_programming.md))
