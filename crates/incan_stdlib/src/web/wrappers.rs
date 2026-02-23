//! Axum type wrappers for IncanSource newtypes.
//!
//! These are simple newtype wrappers around Axum types that implement `IntoResponse`.
//! The Incan compiler wraps these again in generated newtypes with trait delegation.
//!
//! Example generated code:
//! ```rust
//! // In stdlib/web/response.incn:
//! // type Json[T] = newtype AxumJson[T]
//!
//! // Generated Rust:
//! pub struct Json<T>(pub incan_stdlib::web::AxumJson<T>);
//! impl<T: Serialize> IntoResponse for Json<T> {
//!     fn into_response(self) -> Response {
//!         self.0.into_response() // Trait delegation
//!     }
//! }
//! ```

use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Wrapper for `axum::Json<T>`.
///
/// Implements `IntoResponse` by delegating to the inner `axum::Json`.
#[derive(Debug, Clone)]
pub struct AxumJson<T: Serialize>(pub axum::Json<T>);

impl<T: Serialize> AxumJson<T> {
    pub fn new(value: T) -> Self {
        Self(axum::Json(value))
    }
}

impl<T: Serialize> IntoResponse for AxumJson<T> {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}

/// Wrapper for `axum::response::Html<String>`.
///
/// Implements `IntoResponse` by delegating to the inner `axum::response::Html`.
#[derive(Debug, Clone)]
pub struct AxumHtml(pub axum::response::Html<String>);

impl AxumHtml {
    pub fn new(content: String) -> Self {
        Self(axum::response::Html(content))
    }
}

impl IntoResponse for AxumHtml {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}

/// Wrapper for `axum::response::Response`.
///
/// Implements `IntoResponse` by returning itself (Response already is IntoResponse).
/// Provides builder methods for common response types.
#[derive(Debug)]
pub struct AxumResponse(pub Response);

impl AxumResponse {
    pub fn new(response: Response) -> Self {
        Self(response)
    }

    /// Create an HTML response (200 OK) with Content-Type: text/html.
    pub fn html<S: Into<String>>(content: S) -> Self {
        Self(axum::response::Html(content.into()).into_response())
    }

    /// Create a plain text response (200 OK).
    pub fn text<S: Into<String>>(content: S) -> Self {
        Self(content.into().into_response())
    }

    /// Create an empty 200 OK response.
    pub fn ok() -> Self {
        Self(Response::new(axum::body::Body::empty()))
    }

    /// Create an empty 201 Created response.
    pub fn created() -> Self {
        Self::status(201, "")
    }

    /// Create a 204 No Content response.
    pub fn no_content() -> Self {
        use axum::http::StatusCode;
        // This can't fail with valid status code and empty body
        let response = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .expect("INVARIANT: building a 204 No Content response should never fail");
        Self(response)
    }

    /// Create a 400 Bad Request response.
    pub fn bad_request<S: Into<String>>(message: S) -> Self {
        use axum::http::StatusCode;
        Self((StatusCode::BAD_REQUEST, message.into()).into_response())
    }

    /// Create a 404 Not Found response.
    pub fn not_found<S: Into<String>>(message: S) -> Self {
        use axum::http::StatusCode;
        Self((StatusCode::NOT_FOUND, message.into()).into_response())
    }

    /// Create a 500 Internal Server Error response.
    pub fn internal_error<S: Into<String>>(message: S) -> Self {
        use axum::http::StatusCode;
        Self((StatusCode::INTERNAL_SERVER_ERROR, message.into()).into_response())
    }

    /// Create a response with custom status code.
    ///
    /// If `code` is not a valid HTTP status code, this falls back to 500.
    pub fn status<S: Into<String>>(code: i64, body: S) -> Self {
        use axum::http::StatusCode;
        let body = body.into();
        match u16::try_from(code).ok().and_then(|v| StatusCode::from_u16(v).ok()) {
            Some(status) => Self((status, body).into_response()),
            None => {
                let msg = format!("invalid HTTP status code {code}: {body}");
                eprintln!("[incan] warning: {msg}");
                Self((StatusCode::INTERNAL_SERVER_ERROR, msg).into_response())
            }
        }
    }

    /// Create a 302 redirect response with Location header.
    pub fn redirect<S: Into<String>>(location: S) -> Self {
        use axum::http::{StatusCode, header};
        let location = location.into();
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location)
            .body(axum::body::Body::empty())
            .expect("INVARIANT: building a 302 redirect response should never fail");
        Self(response)
    }
}

impl IntoResponse for AxumResponse {
    fn into_response(self) -> Response {
        self.0
    }
}
