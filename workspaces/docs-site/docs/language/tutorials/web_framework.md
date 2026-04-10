# Web Framework

Incan includes a web framework that compiles to [Axum](https://docs.rs/axum/latest/axum/),
giving you Flask/FastAPI-like syntax with native Rust performance.

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

```incan
from std.web import App, route, Response, Json
import std.async

@derive(Serialize)
model Greeting:
    message: str

@route("/")
async def index() -> Response:
    return Response.html("<h1>Hello from Incan!</h1>")

@route("/api/greet/{name}")
async def greet(name: str) -> Json[Greeting]:
    return Json(Greeting(message=f"Hello, {name}!"))

def main() -> None:
    app = App()
    app.run(port=8080)
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

Specify allowed methods with the `methods` parameter.
Handlers can be registered for multiple HTTP methods by passing multiple entries.
Import the method constants from the web prelude (e.g. `GET`, `POST`).
Supported methods are `GET`, `POST`, `PUT`, `DELETE`, and `PATCH`.

```incan
from std.web import route, Json, Response, GET, POST, PUT, DELETE
import std.async

@route("/items/ping", methods=[GET, POST])
async def ping_items() -> Response:
    return Response.ok()

@route("/items", methods=[GET])
async def list_items() -> Json[list[Item]]:
    ...

@route("/items", methods=[POST])
async def create_item(body: Json[CreateItem]) -> Json[Item]:
    ...

@route("/items/{id}", methods=[PUT])
async def update_item(id: int) -> Response:
    ...

@route("/items/{id}", methods=[DELETE])
async def delete_item(id: int) -> Response:
    ...
```

## Responses

### JSON Responses

Use `Json[T]` for JSON responses. The inner type must have `@derive(Serialize)`:

```incan
from std.web import route, Json
import std.async

@derive(Serialize)
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
    return Response.created()  # 201

@route("/empty")
async def empty() -> Response:
    return Response.no_content()  # 204

@route("/error")
async def error() -> Response:
    return Response.bad_request("Invalid input")  # 400

@route("/missing")
async def missing() -> Response:
    return Response.not_found("Resource not found")  # 404

@route("/server-error")
async def server_error() -> Response:
    return Response.internal_error("Something went wrong")  # 500
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

### Query Parameters

Use `Query[T]` for query string parameters:

```incan
from std.web import route, Json, Query
import std.async

@derive(Deserialize)
model SearchParams:
    q: str
    limit: int = 10

@route("/search")
async def search(params: Query[SearchParams]) -> Json[list[Result]]:
    results = do_search(params.q, params.limit)
    return Json(results)
```

### JSON Body

Use `Json[T]` as a parameter for JSON request bodies:

```incan
from std.web import route, Json, POST
import std.async

@derive(Deserialize)
model CreateUser:
    name: str
    email: str

@route("/users", methods=[POST])
async def create_user(body: Json[CreateUser]) -> Json[User]:
    user = User(id=1, name=body.name, email=body.email)
    return Json(user)
```

## Application

### Starting the Server

Create an `App` and call `run()`:

```incan
from std.web import App

def main() -> None:
    app = App()
    app.run(host="0.0.0.0", port=3000)
```

Parameters:

- `host`: Bind address (default: `"127.0.0.1"`)
- `port`: Port number (default: `8080`)

## How It Works

When you compile an Incan web application:

1. **Routes are collected** from `@route` decorators
2. **Handlers become async Rust functions** with Axum extractors
3. **Models with `@derive(Serialize/Deserialize)`** get serde derives
4. **`app.run()`** becomes Axum router setup + tokio server

The generated Rust code uses:

- `axum::Router` for routing
- `axum::Json` for JSON request/response
- `axum::extract::Path` for path parameters
- `axum::extract::Query` for query parameters
- `tokio` for async runtime

## Complete Example

```incan
"""
A simple REST API for managing items.
"""

from std.web import App, route, Response, Json, GET, POST, DELETE
import std.async


@derive(Serialize, Deserialize)
model Item:
    id: int
    name: str
    price: float


# In-memory storage (in a real app, use a database)
items: list[Item] = []


@route("/api/items", methods=[GET])
async def list_items() -> Json[list[Item]]:
    """List all items."""
    return Json(items)


@route("/api/items", methods=[POST])
async def create_item(body: Json[Item]) -> Json[Item]:
    """Create a new item."""
    items.append(body.value)
    return Json(body.value)


@route("/api/items/{id}", methods=[GET])
async def get_item(id: int) -> Response:
    """Get an item by ID."""
    for item in items:
        if item.id == id:
            return Json(item)
    return Response.not_found("Item not found")


@route("/api/items/{id}", methods=[DELETE])
async def delete_item(id: int) -> Response:
    """Delete an item by ID."""
    for i, item in enumerate(items):
        if item.id == id:
            items.pop(i)
            return Response.no_content()
    return Response.not_found("Item not found")


def main() -> None:
    println("Starting API server at http://localhost:8080")
    app = App()
    app.run(port=8080)
```

## Performance

Since Incan compiles to Rust/Axum, your web application runs with:

- **Native performance** — no interpreter overhead
- **Zero-cost async** — Tokio's efficient async runtime
- **No garbage collector** — predictable latency
- **Low memory usage** — Rust's ownership model

This makes Incan ideal for high-performance APIs and microservices.

## See Also

- [Error Handling](../explanation/error_handling.md) - Working with `Result` types
- [Derives & Traits](../reference/derives_and_traits.md) - Drop trait for custom cleanup
- [File I/O](../how-to/file_io.md) - Reading, writing, and path handling
- [Async Programming](../how-to/async_programming.md) - Async/await with Tokio
- [Imports & Modules](../explanation/imports_and_modules.md) - Module system, imports, and built-in functions
- [Rust Interop](../how-to/rust_interop.md) - Using Rust crates directly from Incan
