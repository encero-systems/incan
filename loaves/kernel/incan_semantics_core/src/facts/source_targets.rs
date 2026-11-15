//! Semantic source targets and source target kinds.

use super::*;

/// Compiler-proven source declaration target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticSourceTarget {
    pub module_path: Vec<String>,
    pub name: String,
    pub kind: SemanticSourceTargetKind,
}

impl SemanticSourceTarget {
    /// Build a source target from structured module path, name, and kind fields.
    pub fn new(module_path: Vec<String>, name: impl Into<String>, kind: SemanticSourceTargetKind) -> Self {
        Self {
            module_path,
            name: name.into(),
            kind,
        }
    }

    /// Build a source target while accepting the current frontend declaration kind spelling.
    pub fn from_kind_str(module_path: Vec<String>, name: impl Into<String>, kind: &str) -> Self {
        Self::new(module_path, name, SemanticSourceTargetKind::from_kind_str(kind))
    }
}

impl fmt::Display for SemanticSourceTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.module_path.is_empty() {
            write!(f, "{}:<module>::{}", self.kind, self.name)
        } else {
            write!(f, "{}:{}::{}", self.kind, self.module_path.join("::"), self.name)
        }
    }
}

/// Declaration category a canonical identity or semantic target names.
///
/// This is RFC 120's `kind` field. It deliberately covers every binding form the identity model reaches, not only the
/// declaration kinds today's codegraph targets happen to record, so a member and a local never have to be told apart
/// by a string. [`Self::Other`] remains for a frontend spelling this vocabulary has not adopted yet; it is a gap
/// marker, never a category.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticSourceTargetKind {
    /// A `def` declaration, free or associated.
    Function,
    /// A `model` declaration.
    Model,
    /// A `class` declaration.
    Class,
    /// A `newtype` declaration.
    Newtype,
    /// A `rusttype` declaration binding a Rust type.
    Rusttype,
    /// An `enum` declaration.
    Enum,
    /// A `type X = ...` alias.
    TypeAlias,
    /// A `partial` projection declaration.
    Partial,
    /// One variant of an enum.
    Variant,
    /// A `trait` declaration.
    Trait,
    /// A `capability` declaration naming an RFC 104 ambient runtime authority.
    Capability,
    /// A field on a nominal type.
    Field,
    /// A method on a nominal type or trait.
    Method,
    /// A computed property on a nominal type.
    Property,
    /// A `const` declaration.
    Const,
    /// A `static` storage cell.
    Static,
    /// A binding introduced inside a body by `let`, `mut`, assignment, `for`, `with ... as`, or `except ... as`.
    Local,
    /// A declared callable parameter.
    Parameter,
    /// A receiver binding (`self` or `cls`).
    Receiver,
    /// A generic type parameter, scoped to the declaration that introduces it.
    GenericBinder,
    /// A module.
    Module,
    /// An item reached through `rust::`.
    RustItem,
    /// A compiler-owned builtin beneath the ordinary lexical scope chain.
    Builtin,
    /// A frontend declaration spelling this vocabulary has not adopted. A gap marker, not a category: a consumer that
    /// branches on it is branching on a string.
    Other(String),
}

impl SemanticSourceTargetKind {
    /// Convert the current frontend declaration kind spelling into a semantic target kind.
    pub fn from_kind_str(kind: &str) -> Self {
        match kind {
            "function" => Self::Function,
            "model" => Self::Model,
            "class" => Self::Class,
            "newtype" => Self::Newtype,
            "rusttype" => Self::Rusttype,
            "enum" => Self::Enum,
            "type_alias" => Self::TypeAlias,
            "partial" => Self::Partial,
            "variant" => Self::Variant,
            "trait" => Self::Trait,
            "capability" => Self::Capability,
            "field" => Self::Field,
            "method" => Self::Method,
            "property" => Self::Property,
            "const" => Self::Const,
            "static" => Self::Static,
            "local" => Self::Local,
            "parameter" => Self::Parameter,
            "receiver" => Self::Receiver,
            "generic_binder" => Self::GenericBinder,
            "module" => Self::Module,
            "rust_item" => Self::RustItem,
            "builtin" => Self::Builtin,
            other => Self::Other(other.to_string()),
        }
    }

    /// Return the compact snapshot spelling for this source target kind.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Function => "function",
            Self::Model => "model",
            Self::Class => "class",
            Self::Newtype => "newtype",
            Self::Rusttype => "rusttype",
            Self::Enum => "enum",
            Self::TypeAlias => "type_alias",
            Self::Partial => "partial",
            Self::Variant => "variant",
            Self::Trait => "trait",
            Self::Capability => "capability",
            Self::Field => "field",
            Self::Method => "method",
            Self::Property => "property",
            Self::Const => "const",
            Self::Static => "static",
            Self::Local => "local",
            Self::Parameter => "parameter",
            Self::Receiver => "receiver",
            Self::GenericBinder => "generic_binder",
            Self::Module => "module",
            Self::RustItem => "rust_item",
            Self::Builtin => "builtin",
            Self::Other(kind) => kind,
        }
    }
}

impl fmt::Display for SemanticSourceTargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
