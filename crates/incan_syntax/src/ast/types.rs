//! Type AST node and its `Display` implementation.

use std::fmt;

use super::{Ident, IntLiteral, Spanned};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Simple type: `int`, `str`, `MyType`
    Simple(Ident),
    /// Rust-style qualified path in type position: `proto_mod::Binary`, `std::time::Instant`.
    ///
    /// At least two segments. Used with `rusttype` when the backing type lives under an imported Rust module binding.
    Qualified(Vec<Ident>),
    /// Generic type: `List[T]`, `Result[T, E]`
    Generic(Ident, Vec<Spanned<Type>>),
    /// Primitive type with RFC 017 constraint predicates, such as `int[ge=0]`.
    ConstrainedPrimitive(Ident, Vec<Spanned<TypeConstraint>>),
    /// Integer literal in type-argument position, used by parameterized numeric types such as `decimal[10, 2]`.
    IntLiteral(IntLiteral),
    /// Function type: `(int, str) -> bool`
    Function(Vec<Spanned<Type>>, Box<Spanned<Type>>),
    /// Immutable reference type: `&T`
    Ref(Box<Spanned<Type>>),
    /// Mutable reference type: `&mut T`
    RefMut(Box<Spanned<Type>>),
    /// Unit type
    Unit,
    /// Tuple type: `(int, str)`
    Tuple(Vec<Spanned<Type>>),
    /// Self type - refers to the implementing type in traits
    SelfType,
    /// Call-site type inference placeholder (`_` in `f[int, _](...)`), RFC 054.
    Infer,
}

/// Ordered comparison key accepted inside an RFC 017 constrained primitive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeConstraintKey {
    /// Greater than or equal: `ge=...`.
    Ge,
    /// Strictly greater than: `gt=...`.
    Gt,
    /// Less than or equal: `le=...`.
    Le,
    /// Strictly less than: `lt=...`.
    Lt,
}

impl TypeConstraintKey {
    /// Parse the source spelling for a constrained primitive comparison key.
    pub fn parse_spelling(value: &str) -> Option<Self> {
        match value {
            "ge" => Some(Self::Ge),
            "gt" => Some(Self::Gt),
            "le" => Some(Self::Le),
            "lt" => Some(Self::Lt),
            _ => None,
        }
    }

    /// Return the canonical source spelling for the constraint key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ge => "ge",
            Self::Gt => "gt",
            Self::Le => "le",
            Self::Lt => "lt",
        }
    }
}

impl fmt::Display for TypeConstraintKey {
    /// Format the constraint key using its canonical RFC 017 spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One parsed RFC 017 primitive type constraint, preserving the literal spelling for formatting.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeConstraint {
    /// Constraint comparison operator.
    pub key: TypeConstraintKey,
    /// Integer literal constraint value accepted by this parser slice.
    pub value: IntLiteral,
}

impl fmt::Display for Type {
    /// Format a type using Incan source syntax.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Simple(name) => write!(f, "{}", name),
            Type::Qualified(segments) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        write!(f, "::")?;
                    }
                    write!(f, "{}", seg)?;
                }
                Ok(())
            }
            Type::Generic(name, args) => {
                write!(f, "{}[", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg.node)?;
                }
                write!(f, "]")
            }
            Type::ConstrainedPrimitive(name, constraints) => {
                write!(f, "{}[", name)?;
                for (i, constraint) in constraints.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}={}", constraint.node.key, constraint.node.value.repr)?;
                }
                write!(f, "]")
            }
            Type::IntLiteral(value) => write!(f, "{}", value.repr),
            Type::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p.node)?;
                }
                write!(f, ") -> {}", ret.node)
            }
            Type::Ref(inner) => write!(f, "&{}", inner.node),
            Type::RefMut(inner) => write!(f, "&mut {}", inner.node),
            Type::Unit => write!(f, "Unit"),
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e.node)?;
                }
                write!(f, ")")
            }
            Type::SelfType => write!(f, "Self"),
            Type::Infer => write!(f, "_"),
        }
    }
}
