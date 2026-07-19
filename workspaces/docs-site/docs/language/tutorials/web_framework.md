# Web framework guide

This page is the task-oriented guide to Incan's hosted web framework. For a linear first experience, start with [Build your first API](build_your_first_api.md).

Incan's web framework currently lowers to [Axum](https://docs.rs/axum/latest/axum/), giving you Flask/FastAPI-like syntax with a native async server backend.

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

```incan
--8<-- "_snippets/language/examples/verified_web_quick_start.incn"
```

Build and run:

```bash
incan build examples/web/hello_web.incn
./target/incan/.cargo-target/release/hello_web
```

Note: the first build may download Rust crates via Cargo (can take minutes) and requires internet access.

## Routes

Define routes using the `@route` decorator:

```incan
from std.web import route, Response, GET, POST
import std.async

@route("/path")
async def handler() -> Response:
    ...

@route("/api/resource", methods=[GET])
async def get_resource() -> Response:
    ...

@route("/api/resource", methods=[POST])
async def create_resource() -> Response:
    ...
```

### Path Parameters

Use `{name}` syntax for path parameters:

```incan
from std.web import route, Json
import std.async

@route("/users/{id}")
async def get_user(id: int) -> Json[User]:
    user = find_user(id)?
    return Json(user)

@route("/posts/{year}/{month}")
async def get_posts(year: int, month: int) -> Json[list[Post]]:
    return Json(fetch_posts(year, month))
```

### HTTP Methods

Specify allowed methods with the `methods` parameter. Handlers can be registered for multiple HTTP methods by passing multiple entries. Import the method constants from the web prelude (e.g. `GET`, `POST`). Supported methods are `GET`, `POST`, `PUT`, `DELETE`, and `PATCH`.

```incan
from std.web import route, Json, Response, GET, POST, PUT, DELETE
import std.async

@route("/items/ping", methods=[GET, POST])
async def ping_items() -> Response:
    return Response.ok()

@route("/items", methods=[GET])
async def list_items() -> Json[list[Item]]:
    ...

@route("/items/reindex", methods=[POST])
async def reindex_items() -> Response:
    return Response.status(202, "reindex queued")

@route("/items/{id}", methods=[PUT])
async def update_item(id: int) -> Response:
    return Response.ok()

@route("/items/{id}", methods=[DELETE])
async def delete_item(id: int) -> Response:
    return Response.ok()
```

## Responses

### JSON Responses

Use `Json[T]` for JSON responses. The inner type must have `@derive(json)`:

```incan
from std.web import route, Json
from std.serde import json
import std.async

@derive(json)
model User:
    id: int
    name: str
    email: str

@route("/api/user/{id}")
async def get_user(id: int) -> Json[User]:
    user = User(id=id, name="Alice", email="alice@example.com")
    return Json(user)
```

### HTML Responses

Return HTML with `Response.html()`:

```incan
from std.web import route, Response
import std.async

@route("/")
async def index() -> Response:
    return Response.html("<h1>Welcome!</h1>")
```

### Status Codes

Use `Response` methods for different status codes:

```incan
from std.web import route, Response
import std.async

@route("/health")
async def health() -> Response:
    return Response.ok()  # 200

@route("/created")
async def created() -> Response:
    return Response.status(201, "created")

@route("/empty")
async def empty() -> Response:
    return Response.status(204, "")

@route("/error")
async def error() -> Response:
    return Response.status(400, "Invalid input")

@route("/missing")
async def missing() -> Response:
    return Response.status(404, "Resource not found")

@route("/server-error")
async def server_error() -> Response:
    return Response.status(500, "Something went wrong")
```

## Request Data

### Extracting Path Parameters

Path parameters are automatically extracted into function arguments:

```incan
from std.web import route, Json
import std.async

@route("/users/{user_id}/posts/{post_id}")
async def get_post(user_id: int, post_id: int) -> Json[Post]:
    ...
```

### Query parameters and JSON bodies

Typed `Query[T]` and `Json[T]` request extractors are part of the intended surface, but the current development compiler accepts them during typechecking and then emits an invalid Rust handler signature. Until [the extractor code-generation bug is fixed](https://github.com/encero-systems/incan/issues/867), this verified guide does not present those forms as runnable examples.

## Application

### Starting the Server

Call the static `App.run()` entrypoint:

```incan
from std.web import App

def main() -> None:
    App.run(host="0.0.0.0", port=3000)
```

Parameters:

- `host`: Bind address (default: `"127.0.0.1"`)
- `port`: Port number (default: `8080`)

## How It Works

When you compile an Incan web application:

1. **Routes are collected** from `@route` decorators
2. **Handlers become async Rust functions** with Axum extractors
3. **Models with `@derive(json)`** get serde derives
4. **`App.run()`** becomes Axum router setup + tokio server

The generated Rust code uses:

- `axum::Router` for routing
- `axum::Json` for JSON request/response
- `axum::extract::Path` for path parameters
- `axum::extract::Query` for query parameters
- `tokio` for async runtime

## Complete Example

```incan
--8<-- "_snippets/language/examples/verified_web_framework_complete.incn"
```

The example is deliberately read-only. Once you add persistence, keep database failures explicit and give each handler one coherent response type across all branches.

## Performance

With the current Rust/Axum-backed path, your web application runs with:

- **Native server path** — no Python interpreter process in the request path
- **Tokio-backed async** — an efficient async runtime without Python event-loop compatibility claims
- **Deterministic ownership** — generated Rust does not depend on a tracing garbage collector
- **Workload-dependent performance** — latency and memory use still depend on handlers, dependencies, traffic, and deployment settings, so benchmark the service you intend to run

That makes Incan a reasonable fit for API and microservice code where Python-readable source and a native async server backend both matter.

## See Also

- [Error Handling](../explanation/error_handling.md) - Working with `Result` types
- [Derives & Traits](../reference/derives_and_traits.md) - Drop trait for custom cleanup
- [File I/O](../how-to/file_io.md) - Reading, writing, and path handling
- [Async Programming](../how-to/async_programming.md) - Async/await with Tokio
- [Imports & Modules](../explanation/imports_and_modules.md) - Module system, imports, and built-in functions
- [Rust Interop](../how-to/rust_interop.md) - Using Rust crates directly from Incan
