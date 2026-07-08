//! Backend-neutral semantic type model and unstable ABI v0 hooks.
//!
//! This module is intentionally compiler-facing. It gives HIR, semantic facts, diagnostics, and future backends an
//! Incan-owned type vocabulary without treating emitted Rust spelling as the source of language semantics.

use std::fmt;

/// Backend-neutral Incan type universe used by v0.5 middle-end facts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncanType {
    Primitive(IncanPrimitiveType),
    Named(String),
    Generic {
        base: String,
        args: Vec<IncanType>,
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
            | Self::Primitive(IncanPrimitiveType::Bool | IncanPrimitiveType::Unit) => AbiV0Ownership::CopyOrTrivial,
            Self::Ref(_) => AbiV0Ownership::Borrowed,
            Self::RefMut(_) => AbiV0Ownership::MutBorrowed,
            Self::TypeVar(_) | Self::SelfType | Self::Infer | Self::Unknown => AbiV0Ownership::Unknown,
            _ => AbiV0Ownership::Owned,
        }
    }

    /// Infer the broad runtime representation category used by ABI v0 facts.
    fn abi_v0_representation(&self) -> AbiV0Representation {
        match self {
            Self::Primitive(_) => AbiV0Representation::BuiltinValue,
            Self::Named(_) => AbiV0Representation::SourceNominal,
            Self::Generic { .. } => AbiV0Representation::GenericInstance,
            Self::Function { .. } => AbiV0Representation::FunctionValue,
            Self::TypeToken(_) => AbiV0Representation::TypeToken,
            Self::Tuple(_) => AbiV0Representation::Tuple,
            Self::TypeVar(_) => AbiV0Representation::TypeParameter,
            Self::SelfType => AbiV0Representation::SelfType,
            Self::Ref(_) | Self::RefMut(_) => AbiV0Representation::Borrow,
            Self::RustInteropPath(_) => AbiV0Representation::RustInterop,
            Self::Infer | Self::Unknown => AbiV0Representation::Unknown,
        }
    }
}

impl fmt::Display for IncanType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primitive(primitive) => write!(f, "{primitive}"),
            Self::Named(name) | Self::TypeVar(name) => write!(f, "{name}"),
            Self::Generic { base, args } => write_joined_type_args(f, base, args),
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
    Numeric(String),
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
            Self::Numeric(name) => write!(f, "{name}"),
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

/// Runtime service hooks that a type may require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiV0RuntimeRequirement {
    RuntimeHelper(String),
    HostedStd,
    Allocator,
    PanicStrategy,
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
                },
                IncanCallableParam {
                    name: Some("rest".to_string()),
                    ty: IncanType::Primitive(IncanPrimitiveType::Str),
                    kind: IncanCallableParamKind::RestPositional,
                    has_default: false,
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
