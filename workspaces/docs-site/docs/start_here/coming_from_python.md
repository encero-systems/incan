# Coming from Python (apps)

This page routes Python developers who are evaluating Incan for application code, services, typed domain packages, and deployment-oriented tooling.

<aside class="inc-bridge-note inc-incus-slot" data-incus-category="python" aria-label="Python to Incan mental model">
  <span class="inc-eyebrow">Python → Incan</span>
  <strong>Keep the readable application-code shape. Move more failure, type, and deployment facts into contracts the compiler can check.</strong>
</aside>

## Install first

If you use Python tooling day to day, `pipx` is the cleanest package-manager entrypoint because it keeps the command package isolated from project environments while still installing the verified Incan toolchain archive. The installer also provisions the Rust backend when needed, so a fresh machine can move straight from install to `incan run`:

```bash
pipx install incan
incan --version
```

The direct installer is the same toolchain release path and is useful in shell scripts, CI images, and environments where you do not want another package manager involved:

```bash
--8<-- "_snippets/commands/direct_install.sh"
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

<div class="inc-route-grid">
  <a class="inc-route-card" href="../../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Start</span><strong>Install and run</strong><span>Create a starter project and complete the normal run, test, and release-build loop.</span></a>
  <a class="inc-route-card" href="../../language/tutorials/book/"><span class="inc-eyebrow">Learn</span><strong>Read the basics</strong><span>Work through the language in short, sequential chapters with runnable exercises.</span></a>
  <a class="inc-route-card" href="../../language/tutorials/build_your_first_api/"><span class="inc-eyebrow">Build</span><strong>Create an API</strong><span>Run the built-in web framework and serve typed JSON endpoints.</span></a>
  <a class="inc-route-card" href="../../language/how-to/imports_and_modules/"><span class="inc-eyebrow">Scale out</span><strong>Use modules</strong><span>Move from a single file to multi-file applications and module-owned state.</span></a>
</div>

Then add [tests](../language/how-to/testing_stdlib.md), [formatting](../tooling/how-to/formatting.md), the [testing CLI](../tooling/how-to/testing.md), and [editor support](../tooling/how-to/editor_setup.md). If anything fails, use [Troubleshooting](../tooling/how-to/troubleshooting.md).

## Explanation

- [Why Incan?](../language/explanation/why_incan.md)
- [How Incan works](../language/explanation/how_incan_works.md)

## Mental model translations (high level)

- **errors**: exceptions vs Result/Option (see: [Error Handling](../language/explanation/error_handling.md))
- **models**: dataclasses/Pydantic vs models/derives (see: [Models & Classes](../language/explanation/models_and_classes/index.md))
- **interfaces**: Protocols/ABCs vs traits/derives (see: [Derives & Traits](../language/reference/derives_and_traits.md))
- **async**: asyncio mental model mapping (see: [Async Programming](../language/how-to/async_programming.md))
