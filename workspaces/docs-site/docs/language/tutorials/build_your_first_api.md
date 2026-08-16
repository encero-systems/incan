# Preview a typed API

This tutorial walks through the built-in web framework and a typed JSON endpoint as a source-verified preview.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Service or application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started and the prepared 0.5 development toolchain</dd></div>
    <div><dt>Time</dt><dd>15–20 minutes</dd></div>
    <div><dt>Verified</dt><dd>Source, type contract, and Rust lowering verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code>; <code>std.web</code> execution needs a compatible Loaf beyond the 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Source-verified preview</dd></div>
    <div><dt>Outcome</dt><dd>A checked API surface with typed responses</dd></div>
    <div><dt>Artifacts</dt><dd>Checked server source and four route contracts</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 3" aria-label="API tutorial steps">
  <li><strong>Check</strong>Verify the source contract</li>
  <li><strong>Trace</strong>Follow the route shapes</li>
  <li><strong>Understand</strong>Trace typed responses</li>
</ol>

Prerequisite: [Install and prepare the development toolchain](../../tooling/how-to/install_and_run.md#follow-the-05-development-docs).

!!! info "Why this is a preview"
    The source and generated-Rust lowering are checked by the repository suite. The finite 0.5 release envelope does not ship a compatible `std.web` Loaf, so this page does not instruct you to build or run a server that the active release toolchain cannot authorize.

## Step 1: Check the hello web example

The repository includes the complete source:

- Source: `examples/web/hello_web.incn`
- GitHub: `https://github.com/encero-systems/incan/blob/main/examples/web/hello_web.incn`

Check it from the Incan repository root:

```bash
incan check examples/web/hello_web.incn
```

This proves the declarations and handler contracts against the current compiler. It does not materialize a native web runtime.

## Step 2: Trace the endpoints

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

The route declarations, path-parameter types, handler signatures, and response-model boundaries pass source verification. Runtime HTTP behavior remains outside the 0.5 release envelope.

</section>

## Step 3: Understand what you’re seeing

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

You checked four route contracts and followed one request shape from route selection to a typed response model without claiming unavailable runtime execution.

</section>

## Continue

- [Web framework guide](web_framework.md)
- [Error handling](../explanation/error_handling.md)
- [Build and consume an Incan library](../../tooling/tutorials/build_and_consume_library.md) to study the source-verified package boundary
