//! Semantic registry subject kinds, values and entries.

use super::*;

/// Compiler-recognized registry subject category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticRegistrySubjectKind {
    Function,
    Method,
    CompilationUnit,
    Package,
}

impl SemanticRegistrySubjectKind {
    /// Return the stable machine spelling of this registry subject category.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::CompilationUnit => "compilation_unit",
            Self::Package => "package",
        }
    }
}

impl fmt::Display for SemanticRegistrySubjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source-level value that the compiler can snapshot without evaluating user code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SemanticRegistryValue {
    Int(i64),
    Float(String),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    /// A concrete Incan type token retained as its canonical checked spelling.
    Type(String),
    Option(Box<SemanticRegistryValue>),
    List(Vec<SemanticRegistryValue>),
    Dict(Vec<(SemanticRegistryValue, SemanticRegistryValue)>),
    ConstRef(Vec<String>),
    Newtype {
        name: String,
        value: Box<SemanticRegistryValue>,
    },
    Model {
        name: String,
        fields: Vec<(String, SemanticRegistryValue)>,
    },
}

impl fmt::Display for SemanticRegistryValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => f.write_str(value),
            Self::Bool(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "{value:?}"),
            Self::Bytes(value) => write!(f, "bytes:{value:?}"),
            Self::None => f.write_str("None"),
            Self::Type(value) => write!(f, "Type[{value}]"),
            Self::Option(value) => write!(f, "Some({value})"),
            Self::List(values) => {
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    value.fmt(f)?;
                }
                f.write_str("]")
            }
            Self::Dict(entries) => {
                f.write_str("{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    key.fmt(f)?;
                    f.write_str(": ")?;
                    value.fmt(f)?;
                }
                f.write_str("}")
            }
            Self::ConstRef(path) => f.write_str(&path.join(".")),
            Self::Newtype { name, value } => write!(f, "{name}({value})"),
            Self::Model { name, fields } => {
                write!(f, "{name}(")?;
                for (index, (field, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{field}={value}")?;
                }
                f.write_str(")")
            }
        }
    }
}

/// One checked registry entry, preserved as structured source data rather than a rendered descriptor string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticRegistryEntry {
    pub registry: CompilerNodeId,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    pub subject_identity: String,
}

impl fmt::Display for SemanticRegistryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "registry={} key={} descriptor={} subject={}:{}",
            self.registry, self.key, self.descriptor, self.subject_kind, self.subject_identity
        )
    }
}
