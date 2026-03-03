//! HTTP method vocabulary for web route metadata.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for supported HTTP methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethodId {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// Metadata entry for an HTTP method.
pub type HttpMethodInfo = LangItemInfo<HttpMethodId>;

/// Registry of supported HTTP methods.
pub const HTTP_METHODS: &[HttpMethodInfo] = &[
    info(HttpMethodId::Get, "GET", &["get"], "HTTP GET", RFC::_000, Since(0, 1)),
    info(
        HttpMethodId::Post,
        "POST",
        &["post"],
        "HTTP POST",
        RFC::_000,
        Since(0, 1),
    ),
    info(HttpMethodId::Put, "PUT", &["put"], "HTTP PUT", RFC::_000, Since(0, 1)),
    info(
        HttpMethodId::Delete,
        "DELETE",
        &["delete"],
        "HTTP DELETE",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        HttpMethodId::Patch,
        "PATCH",
        &["patch"],
        "HTTP PATCH",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a method name to its stable id.
pub fn from_str(name: &str) -> Option<HttpMethodId> {
    let name = name.trim();
    HTTP_METHODS
        .iter()
        .find(|m| m.canonical.eq_ignore_ascii_case(name) || m.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .map(|m| m.id)
}

/// Return the canonical spelling for a method.
pub fn as_str(id: HttpMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the metadata entry for a method.
pub fn info_for(id: HttpMethodId) -> &'static HttpMethodInfo {
    HTTP_METHODS
        .iter()
        .find(|m| m.id == id)
        .expect("INVARIANT: http method info missing")
}

const fn info(
    id: HttpMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> HttpMethodInfo {
    LangItemInfo {
        id,
        canonical,
        aliases,
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
