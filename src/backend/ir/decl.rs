//! IR declaration definitions

use super::{IrSpan, IrStmt, IrType, Mutability};
use incan_core::interop::is_rust_capability_bound;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin, encode_incan_symbol_identity};

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
        /// Exact source declaration projected by this alias, when it targets linker-visible Incan storage or code.
        target_canonical: Option<CanonicalSymbolId>,
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
        /// Whether this storage cell comes from an exact source declaration or compiler generation.
        provenance: IrStaticProvenance,
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

/// Provenance of one IR static storage cell.
///
/// This makes a source static without canonical identity unrepresentable after lowering. Compiler-generated caches
/// and decorator bindings deliberately retain synthetic Rust names and never masquerade as source declarations.
#[derive(Debug, Clone)]
pub enum IrStaticProvenance {
    Source(CanonicalSymbolId),
    CompilerGenerated,
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
    /// Compiler-owned identity of the imported declaration, when import resolution proved one.
    pub canonical: Option<CanonicalSymbolId>,
    /// Whether this import item binds an Incan `static` storage cell.
    ///
    /// Static declarations use Rust global naming in generated code, so imported static items must emit the provider's
    /// static identifier and, when aliased, the local static identifier instead of treating the source spelling as an
    /// ordinary Rust value binding.
    pub is_static: bool,
    /// Whether this imported item must be publicly reexported even when the source import itself is private.
    ///
    /// Public aliases of imported overload sets project concrete emitted Rust functions. The aliasing module needs to
    /// reexport those concrete functions so downstream facades do not reach through its private imports.
    pub force_reexport: bool,
    /// Metadata provided when this item is a Rust trait import.
    ///
    /// Extension-trait imports can be used by Rust method lookup without appearing as identifiers in emitted tokens.
    /// Codegen uses this metadata to retain imports selected by frontend method-call analysis.
    pub rust_trait_import: Option<IrRustTraitImport>,
}

impl IrImportItem {
    /// Return the provider's Rust item projection without recovering meaning from an emitted name.
    pub fn emitted_name(&self) -> String {
        self.canonical
            .as_ref()
            .filter(|identity| is_projected_source_symbol(identity))
            .map(encode_incan_symbol_identity)
            .unwrap_or_else(|| self.name.clone())
    }

    /// Return the local Rust binding created by this import.
    ///
    /// A source alias carries its target's identity, so every spelling of one function binds the same backend
    /// projection. Source spelling remains separately available in frontend facts and package metadata.
    pub fn emitted_binding_name(&self) -> String {
        if self.canonical.as_ref().is_some_and(is_projected_source_symbol) {
            return self.emitted_name();
        }
        self.alias.clone().unwrap_or_else(|| self.name.clone())
    }

    /// Return the source-local spelling used to decide reachability and static initialization.
    pub fn source_binding_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// Return whether an import targets a linker-visible source symbol with an Incan-owned projection.
///
/// Top-level partial declarations emit ordinary Rust wrapper functions and therefore follow the same exact canonical
/// import projection as source functions. Method-partial forwarding helpers are generated implementation details and
/// never reach this import surface with a `Partial` declaration identity. Source statics are physical storage symbols,
/// so aliases and re-exports bind the defining declaration's projection rather than minting another name.
pub(super) fn is_projected_source_symbol(identity: &CanonicalSymbolId) -> bool {
    matches!(
        identity.kind,
        SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial | SemanticSourceTargetKind::Static
    ) && matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
}

/// IR trait definition
#[derive(Debug, Clone)]
pub struct IrTrait {
    pub name: String,
    /// Compiler-recognised callable role established from the canonical source declaration identity during lowering.
    ///
    /// Keeping this semantic fact in IR prevents emission from rediscovering `std.traits.callable` through a generated
    /// provider's crate-local module path, where the public `std` mount is intentionally absent.
    pub source_callable: Option<incan_core::lang::callables::CallableTraitId>,
    /// Source docstring attached to the trait, when present.
    pub docstring: Option<String>,
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
    /// Canonical source module that owns the implemented trait, when known.
    pub trait_module_path: Option<Vec<String>>,
    /// Canonical source declaration name before local import aliasing, when known.
    pub trait_source_name: Option<String>,
    /// Concrete type arguments for the implemented trait (e.g. `impl<T> Boxed<T> for Cell<T>`), RFC 042.
    pub trait_type_args: Vec<IrType>,
    /// Associated type items emitted inside trait impl blocks.
    pub associated_types: Vec<IrAssociatedType>,
    /// Methods in this impl block
    pub methods: Vec<IrFunction>,
    /// Recoverable inherent entry points emitted beside Rust trait-ABI methods.
    ///
    /// Rust requires an implementation's slot spelling to match the trait declaration. A source-declared concrete
    /// implementation therefore retains that ABI spelling inside `impl Trait for Type` and gets a separate inherent
    /// `incan-v1` entry point carrying the implementation declaration's exact identity.
    pub method_projections: Vec<IrMethodProjection>,
    /// Unambiguous source-spelled Rust entry points that forward to canonical inherent method implementations.
    ///
    /// Canonical projections remain the only authored implementations and all generated Incan calls target them.
    /// Library artifacts retain these wrappers solely for their established native Rust surface. When Incan uses one
    /// spelling for distinct type-owned and instance-owned declarations, no wrapper is recorded because Rust cannot
    /// overload inherent associated items by receiver shape.
    pub source_method_projections: Vec<IrSourceMethodProjection>,
}

/// One source method whose Rust trait slot needs a separate recoverable Incan-origin entry point.
#[derive(Debug, Clone)]
pub struct IrMethodProjection {
    /// Rust ABI slot invoked by the recoverable entry point.
    pub abi_method_name: String,
    /// Exact compiler-owned identity encoded into the inherent entry-point name.
    pub identity: CanonicalSymbolId,
}

/// One unambiguous source method spelling retained as a native Rust forwarding entry point.
#[derive(Debug, Clone)]
pub struct IrSourceMethodProjection {
    /// Source spelling exposed to native Rust consumers.
    pub source_name: String,
    /// Exact compiler-owned identity of the canonical implementation.
    pub identity: CanonicalSymbolId,
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
    /// Source docstring attached to the callable, when present.
    pub docstring: Option<String>,
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
    /// Rust ABI symbol named by the source `@rust.extern` declaration.
    ///
    /// The emitted Incan wrapper may use a canonical identity projection, so its linker-visible name cannot also be
    /// used to address the host-owned Rust implementation. `None` is required for ordinary Incan callables.
    pub rust_extern_name: Option<String>,
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
    /// Optional default plan used for call-site argument filling.
    pub default: Option<FunctionParamDefault>,
}

/// The source of an optional [`FunctionParam`] argument value.
///
/// Source defaults remain an IR expression that the caller materializes. A captured partial preset deliberately has
/// no source expression: its synthesized local closure owns the construction-time value, and omitted calls must
/// pass `None` to its `Option<T>` slot so that closure selects that captured value. This distinction prevents legacy
/// Rust lowering from re-evaluating a runtime-local preset at every invocation.
#[derive(Debug, Clone)]
pub enum FunctionParamDefault {
    /// A default expression declared by the callable's source definition.
    Source(Box<super::IrExpr>),
    /// A local partial's construction-time captured, name-overrideable preset.
    CapturedPartialPreset,
}

impl FunctionParamDefault {
    /// Construct a source-owned default plan without inflating every parameter's in-memory representation.
    pub fn source(expr: super::IrExpr) -> Self {
        Self::Source(Box::new(expr))
    }
}

/// IR struct definition
#[derive(Debug, Clone)]
pub struct IrStruct {
    /// Source declaration category retained so emission does not infer model, class, or newtype semantics from shape.
    pub kind: IrStructKind,
    pub name: String,
    /// Source docstring attached to the type declaration, when present.
    pub docstring: Option<String>,
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

/// Source declaration category for a struct-shaped IR nominal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrStructKind {
    /// A value-oriented `model` declaration.
    Model,
    /// An identity-oriented `class` declaration.
    Class,
    /// A validated or transparent `newtype` declaration.
    Newtype,
}

/// Struct field
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: IrType,
    /// Source-level Incan type spelling used for generated reflection metadata.
    ///
    /// This is deliberately separate from [`Self::ty`], which remains the semantic and Rust-emission authority.
    pub surface_type_name: Option<String>,
    pub visibility: Visibility,
    /// Whether runtime reflection and source access must hide this field outside the declaring nominal type.
    pub is_type_private: bool,
    /// Optional default initializer expression for this field (used for construction when omitted).
    pub default: Option<super::IrExpr>,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// IR enum definition
#[derive(Debug, Clone)]
pub struct IrEnum {
    pub name: String,
    /// Source docstring attached to the enum declaration, when present.
    pub docstring: Option<String>,
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
    /// Canonical source `CallableN` capability.
    ///
    /// The generated callable-trait declaration provides blanket implementations for native Rust functions and
    /// closures. Keeping the nominal source trait in generic signatures also permits ordinary Incan models that adopt
    /// `CallableN` to cross the same boundary without call-site rewriting.
    SourceCallable,
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

    /// Create a canonical source-callable bound with its complete `Args..., Return` type argument list.
    pub fn source_callable(trait_path: impl Into<String>, type_args: Vec<IrType>) -> Self {
        Self {
            trait_path: trait_path.into(),
            type_args,
            assoc_types: Vec::new(),
            origin: IrTraitBoundOrigin::SourceCallable,
        }
    }
}

/// A type parameter with its trait bounds in IR.
///
/// RFC 023: Combines explicit `with` bounds from the source with bounds inferred from usage in the function body.
#[derive(Debug, Clone, PartialEq, Eq)]
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
