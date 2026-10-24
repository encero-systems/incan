# Build a typed API

This tutorial builds and runs the included web application, then traces one request from a route declaration to a typed JSON response.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Service or application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting started with compiled dev.8 web and async packages</dd></div>
    <div><dt>Time</dt><dd>15–20 minutes</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
    <div><dt>Status</dt><dd>Source exercise; native compilation is refused in the tested dev.8 installation</dd></div>
    <div><dt>Outcome</dt><dd>A running native API with typed responses</dd></div>
    <div><dt>Artifacts</dt><dd>Native server and four route contracts</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 3" aria-label="API tutorial steps">
  <li><strong>Run</strong>Start the native server</li>
  <li><strong>Request</strong>Exercise the route shapes</li>
  <li><strong>Trace</strong>Follow typed responses</li>
</ol>

Prerequisite: [Install the dev.8 toolchain](../../tooling/how-to/install_and_run.md).

## Step 1: Run the hello web example

The repository includes the complete source:

- Source: `examples/web/hello_web.incn`
- GitHub: `https://github.com/encero-systems/incan/blob/main/examples/web/hello_web.incn`

Build it, then start the server from the Incan repository root:

```bash
incan build examples/web/hello_web.incn
incan run examples/web/hello_web.incn
```

The tested dev.8 driver refuses this example with a nonlocal model-constructor error before starting a server. The commands require compiled web, async and data packages, but their presence alone does not remove this compiler refusal.

## Step 2: Request the endpoints

These requests exercise a successfully built server. They cannot complete against this example while the native compilation refusal prevents startup:

```bash
curl http://127.0.0.1:8080/
curl http://127.0.0.1:8080/api/greet/World
curl http://127.0.0.1:8080/api/user/42
curl http://127.0.0.1:8080/health
```

```mermaid
flowchart LR
  A["curl request"] --> B["Incan route"]
  B --> C["typed handler"]
  C --> D["Json response model"]
  D --> E["HTTP response"]
```

<p class="inc-diagram-caption">The route selects a typed handler; the response model is serialized at the HTTP boundary.</p>

| Route | Checked return boundary |
| --- | --- |
| `/` | `Response` containing HTML |
| `/api/greet/{name}` | `Json[Greeting]` |
| `/api/user/{id}` | `Json[User]` with an integer path parameter |
| `/health` | successful `Response` |

<section class="inc-learning-panel inc-learning-panel--result" data-label="Expected" markdown="1">

The source declares four route contracts. A successful server build and responses to the requests above are the runtime acceptance condition.

</section>

## Step 3: Trace what you’re seeing

The example demonstrates:

- `@route("/path")` for routes (imported from `std.web.routing`)
- `Json[T]` for JSON responses
- `@derive(json)` for response models
- `async def` handlers (async/await)

Learn more:

- Web framework guide: [Web framework guide](web_framework.md)
- Models: [Models & Classes](../explanation/models_and_classes/index.md)
- Errors: [Error Handling](../explanation/error_handling.md)
- Modules: [Imports and modules (how-to)](../how-to/imports_and_modules.md)

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You can trace the source from route selection to a typed response model. The tested dev.8 driver does not satisfy the native server completion condition.

</section>

## Continue

- [Web framework guide](web_framework.md)
- [Error handling](../explanation/error_handling.md)
- [Build and consume an Incan library](../../tooling/tutorials/build_and_consume_library.md) to build a reusable package boundary
