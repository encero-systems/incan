# Build a typed API

This tutorial builds and runs the included web application, then traces one request from a route declaration to a typed JSON response.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Service or application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting started with compiled dev.8 web and async packages</dd></div>
    <div><dt>Time</dt><dd>15–20 minutes</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
    <div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
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

The server prints its routes and listens on `127.0.0.1:8080` until you stop it with Ctrl+C.

## Step 2: Request the endpoints

From a second terminal, request each route:

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

The home page returns its HTML, `/api/greet/World` returns `{"message":"Hello, World!"}`, `/api/user/42` returns `{"id":42,"name":"User 42","email":"user42@example.com"}`, and `/health` returns status 200.

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

You ran a native server and traced each request from route selection to a typed response model.

</section>

## Continue

- [Web framework guide](web_framework.md)
- [Error handling](../explanation/error_handling.md)
- [Build and consume an Incan library](../../tooling/tutorials/build_and_consume_library.md) to build a reusable package boundary
