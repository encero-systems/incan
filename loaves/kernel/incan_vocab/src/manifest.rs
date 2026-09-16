//! Shared manifest DTOs carried inside a vocabulary registration.
//!
//! The types in this module are intentionally plain data structures. They are designed to be easy for companion crates
//! to construct and easy for the compiler to serialize into library artifacts.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Manifest format version for forward-compatible evolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum ManifestFormatVersion {
    /// Initial stable manifest DTO shape.
    #[default]
    V1,
}

/// Machine-readable library surface metadata contributed by a companion crate registration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LibraryManifest {
    /// Version of the DTO contract used by this payload.
    pub format_version: ManifestFormatVersion,
    /// Modules exposed by the library's vocab companion metadata.
    #[cfg_attr(feature = "serde", serde(default))]
    pub modules: Vec<ModuleExport>,
    /// Named helper bindings that desugarers may reference symbolically.
    ///
    /// Each binding maps a stable helper key such as `filter` to a public library export that the
    /// compiler can import under a hidden alias before lowering desugared code back into the host
    /// AST.
    #[cfg_attr(feature = "serde", serde(default))]
    pub helper_bindings: Vec<HelperBinding>,
    /// Additional Cargo dependencies required by the library's generated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub required_dependencies: Vec<CargoDependency>,
    /// Stdlib feature flags required by the library's generated surface.
    #[cfg_attr(feature = "serde", serde(default))]
    pub required_stdlib_features: Vec<String>,
}

/// Stable symbolic binding for a helper function used by desugarers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HelperBinding {
    /// Desugarer-facing helper key, for example `filter`.
    pub key: String,
    /// Public library export name that should be imported when the helper is used.
    pub exported_name: String,
}

/// Metadata for one library module.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ModuleExport {
    /// Logical module path using Incan's import spelling.
    pub path: String,
    /// Functions exported from this module.
    #[cfg_attr(feature = "serde", serde(default))]
    pub functions: Vec<FunctionExport>,
    /// Types exported from this module.
    #[cfg_attr(feature = "serde", serde(default))]
    pub types: Vec<TypeExport>,
}

/// Metadata for one exported function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FunctionExport {
    /// Exported function name.
    pub name: String,
    /// Parameter list as `(name, type)` pairs.
    #[cfg_attr(feature = "serde", serde(default))]
    pub params: Vec<(String, TypeRef)>,
    /// Return type, if the companion crate wants to specify it.
    pub return_type: Option<TypeRef>,
    /// Whether the function is async.
    pub is_async: bool,
}

/// Metadata for one exported type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TypeExport {
    /// Exported type name.
    pub name: String,
    /// High-level kind of type being exported.
    pub kind: TypeExportKind,
    /// Generic type parameter names.
    #[cfg_attr(feature = "serde", serde(default))]
    pub type_params: Vec<String>,
    /// Fields exposed by the type.
    #[cfg_attr(feature = "serde", serde(default))]
    pub fields: Vec<FieldExport>,
    /// Methods exposed by the type.
    #[cfg_attr(feature = "serde", serde(default))]
    pub methods: Vec<FunctionExport>,
}

/// High-level category of exported type metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum TypeExportKind {
    /// Model-like product type.
    #[default]
    Model,
    /// Class-like type.
    Class,
    /// Enum or tagged union.
    Enum,
    /// Trait or interface surface.
    Trait,
    /// Newtype wrapper.
    Newtype,
}

/// Metadata for one field exported by a type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct FieldExport {
    /// Field name as exposed to Incan users.
    pub name: String,
    /// Field type metadata.
    pub field_type: TypeRef,
    /// Whether the field has a default value.
    pub has_default: bool,
}

/// Type reference used by manifest DTOs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum TypeRef {
    /// Plain named type such as `Widget`.
    Named(String),
    /// Generic type application such as `Result[T, E]`.
    Generic(String, Vec<TypeRef>),
    /// Optional type wrapper.
    Optional(Box<TypeRef>),
    /// Union of multiple possible types.
    Union(Vec<TypeRef>),
}

impl TypeRef {
    /// Create a named type reference.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use incan_vocab::TypeRef;
    ///
    /// let widget = TypeRef::named("Widget");
    /// assert_eq!(widget, TypeRef::Named("Widget".to_string()));
    /// ```
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self::Named(name.to_string())
    }
}

impl Default for TypeRef {
    fn default() -> Self {
        Self::Named("Unknown".to_string())
    }
}

/// A Cargo dependency required by the library's generated surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CargoDependency {
    /// Crate name that should appear in Cargo metadata.
    pub crate_name: String,
    /// How the dependency should be sourced.
    pub source: CargoDependencySource,
}

/// Source descriptor for a required Cargo dependency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum CargoDependencySource {
    /// A registry dependency with a version requirement.
    Version(String),
    /// A local path dependency.
    Path(String),
}
