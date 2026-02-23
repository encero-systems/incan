//! `math` surface vocabulary.
//!
//! This is the vocabulary for the `math` module used by docs/examples (`math.sqrt(...)`, `math.pi`, ...).

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

pub const MATH_MODULE_NAME: &str = "math";

/// Stable identifier for `math.<fn>(...)` functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MathFnId {
    Sqrt,
    Abs,
    Floor,
    Ceil,
    Pow,
    Exp,
    Log,
    Log10,
    Log2,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Atan2,
}

pub type MathFnInfo = LangItemInfo<MathFnId>;

pub const MATH_FUNCTIONS: &[MathFnInfo] = &[
    info_fn(MathFnId::Sqrt, "sqrt", &[], "Square root.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Abs, "abs", &[], "Absolute value.", RFC::_000, Since(0, 1)),
    info_fn(
        MathFnId::Floor,
        "floor",
        &[],
        "Floor (round down).",
        RFC::_000,
        Since(0, 1),
    ),
    info_fn(MathFnId::Ceil, "ceil", &[], "Ceil (round up).", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Pow, "pow", &[], "Power function.", RFC::_000, Since(0, 1)),
    info_fn(
        MathFnId::Exp,
        "exp",
        &[],
        "Exponentiation (e^x).",
        RFC::_000,
        Since(0, 1),
    ),
    info_fn(MathFnId::Log, "log", &[], "Natural logarithm.", RFC::_000, Since(0, 1)),
    info_fn(
        MathFnId::Log10,
        "log10",
        &[],
        "Base-10 logarithm.",
        RFC::_000,
        Since(0, 1),
    ),
    info_fn(MathFnId::Log2, "log2", &[], "Base-2 logarithm.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Sin, "sin", &[], "Sine.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Cos, "cos", &[], "Cosine.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Tan, "tan", &[], "Tangent.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Asin, "asin", &[], "Arcsine.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Acos, "acos", &[], "Arccosine.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Atan, "atan", &[], "Arctangent.", RFC::_000, Since(0, 1)),
    info_fn(MathFnId::Sinh, "sinh", &[], "Hyperbolic sine.", RFC::_000, Since(0, 1)),
    info_fn(
        MathFnId::Cosh,
        "cosh",
        &[],
        "Hyperbolic cosine.",
        RFC::_000,
        Since(0, 1),
    ),
    info_fn(
        MathFnId::Tanh,
        "tanh",
        &[],
        "Hyperbolic tangent.",
        RFC::_000,
        Since(0, 1),
    ),
    info_fn(
        MathFnId::Atan2,
        "atan2",
        &[],
        "Two-argument arctangent.",
        RFC::_000,
        Since(0, 1),
    ),
];

pub fn fn_from_str(name: &str) -> Option<MathFnId> {
    if let Some(f) = MATH_FUNCTIONS.iter().find(|f| f.canonical == name) {
        return Some(f.id);
    }
    MATH_FUNCTIONS
        .iter()
        .find(|f| {
            let aliases: &[&str] = f.aliases;
            aliases.contains(&name)
        })
        .map(|f| f.id)
}

pub fn fn_as_str(id: MathFnId) -> &'static str {
    fn_info_for(id).canonical
}

pub fn fn_info_for(id: MathFnId) -> &'static MathFnInfo {
    MATH_FUNCTIONS
        .iter()
        .find(|f| f.id == id)
        .expect("INVARIANT: math fn info missing")
}

/// Stable identifier for `math.<const>` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MathConstId {
    Pi,
    E,
    Tau,
    Inf,
    Nan,
}

pub type MathConstInfo = LangItemInfo<MathConstId>;

pub const MATH_CONSTANTS: &[MathConstInfo] = &[
    info_const(MathConstId::Pi, "pi", &[], "The constant π.", RFC::_000, Since(0, 1)),
    info_const(MathConstId::E, "e", &[], "The constant e.", RFC::_000, Since(0, 1)),
    info_const(
        MathConstId::Tau,
        "tau",
        &[],
        "The constant τ (2π).",
        RFC::_000,
        Since(0, 1),
    ),
    info_const(
        MathConstId::Inf,
        "inf",
        &[],
        "Positive infinity.",
        RFC::_000,
        Since(0, 1),
    ),
    info_const(
        MathConstId::Nan,
        "nan",
        &[],
        "Not a number (NaN).",
        RFC::_000,
        Since(0, 1),
    ),
];

pub fn const_from_str(name: &str) -> Option<MathConstId> {
    if let Some(c) = MATH_CONSTANTS.iter().find(|c| c.canonical == name) {
        return Some(c.id);
    }
    MATH_CONSTANTS
        .iter()
        .find(|c| {
            let aliases: &[&str] = c.aliases;
            aliases.contains(&name)
        })
        .map(|c| c.id)
}

pub fn const_as_str(id: MathConstId) -> &'static str {
    const_info_for(id).canonical
}

pub fn const_info_for(id: MathConstId) -> &'static MathConstInfo {
    MATH_CONSTANTS
        .iter()
        .find(|c| c.id == id)
        .expect("INVARIANT: math const info missing")
}

const fn info_fn(
    id: MathFnId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> MathFnInfo {
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

const fn info_const(
    id: MathConstId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> MathConstInfo {
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
