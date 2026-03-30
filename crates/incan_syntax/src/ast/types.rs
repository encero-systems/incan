//! Type AST node and its `Display` implementation.

use std::fmt;

use super::{Ident, Spanned};

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
    /// Function type: `(int, str) -> bool`
    Function(Vec<Spanned<Type>>, Box<Spanned<Type>>),
    /// Unit type
    Unit,
    /// Tuple type: `(int, str)`
    Tuple(Vec<Spanned<Type>>),
    /// Self type - refers to the implementing type in traits
    SelfType,
}

impl fmt::Display for Type {
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
        }
    }
}
