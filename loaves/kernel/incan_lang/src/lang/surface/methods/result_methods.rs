//! Result method surface vocabulary.

use super::LangItemInfo;
use crate::lang::registry::{RFC, RfcId, Since, Stability};

/// Stable identifier for an RFC 070 `Result[T, E]` combinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ResultMethodId {
    Map,
    MapErr,
    AndThen,
    OrElse,
    Inspect,
    InspectErr,
    Unwrap,
    UnwrapOr,
    /// Report whether the result carries an `Ok` payload.
    IsOk,
    /// Report whether the result carries an `Err` payload.
    IsErr,
}

pub type ResultMethodInfo = LangItemInfo<ResultMethodId>;

pub const RESULT_METHODS: &[ResultMethodInfo] = &[
    info(
        ResultMethodId::Map,
        "map",
        &[],
        "Transform an Ok payload while preserving Err.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::MapErr,
        "map_err",
        &[],
        "Transform an Err payload while preserving Ok.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::AndThen,
        "and_then",
        &[],
        "Chain a Result-returning operation from an Ok payload.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::OrElse,
        "or_else",
        &[],
        "Recover or remap through a Result-returning operation from an Err payload.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::Inspect,
        "inspect",
        &[],
        "Pass the Ok payload to a callback and return the original Result unchanged.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::InspectErr,
        "inspect_err",
        &[],
        "Pass the Err payload to a callback and return the original Result unchanged.",
        RFC::_070,
        Since(0, 3),
    ),
    info(
        ResultMethodId::Unwrap,
        "unwrap",
        &[],
        "Return the Ok payload or panic.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ResultMethodId::UnwrapOr,
        "unwrap_or",
        &[],
        "Return the Ok payload or a default value.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ResultMethodId::IsOk,
        "is_ok",
        &[],
        "Return whether the result contains an Ok payload.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ResultMethodId::IsErr,
        "is_err",
        &[],
        "Return whether the result contains an Err payload.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a result method spelling to its stable id.
pub fn from_str(name: &str) -> Option<ResultMethodId> {
    super::from_str_impl(RESULT_METHODS, name)
}

/// Return the canonical spelling for a result method.
pub fn as_str(id: ResultMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a result method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: ResultMethodId) -> &'static ResultMethodInfo {
    super::info_for_impl(RESULT_METHODS, id, "result method info missing")
}

/// Construct result method metadata for the static registry.
const fn info(
    id: ResultMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> ResultMethodInfo {
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
