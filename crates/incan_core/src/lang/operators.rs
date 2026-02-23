//! Operator vocabulary.
//!
//! This module defines the canonical operator set (symbol operators like `+` and word operators
//! like `and`) along with basic metadata such as precedence, associativity, and fixity.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**.
//! - Some operators are spelled using reserved words (e.g. `"and"`). Those entries have
//!   [`OperatorInfo::is_keyword_spelling`] set to `true`.
//! - Word-operator spellings may also appear in the keyword registry ([`crate::lang::keywords`]); use this module when
//!   you need operator semantics like precedence.
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::operators::{self, OperatorId};
//!
//! assert_eq!(operators::from_str("+"), Some(OperatorId::Plus));
//! assert_eq!(operators::info_for(OperatorId::Plus).precedence, 50);
//! ```

use super::registry::{Example, RFC, RfcId, Since, Stability};

/// Define how operators associate when chained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Associativity {
    Left,
    Right,
    None,
}

/// Define whether an operator is infix (binary) or prefix (unary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fixity {
    Infix,
    Prefix,
}

/// Stable identifier for every operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorId {
    // Arithmetic
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    SlashSlash,
    Percent,

    // Comparison
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    // Assignment
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    SlashSlashEq,
    PercentEq,

    // Ranges
    DotDot,
    DotDotEq,

    // Word operators
    And,
    Or,
    Not,
    In,
    Is,
}

/// Metadata for an operator.
///
/// ## Notes
/// - `spellings` may contain multiple accepted spellings for the same operator id (synonyms).
/// - `precedence` is a relative ordering where higher binds tighter. The absolute scale is an implementation detail,
///   but must be consistent across the parser.
#[derive(Debug, Clone, Copy)]
pub struct OperatorInfo {
    pub id: OperatorId,
    pub spellings: &'static [&'static str],
    pub precedence: u8,
    pub associativity: Associativity,
    pub fixity: Fixity,
    pub is_keyword_spelling: bool,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of all operators.
pub const OPERATORS: &[OperatorInfo] = &[
    // Arithmetic
    op(
        OperatorId::Plus,
        &["+"],
        50,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Minus,
        &["-"],
        50,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Star,
        &["*"],
        60,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::StarStar,
        &["**"],
        70,
        Associativity::Right,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Slash,
        &["/"],
        60,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::SlashSlash,
        &["//"],
        60,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Percent,
        &["%"],
        60,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    // Comparison
    op(
        OperatorId::EqEq,
        &["=="],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::NotEq,
        &["!="],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Lt,
        &["<"],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::LtEq,
        &["<="],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Gt,
        &[">"],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::GtEq,
        &[">="],
        40,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    // Assignment
    op(
        OperatorId::Eq,
        &["="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::PlusEq,
        &["+="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::MinusEq,
        &["-="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::StarEq,
        &["*="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::SlashEq,
        &["/="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::SlashSlashEq,
        &["//="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::PercentEq,
        &["%="],
        10,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    // Ranges
    op(
        OperatorId::DotDot,
        &[".."],
        30,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::DotDotEq,
        &["..="],
        30,
        Associativity::Left,
        Fixity::Infix,
        false,
        RFC::_000,
        Since(0, 1),
    ),
    // Word operators (keyword spellings)
    op(
        OperatorId::And,
        &["and"],
        35,
        Associativity::Left,
        Fixity::Infix,
        true,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Or,
        &["or"],
        35,
        Associativity::Left,
        Fixity::Infix,
        true,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Not,
        &["not"],
        45,
        Associativity::Left,
        Fixity::Prefix,
        true,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::In,
        &["in"],
        35,
        Associativity::Left,
        Fixity::Infix,
        true,
        RFC::_000,
        Since(0, 1),
    ),
    op(
        OperatorId::Is,
        &["is"],
        35,
        Associativity::Left,
        Fixity::Infix,
        true,
        RFC::_000,
        Since(0, 1),
    ),
];

/// Return the full metadata entry for an operator.
///
/// ## Parameters
/// - `id`: Operator identifier.
///
/// ## Returns
/// - The associated [`OperatorInfo`] from [`OPERATORS`].
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: OperatorId) -> &'static OperatorInfo {
    OPERATORS
        .iter()
        .find(|o| o.id == id)
        .expect("INVARIANT: operator info missing")
}

/// Resolve an operator spelling to its identifier.
///
/// ## Parameters
/// - `spelling`: Candidate operator token (symbol or word operator).
///
/// ## Returns
/// - `Some(OperatorId)` if the spelling exists in [`OPERATORS`].
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-sensitive**.
pub fn from_str(spelling: &str) -> Option<OperatorId> {
    OPERATORS
        .iter()
        .find(|o| {
            let spellings: &[&str] = o.spellings;
            spellings.contains(&spelling)
        })
        .map(|o| o.id)
}

// --- helpers -----------------------------------------------------------------
#[allow(clippy::too_many_arguments)]
const fn op(
    id: OperatorId,
    spellings: &'static [&'static str],
    precedence: u8,
    associativity: Associativity,
    fixity: Fixity,
    is_keyword_spelling: bool,
    introduced_in_rfc: RfcId,
    since: Since,
) -> OperatorInfo {
    OperatorInfo {
        id,
        spellings,
        precedence,
        associativity,
        fixity,
        is_keyword_spelling,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
