//! Minimal web runtime for Incan-generated web programs.
//!
//! Provided types:
//! - `App` (dummy holder with blocking `run` that serves the router)
//! - `Response` helpers (`html`, `ok`)
//! - `Json<T>` wrapper that implements `IntoResponse`
//! - HTTP method constants (`GET`, ...)

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::OnceLock;

use axum::http::{StatusCode, header};
use axum::response::{Html as AxumHtml, IntoResponse, Response as AxumResponse};
use axum::{Router, routing::get};
use serde::Serialize;
use tokio::runtime::Runtime;

pub const GET: &str = "GET";
pub const POST: &str = "POST";
pub const PUT: &str = "PUT";
pub const DELETE: &str = "DELETE";
pub const PATCH: &str = "PATCH";
pub const HEAD: &str = "HEAD";
pub const OPTIONS: &str = "OPTIONS";

// TODO: make these significantly smaller ints once Incan supports that
pub const HTTP_OK: i64 = 200;
pub const HTTP_CREATED: i64 = 201;
pub const HTTP_NO_CONTENT: i64 = 204;
pub const HTTP_BAD_REQUEST: i64 = 400;
pub const HTTP_UNAUTHORIZED: i64 = 401;
pub const HTTP_FORBIDDEN: i64 = 403;
pub const HTTP_NOT_FOUND: i64 = 404;
pub const HTTP_INTERNAL_ERROR: i64 = 500;

static ROUTER: OnceLock<Router> = OnceLock::new();

#[doc(hidden)]
pub mod __private {
    pub use axum::Router;
    pub use axum::extract;
    pub use axum::response;
    pub use axum::routing;
}

#[doc(hidden)]
#[macro_export]
macro_rules! __incan_router {
    (
        wrappers: [ $($wrapper:item)* ],
        routes: [ $( ($path:literal, $method:ident, $wrapper_name:ident) ),* $(,)? ]
    ) => {
        $($wrapper)*

        fn __incan_web_router() -> ::incan_stdlib::web::__private::Router {
            let mut router = ::incan_stdlib::web::__private::Router::new();
            $(
                router = router.route(
                    $path,
                    ::incan_stdlib::web::__private::routing::$method($wrapper_name)
                );
            )*
            router
        }
    };
}

#[doc(hidden)]
pub use crate::__incan_router;

/// Register the generated router for the `App::run` entrypoint.
///
/// This only captures the first router; subsequent calls are ignored.
pub fn set_router(router: Router) {
    // TODO: report duplicate router registration instead of ignoring.
    let _ = ROUTER.set(router);
}

/// Minimal application handle for generated web programs.
#[derive(Default)]
pub struct App {}

impl App {
    /// Create a new app handle.
    pub fn new() -> Self {
        Self {}
    }

    /// Blocking run helper so sync `main` functions can start the server.
    ///
    /// # Panics
    ///
    /// Panics if the bind address is invalid, the Tokio runtime cannot be created,
    /// the TCP listener fails to bind, or the server returns an error.
    pub fn run(&self, host: &str, port: i64) {
        // TODO: return a Result and surface runtime errors without panicking.
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .unwrap_or_else(|e| panic!("invalid bind address: {e}"));

        let router = ROUTER
            .get()
            .cloned()
            .unwrap_or_else(|| Router::new().route("/", get(|| async { "OK" })));

        let rt = Runtime::new().unwrap_or_else(|e| panic!("failed to create tokio runtime: {e}"));
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|e| panic!("server error: {e}"));
        })
    }
}

/// JSON response wrapper (mirrors `axum::Json`).
///
/// Incan-generated handlers can return `Json<T>` to emit a JSON response.
pub struct Json<T> {
    pub value: T,
}

impl<T> Json<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> AxumResponse {
        axum::Json(self.value).into_response()
    }
}

/// Query string extractor wrapper (mirrors `axum::extract::Query`).
pub struct Query<T> {
    pub value: T,
}

impl<T> Query<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

/// HTML response wrapper (mirrors the Incan `Html` surface type).
///
/// In Incan, handlers can return `Html` directly:
///
/// ```text
/// @route("/")
/// async def index() -> Html:
///   return Html("<h1>Hello</h1>")
/// ```
///
/// At runtime this is emitted as an axum HTML response with `text/html` content type.
#[derive(Debug, Clone)]
pub struct Html(pub String);

impl IntoResponse for Html {
    fn into_response(self) -> AxumResponse {
        AxumHtml(self.0).into_response()
    }
}

/// Response wrapper returned by helper constructors like `Response::html`.
pub struct Response(pub AxumResponse);

impl Response {
    /// Create an HTML response.
    pub fn html<S: Into<String>>(content: S) -> Self {
        Response(AxumHtml(content.into()).into_response())
    }

    /// Create an empty 200 OK response.
    pub fn ok() -> Self {
        Response(AxumResponse::new(axum::body::Body::empty()))
    }

    /// Create a plain text response (200 OK).
    pub fn text<S: Into<String>>(content: S) -> Self {
        Response(content.into().into_response())
    }

    /// Create an empty 201 Created response.
    pub fn created() -> Self {
        Self::status(HTTP_CREATED, "")
    }

    /// Create a 204 No Content response.
    pub fn no_content() -> Self {
        // Safety: StatusCode::NO_CONTENT and an empty body are always valid inputs to the
        // response builder, so this cannot fail in practice.
        Response(
            AxumResponse::builder()
                .status(StatusCode::NO_CONTENT)
                .body(axum::body::Body::empty())
                .expect("building a 204 No Content response should never fail"),
        )
    }

    /// Create a 400 Bad Request response.
    pub fn bad_request<S: Into<String>>(message: S) -> Self {
        Response((StatusCode::BAD_REQUEST, message.into()).into_response())
    }

    /// Create a 404 Not Found response.
    pub fn not_found<S: Into<String>>(message: S) -> Self {
        Response((StatusCode::NOT_FOUND, message.into()).into_response())
    }

    /// Create a 500 Internal Server Error response.
    pub fn internal_error<S: Into<String>>(message: S) -> Self {
        Response((StatusCode::INTERNAL_SERVER_ERROR, message.into()).into_response())
    }

    /// Create a response with custom status code.
    ///
    /// If `code` is not a valid HTTP status code (e.g. negative, > 999, or not a recognized
    /// status), this falls back to 500 Internal Server Error and includes a diagnostic message
    /// in the response body.
    pub fn status<S: Into<String>>(code: i64, body: S) -> Self {
        let body = body.into();
        match u16::try_from(code).ok().and_then(|v| StatusCode::from_u16(v).ok()) {
            Some(status) => Response((status, body).into_response()),
            None => {
                let msg = format!("invalid HTTP status code {code}: {body}");
                eprintln!("[incan] warning: {msg}");
                Response((StatusCode::INTERNAL_SERVER_ERROR, msg).into_response())
            }
        }
    }

    /// Create a 302 redirect response.
    pub fn redirect<S: Into<String>>(location: S) -> Self {
        let location = location.into();
        // Safety: StatusCode::FOUND and a string Location header are always valid inputs.
        // The only failure mode would be non-ASCII bytes in the location, which is a caller bug.
        let response = AxumResponse::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location)
            .body(axum::body::Body::empty())
            .expect("building a 302 redirect response should never fail");
        Response(response)
    }
}

/// Allow `Response` to be returned from handlers.
impl IntoResponse for Response {
    fn into_response(self) -> AxumResponse {
        self.0
    }
}

/// No-op placeholder so `from std.web import route` resolves at Rust compile time.
///
/// The compiler collects `@route(...)` decorators during codegen; this function is not used at runtime.
pub fn route(_path: &str) {}
