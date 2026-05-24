//! IR declaration definitions

use super::{IrSpan, IrStmt, IrType, Mutability};
use incan_core::interop::is_rust_capability_bound;

/// An IR declaration
#[derive(Debug, Clone)]
pub struct IrDecl {
    pub kind: IrDeclKind,
    pub span: IrSpan,
}

impl IrDecl {
    pub fn new(kind: IrDeclKind) -> Self {
        Self {
            kind,
            span: IrSpan::default(),
        }
    }

    pub fn with_span(mut self, span: IrSpan) -> Self {
        self.span = span;
        self
    }
}

/// Declaration kinds
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum IrDeclKind {
    /// Function definition
    Function(IrFunction),

    /// Struct definition
    Struct(IrStruct),

    /// Enum definition
    Enum(IrEnum),

    /// Trait definition
    Trait(IrTrait),

    /// Type alias (`pub type X<T> = Y<T>`)
    TypeAlias {
        visibility: Visibility,
        name: String,
        type_params: Vec<IrTypeParam>,
        ty: IrType,
        /// `true` when this alias came from `type X = rusttype Y`.
        is_rusttype: bool,
        /// Optional interop conversion edges declared in a `rusttype` block (`interop:`).
        interop_edges: Vec<IrInteropEdge>,
    },

    /// Symbol alias (`pub use target as name`) for declaration-level callable/type aliases.
    SymbolAlias {
        visibility: Visibility,
        name: String,
        target_path: Vec<String>,
        target_origin: Option<IrImportOrigin>,
        target_qualifier: Option<IrImportQualifier>,
    },

    /// Constant
    Const {
        visibility: Visibility, // pub or private
        name: String,
        ty: IrType,
        value: super::IrExpr,
    },

    /// Runtime-initialized module storage cell.
    Static {
        visibility: Visibility,
        name: String,
        ty: IrType,
        value: super::IrExpr,
    },

    /// Import (preserved for codegen)
    Import {
        visibility: Visibility,
        origin: IrImportOrigin,
        qualifier: IrImportQualifier,
        path: Vec<String>,
        alias: Option<String>,
        /// Specific items being imported (for `from x import a, b`)
        items: Vec<IrImportItem>,
    },

    /// Impl block for methods on structs/enums
    Impl(IrImpl),
}

/// Direction of a lowered `interop:` edge (RFC 041).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrInteropDirection {
    /// `from S ...` edge (source into the rusttype surface).
    From,
    /// `into T ...` edge (rusttype surface into target).
    Into,
}

/// Adapter mode for a lowered `interop:` edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrInteropAdapterKind {
    /// Infallible adapter (`via`).
    Via,
    /// Fallible adapter (`try`).
    Try,
}

/// A lowered interop edge attached to a `rusttype` alias.
#[derive(Debug, Clone)]
pub struct IrInteropEdge {
    pub direction: IrInteropDirection,
    pub ty: IrType,
    pub adapter_kind: IrInteropAdapterKind,
    pub adapter: super::IrExpr,
}

/// Semantic origin of an import.
///
/// This keeps `pub::` imports first-class in IR so lowering/emission can preserve library dependency semantics without
/// overloading path segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrImportOrigin {
    /// Standard Incan module or Rust import.
    Standard,
    /// Library import resolved from `[dependencies]` (`pub::name`).
    PubLibrary { dependency_key: String },
}

/// How an import path should be qualified in generated Rust.
///
/// ## Background (why this exists)
/// In Rust 2018+ module paths in `use ...` are **not implicitly crate-rooted** when emitted inside a submodule. For
/// example, inside `store::json_store`, `use db::schema::Database;` resolves as `store::json_store::db::...` (or an
/// external crate), not `crate::db::...`. For multi-file Incan projects this commonly needs an explicit `crate::` (or
/// `super::`) prefix for correctness.
///
/// We preserve the required qualification intent in IR so codegen can emit correct `use` paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrImportQualifier {
    /// No qualifier (external crate or special-case import).
    None,
    /// Decide at emit-time whether this should be `crate::...` or unqualified.
    ///
    /// This is used to avoid semantic regressions: `import serde::Serialize` should remain an external crate import
    /// unless `serde` is a known internal module root in the current compilation unit.
    ///
    /// The emitter uses the set of known internal module roots (for multi-file builds) to decide whether to prefix.
    Auto,
    /// Prefix with `crate::` (absolute import in the current crate).
    Crate,
    /// Prefix with `super::` repeated N times (relative import).
    Super(usize),
}

/// Metadata for one Rust trait import that can participate in extension-method lookup.
#[derive(Debug, Clone)]
pub struct IrRustTraitImport {
    /// Canonical import path used by Incan for this trait binding.
    pub trait_path: String,
    /// Resolved Rust definition path after re-export resolution, when available.
    pub definition_path: Option<String>,
    /// Method names this trait can place in Rust method-lookup scope.
    pub methods: Vec<String>,
}

/// An item in a from ... import statement
#[derive(Debug, Clone)]
pub struct IrImportItem {
    pub name: String,
    pub alias: Option<String>,
    /// Whether this import item binds an Incan `static` storage cell.
    ///
    /// Static declarations use Rust global naming in generated code, so imported static items must emit the provider's
    /// static identifier and, when aliased, the local static identifier instead of treating the source spelling as an
    /// ordinary Rust value binding.
    pub is_static: bool,
    /// Metadata provided when this item is a Rust trait import.
    ///
    /// Extension-trait imports can be used by Rust method lookup without appearing as identifiers in emitted tokens.
    /// Codegen uses this metadata to retain imports selected by frontend method-call analysis.
    pub rust_trait_import: Option<IrRustTraitImport>,
}

/// IR trait definition
#[derive(Debug, Clone)]
pub struct IrTrait {
    pub name: String,
    /// Generic parameters (`trait Foo[T]: ...`), including `with` bounds from the source (RFC 023 / RFC 042).
    pub type_params: Vec<IrTypeParam>,
    /// Direct supertraits for the generated Rust trait header (`trait Foo: Bar + Baz<T> {}`), RFC 042.
    ///
    /// Each entry is a Rust trait path string (possibly `::`-separated, as for [`IrTraitBound::trait_path`]) plus
    /// concrete type arguments for that bound.
    pub supertraits: Vec<(String, Vec<IrType>)>,
    /// Methods with default implementations
    pub methods: Vec<IrFunction>,
    pub visibility: Visibility,
}

/// IR impl block definition
#[derive(Debug, Clone)]
pub struct IrImpl {
    /// The type being implemented on (e.g., "Dog")
    pub target_type: String,
    /// Type parameters for the impl block
    pub type_params: Vec<IrTypeParam>,
    /// The trait being implemented, if any.
    pub trait_name: Option<String>,
    /// Concrete type arguments for the implemented trait (e.g. `impl<T> Boxed<T> for Cell<T>`), RFC 042.
    pub trait_type_args: Vec<IrType>,
    /// Associated type items emitted inside trait impl blocks.
    pub associated_types: Vec<IrAssociatedType>,
    /// Methods in this impl block
    pub methods: Vec<IrFunction>,
}

/// IR associated type item inside a trait impl.
#[derive(Debug, Clone)]
pub struct IrAssociatedType {
    pub name: String,
    pub ty: IrType,
}

/// IR function definition
#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<FunctionParam>,
    pub return_type: IrType,
    pub body: Vec<IrStmt>,
    pub is_async: bool,
    pub is_generator: bool,
    pub visibility: Visibility,
    /// Type parameters for generics, with optional trait bounds (RFC 023).
    pub type_params: Vec<IrTypeParam>,
    /// RFC 023: Whether this function is `@rust.extern` — its body is provided by a Rust backing module.
    ///
    /// When `true`, emission should generate a delegation call to `<rust_module_path>::<name>()` instead of compiling
    /// the Incan body. The `rust_module_path` is stored on `IrProgram`.
    pub is_extern: bool,
    /// Passthrough Rust attributes collected from decorators.
    ///
    /// Example: `@route("/users/{id}")` imported from a `rust.module("incan_web_macros")` stub becomes
    /// `#[incan_web_macros::route("/users/{id}")]`.
    pub rust_attributes: Vec<IrRustAttribute>,
    /// Targeted Rust lint suppressions from RFC 057 `@rust.allow(...)`.
    pub lint_allows: Vec<IrRustLintAllow>,
}

/// Function parameter
#[derive(Debug, Clone)]
pub struct FunctionParam {
    pub name: String,
    pub ty: IrType,
    pub mutability: Mutability,
    pub is_self: bool,
    /// Surface call-binding kind preserved for RFC 038 rest parameters.
    pub kind: crate::frontend::ast::ParamKind,
    /// Optional default argument expression (used for call-site default filling).
    pub default: Option<super::IrExpr>,
}

/// IR struct definition
#[derive(Debug, Clone)]
pub struct IrStruct {
    pub name: String,
    pub fields: Vec<StructField>,
    pub derives: Vec<String>,
    pub visibility: Visibility,
    /// Type parameters for generics, with optional trait bounds (RFC 023).
    pub type_params: Vec<IrTypeParam>,
    /// Derive names that should be qualified with a Rust module path.
    ///
    /// Key is the derive name, value is the module path from `rust.module(...)`.
    pub derive_rust_modules: std::collections::HashMap<String, String>,
    /// Targeted Rust lint suppressions from RFC 057 `@rust.allow(...)`.
    pub lint_allows: Vec<IrRustLintAllow>,
}

/// Struct field
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: IrType,
    pub visibility: Visibility,
    /// Optional default initializer expression for this field (used for construction when omitted).
    pub default: Option<super::IrExpr>,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// IR enum definition
#[derive(Debug, Clone)]
pub struct IrEnum {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    /// Alias name to canonical variant name.
    pub variant_aliases: Vec<EnumVariantAlias>,
    /// Value enum backing type, when this enum is an RFC 032 value enum.
    pub value_type: Option<IrEnumValueType>,
    pub derives: Vec<String>,
    pub visibility: Visibility,
    /// Type parameters for generics, with optional trait bounds (RFC 023).
    pub type_params: Vec<IrTypeParam>,
    /// Derive names that should be qualified with a Rust module path.
    ///
    /// Key is the derive name, value is the module path from `rust.module(...)`.
    pub derive_rust_modules: std::collections::HashMap<String, String>,
    /// Targeted Rust lint suppressions from RFC 057 `@rust.allow(...)`.
    pub lint_allows: Vec<IrRustLintAllow>,
}

/// Alias for an enum variant.
#[derive(Debug, Clone)]
pub struct EnumVariantAlias {
    pub name: String,
    pub target: String,
}

/// A passthrough Rust attribute generated from an Incan decorator.
#[derive(Debug, Clone)]
pub struct IrRustAttribute {
    pub module_path: String,
    pub name: String,
    pub args: Vec<IrRustAttrArg>,
}

/// A targeted Rust lint suppression emitted as `#[allow(...)]` on one generated item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrRustLintAllow {
    /// Rust lint path preserved from the source string literal, e.g. `dead_code` or `clippy::too_many_arguments`.
    pub lint: String,
}

/// Rust attribute argument kinds.
#[derive(Debug, Clone)]
pub enum IrRustAttrArg {
    Positional(String),
    Named { name: String, value: String },
}

/// Enum variant
#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub fields: VariantFields,
    /// Raw RFC 032 value for this variant when the parent enum is a value enum.
    pub raw_value: Option<IrEnumValue>,
}

/// Primitive backing type for an RFC 032 value enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrEnumValueType {
    String,
    Int,
}

/// Raw per-variant value for an RFC 032 value enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrEnumValue {
    String(String),
    Int(i64),
}

/// Variant fields (unit, tuple, or struct)
#[derive(Debug, Clone)]
pub enum VariantFields {
    Unit,
    Tuple(Vec<IrType>),
    Struct(Vec<StructField>),
}

// ============================================================================
// Type Parameters and Trait Bounds (RFC 023)
// ============================================================================

/// A Rust trait bound for a generic type parameter.
///
/// RFC 023: Represents a single trait bound in the emitted Rust `where` clause or inline bound syntax (e.g.,
/// `PartialEq`, `std::fmt::Display`, `std::ops::Add<Output = T>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrTraitBound {
    /// Rust trait path (e.g., `"PartialEq"`, `"std::fmt::Display"`, `"std::ops::Add"`).
    pub trait_path: String,
    /// Optional generic type arguments (e.g. `i64` in `Collection<i64>`).
    pub type_args: Vec<IrType>,
    /// Optional associated type constraints (e.g., `Output = T` for `Add<Output = T>`).
    pub assoc_types: Vec<(String, IrType)>,
    /// Distinguishes compiler-managed Rust capability markers (`Send`, `Sync`, `Static`) from regular trait paths.
    pub origin: IrTraitBoundOrigin,
}

/// Origin classification for an IR trait bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrTraitBoundOrigin {
    /// Standard trait bound (Incan-mapped or user-defined trait path).
    Standard,
    /// Rust-native capability marker from `std.rust`.
    RustCapability,
}

impl IrTraitBound {
    /// Create a simple trait bound with no associated types.
    pub fn simple(trait_path: impl Into<String>) -> Self {
        Self {
            trait_path: trait_path.into(),
            type_args: Vec::new(),
            assoc_types: Vec::new(),
            origin: IrTraitBoundOrigin::Standard,
        }
    }

    /// Create a trait bound with concrete generic arguments.
    pub fn with_type_args(trait_path: impl Into<String>, type_args: Vec<IrType>) -> Self {
        Self {
            trait_path: trait_path.into(),
            type_args,
            assoc_types: Vec::new(),
            origin: IrTraitBoundOrigin::Standard,
        }
    }

    /// Create a trait bound with an `Output = T` associated type constraint.
    pub fn with_output(trait_path: impl Into<String>, output_type: IrType) -> Self {
        Self {
            trait_path: trait_path.into(),
            type_args: Vec::new(),
            assoc_types: vec![("Output".to_string(), output_type)],
            origin: IrTraitBoundOrigin::Standard,
        }
    }

    /// Create a bound and classify Rust capability markers.
    pub fn with_type_args_classified(trait_path: impl Into<String>, type_args: Vec<IrType>) -> Self {
        let trait_path = trait_path.into();
        let origin = if is_rust_capability_bound(trait_path.as_str()) {
            IrTraitBoundOrigin::RustCapability
        } else {
            IrTraitBoundOrigin::Standard
        };
        Self {
            trait_path,
            type_args,
            assoc_types: Vec::new(),
            origin,
        }
    }
}

/// A type parameter with its trait bounds in IR.
///
/// RFC 023: Combines explicit `with` bounds from the source with bounds inferred from usage in the function body.
#[derive(Debug, Clone)]
pub struct IrTypeParam {
    /// The type parameter name (e.g., `"T"`, `"E"`).
    pub name: String,
    /// Combined trait bounds (explicit + inferred), deduplicated.
    pub bounds: Vec<IrTraitBound>,
}

impl IrTypeParam {
    /// Create a type parameter with no bounds.
    pub fn bare(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bounds: Vec::new(),
        }
    }
}

/// Visibility modifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
    Crate,
}

impl Visibility {
    pub fn rust_keyword(&self) -> &'static str {
        match self {
            Visibility::Private => "",
            Visibility::Public => "pub ",
            Visibility::Crate => "pub(crate) ",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visibility_rust_keyword() {
        assert_eq!(Visibility::Private.rust_keyword(), "");
        assert_eq!(Visibility::Public.rust_keyword(), "pub ");
        assert_eq!(Visibility::Crate.rust_keyword(), "pub(crate) ");
    }
}
