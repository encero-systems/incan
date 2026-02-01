//! Minimal web runtime for Incan-generated web programs.
//!
//! Provided types:
//! - `App` (dummy holder with blocking `run` that serves the router)
//! - `Response` helpers (`html`, `ok`)
//! - `Json<T>` wrapper that implements `IntoResponse`
//! - HTTP method constants (`GET`, ...)

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::response::{Html, IntoResponse, Response as AxumResponse};
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

static ROUTER: OnceLock<Router> = OnceLock::new();

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
pub struct Json<T>(pub T);

impl<T> IntoResponse for Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> AxumResponse {
        axum::Json(self.0).into_response()
    }
}

/// Response wrapper returned by helper constructors like `Response::html`.
pub struct Response(pub AxumResponse);

impl Response {
    /// Create an HTML response.
    pub fn html<S: Into<String>>(content: S) -> Self {
        Response(Html(content.into()).into_response())
    }

    /// Create an empty 200 OK response.
    pub fn ok() -> Self {
        Response(AxumResponse::new(axum::body::Body::empty()))
    }
}

/// Allow `Response` to be returned from handlers.
impl IntoResponse for Response {
    fn into_response(self) -> AxumResponse {
        self.0
    }
}

/// No-op placeholder so `from web import route` resolves at Rust compile time.
///
/// The compiler collects `@route(...)` decorators during codegen; this function is not used at runtime.
pub fn route(_path: &str) {}
