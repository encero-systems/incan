//! A reqwest-shaped client: `post` takes anything `IntoUrl`, `json` borrows its payload.

/// Anything that names a URL.
pub trait IntoUrl {
    /// The URL text.
    fn url(&self) -> String;
}

impl IntoUrl for &str {
    /// The text itself.
    fn url(&self) -> String {
        self.to_string()
    }
}

/// A client that builds requests.
pub struct Client;

/// A request under construction.
pub struct RequestBuilder {
    pub url: String,
    pub payloads: usize,
}

impl Client {
    /// A fresh client.
    pub fn new() -> Client {
        Client
    }

    /// Start a POST to the URL.
    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        RequestBuilder {
            url: url.url(),
            payloads: 0,
        }
    }
}

impl Default for Client {
    /// The `new` value.
    fn default() -> Self {
        Client::new()
    }
}

impl RequestBuilder {
    /// Attach a JSON payload by reference.
    pub fn json<T: ?Sized>(self, _payload: &T) -> RequestBuilder {
        RequestBuilder {
            url: self.url,
            payloads: self.payloads + 1,
        }
    }
}
