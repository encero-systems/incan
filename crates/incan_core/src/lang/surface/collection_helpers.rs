//! Built-in collection helper surface vocabulary.
//!
//! These helpers live on built-in collection type surfaces such as `list`, but they are not ordinary instance
//! methods on collection values. Keep them separate from `methods.rs` so tooling and docs do not accidentally imply
//! that `xs.repeat(...)` is valid for a runtime list value.

use crate::lang::registry::{LangItemInfo, RFC, Since, Stability};

/// Stable identifier for a built-in collection helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinCollectionHelperId {
    ListRepeat,
}

/// Metadata for a built-in collection helper.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinCollectionHelperInfo {
    pub item: LangItemInfo<BuiltinCollectionHelperId>,
    pub receiver: &'static str,
    pub member: &'static str,
    pub signature: &'static str,
}

/// Registry of import-free collection helpers.
pub const BUILTIN_COLLECTION_HELPERS: &[BuiltinCollectionHelperInfo] = &[BuiltinCollectionHelperInfo {
    item: LangItemInfo {
        id: BuiltinCollectionHelperId::ListRepeat,
        canonical: "list.repeat",
        aliases: &[],
        description: "Create a list containing `count` clone-derived copies of `value`; negative counts raise `ValueError`.",
        introduced_in_rfc: RFC::_069,
        since: Since(0, 3),
        stability: Stability::Stable,
        examples: &[],
    },
    receiver: "list",
    member: "repeat",
    signature: "list.repeat[T](value: T, count: int) -> list[T]",
}];

/// Resolve a helper receiver/member pair to its stable id.
pub fn from_parts(receiver: &str, member: &str) -> Option<BuiltinCollectionHelperId> {
    BUILTIN_COLLECTION_HELPERS
        .iter()
        .find(|helper| helper.receiver == receiver && helper.member == member)
        .map(|helper| helper.item.id)
}

/// Return the full metadata entry for a helper id.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: BuiltinCollectionHelperId) -> &'static BuiltinCollectionHelperInfo {
    BUILTIN_COLLECTION_HELPERS
        .iter()
        .find(|helper| helper.item.id == id)
        .unwrap_or_else(|| panic!("built-in collection helper info missing"))
}

/// Return the canonical full name for a built-in collection helper.
pub fn full_name(id: BuiltinCollectionHelperId) -> &'static str {
    info_for(id).item.canonical
}

/// Return the receiver spelling for a built-in collection helper.
pub fn receiver(id: BuiltinCollectionHelperId) -> &'static str {
    info_for(id).receiver
}

/// Return the member spelling for a built-in collection helper.
pub fn member(id: BuiltinCollectionHelperId) -> &'static str {
    info_for(id).member
}

/// Return the signature displayed by docs and tooling for a built-in collection helper.
pub fn signature(id: BuiltinCollectionHelperId) -> &'static str {
    info_for(id).signature
}
