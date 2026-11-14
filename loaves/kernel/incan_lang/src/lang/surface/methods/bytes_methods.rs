//! `bytes` method surface vocabulary.
//!
//! Runtime `bytes` values expose the text return trip here; byte-level helpers stay on the `FrozenBytes` and
//! builtin surfaces.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a runtime `bytes` method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BytesMethodId {
    /// `data.decode(encoding="utf-8", errors="strict")` returns the bytes as text (#1668).
    Decode,
    /// Borrow the runtime byte buffer as its immutable native slice view.
    AsSlice,
}

pub type BytesMethodInfo = LangItemInfo<BytesMethodId>;

/// Registry of all runtime `bytes` methods.
pub const BYTES_METHODS: &[BytesMethodInfo] = &[
    info(
        BytesMethodId::Decode,
        "decode",
        &[],
        "Return `Result[str, ValidationError]`: the bytes decoded as UTF-8 text, or an error whose `code` is `invalid-utf8` (under `errors=\"strict\"`, the default), `unknown-encoding`, or `unknown-errors-policy`; `errors=\"replace\"` substitutes U+FFFD and always succeeds.",
        RFC::_009,
        Since(0, 6),
    ),
    info(
        BytesMethodId::AsSlice,
        "as_slice",
        &[],
        "Borrow the runtime byte buffer as an immutable native byte slice.",
        RFC::_009,
        Since(0, 6),
    ),
];

/// Resolve a `bytes` method spelling to its stable id.
pub fn from_str(name: &str) -> Option<BytesMethodId> {
    super::from_str_impl(BYTES_METHODS, name)
}

/// Return the canonical spelling for a `bytes` method.
pub fn as_str(id: BytesMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a `bytes` method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: BytesMethodId) -> &'static BytesMethodInfo {
    super::info_for_impl(BYTES_METHODS, id, "bytes method info missing")
}

/// Build one stable `bytes` method registry entry; every entry is `Stable` and carries no examples yet.
const fn info(
    id: BytesMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> BytesMethodInfo {
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
