//! Float method surface vocabulary.
//!
//! These are the user-facing methods that the typechecker treats as available on `float`.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a float method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatMethodId {
    // f64-like math
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Round,
    Sin,
    Cos,
    Tan,
    Exp,
    Ln,
    Log2,
    Log10,

    // predicates
    IsNan,
    IsInfinite,
    IsFinite,

    // powers
    Powi,
    Powf,
}

pub type FloatMethodInfo = LangItemInfo<FloatMethodId>;

/// Registry of all float methods.
pub const FLOAT_METHODS: &[FloatMethodInfo] = &[
    info(FloatMethodId::Sqrt, "sqrt", &[], "Square root.", RFC::_009, Since(0, 1)),
    info(
        FloatMethodId::Abs,
        "abs",
        &[],
        "Absolute value.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Floor,
        "floor",
        &[],
        "Round down to the nearest integer (as float).",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Ceil,
        "ceil",
        &[],
        "Round up to the nearest integer (as float).",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Round,
        "round",
        &[],
        "Round to the nearest integer (as float).",
        RFC::_009,
        Since(0, 1),
    ),
    info(FloatMethodId::Sin, "sin", &[], "Sine.", RFC::_009, Since(0, 1)),
    info(FloatMethodId::Cos, "cos", &[], "Cosine.", RFC::_009, Since(0, 1)),
    info(FloatMethodId::Tan, "tan", &[], "Tangent.", RFC::_009, Since(0, 1)),
    info(
        FloatMethodId::Exp,
        "exp",
        &[],
        "Exponentiation (e^x).",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Ln,
        "ln",
        &[],
        "Natural logarithm.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Log2,
        "log2",
        &[],
        "Base-2 logarithm.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Log10,
        "log10",
        &[],
        "Base-10 logarithm.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::IsNan,
        "is_nan",
        &[],
        "Return true if this value is NaN.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::IsInfinite,
        "is_infinite",
        &[],
        "Return true if this value is ±infinity.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::IsFinite,
        "is_finite",
        &[],
        "Return true if this value is finite.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Powi,
        "powi",
        &[],
        "Raise to an integer power.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FloatMethodId::Powf,
        "powf",
        &[],
        "Raise to a float power.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a float method spelling to its stable id.
pub fn from_str(name: &str) -> Option<FloatMethodId> {
    super::from_str_impl(FLOAT_METHODS, name)
}

/// Return the canonical spelling for a float method.
pub fn as_str(id: FloatMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a float method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: FloatMethodId) -> &'static FloatMethodInfo {
    super::info_for_impl(FLOAT_METHODS, id, "float method info missing")
}

const fn info(
    id: FloatMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> FloatMethodInfo {
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
