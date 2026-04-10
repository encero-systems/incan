# Build your first API (tutorial)

This tutorial walks you through running the built-in web framework and serving a JSON endpoint.

Prerequisite: [Install, build, and run](../../tooling/how-to/install_and_run.md).

!!! note "If something fails"
    If you hit errors while building/running, start with [Troubleshooting](../../tooling/how-to/troubleshooting.md).
    If it still looks like a bug, please [file an issue on GitHub](https://github.com/dannys-code-corner/incan/issues).

## Step 1: Run the hello web example

The repo includes a runnable example:

- Source: `examples/web/hello_web.incn`
- GitHub: `https://github.com/dannys-code-corner/incan/blob/main/examples/web/hello_web.incn`

Build it:

```bash
incan build examples/web/hello_web.incn
```

--8<-- "_snippets/callouts/no_install_fallback.md"

Note: the first build may download Rust crates via Cargo (can take minutes) and requires internet access.

Run the compiled binary:

```bash
./target/incan/.cargo-target/release/hello_web
```

## Step 2: Hit the endpoints

In another terminal:

```bash
curl http://localhost:8080/
curl http://localhost:8080/api/greet/World
curl http://localhost:8080/api/user/42
curl http://localhost:8080/health
```

## Step 3: Understand what you’re seeing

The example demonstrates:

- `@route("/path")` for routes (imported from `std.web.routing`)
- `Json[T]` for JSON responses
- `@derive(Serialize)` for response models
- `async def` handlers (async/await)

Learn more:

- Web framework guide: [Web Framework](web_framework.md)
- Models: [Models & Classes](../explanation/models_and_classes/index.md)
- Errors: [Error Handling](../explanation/error_handling.md)
- Modules: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
