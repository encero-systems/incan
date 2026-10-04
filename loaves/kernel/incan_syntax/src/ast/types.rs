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
    /// Namespace-qualified Incan type path: `c.i32`, `json.Value`.
    ///
    /// Unlike [`Type::Qualified`], this preserves ordinary Incan namespace spelling rather than a Rust path.
    Dotted(Vec<Ident>),
    /// Generic type: `List[T]`, `Result[T, E]`
    Generic(Ident, Vec<Spanned<Type>>),
    /// Generic type whose constructor is namespace-qualified: `c.ConstPtr[c.i32]`.
    DottedGeneric(Vec<Ident>, Vec<Spanned<Type>>),
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
    /// `mut`-marked parameter of a callable type: the `mut T` in `(mut T, int) -> R`.
    ///
    /// The marker says the callable's changes to that argument are visible to the caller, which is what a `mut`
    /// parameter of a `def` already means. The parser produces it only for a parameter of a `(...) -> R` callable type
    /// and refuses it in a tuple or grouped type.
    MutParam(Box<Spanned<Type>>),
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

impl Type {
    /// Return whether this annotation mentions `name` as a bare or applied type name anywhere in its structure.
    ///
    /// This is a purely syntactic query over the annotation: `name` counts when it is the whole type (`T`), the head
    /// of a generic application (`T[int]`), a generic argument (`list[T]`, `(int, T)`, `(T) -> int`), or a
    /// constrained-primitive base. Namespace-qualified and Rust-qualified paths are compared by their first segment
    /// only, which is never a type parameter, so they do not count. The typechecker uses this to decide whether a
    /// declared type parameter is stored by a declaration's representation (#1370).
    pub fn mentions_name(&self, name: &str) -> bool {
        match self {
            Type::Simple(ident) | Type::ConstrainedPrimitive(ident, _) => ident == name,
            Type::Generic(head, args) => head == name || args.iter().any(|arg| arg.node.mentions_name(name)),
            Type::DottedGeneric(_, args) => args.iter().any(|arg| arg.node.mentions_name(name)),
            Type::Function(params, ret) => {
                params.iter().any(|param| param.node.mentions_name(name)) || ret.node.mentions_name(name)
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => inner.node.mentions_name(name),
            Type::Tuple(elems) => elems.iter().any(|elem| elem.node.mentions_name(name)),
            Type::Qualified(_) | Type::Dotted(_) | Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {
                false
            }
        }
    }
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
            Type::Dotted(segments) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        write!(f, ".")?;
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
            Type::DottedGeneric(segments, args) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        write!(f, ".")?;
                    }
                    write!(f, "{}", seg)?;
                }
                write!(f, "[")?;
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
            Type::MutParam(inner) => write!(f, "mut {}", inner.node),
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

#[cfg(test)]
mod tests {
    use super::super::Span;
    use super::{Spanned, Type};

    fn spanned(ty: Type) -> Spanned<Type> {
        Spanned::new(ty, Span::default())
    }

    fn simple(name: &str) -> Spanned<Type> {
        spanned(Type::Simple(name.to_string()))
    }

    /// Issue #1370: a parameter counts wherever it appears as a type name, and a qualified path's segments are
    /// never parameters.
    #[test]
    fn mentions_name_finds_the_parameter_at_any_nesting() {
        assert!(Type::Simple("T".to_string()).mentions_name("T"));
        assert!(!Type::Simple("str".to_string()).mentions_name("T"));
        assert!(Type::Generic("T".to_string(), vec![simple("int")]).mentions_name("T"));
        assert!(
            Type::Generic(
                "Dict".to_string(),
                vec![
                    simple("str"),
                    spanned(Type::Generic("list".to_string(), vec![simple("T")]))
                ]
            )
            .mentions_name("T")
        );
        assert!(Type::Tuple(vec![simple("int"), simple("T")]).mentions_name("T"));
        assert!(Type::Function(vec![simple("int")], Box::new(simple("T"))).mentions_name("T"));
        assert!(Type::Ref(Box::new(simple("T"))).mentions_name("T"));
        assert!(Type::MutParam(Box::new(simple("T"))).mentions_name("T"));
        assert!(Type::DottedGeneric(vec!["c".to_string(), "Ptr".to_string()], vec![simple("T")]).mentions_name("T"));
        assert!(!Type::Qualified(vec!["T".to_string(), "Inner".to_string()]).mentions_name("T"));
        assert!(!Type::Dotted(vec!["T".to_string(), "Inner".to_string()]).mentions_name("T"));
        assert!(!Type::SelfType.mentions_name("T"));
    }
}
