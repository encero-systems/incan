//! Minimal web runtime for Incan-generated web programs.
//!
//! Provided types:
//! - `App` (dummy holder with blocking `run` that serves the router)
//! - `Response` helpers (`html`, `ok`)
//! - `Json<T>` wrapper that implements `IntoResponse`
//! - HTTP method constants (`GET`, ...)

pub mod wrappers;

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
    /// Panics if the bind address is invalid, the Tokio runtime cannot be created, the TCP listener fails to bind, or
    /// the server returns an error.
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
// Re-export wrapper types with their public-facing names
pub use wrappers::AxumHtml as Html;
pub use wrappers::AxumJson as Json;
pub use wrappers::AxumResponse as Response;

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

/// No-op placeholder so `from std.web import route` resolves at Rust compile time.
///
/// The compiler collects `@route(...)` decorators during codegen; this function is not used at runtime.
pub fn route(_path: &str) {}
