//! Backend-neutral semantic type model and unstable ABI v0 hooks.
//!
//! This module is intentionally compiler-facing. It gives HIR, semantic facts, diagnostics, and future backends an
//! Incan-owned type vocabulary without treating emitted Rust spelling as the source of language semantics.

use std::fmt;

use incan_core::lang::types::numerics::{self, NumericTypeId};

use serde::{Deserialize, Serialize};

/// Read the arity of a Rust tuple type spelling, or `None` when the shape cannot be established.
///
/// This is the single place the compiler decides whether an interop value is destructurable, shared by the
/// typechecker's statement/loop destructuring and by Body IR lowering so the two can never disagree about what
/// counts as tuple-shaped. `None` means "not proven to be a tuple" — including opaque named paths like
/// `rust::String` — and callers must refuse rather than assume, because a wrong `yes` re-emits the `.0`/`.1`
/// field projection into a fieldless value that #1132 exists to prevent.
///
/// Parentheses alone do not make a tuple. Rust spells a one-element tuple `(String,)`; plain `(String)` is just a
/// parenthesised `String` and has no `.0` field at all. The distinguishing fact is a comma at depth zero, so a
/// spelling with none is reported as unverifiable rather than as a one-element tuple. Commas nested inside a
/// generic (`(String, HashMap<K, V>)`) are not counted, and a trailing comma does not add an element.
///
/// Parsing a type *string* is a deliberate stopgap. The durable fix is structured Rust type-shape metadata, so
/// that interop shape is a fact the compiler is given rather than one it re-derives from spelling.
pub fn rust_tuple_arity(path: &str) -> Option<usize> {
    let path = path.trim();
    let path = path.strip_prefix("rust::").unwrap_or(path);
    let inner = path.strip_prefix('(')?.strip_suffix(')')?;
    if inner.trim().is_empty() {
        // `()` is the unit type: a genuine zero-element tuple.
        return Some(0);
    }

    let mut depth = 0usize;
    let mut separators = 0usize;
    let mut trailing_separator = false;
    for character in inner.chars() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                separators += 1;
                trailing_separator = true;
                continue;
            }
            _ => {}
        }
        if !character.is_whitespace() {
            trailing_separator = false;
        }
    }

    if separators == 0 {
        // `(String)` — a parenthesised type, not a one-element tuple.
        return None;
    }
    // A trailing comma closes the final element rather than opening another: `(String,)` is one element.
    Some(if trailing_separator { separators } else { separators + 1 })
}

/// Backend-neutral Incan type universe used by v0.5 middle-end facts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncanType {
    /// Compiler-internal bottom type used for diverging interop expressions.
    Never,
    Primitive(IncanPrimitiveType),
    Named(String),
    Generic {
        base: String,
        args: Vec<IncanType>,
    },
    /// A checked fixed-scale decimal type with compiler-owned precision and scale.
    Decimal {
        precision: u8,
        scale: u8,
    },
    Function {
        params: Vec<IncanCallableParam>,
        return_type: Box<IncanType>,
    },
    TypeToken(Box<IncanType>),
    Tuple(Vec<IncanType>),
    TypeVar(String),
    SelfType,
    Ref(Box<IncanType>),
    RefMut(Box<IncanType>),
    RustInteropPath(String),
    Infer,
    Unknown,
}

impl IncanType {
    /// Return unstable ABI v0 metadata scaffolding for this type.
    ///
    /// The result is intentionally conservative. It records identity, ownership/drop policy, representation category,
    /// and explicit slots for future target/runtime facts without promising a stable public ABI.
    pub fn abi_v0_facts(&self) -> AbiV0TypeFacts {
        AbiV0TypeFacts {
            identity: AbiV0TypeIdentity {
                canonical: self.to_string(),
            },
            ownership: self.abi_v0_ownership(),
            runtime_requirements: Vec::new(),
            representation: self.abi_v0_representation(),
            reserved: AbiV0ReservedFacts::default(),
        }
    }

    /// Infer the conservative ownership category used by ABI v0 facts.
    fn abi_v0_ownership(&self) -> AbiV0Ownership {
        match self {
            Self::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Numeric(_))
            | Self::Primitive(IncanPrimitiveType::Bool | IncanPrimitiveType::Unit)
            | Self::Decimal { .. } => AbiV0Ownership::CopyOrTrivial,
            Self::Ref(_) => AbiV0Ownership::Borrowed,
            Self::RefMut(_) => AbiV0Ownership::MutBorrowed,
            Self::Never | Self::TypeVar(_) | Self::SelfType | Self::Infer | Self::Unknown => AbiV0Ownership::Unknown,
            _ => AbiV0Ownership::Owned,
        }
    }

    /// Infer the broad runtime representation category used by ABI v0 facts.
    fn abi_v0_representation(&self) -> AbiV0Representation {
        match self {
            Self::Primitive(_) => AbiV0Representation::BuiltinValue,
            Self::Named(_) => AbiV0Representation::SourceNominal,
            Self::Decimal { .. } => AbiV0Representation::BuiltinValue,
            Self::Generic { .. } => AbiV0Representation::GenericInstance,
            Self::Function { .. } => AbiV0Representation::FunctionValue,
            Self::TypeToken(_) => AbiV0Representation::TypeToken,
            Self::Tuple(_) => AbiV0Representation::Tuple,
            Self::TypeVar(_) => AbiV0Representation::TypeParameter,
            Self::SelfType => AbiV0Representation::SelfType,
            Self::Ref(_) | Self::RefMut(_) => AbiV0Representation::Borrow,
            Self::RustInteropPath(_) => AbiV0Representation::RustInterop,
            Self::Never | Self::Infer | Self::Unknown => AbiV0Representation::Unknown,
        }
    }
}

impl fmt::Display for IncanType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Never => write!(f, "!"),
            Self::Primitive(primitive) => write!(f, "{primitive}"),
            Self::Named(name) | Self::TypeVar(name) => write!(f, "{name}"),
            Self::Generic { base, args } => write_joined_type_args(f, base, args),
            Self::Decimal { precision, scale } => write!(f, "decimal[{precision}, {scale}]"),
            Self::Function { params, return_type } => {
                write!(f, "(")?;
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {return_type}")
            }
            Self::TypeToken(inner) => write!(f, "Type[{inner}]"),
            Self::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
            Self::SelfType => write!(f, "Self"),
            Self::Ref(inner) => write!(f, "&{inner}"),
            Self::RefMut(inner) => write!(f, "&mut {inner}"),
            Self::RustInteropPath(path) => write!(f, "rust::{path}"),
            Self::Infer => write!(f, "_"),
            Self::Unknown => write!(f, "?"),
        }
    }
}

/// Write `base[arg, ...]` type displays without allocating an intermediate string.
fn write_joined_type_args(f: &mut fmt::Formatter<'_>, base: &str, args: &[IncanType]) -> fmt::Result {
    write!(f, "{base}[")?;
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{arg}")?;
    }
    write!(f, "]")
}

/// Primitive and primitive-like Incan types with compiler-owned semantics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncanPrimitiveType {
    Int,
    Float,
    Numeric(NumericTypeId),
    Bool,
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    Unit,
}

impl fmt::Display for IncanPrimitiveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "int"),
            Self::Float => write!(f, "float"),
            Self::Numeric(id) => write!(f, "{}", numerics::as_str(*id)),
            Self::Bool => write!(f, "bool"),
            Self::Str => write!(f, "str"),
            Self::Bytes => write!(f, "bytes"),
            Self::FrozenStr => write!(f, "FrozenStr"),
            Self::FrozenBytes => write!(f, "FrozenBytes"),
            Self::Unit => write!(f, "Unit"),
        }
    }
}

/// Callable parameter metadata preserved in semantic function types.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IncanCallableParam {
    pub name: Option<String>,
    pub ty: IncanType,
    pub kind: IncanCallableParamKind,
    pub has_default: bool,
    /// Whether this local partial parameter is defaulted from its closure's construction-time capture.
    ///
    /// A caller may override this parameter by name. Positional invocation instead skips it and fills the remaining
    /// residual parameters in declaration order. Ordinary callable parameters always set this to `false`.
    pub is_partial_preset: bool,
}

impl fmt::Display for IncanCallableParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            IncanCallableParamKind::Normal => write!(f, "{}", self.ty),
            IncanCallableParamKind::RestPositional => write!(f, "*{}", self.ty),
            IncanCallableParamKind::RestKeyword => write!(f, "**{}", self.ty),
        }
    }
}

/// Source-level parameter shape for semantic callable types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncanCallableParamKind {
    Normal,
    RestPositional,
    RestKeyword,
}

/// Unstable ABI v0 metadata for one semantic type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiV0TypeFacts {
    pub identity: AbiV0TypeIdentity,
    pub ownership: AbiV0Ownership,
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    pub representation: AbiV0Representation,
    pub reserved: AbiV0ReservedFacts,
}

/// ABI v0 type identity. This is not a stable public ABI symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiV0TypeIdentity {
    pub canonical: String,
}

/// Conservative ownership/drop policy hook for ABI v0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiV0Ownership {
    CopyOrTrivial,
    Owned,
    Borrowed,
    MutBorrowed,
    Unknown,
}

impl AbiV0Ownership {
    /// Return whether this category is a trivial bitwise copy.
    ///
    /// Body IR v0 uses this to decide whether a place-read gets an
    /// [`OwnershipFact::Copy`](crate::body_ir::OwnershipFact::Copy) decision outright, or needs a move/clone/borrow
    /// refinement based on last-use analysis. Borrowed/MutBorrowed and Unknown are never trivially copy: a borrow
    /// still needs its own explicit reference decision, and an unknown ownership category must not be silently
    /// treated as copyable.
    pub const fn is_trivially_copy(self) -> bool {
        matches!(self, Self::CopyOrTrivial)
    }
}

/// Runtime service hooks that a type may require.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbiV0RuntimeRequirement {
    RuntimeHelper(String),
    HostedStd,
    Allocator,
    PanicStrategy,
    /// An async task runtime, required by a body containing `await` or `race for` (#1164).
    ///
    /// Mirrors the surface-level [`crate::RuntimeRequirement::AsyncRuntime`] fact so a consumer reads the
    /// requirement off the body it applies to, instead of re-deriving it by scanning the program's imports and
    /// declaration modifiers.
    AsyncRuntime,
}

/// ABI representation category known to the compiler today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiV0Representation {
    BuiltinValue,
    SourceNominal,
    GenericInstance,
    FunctionValue,
    TypeToken,
    Tuple,
    TypeParameter,
    SelfType,
    Borrow,
    RustInterop,
    Unknown,
}

/// Reserved ABI v0 slots for target/runtime facts that are not implemented yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AbiV0ReservedFacts {
    pub layout: Option<String>,
    pub repr: Option<String>,
    pub alignment: Option<String>,
    pub no_std_availability: Option<String>,
    pub panic_strategy: Option<String>,
    pub allocator: Option<String>,
    pub target_profile: Option<String>,
    pub runtime_layer: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_types_render_canonical_incan_spelling() {
        let ty = IncanType::Function {
            params: vec![
                IncanCallableParam {
                    name: Some("item".to_string()),
                    ty: IncanType::Generic {
                        base: "List".to_string(),
                        args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
                    },
                    kind: IncanCallableParamKind::Normal,
                    has_default: false,
                    is_partial_preset: false,
                },
                IncanCallableParam {
                    name: Some("rest".to_string()),
                    ty: IncanType::Primitive(IncanPrimitiveType::Str),
                    kind: IncanCallableParamKind::RestPositional,
                    has_default: false,
                    is_partial_preset: false,
                },
            ],
            return_type: Box::new(IncanType::Tuple(vec![
                IncanType::Primitive(IncanPrimitiveType::Bool),
                IncanType::RustInteropPath("std::path::PathBuf".to_string()),
            ])),
        };

        assert_eq!(ty.to_string(), "(List[int], *str) -> (bool, rust::std::path::PathBuf)");
    }

    #[test]
    fn semantic_never_type_is_internal_and_representation_free() {
        let facts = IncanType::Never.abi_v0_facts();

        assert_eq!(facts.identity.canonical, "!");
        assert_eq!(facts.ownership, AbiV0Ownership::Unknown);
        assert_eq!(facts.representation, AbiV0Representation::Unknown);
    }

    #[test]
    fn abi_v0_facts_mark_known_and_reserved_slots() {
        let borrowed = IncanType::Ref(Box::new(IncanType::Primitive(IncanPrimitiveType::Str))).abi_v0_facts();
        let interop = IncanType::RustInteropPath("rubato::Fft".to_string()).abi_v0_facts();

        assert_eq!(borrowed.ownership, AbiV0Ownership::Borrowed);
        assert_eq!(borrowed.representation, AbiV0Representation::Borrow);
        assert_eq!(interop.identity.canonical, "rust::rubato::Fft");
        assert_eq!(interop.representation, AbiV0Representation::RustInterop);
        assert_eq!(interop.reserved, AbiV0ReservedFacts::default());
    }
}

#[cfg(test)]
mod rust_tuple_arity_tests {
    use super::rust_tuple_arity;

    /// Parentheses alone do not make a tuple (#1132).
    ///
    /// Reading `(String)` as a one-element tuple would let a one-name destructure lower to `.0` on a `String`,
    /// recreating the raw-Rust failure through a narrower spelling than the `int` case the issue started from.
    #[test]
    fn a_parenthesised_type_is_not_a_one_element_tuple() {
        assert_eq!(rust_tuple_arity("(String)"), None);
        assert_eq!(rust_tuple_arity("(std::vec::Vec<u8>)"), None);
        assert_eq!(rust_tuple_arity("( String )"), None);
    }

    #[test]
    fn a_trailing_comma_marks_a_genuine_one_element_tuple() {
        assert_eq!(rust_tuple_arity("(String,)"), Some(1));
        assert_eq!(rust_tuple_arity("(String, )"), Some(1));
    }

    #[test]
    fn multi_element_and_nested_generic_spellings_keep_their_arity() {
        assert_eq!(rust_tuple_arity("(A, B)"), Some(2));
        assert_eq!(rust_tuple_arity("(A, B,)"), Some(2));
        assert_eq!(rust_tuple_arity("(String,incan_stdlib::json::JsonValue)"), Some(2));
        // A generic's own commas sit at depth one and must not inflate the count.
        assert_eq!(rust_tuple_arity("(String, HashMap<K, V>)"), Some(2));
        assert_eq!(rust_tuple_arity("(HashMap<K, V>, Vec<(A, B)>)"), Some(2));
    }

    #[test]
    fn unit_is_a_zero_element_tuple_and_opaque_paths_stay_unverifiable() {
        assert_eq!(rust_tuple_arity("()"), Some(0));
        assert_eq!(rust_tuple_arity("String"), None);
        assert_eq!(rust_tuple_arity("std::collections::HashMap<K, V>"), None);
        assert_eq!(rust_tuple_arity("rust::(A, B)"), Some(2));
    }
}
