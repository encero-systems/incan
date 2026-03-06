//! Minimal web runtime for Incan-generated web programs.
//!
//! Web route registration is inventory-driven. The compiler/proc-macro layer emits `inventory::submit!` calls
//! containing `RouteEntry` records, and `App::run` builds the router from those records at runtime.

// FIXME: this module need to be rewritten in incan once the appropriate RFCs are implemented

use std::net::SocketAddr;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{Html as AxumHtmlInner, IntoResponse, Response as AxumRawResponse};
use tokio::runtime::Runtime;

// Re-export axum types so stdlib Incan modules can import them via `incan_stdlib::web::Json`,
// `incan_stdlib::web::Html`, etc. These are the canonical "web response" types for Incan programs.
pub use axum::Json;
pub use axum::response::Html;

pub type AxumHtml = axum::response::Html<String>;
pub type AxumJson<T> = axum::Json<T>;

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

pub struct RouteEntry {
    pub path: &'static str,
    pub method: &'static str,
    pub register: fn(Router) -> Router,
}

impl RouteEntry {
    #[must_use]
    pub const fn new(path: &'static str, method: &'static str, register: fn(Router) -> Router) -> Self {
        Self { path, method, register }
    }
}

inventory::collect!(RouteEntry);

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

        let mut router = Router::new();
        for entry in inventory::iter::<RouteEntry> {
            router = (entry.register)(router);
        }

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

/// Start the web server.
///
/// This is the free-function delegate called by the `App::run` static method generated from `app.incn`. It creates a
/// temporary `App` instance and delegates to the instance method so that the `@staticmethod @rust.extern` wiring works
/// without an instance in scope.
///
/// # Panics
///
/// Panics if the bind address is invalid, the Tokio runtime cannot be created, the TCP listener fails to bind, or the
/// server returns an error.
pub fn run(host: String, port: i64) {
    App::new().run(&host, port);
}

#[must_use]
pub fn response_html(content: String) -> AxumRawResponse {
    AxumHtmlInner(content).into_response()
}

#[must_use]
pub fn response_ok() -> AxumRawResponse {
    AxumRawResponse::new(axum::body::Body::empty())
}

#[must_use]
pub fn response_status(code: i64, body: String) -> AxumRawResponse {
    let status = u16::try_from(code)
        .ok()
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, body).into_response()
}

#[must_use]
pub fn response_redirect(location: String) -> AxumRawResponse {
    match axum::response::Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
    {
        Ok(response) => response,
        Err(_err) => (StatusCode::INTERNAL_SERVER_ERROR, "failed to build redirect response").into_response(),
    }
}
