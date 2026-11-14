use super::LangItemInfo;
use crate::lang::registry::{RFC, RfcId, Since, Stability};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OptionMethodId {
    /// Borrow the payload mutably without consuming the option.
    AsMut,
    Copied,
    UnwrapOr,
    Unwrap,
    /// Report whether the option carries a payload.
    IsSome,
    /// Report whether the option is empty.
    IsNone,
    /// Clone the option and its present payload through its compiler-owned contract.
    Clone,
}

pub type OptionMethodInfo = LangItemInfo<OptionMethodId>;

pub const OPTION_METHODS: &[OptionMethodInfo] = &[
    info(
        OptionMethodId::AsMut,
        "as_mut",
        &[],
        "Borrow the contained value mutably without consuming the option.",
        RFC::_000,
        Since(0, 6),
    ),
    info(
        OptionMethodId::Copied,
        "copied",
        &[],
        "Copy from Option[&T] to Option[T] when T: Copy.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        OptionMethodId::UnwrapOr,
        "unwrap_or",
        &[],
        "Return the contained value or a default.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        OptionMethodId::Unwrap,
        "unwrap",
        &[],
        "Return the contained value or panic.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        OptionMethodId::IsSome,
        "is_some",
        &[],
        "Return whether the option contains a payload.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        OptionMethodId::IsNone,
        "is_none",
        &[],
        "Return whether the option contains no payload.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        OptionMethodId::Clone,
        "clone",
        &[],
        "Clone the option and its present payload.",
        RFC::_000,
        Since(0, 1),
    ),
];

pub fn from_str(name: &str) -> Option<OptionMethodId> {
    super::from_str_impl(OPTION_METHODS, name)
}

pub fn as_str(id: OptionMethodId) -> &'static str {
    info_for(id).canonical
}

pub fn info_for(id: OptionMethodId) -> &'static OptionMethodInfo {
    super::info_for_impl(OPTION_METHODS, id, "option method info missing")
}

const fn info(
    id: OptionMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> OptionMethodInfo {
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
