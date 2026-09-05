//! IR (Intermediate Representation) → Rust code emission.
//!
//! This module defines [`IrEmitter`] and wires together the focused submodules that implement IR → Rust emission.
//! The heavy lifting lives in those submodules; `mod.rs` is intentionally thin.
//!
//! ## Notes
//! - Emission produces a Rust syntax tree (`syn`) and formats it via `prettyplease`.
//! - Ownership/borrow/string conversions are centralized in `backend::ir::conversions` and should not be reimplemented
//!   ad-hoc in emission code.
//!
//! ## See also
//! - [`crate::backend::ir::conversions`]: conversion policy for emitted Rust
//! - `program`: program-level emission and formatting
//! - `decls`: item/declaration emission
//! - `statements`: statement emission
//! - `expressions`: expression emission
//! - `types`: type/pattern/operator helpers
//! - `consts`: RFC-008 const validation and const-friendly helpers

mod consts;
mod decls;
mod errors;
mod expressions;
mod program;
mod statements;
mod types;

pub use errors::EmitError;

/// Rust derive path emitted for Serde serialization.
pub(super) const SERDE_SERIALIZE_DERIVE: &str = "serde::Serialize";
/// Rust derive path emitted for Serde deserialization.
pub(super) const SERDE_DESERIALIZE_DERIVE: &str = "serde::Deserialize";

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::decl::{
    FunctionParam, IrDeclKind, IrEnumValue, IrEnumValueType, IrImportOrigin, IrImportQualifier, IrStaticProvenance,
    IrStruct, IrStructKind, VariantFields, Visibility,
};
use super::expr::{
    IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrListEntry, Literal as IrLiteral, TypedExpr, VarAccess,
    VarRefKind,
};
use super::types::{IR_UNION_TYPE_NAME, IrType, Mutability};
use super::{FunctionRegistry, FunctionSignature, IrProgram};
use crate::frontend::api_metadata::{
    ApiDeclaration, class_export_from_api, enum_export_from_api, function_export_from_api, model_export_from_api,
    newtype_export_from_api,
};
use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::frontend::module::logical_source_path_candidates;
use crate::frontend::symbols::ResolvedType;
use crate::library_manifest::{
    ExportIdentityKind, FieldExport, FieldVisibilityExport, LibraryManifest, MethodExport, NewtypeExport,
    ParamDefaultCallSignatureExport, ParamDefaultExport, ParamExport, ParamKindExport, TypeRef,
    resolved_type_from_manifest_type_ref,
};
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::{rust_keywords, stdlib};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolOrigin, encode_incan_symbol_identity};

/// Value-enum metadata loaded from a `.incnlib` dependency for consumer-side trait bridges.
#[derive(Debug, Clone)]
pub(crate) struct ExternalOrdinalValueEnum {
    /// Dependency key used as the generated Rust crate alias.
    pub dependency_key: String,
    /// Exported enum name.
    pub name: String,
    /// Stable serialized type identity.
    pub type_identity: String,
    /// Primitive value-enum backing family.
    pub value_type: IrEnumValueType,
    /// Raw values in declaration/export order.
    pub values: Vec<IrEnumValue>,
}

/// User-authored `OrdinalKey` adopter loaded from a `.incnlib` dependency for consumer-side trait bridges.
#[derive(Debug, Clone)]
pub(crate) struct ExternalOrdinalCustomKey {
    /// Dependency key used as the generated Rust crate alias.
    pub dependency_key: String,
    /// Exported type name.
    pub name: String,
    /// Whether the producer exported an explicit `ordinal_hash` method.
    pub has_ordinal_hash: bool,
    /// Whether the producer exported an explicit `ordinal_bytes_equal` method.
    pub has_ordinal_bytes_equal: bool,
}

/// Cross-module callable-name resolver metadata keyed by a concrete function-pointer signature.
#[derive(Debug, Clone)]
pub(crate) struct CallableNameResolution {
    pub(super) params: Vec<IrType>,
    pub(super) ret: IrType,
    pub(super) module_paths: Vec<Vec<String>>,
}

/// Callable-name usage facts collected from one lowered program.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallableNameUseFacts {
    pub(crate) signature_keys: HashSet<String>,
    pub(crate) function_arg_signature_keys: HashSet<String>,
    pub(crate) generic_trait_used: bool,
}

/// Generated callable-name symbol roles for one concrete function-pointer signature.
#[derive(Debug, Clone, Copy)]
enum CallableNameSymbolRole {
    /// Resolve a function pointer to a source name, using static candidates first and dynamic registrations second.
    Resolver,
    /// Return the shared dynamic-name storage for generic/decorated callables with this signature.
    Registry,
    /// Insert or update one dynamic callable-name registration for this signature.
    Register,
}

impl CallableNameSymbolRole {
    /// Return the stable generated Rust symbol prefix for this helper role.
    const fn prefix(self) -> &'static str {
        match self {
            Self::Resolver => "__incan_callable_name",
            Self::Registry => "__incan_callable_name_registry",
            Self::Register => "__incan_register_callable_name",
        }
    }
}

/// Usage facts collected before Rust emission.
///
/// This analysis is intentionally about generated Rust lints, not source-language reachability diagnostics. It records
/// which declarations, imports, methods, and fields the emitted Rust must retain so emission can prune avoidable unused
/// Rust items and narrowly mark unavoidable semantic retention points.
#[derive(Clone, Default)]
pub(super) struct GeneratedUseAnalysis {
    /// Top-level declaration names that must be emitted.
    pub(super) reachable_items: HashSet<String>,
    /// Import binding names that are referenced by emitted code.
    pub(super) used_imports: HashSet<String>,
    /// Rust trait imports that are used implicitly by extension-method lookup.
    pub(super) used_extension_trait_imports: HashSet<String>,
    /// Struct/class fields that are read by emitted code.
    pub(super) read_fields: HashSet<(String, String)>,
    /// Methods that are called by emitted code.
    pub(super) used_methods: HashSet<(String, String)>,
    /// Function-like constructor names that are called by emitted code.
    pub(super) used_constructors: HashSet<String>,
    /// Type names whose Rust visibility prevents private helper methods from warning when retained.
    pub(super) public_types: HashSet<String>,
    /// Source-owned callable object types used as non-Copy `Result.inspect` / `inspect_err` observers.
    pub(super) result_observer_callable_types: HashSet<String>,
    /// Top-level function values adapted to a borrowed function-pointer parameter.
    pub(super) borrowed_function_adapters: HashSet<(String, Vec<usize>)>,
    /// Concrete function-pointer signatures whose values read `__name__`.
    pub(super) callable_name_signature_keys: HashSet<String>,
    /// Concrete top-level function signatures passed through reachable calls.
    pub(super) callable_name_function_arg_signature_keys: HashSet<String>,
    /// Whether a generic callable parameter reads `__name__` through the generated callable-name trait.
    pub(super) uses_generic_callable_name_trait: bool,
}

impl GeneratedUseAnalysis {
    /// Return whether generated Rust should retain an impl method under the current program-level preservation mode.
    pub(super) fn should_retain_method(
        &self,
        preserve_public_items: bool,
        target_type: &str,
        method_name: &str,
        visibility: &Visibility,
    ) -> bool {
        self.public_types.contains(target_type)
            || (!preserve_public_items
                && !matches!(visibility, Visibility::Private)
                && self.reachable_items.contains(target_type))
            || self
                .used_methods
                .contains(&(target_type.to_string(), method_name.to_string()))
    }
}

/// Exact provider identity used to keep same-named constructor contracts distinct during emission.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ConstructorProviderIdentity {
    /// An ordinary source module identified by its canonical logical path.
    SourceModule(Vec<String>),
    /// A compiled public dependency identified by its dependency key.
    PublicDependency(String),
}

/// One public source import that projects a constructor under a facade-owned export name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceConstructorReexport {
    exporting_module: Vec<String>,
    target_module_candidates: Vec<Vec<String>>,
    target_name: String,
    exported_name: String,
}

/// Manifest-backed nominal constructor facts that preserve the source declaration category.
#[derive(Clone)]
struct ManifestConstructorShape {
    kind: IrStructKind,
    fields: Vec<FieldExport>,
}

/// Rust construction surface selected for one nominal type.
///
/// Provider bridges preserve Incan's checked field-privacy boundary across generated Rust crates. A bridge may carry
/// every public field, but it must never accept a type-private field from a consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StructConstructorSurface {
    /// Construct with a Rust struct literal because every field is available in the current module.
    DirectStructLiteral,
    /// Expose the existing public free constructor with every field as an argument.
    PublicAllFields,
    /// Expose a public provider bridge that accepts only public fields and owns private defaults.
    PublicBridge,
    /// Retain a full-fields helper for generated defining-module code without exporting it.
    PrivateAllFields,
    /// Retain a full-fields helper for generated code in the current crate.
    CrateAllFields,
    /// Do not emit a free constructor because an external caller would have to provide a private field.
    Absent,
}

/// Constructor metadata that selects a safe Rust construction shape for one nominal type.
#[derive(Clone)]
pub(super) struct StructConstructorMetadata {
    provider_identity: Option<ConstructorProviderIdentity>,
    fields: Vec<String>,
    field_types: HashMap<String, IrType>,
    field_defaults: HashMap<String, super::IrExpr>,
    default_fields: HashSet<String>,
    field_aliases: HashMap<String, String>,
    type_private_fields: HashSet<String>,
    constructor_surface: StructConstructorSurface,
}

impl StructConstructorMetadata {
    /// Select the external Rust construction surface for one nominal declaration.
    ///
    /// Models seal private constructor inputs outside their owner. Classes retain their complete constructor input
    /// surface even though later member access still observes field privacy.
    fn external_constructor_surface(
        kind: IrStructKind,
        type_private_fields: &HashSet<String>,
        default_fields: &HashSet<String>,
    ) -> StructConstructorSurface {
        if type_private_fields.is_empty() {
            return StructConstructorSurface::DirectStructLiteral;
        }
        match kind {
            IrStructKind::Model if type_private_fields.iter().all(|field| default_fields.contains(field)) => {
                StructConstructorSurface::PublicBridge
            }
            IrStructKind::Model => StructConstructorSurface::Absent,
            IrStructKind::Class => StructConstructorSurface::PublicAllFields,
            IrStructKind::Newtype => StructConstructorSurface::DirectStructLiteral,
        }
    }

    /// Build constructor-emission metadata from one lowered source-defined struct.
    fn from_struct(s: &IrStruct) -> Self {
        Self {
            provider_identity: None,
            fields: s.fields.iter().map(|field| field.name.clone()).collect(),
            field_types: s
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect(),
            field_defaults: s
                .fields
                .iter()
                .filter_map(|field| {
                    field
                        .default
                        .as_ref()
                        .map(|default| (field.name.clone(), default.clone()))
                })
                .collect(),
            default_fields: s
                .fields
                .iter()
                .filter(|field| field.default.is_some())
                .map(|field| field.name.clone())
                .collect(),
            field_aliases: s
                .fields
                .iter()
                .filter_map(|field| {
                    field
                        .alias
                        .as_ref()
                        .filter(|alias| *alias != &field.name)
                        .map(|alias| (alias.clone(), field.name.clone()))
                })
                .collect(),
            type_private_fields: s
                .fields
                .iter()
                .filter(|field| field.is_type_private)
                .map(|field| field.name.clone())
                .collect(),
            constructor_surface: StructConstructorSurface::DirectStructLiteral,
        }
    }

    /// Build exact constructor metadata for a checked ordinary source dependency.
    ///
    /// The canonical module identity prevents a same-named type from another source module from supplying the bridge
    /// ABI by short-name or field-shape coincidence. Models expose a provider bridge only when every private field has
    /// a provider-local default; classes preserve their complete constructor input surface.
    fn from_source_dependency(module_path: &[String], s: &IrStruct) -> Self {
        let mut metadata = Self::from_struct(s);
        metadata.provider_identity = Some(ConstructorProviderIdentity::SourceModule(module_path.to_vec()));
        metadata.constructor_surface =
            Self::external_constructor_surface(s.kind, &metadata.type_private_fields, &metadata.default_fields);
        metadata
    }

    /// Build constructor-emission metadata from one compiled-library export.
    ///
    /// Manifest defaults may be intentionally non-materializable to consumers, so their `has_default` flag remains
    /// authoritative for provider-bridge eligibility even when no serialized expression is available.
    fn from_manifest_fields(library: &str, kind: IrStructKind, fields: &[FieldExport]) -> Self {
        let type_private_fields = fields
            .iter()
            .filter(|field| matches!(field.visibility, FieldVisibilityExport::Private))
            .map(|field| field.name.clone())
            .collect::<HashSet<_>>();
        let default_fields = fields
            .iter()
            .filter(|field| field.has_default || field.default.is_some())
            .map(|field| field.name.clone())
            .collect::<HashSet<_>>();
        let constructor_surface = Self::external_constructor_surface(kind, &type_private_fields, &default_fields);
        Self {
            provider_identity: Some(ConstructorProviderIdentity::PublicDependency(library.to_string())),
            fields: fields.iter().map(|field| field.name.clone()).collect(),
            field_types: fields
                .iter()
                .map(|field| (field.name.clone(), IrEmitter::manifest_type_ref_to_ir_type(&field.ty)))
                .collect(),
            field_defaults: fields
                .iter()
                .filter_map(|field| {
                    field
                        .default
                        .as_ref()
                        .and_then(|default| IrEmitter::manifest_default_to_ir_expr(library, default))
                        .map(|default| (field.name.clone(), default))
                })
                .collect(),
            default_fields,
            field_aliases: fields
                .iter()
                .filter_map(|field| {
                    field
                        .alias
                        .as_ref()
                        .filter(|alias| *alias != &field.name)
                        .map(|alias| (alias.clone(), field.name.clone()))
                })
                .collect(),
            type_private_fields,
            constructor_surface,
        }
    }

    /// Return whether this metadata selects the provider-owned public-field bridge.
    fn uses_provider_bridge(&self) -> bool {
        matches!(self.constructor_surface, StructConstructorSurface::PublicBridge)
    }

    /// Return whether calls must target a generated free constructor instead of a Rust struct literal.
    fn uses_constructor_function(&self) -> bool {
        matches!(
            self.constructor_surface,
            StructConstructorSurface::PublicAllFields
                | StructConstructorSurface::PublicBridge
                | StructConstructorSurface::PrivateAllFields
                | StructConstructorSurface::CrateAllFields
        )
    }

    /// Iterate in declaration order over the fields accepted by the selected Rust constructor surface.
    ///
    /// A provider bridge filters out type-private fields so no external generated crate can serialize a value across
    /// the nominal privacy boundary.
    fn constructor_fields(&self) -> impl Iterator<Item = &String> {
        self.fields
            .iter()
            .filter(|field| !self.uses_provider_bridge() || !self.type_private_fields.contains(field.as_str()))
    }

    /// Return whether `field` is private to the owning nominal type.
    fn field_is_type_private(&self, field: &str) -> bool {
        self.type_private_fields.contains(field)
    }

    /// Resolve a source-facing field name or alias to the canonical Rust field name.
    fn canonical_field_name<'a>(&'a self, field: &'a str) -> Option<&'a str> {
        if self.field_types.contains_key(field) {
            Some(field)
        } else {
            self.field_aliases.get(field).map(String::as_str)
        }
    }

    /// Return whether every provided named field exists on this constructor variant.
    fn supports_named_fields(&self, provided: &HashSet<&str>) -> bool {
        provided.iter().all(|field| self.canonical_field_name(field).is_some())
    }

    /// Return whether provided fields plus declared defaults can construct this variant.
    fn constructible_from(&self, provided: &HashSet<&str>) -> bool {
        let provided = provided
            .iter()
            .filter_map(|field| self.canonical_field_name(field))
            .collect::<HashSet<_>>();
        self.fields
            .iter()
            .all(|field| provided.contains(field.as_str()) || self.default_fields.contains(field))
    }
}

/// Emit Rust source code from typed IR.
///
/// This is the main entry point for the IR → Rust backend stage. It is stateful because it:
/// - tracks which imports/features are required,
/// - records auxiliary typing metadata needed for emission (e.g. enum variant fields),
/// - caches resolvable const string values to emit `concat!(...)` in const contexts.
///
/// ## Notes
/// - The public API is `emit_program()` (implemented in `program.rs`).
/// - Most emission helpers are implemented on this type across submodules.
pub struct IrEmitter<'a> {
    emit_strict_generated_lint_denies: bool,
    /// Whether public source items should be emitted even when this crate does not reference them.
    preserve_public_items: bool,
    /// Whether local value enums should receive stdlib `OrdinalKey` impls.
    emit_std_ordinal_value_enum_impls: bool,
    /// Whether local newtypes should receive compiler-provided `TryFrom[str]` impls.
    emit_std_string_try_from_newtype_impls: bool,
    /// Public value enums imported from `.incnlib` dependencies that need this crate's local `OrdinalKey`.
    external_ordinal_value_enums: Vec<ExternalOrdinalValueEnum>,
    /// Public user-authored key types imported from `.incnlib` dependencies that need this crate's local `OrdinalKey`.
    external_ordinal_custom_keys: Vec<ExternalOrdinalCustomKey>,
    /// Public serialized identities for locally emitted value enums, keyed by source identity (`module.Type`).
    public_ordinal_type_identities: HashMap<String, String>,
    /// Private items that generated code outside the emitted IR body will call.
    externally_reachable_items: HashSet<String>,
    /// Pre-emission usage facts used to avoid generated `dead_code` and `unused_imports` suppressions.
    generated_use_analysis: RefCell<GeneratedUseAnalysis>,
    /// Rust overload implementation imports already emitted in the current module.
    ///
    /// Source aliases can project the same overloaded callable under another source name. The public metadata keeps
    /// both source names, but Rust still has one concrete implementation symbol, so a facade importing the canonical
    /// name and the alias must not emit the same `use`/`pub use` binding twice.
    emitted_overload_import_bindings: RefCell<HashSet<String>>,
    /// Whether to emit the Zen of Incan in main
    emit_zen_in_main: bool,
    /// Whether serde is needed for emitted Rust derives or helpers.
    needs_serde: RefCell<bool>,
    /// Function registry for module-local call-site default argument filling and type-aware argument conversion.
    function_registry: &'a FunctionRegistry,
    /// Exact Rust projections for source static bindings in the module currently being emitted.
    ///
    /// Keys are compiler-retained local bindings, not recovered artifact names. Generated and host statics are absent
    /// and continue through the ordinary Rust global-style identifier path.
    static_projections: RefCell<HashMap<String, String>>,
    /// Cross-module registry used only for IR calls that carry an explicit canonical callee path.
    canonical_function_registry: Option<FunctionRegistry>,
    /// Exact compiled-provider function identities keyed by their public `std.<module>.<name>` path.
    compiled_sdk_function_identities: HashMap<Vec<String>, CanonicalSymbolId>,
    /// Public provider paths with more than one distinct function identity.
    ambiguous_compiled_sdk_function_paths: HashSet<Vec<String>>,
    /// Track struct derives for generating serde methods in impl blocks
    struct_derives: std::collections::HashMap<String, Vec<String>>,
    /// Current function's return type (for applying conversions in return statements)
    current_function_return_type: RefCell<Option<IrType>>,
    /// Generic parameters in scope around an emitted method and whether the owner is a trait declaration.
    ///
    /// Rust precise-capture lists for return-position `impl Trait` must mention every surrounding type parameter.
    current_method_owner_type_params: RefCell<Option<(bool, Vec<String>)>>,
    /// Functions imported from external Rust crates
    external_rust_functions: std::collections::HashSet<String>,
    /// Enum variant field typing lookup: (EnumName, VariantName) -> VariantFields
    enum_variant_fields: std::collections::HashMap<(String, String), VariantFields>,
    /// Enum variant alias lookup: (EnumName, AliasName) -> CanonicalVariantName
    enum_variant_aliases: std::collections::HashMap<(String, String), String>,
    /// Struct field type lookup: (StructName, FieldName) -> IrType
    struct_field_types: std::collections::HashMap<(String, String), IrType>,
    /// Source-level field type spelling used solely for generated reflection metadata.
    struct_field_surface_type_names: std::collections::HashMap<(String, String), Option<String>>,
    /// Struct fields whose ordinary source and runtime-reflection boundary is the declaring nominal type.
    struct_type_private_fields: std::collections::HashSet<(String, String)>,
    /// Struct field name order (as declared): StructName -> [FieldName...]
    struct_field_names: std::collections::HashMap<String, Vec<String>>,
    /// Struct field alias lookup: (StructName, FieldName) -> alias
    struct_field_aliases: std::collections::HashMap<(String, String), Option<String>>,
    /// Struct field description lookup: (StructName, FieldName) -> description
    struct_field_descriptions: std::collections::HashMap<(String, String), Option<String>>,
    /// Struct field default expressions: (StructName, FieldName) -> default expr
    struct_field_defaults: std::collections::HashMap<(String, String), super::IrExpr>,
    /// Constructor metadata variants for source-defined structs that share a simple name across modules.
    struct_constructor_metadata: HashMap<String, Vec<StructConstructorMetadata>>,
    /// Constructor metadata keyed by canonical ordinary source-module identity and provider declaration name.
    source_dependency_constructor_metadata: HashMap<(Vec<String>, String), StructConstructorMetadata>,
    /// Public source-import projections waiting to inherit their exact provider constructor metadata.
    source_dependency_constructor_reexports: Vec<SourceConstructorReexport>,
    /// Whether newly seeded source metadata may resolve one or more retained public projections.
    source_dependency_constructor_reexports_dirty: bool,
    /// Constructor metadata keyed by the exact public dependency and namespace-relative declaration path.
    pub_dependency_constructor_metadata: HashMap<(String, Vec<String>), StructConstructorMetadata>,
    /// Unambiguous public nominal types keyed by short name and their Rust-visible provider path.
    ///
    /// Crate-root anonymous union wrappers are emitted before ordinary `use` items. A wrapper can therefore retain a
    /// provider-local member name from checked `pub::` metadata even when the consumer never imported that member
    /// directly. Keep the checked provider path so that payload can be emitted without relying on crate-root
    /// re-exports that the provider did not declare.
    public_dependency_type_paths: HashMap<String, Vec<String>>,
    /// Transparent local type aliases keyed by alias name.
    type_aliases: HashMap<String, IrType>,
    /// Incan `rusttype` aliases that should use compiler-owned call conversion rules at the surface boundary.
    rusttype_alias_names: HashSet<String>,
    /// Source newtypes keyed by the exact Rust nominal type that backs their single carrier field.
    ///
    /// A newtype may invoke an associated Rust function on its carrier while implementing its source API. The carrier
    /// retains its qualified Rust identity, but the call needs the source newtype's ownership signature. This mapping
    /// permits that explicit relationship without treating arbitrary same-named Rust and Incan types as equivalent.
    newtype_backing_type_names: HashMap<String, HashSet<String>>,
    /// Method signature lookup for Incan-owned nominal receivers, including imported modules.
    method_signatures: HashMap<(String, String), FunctionSignature>,
    /// Exact emitted projections for source members keyed by nominal owner and source declaration name.
    member_projections: HashMap<(String, String), CanonicalSymbolId>,
    /// Member keys that resolve to more than one distinct declaration identity.
    ambiguous_member_projections: HashSet<(String, String)>,
    /// Impl-level generic parameter order for method signatures.
    method_signature_type_params: HashMap<(String, String), Vec<String>>,
    /// Whether we're currently emitting a return expression (allows moves instead of clones)
    in_return_context: RefCell<bool>,
    /// Map of const string bindings to their literal values (for const folding of string adds)
    const_string_literals: std::collections::HashMap<String, String>,
    /// Const declarations keyed by source binding so nested const-safe model values can prove immutable references.
    const_bindings: std::collections::HashMap<String, (IrType, TypedExpr)>,
    /// Map of type name -> module path segments for dependency modules.
    type_module_paths: HashMap<String, Vec<String>>,
    /// Nominal declarations owned by the program currently being emitted.
    local_nominal_type_names: HashSet<String>,
    /// Provider module paths owned by linked compiled SDK providers.
    ///
    /// These paths do not use the consumer-only `__incan_std` namespace, so generated support fast paths need an
    /// explicit ownership marker rather than inferring ownership from a short module name such as `collections`.
    compiled_sdk_module_paths: HashSet<Vec<String>>,
    /// Nominal type names published by each linked compiled SDK provider module.
    compiled_sdk_type_module_paths: HashMap<String, HashSet<Vec<String>>>,
    /// Type names that are declared in multiple modules (ambiguous).
    ambiguous_type_names: HashSet<String>,
    /// Map of value name -> module path segments for dependency modules.
    value_module_paths: HashMap<String, Vec<String>>,
    /// Value names that are declared in multiple modules (ambiguous).
    ambiguous_value_names: HashSet<String>,
    /// Imported enum type names discovered from dependency modules.
    ///
    /// Imported enums usually lower to `IrType::Struct(name)` in consumer modules, so for-loop emission needs this
    /// side-channel to recognize that `list[name]` elements should be iterated as owned enum values.
    dependency_enum_types: HashSet<String>,
    /// Known internal module roots for this compilation unit (e.g. {"db", "store"}).
    ///
    /// Used to disambiguate crate-internal module imports vs external crate imports when emitting `use` paths.
    internal_module_roots: HashSet<String>,
    /// Canonical paths of ordinary source modules available in this generated crate.
    ///
    /// Source resolution accepts an unqualified import beside the current module before falling back to the source
    /// root. Emission needs the same path so it can produce `crate::parent::sibling`, rather than a nonexistent Cargo
    /// crate named after the sibling leaf.
    source_module_paths: HashSet<Vec<String>>,
    /// Canonical path of the source module currently being emitted.
    current_source_module_path: Option<Vec<String>>,
    /// Canonical package identity of the compilation unit currently being emitted.
    ///
    /// Package-owned symbols retain their `pub::<package>::...` identity even inside the package that declares them.
    /// Emission uses this context only to render those self-package references through `crate::...`; consumers still
    /// render the same identities through the linked dependency crate.
    current_package_identity: Option<String>,
    /// RFC 023: The `rust.module("path::to::module")` Rust backing path, if declared.
    ///
    /// When set, `@rust.extern` functions emit delegation calls to `<rust_module_path>::<fn_name>()` instead of
    /// compiling their Incan bodies.
    rust_module_path: Option<String>,
    /// Rust import path tracking: maps imported type names (incl. aliases) to their original module paths.
    ///
    /// Key: type name as seen in Incan code (e.g., "AxumResponse" for `import Response as AxumResponse`)
    /// Value: original module path (e.g., ["axum", "response"])
    ///
    /// Used by derive passthrough and newtype emission to locate the original Rust crate path for
    /// imported types.
    rust_import_paths: RefCell<std::collections::HashMap<String, Vec<String>>>,
    /// Local newtype construction plans, including conservative fallbacks when checked metadata is unavailable.
    newtype_construction: HashMap<String, super::IrNewtypeConstructionPlan>,
    /// Whether the currently emitted module has initialization work that every callable entrypoint must perform.
    ///
    /// Local statics and compiler-generated module registrations share the same once-only init guard. A module can
    /// therefore need initialization even when its only static is imported from a source sibling.
    module_needs_initialization: RefCell<bool>,
    /// Imported static bindings that need their defining module's static-init guard before use.
    imported_static_init_bindings: RefCell<HashSet<String>>,
    /// Imported static bindings re-exported by this module whose defining module's static-init guard should be
    /// chained from this module's init helper.
    imported_static_module_init_bindings: RefCell<Vec<String>>,
    /// Whether expression emission is currently inside a static initializer.
    ///
    /// Used to avoid recursively forcing the module-wide static init helper while generating static initializer code.
    in_static_initializer: RefCell<bool>,
    /// Whether this program emits an RFC 088 `.sum()` call that needs local newtype `Sum` implementations.
    iterator_sum_used: RefCell<bool>,
    /// Whether canonical calls to internal modules should be emitted with explicit `crate::...` paths.
    ///
    /// Normal imported calls use ordinary local bindings and imports. Default argument expressions are different: they
    /// can be expanded at a caller outside the defining module, so imported helper calls inside those defaults need a
    /// durable crate-qualified path.
    qualify_internal_canonical_paths: RefCell<bool>,
    /// Whether anonymous ordinary union wrapper references should be emitted as crate-root paths.
    ///
    /// Multi-file source modules share generated ordinary union wrappers through the crate root so same-shaped unions
    /// remain one Rust nominal type across module boundaries.
    qualify_union_types_from_crate: bool,
    /// Extra anonymous union shapes that should be emitted in this module in addition to locally referenced shapes.
    generated_union_types: HashMap<String, IrType>,
    /// Whether this module should emit generated ordinary union wrapper definitions.
    emit_generated_union_definitions: bool,
    /// Stack of statement-slice analyses describing which local `StaticBinding` names need mutable Rust bindings.
    ///
    /// An Incan alias like `let live = ITEMS` is not source-level `mut`, but if later emitted Rust uses
    /// `live.with_mut(...)` the local wrapper still must be declared `mut`. This stack is pushed per emitted
    /// statement slice so `emit_stmt` can make that decision without reintroducing blanket `mut` noise.
    storage_binding_mut_names: RefCell<Vec<HashSet<String>>>,
    /// Source-owned callable object types used as non-Copy `Result.inspect` / `inspect_err` observers.
    result_observer_callable_types: RefCell<HashSet<String>>,
    /// Callable object types whose borrowed observer helper has already been emitted.
    emitted_result_observer_callable_helpers: RefCell<HashSet<String>>,
    /// Top-level function values adapted to a borrowed function-pointer parameter.
    borrowed_function_adapters: RefCell<HashSet<(String, Vec<usize>)>>,
    /// Current generated Rust module path. The crate root uses an empty path.
    callable_name_current_module_path: Vec<String>,
    /// Concrete callable-name helper modules available to this compilation unit.
    callable_name_resolutions: HashMap<String, CallableNameResolution>,
    /// Concrete callable-name signatures used somewhere in this compilation unit.
    callable_name_used_signature_keys: HashSet<String>,
    /// Local callable registry used for module-local callable-name helpers when the main emitter has a unified
    /// cross-module call registry.
    callable_name_local_registry: Option<FunctionRegistry>,
}

impl<'a> IrEmitter<'a> {
    /// Create an emitter using the function registry that drives call-site default argument filling and type-aware
    /// argument conversion.
    pub fn new(function_registry: &'a FunctionRegistry) -> Self {
        Self {
            emit_strict_generated_lint_denies: false,
            preserve_public_items: true,
            emit_std_ordinal_value_enum_impls: false,
            emit_std_string_try_from_newtype_impls: false,
            external_ordinal_value_enums: Vec::new(),
            external_ordinal_custom_keys: Vec::new(),
            public_ordinal_type_identities: HashMap::new(),
            externally_reachable_items: HashSet::new(),
            generated_use_analysis: RefCell::new(GeneratedUseAnalysis::default()),
            emitted_overload_import_bindings: RefCell::new(HashSet::new()),
            emit_zen_in_main: false,
            needs_serde: RefCell::new(false),
            function_registry,
            static_projections: RefCell::new(HashMap::new()),
            canonical_function_registry: None,
            compiled_sdk_function_identities: HashMap::new(),
            ambiguous_compiled_sdk_function_paths: HashSet::new(),
            struct_derives: std::collections::HashMap::new(),
            current_function_return_type: RefCell::new(None),
            current_method_owner_type_params: RefCell::new(None),
            external_rust_functions: std::collections::HashSet::new(),
            enum_variant_fields: std::collections::HashMap::new(),
            enum_variant_aliases: std::collections::HashMap::new(),
            struct_field_types: std::collections::HashMap::new(),
            struct_field_surface_type_names: std::collections::HashMap::new(),
            struct_type_private_fields: std::collections::HashSet::new(),
            struct_field_names: std::collections::HashMap::new(),
            struct_field_aliases: std::collections::HashMap::new(),
            struct_field_descriptions: std::collections::HashMap::new(),
            struct_field_defaults: std::collections::HashMap::new(),
            struct_constructor_metadata: HashMap::new(),
            source_dependency_constructor_metadata: HashMap::new(),
            source_dependency_constructor_reexports: Vec::new(),
            source_dependency_constructor_reexports_dirty: false,
            pub_dependency_constructor_metadata: HashMap::new(),
            public_dependency_type_paths: HashMap::new(),
            type_aliases: HashMap::new(),
            rusttype_alias_names: HashSet::new(),
            newtype_backing_type_names: HashMap::new(),
            method_signatures: HashMap::new(),
            member_projections: HashMap::new(),
            ambiguous_member_projections: HashSet::new(),
            method_signature_type_params: HashMap::new(),
            in_return_context: RefCell::new(false),
            const_string_literals: std::collections::HashMap::new(),
            const_bindings: std::collections::HashMap::new(),
            type_module_paths: HashMap::new(),
            local_nominal_type_names: HashSet::new(),
            compiled_sdk_module_paths: HashSet::new(),
            compiled_sdk_type_module_paths: HashMap::new(),
            ambiguous_type_names: HashSet::new(),
            value_module_paths: HashMap::new(),
            ambiguous_value_names: HashSet::new(),
            dependency_enum_types: HashSet::new(),
            internal_module_roots: HashSet::new(),
            source_module_paths: HashSet::new(),
            current_source_module_path: None,
            current_package_identity: None,
            rust_module_path: None,
            rust_import_paths: RefCell::new(std::collections::HashMap::new()),
            newtype_construction: HashMap::new(),
            module_needs_initialization: RefCell::new(false),
            imported_static_init_bindings: RefCell::new(HashSet::new()),
            imported_static_module_init_bindings: RefCell::new(Vec::new()),
            in_static_initializer: RefCell::new(false),
            iterator_sum_used: RefCell::new(false),
            qualify_internal_canonical_paths: RefCell::new(false),
            qualify_union_types_from_crate: false,
            generated_union_types: HashMap::new(),
            emit_generated_union_definitions: true,
            storage_binding_mut_names: RefCell::new(Vec::new()),
            result_observer_callable_types: RefCell::new(HashSet::new()),
            emitted_result_observer_callable_helpers: RefCell::new(HashSet::new()),
            borrowed_function_adapters: RefCell::new(HashSet::new()),
            callable_name_current_module_path: Vec::new(),
            callable_name_resolutions: HashMap::new(),
            callable_name_used_signature_keys: HashSet::new(),
            callable_name_local_registry: None,
        }
    }

    /// Plan one named nominal construction through its selected Rust surface.
    ///
    /// Both call-shaped and struct-shaped IR use this path so private-field filtering, default transport, and argument
    /// ordering cannot drift between emitters.
    pub(super) fn emit_named_constructor_arguments(
        &self,
        target_name: &str,
        metadata: &StructConstructorMetadata,
        fields: &[(String, TypedExpr)],
    ) -> Result<Vec<(TokenStream, TokenStream)>, EmitError> {
        let mut provided = HashMap::<&str, &TypedExpr>::new();
        for (field, value) in fields {
            if let Some(canonical) = metadata.canonical_field_name(field) {
                provided.insert(canonical, value);
            }
        }

        if metadata.uses_provider_bridge() && provided.keys().any(|field| metadata.field_is_type_private(field)) {
            return Err(EmitError::Unsupported(format!(
                "private field supplied when constructing '{target_name}' through a provider bridge"
            )));
        }

        let mut planned = Vec::new();
        for field_name in metadata.constructor_fields() {
            let field_ident = Self::rust_ident(field_name);
            let target_ty = metadata.field_types.get(field_name);
            let value = if let Some(value) = provided.get(field_name.as_str()) {
                let value = self.emit_expr_for_use(value, super::ownership::ValueUseSite::StructField { target_ty })?;
                if metadata.uses_constructor_function() && metadata.default_fields.contains(field_name) {
                    quote! { Some(#value) }
                } else {
                    value
                }
            } else if metadata.default_fields.contains(field_name) {
                if metadata.uses_constructor_function() {
                    quote! { None }
                } else {
                    let default_expr = metadata.field_defaults.get(field_name).ok_or_else(|| {
                        EmitError::Unsupported(format!(
                            "default for field '{field_name}' on '{target_name}' cannot be materialized"
                        ))
                    })?;
                    self.emit_expr_for_use(default_expr, super::ownership::ValueUseSite::StructField { target_ty })?
                }
            } else {
                return Err(EmitError::Unsupported(format!(
                    "missing required field '{field_name}' when constructing '{target_name}'"
                )));
            };
            planned.push((quote! { #field_ident }, value));
        }
        Ok(planned)
    }

    /// Configure the generated Rust module path for callable-name helper routing.
    pub(crate) fn set_callable_name_current_module_path(&mut self, path: Vec<String>) {
        self.callable_name_current_module_path = path;
    }

    /// Configure the canonical callable registry for explicit cross-module call paths.
    pub(crate) fn set_canonical_function_registry(&mut self, registry: FunctionRegistry) {
        self.canonical_function_registry = Some(registry);
    }

    /// Return the canonical function registry used for callable-name lookups.
    pub(super) fn canonical_function_registry(&self) -> &FunctionRegistry {
        self.canonical_function_registry
            .as_ref()
            .unwrap_or(self.function_registry)
    }

    /// Resolve a stdlib function path to one compiler-retained identity without reconstructing it from source names.
    pub(super) fn canonical_stdlib_function_identity(&self, path: &[String]) -> Option<&CanonicalSymbolId> {
        let normalized_path;
        let path = match path.first().map(String::as_str) {
            Some(stdlib::STDLIB_ROOT) => path,
            Some(stdlib::INCAN_STD_NAMESPACE) => {
                normalized_path = std::iter::once(stdlib::STDLIB_ROOT.to_string())
                    .chain(path.iter().skip(1).cloned())
                    .collect::<Vec<_>>();
                &normalized_path
            }
            _ => return None,
        };
        let (declaration_name, module_path) = path.get(1..)?.split_last()?;
        if let Some(library) = self.current_package_identity.as_deref()
            && let Some(identity) = self.canonical_function_registry().canonical_package_function_identity(
                library,
                module_path,
                declaration_name,
            )
        {
            return Some(identity);
        }
        if let Some(identity) = (!self.ambiguous_compiled_sdk_function_paths.contains(path))
            .then(|| self.compiled_sdk_function_identities.get(path))
            .flatten()
        {
            return Some(identity);
        }
        let emitted_path = std::iter::once(stdlib::INCAN_STD_NAMESPACE.to_string())
            .chain(path.iter().skip(1).cloned())
            .collect::<Vec<_>>();
        if let Some(identity) = self
            .canonical_function_registry()
            .canonical_identity_for_path(&emitted_path)
        {
            return Some(identity);
        }
        self.canonical_function_registry().canonical_identity_for_path(path)
    }

    /// Return one exact member projection, failing closed when the owner/name pair is ambiguous.
    pub(super) fn member_projection(&self, owner: &str, source_name: &str) -> Option<String> {
        let key = (owner.to_string(), source_name.to_string());
        if self.ambiguous_member_projections.contains(&key) {
            return None;
        }
        self.member_projections.get(&key).map(encode_incan_symbol_identity)
    }

    /// Seed one member projection while preserving ambiguity rather than selecting by insertion order.
    fn register_member_projection(&mut self, owner: &str, source_name: &str, identity: CanonicalSymbolId) {
        let key = (owner.to_string(), source_name.to_string());
        if self.ambiguous_member_projections.contains(&key) {
            return;
        }
        match self.member_projections.get(&key) {
            Some(existing) if existing != &identity => {
                self.member_projections.remove(&key);
                self.ambiguous_member_projections.insert(key);
            }
            Some(_) => {}
            None => {
                self.member_projections.insert(key, identity);
            }
        }
    }

    /// Configure the concrete callable-name helper modules available to this emitter.
    pub(crate) fn set_callable_name_resolutions(&mut self, resolutions: HashMap<String, CallableNameResolution>) {
        self.callable_name_resolutions = resolutions;
    }

    /// Configure the callable-name signatures that are used anywhere in this generated crate.
    pub(crate) fn set_callable_name_used_signature_keys(&mut self, keys: HashSet<String>) {
        self.callable_name_used_signature_keys = keys;
    }

    /// Configure the local callable registry used by generated callable-name helpers.
    pub(crate) fn set_callable_name_local_registry(&mut self, registry: FunctionRegistry) {
        self.callable_name_local_registry = Some(registry);
    }

    /// Add every concrete function-pointer signature from one lowered program to the cross-module resolver map.
    pub(crate) fn add_callable_name_resolutions_for_program(
        out: &mut HashMap<String, CallableNameResolution>,
        module_path: Vec<String>,
        program: &IrProgram,
    ) {
        for (_, signature) in program.function_registry.iter() {
            let params = signature
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect::<Vec<_>>();
            let ret = signature.return_type.clone();
            let Some(key) = Self::callable_name_signature_key(&params, &ret) else {
                continue;
            };
            let resolution = out.entry(key).or_insert_with(|| CallableNameResolution {
                params,
                ret,
                module_paths: Vec::new(),
            });
            if !resolution.module_paths.contains(&module_path) {
                resolution.module_paths.push(module_path.clone());
            }
        }
        for resolution in out.values_mut() {
            resolution.module_paths.sort();
        }
    }

    /// Return a deterministic generated symbol for one callable-name helper role and concrete signature key.
    fn callable_name_symbol_ident(role: CallableNameSymbolRole, key: &str) -> proc_macro2::Ident {
        format_ident!(
            "{}_{:016x}",
            role.prefix(),
            Self::stable_callable_name_hash(key.as_bytes())
        )
    }

    /// Return the generated resolver helper identifier for a concrete callable signature key.
    ///
    /// The resolver checks same-module static function candidates and then the per-signature dynamic registry.
    pub(super) fn callable_name_helper_ident(key: &str) -> proc_macro2::Ident {
        Self::callable_name_symbol_ident(CallableNameSymbolRole::Resolver, key)
    }

    /// Return the generated dynamic-name registration helper identifier for a concrete callable signature key.
    ///
    /// The registration helper records runtime metadata for concrete generic/decorated function values.
    pub(super) fn callable_name_register_ident(key: &str) -> proc_macro2::Ident {
        Self::callable_name_symbol_ident(CallableNameSymbolRole::Register, key)
    }

    /// Return the generated dynamic-name registry accessor identifier for a concrete callable signature key.
    ///
    /// The registry accessor owns the per-signature `OnceLock<Mutex<...>>` used by the registration helper.
    pub(super) fn callable_name_registry_ident(key: &str) -> proc_macro2::Ident {
        Self::callable_name_symbol_ident(CallableNameSymbolRole::Registry, key)
    }

    /// Return a stable signature key for callable-name helpers when the function-pointer type is concrete.
    pub(super) fn callable_name_signature_key(params: &[IrType], ret: &IrType) -> Option<String> {
        if !params.iter().all(Self::callable_name_type_supported) || !Self::callable_name_type_supported(ret) {
            return None;
        }
        let params = params.iter().map(IrType::rust_name).collect::<Vec<_>>().join(", ");
        Some(format!("fn({params}) -> {}", ret.rust_name()))
    }

    /// Build a callable-name signature key from a function signature.
    fn callable_name_signature_key_from_signature(signature: &FunctionSignature) -> Option<String> {
        let params = signature
            .params
            .iter()
            .map(|param| param.ty.clone())
            .collect::<Vec<_>>();
        Self::callable_name_signature_key(&params, &signature.return_type)
    }

    /// Return whether a type can participate in callable-name helper signatures.
    fn callable_name_type_supported(ty: &IrType) -> bool {
        match ty {
            IrType::Unknown | IrType::Generic(_) | IrType::ImplTrait(_) | IrType::SelfType => false,
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner) => Self::callable_name_type_supported(inner),
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::callable_name_type_supported(key) && Self::callable_name_type_supported(value)
            }
            IrType::Tuple(items) => items.iter().all(Self::callable_name_type_supported),
            IrType::TypeToken(inner) => Self::callable_name_type_supported(inner),
            IrType::ExternalUnion { union, .. } => Self::callable_name_type_supported(union),
            IrType::NamedGeneric(_, args) => args.iter().all(Self::callable_name_type_supported),
            IrType::Function { params, ret } => Self::callable_name_signature_key(params, ret).is_some(),
            IrType::Unit
            | IrType::Bool
            | IrType::Int
            | IrType::Float
            | IrType::Decimal { .. }
            | IrType::String
            | IrType::StrRef
            | IrType::StaticStr
            | IrType::FrozenStr
            | IrType::Bytes
            | IrType::StaticBytes
            | IrType::FrozenBytes
            | IrType::Numeric(_)
            | IrType::Struct(_)
            | IrType::Enum(_)
            | IrType::Trait(_)
            | IrType::RustDisplay(_) => true,
        }
    }

    /// Hash a callable-name signature key with a stable FNV-1a variant.
    fn stable_callable_name_hash(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Return callable-name signature keys defined by the current module.
    pub(super) fn local_callable_name_signature_keys(&self) -> HashSet<String> {
        self.callable_name_local_registry()
            .iter()
            .filter_map(|(_, signature)| Self::callable_name_signature_key_from_signature(signature))
            .collect()
    }

    /// Return the local function registry used for callable-name helpers.
    pub(super) fn callable_name_local_registry(&self) -> &FunctionRegistry {
        self.callable_name_local_registry
            .as_ref()
            .unwrap_or(self.function_registry)
    }

    /// Return whether two call-signature types describe the same emitted surface after transparent aliases expand.
    pub(in crate::backend::ir::emit) fn call_signature_type_matches(&self, left: &IrType, right: &IrType) -> bool {
        if left == right {
            return true;
        }
        let left = self.resolve_type_aliases_for_emit(left);
        let right = self.resolve_type_aliases_for_emit(right);
        left == right
            || Self::semantic_signature_type(&left) == Self::semantic_signature_type(&right)
            || Self::imported_nominal_matches_source_nominal(&left, &right)
            || Self::rust_callback_reference_matches_nominal(&left, &right)
            || Self::rust_callback_reference_matches_nominal(&right, &left)
    }

    /// Return whether a fully-qualified imported nominal type is the source annotation's unqualified spelling.
    ///
    /// Source annotations retain the imported name (`Ui`), while inspected callable metadata uses its canonical
    /// spelling (`egui::Ui`). The source name was already resolved during lowering, so accepting this one-sided
    /// qualification difference preserves the source-owned parameter ABI without conflating two qualified types.
    fn imported_nominal_matches_source_nominal(left: &IrType, right: &IrType) -> bool {
        let (left, right) = match (left, right) {
            (
                IrType::Struct(left) | IrType::NamedGeneric(left, _),
                IrType::Struct(right) | IrType::NamedGeneric(right, _),
            ) => (left, right),
            _ => return false,
        };
        (left.contains("::") ^ right.contains("::")) && left.rsplit("::").next() == right.rsplit("::").next()
    }

    /// Return whether an exact borrowed callback display represents an Incan nominal parameter surface.
    ///
    /// Inspected Rust callbacks retain `&mut crate::Ui`, while the source function they immediately invoke owns the
    /// Incan-level `Ui` annotation and emits its parameter as `&mut Ui`. Treat the nominal payload as the same
    /// callable surface so source mutability remains available to argument planning without erasing the Rust borrow.
    fn rust_callback_reference_matches_nominal(reference: &IrType, nominal: &IrType) -> bool {
        let IrType::RustDisplay(display) = reference else {
            return false;
        };
        let display = display.trim_start();
        let display = display
            .strip_prefix("&mut ")
            .or_else(|| display.strip_prefix("& "))
            .or_else(|| display.strip_prefix('&'))
            .map(str::trim_start);
        let Some(display) = display else {
            return false;
        };
        let display_name = display
            .split('<')
            .next()
            .unwrap_or(display)
            .trim()
            .rsplit("::")
            .next()
            .unwrap_or(display);
        let nominal_name = match nominal {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => name.rsplit("::").next(),
            _ => None,
        };
        nominal_name == Some(display_name)
    }

    /// Return the semantic type shape used for callable-surface comparisons.
    fn semantic_signature_type(ty: &IrType) -> IrType {
        match ty {
            IrType::ExternalUnion { union, .. } => Self::semantic_signature_type(union),
            IrType::List(inner) => IrType::List(Box::new(Self::semantic_signature_type(inner))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(Self::semantic_signature_type(key)),
                Box::new(Self::semantic_signature_type(value)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(Self::semantic_signature_type(inner))),
            IrType::Tuple(items) => IrType::Tuple(items.iter().map(Self::semantic_signature_type).collect()),
            IrType::Option(inner) => IrType::Option(Box::new(Self::semantic_signature_type(inner))),
            IrType::Result(ok, err) => IrType::Result(
                Box::new(Self::semantic_signature_type(ok)),
                Box::new(Self::semantic_signature_type(err)),
            ),
            IrType::Function { params, ret } => IrType::Function {
                params: params.iter().map(Self::semantic_signature_type).collect(),
                ret: Box::new(Self::semantic_signature_type(ret)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(Self::semantic_signature_type(inner))),
            IrType::RefMut(inner) => IrType::RefMut(Box::new(Self::semantic_signature_type(inner))),
            IrType::NamedGeneric(name, args) => {
                IrType::NamedGeneric(name.clone(), args.iter().map(Self::semantic_signature_type).collect())
            }
            IrType::TypeToken(inner) => IrType::TypeToken(Box::new(Self::semantic_signature_type(inner))),
            other => other.clone(),
        }
    }

    /// Resolve transparent type aliases before emission decisions that need structural type information.
    pub(in crate::backend::ir::emit) fn resolve_type_aliases_for_emit(&self, ty: &IrType) -> IrType {
        let mut visiting = HashSet::new();
        self.resolve_type_aliases_for_emit_inner(ty, &mut visiting)
    }

    /// Resolve nested transparent aliases while preserving cycles as their original alias names.
    fn resolve_type_aliases_for_emit_inner(&self, ty: &IrType, visiting: &mut HashSet<String>) -> IrType {
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) if self.type_aliases.contains_key(name) => {
                if !visiting.insert(name.clone()) {
                    return ty.clone();
                }
                let Some(target) = self.type_aliases.get(name) else {
                    visiting.remove(name);
                    return ty.clone();
                };
                let resolved = self.resolve_type_aliases_for_emit_inner(target, visiting);
                visiting.remove(name);
                resolved
            }
            IrType::List(inner) => IrType::List(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(self.resolve_type_aliases_for_emit_inner(key, visiting)),
                Box::new(self.resolve_type_aliases_for_emit_inner(value, visiting)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting))),
            IrType::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|item| self.resolve_type_aliases_for_emit_inner(item, visiting))
                    .collect(),
            ),
            IrType::Option(inner) => {
                IrType::Option(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting)))
            }
            IrType::Result(ok, err) => IrType::Result(
                Box::new(self.resolve_type_aliases_for_emit_inner(ok, visiting)),
                Box::new(self.resolve_type_aliases_for_emit_inner(err, visiting)),
            ),
            IrType::NamedGeneric(name, args) if name == IR_UNION_TYPE_NAME => {
                let mut members = Vec::new();
                for arg in args {
                    match self.resolve_type_aliases_for_emit_inner(arg, visiting) {
                        IrType::NamedGeneric(inner_name, inner_args) if inner_name == IR_UNION_TYPE_NAME => {
                            members.extend(inner_args);
                        }
                        resolved => members.push(resolved),
                    }
                }
                members.sort_by_key(IrType::rust_name);
                members.dedup();
                match members.as_slice() {
                    [single] => single.clone(),
                    _ => IrType::NamedGeneric(name.clone(), members),
                }
            }
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                name.clone(),
                args.iter()
                    .map(|arg| self.resolve_type_aliases_for_emit_inner(arg, visiting))
                    .collect(),
            ),
            IrType::Function { params, ret } => IrType::Function {
                params: params
                    .iter()
                    .map(|param| self.resolve_type_aliases_for_emit_inner(param, visiting))
                    .collect(),
                ret: Box::new(self.resolve_type_aliases_for_emit_inner(ret, visiting)),
            },
            IrType::TypeToken(inner) => {
                IrType::TypeToken(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting)))
            }
            IrType::ExternalUnion { library, union } => IrType::ExternalUnion {
                library: library.clone(),
                union: Box::new(self.resolve_type_aliases_for_emit_inner(union, visiting)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting))),
            IrType::RefMut(inner) => {
                IrType::RefMut(Box::new(self.resolve_type_aliases_for_emit_inner(inner, visiting)))
            }
            _ => ty.clone(),
        }
    }

    /// Emit the generated call that initializes local and imported module statics.
    pub(super) fn emit_module_static_init_call(&self) -> TokenStream {
        if *self.module_needs_initialization.borrow() || !self.imported_static_module_init_bindings.borrow().is_empty()
        {
            let init_fn = Self::rust_ident("__incan_init_module_statics");
            quote! { #init_fn(); }
        } else {
            quote! {}
        }
    }

    /// Replace the imported static bindings that need per-static init calls.
    pub(super) fn set_imported_static_init_bindings(&self, bindings: HashSet<String>) {
        *self.imported_static_init_bindings.borrow_mut() = bindings;
    }

    /// Replace imported static modules that need module-level init calls.
    pub(super) fn set_imported_static_module_init_bindings(&self, bindings: Vec<String>) {
        *self.imported_static_module_init_bindings.borrow_mut() = bindings;
    }

    /// Build the generated Rust identifier for an imported static init shim.
    pub(super) fn imported_static_init_ident(name: &str) -> proc_macro2::Ident {
        let mut rendered = String::from("__incan_init_imported_static_");
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                rendered.push(ch.to_ascii_lowercase());
            } else {
                rendered.push('_');
            }
        }
        proc_macro2::Ident::new(&rendered, proc_macro2::Span::call_site())
    }

    /// Return whether a static binding needs its imported init shim called.
    pub(super) fn static_needs_imported_init_call(&self, name: &str) -> bool {
        self.imported_static_init_bindings.borrow().contains(name)
    }

    /// Return whether this exact static reference needs its imported provider's init shim.
    pub(super) fn static_reference_needs_imported_init_call(
        &self,
        name: &str,
        reference_kind: super::expr::IrStaticReferenceKind,
    ) -> bool {
        matches!(reference_kind, super::expr::IrStaticReferenceKind::Source)
            && self.static_needs_imported_init_call(name)
    }

    /// Return whether a static binding needs any imported static init support.
    pub(super) fn static_needs_imported_init_import(&self, name: &str) -> bool {
        self.static_needs_imported_init_call(name)
            || self
                .imported_static_module_init_bindings
                .borrow()
                .iter()
                .any(|binding| binding == name)
    }

    /// Emit initialization for one exact source or generated static reference.
    pub(super) fn emit_static_init_call_for_reference(
        &self,
        name: &str,
        reference_kind: super::expr::IrStaticReferenceKind,
    ) -> TokenStream {
        if self.static_reference_needs_imported_init_call(name, reference_kind) {
            let init_fn = Self::imported_static_init_ident(name);
            quote! { #init_fn(); }
        } else {
            self.emit_module_static_init_call()
        }
    }

    /// Return the private helper method name used to call callable-object observers through a borrowed payload.
    pub(super) fn result_observer_borrowed_method_name() -> &'static str {
        "__incan_result_observer_borrow___call__"
    }

    /// Return the private helper name used to adapt a named function to a borrowed function-pointer parameter.
    pub(super) fn borrowed_function_adapter_name(name: &str, indices: &[usize]) -> String {
        let suffix = indices.iter().map(usize::to_string).collect::<Vec<_>>().join("_");
        format!("__incan_borrow_adapter_{name}_{suffix}")
    }

    /// Store pre-emission facts describing which observer callbacks need borrowed helper emission.
    pub(super) fn set_result_observer_callable_types(&self, callable_types: HashSet<String>) {
        *self.result_observer_callable_types.borrow_mut() = callable_types;
    }

    /// Store pre-emission facts for named function values that need borrowed function-pointer adapters.
    pub(super) fn set_borrowed_function_adapters(&self, adapters: HashSet<(String, Vec<usize>)>) {
        *self.borrowed_function_adapters.borrow_mut() = adapters;
    }

    /// Return whether a source-owned callable object type needs a borrowed observer helper.
    pub(super) fn needs_result_observer_callable_helper(&self, type_name: &str) -> bool {
        self.result_observer_callable_types.borrow().contains(type_name)
    }

    /// Mark a callable-object borrowed observer helper as emitted, returning false if it was already emitted.
    pub(super) fn claim_result_observer_callable_helper(&self, type_name: &str) -> bool {
        self.emitted_result_observer_callable_helpers
            .borrow_mut()
            .insert(type_name.to_string())
    }

    /// Return whether `name` needs a borrowed adapter for the selected parameter indices.
    pub(super) fn needs_borrowed_function_adapter(&self, name: &str, indices: &[usize]) -> bool {
        self.borrowed_function_adapters
            .borrow()
            .contains(&(name.to_string(), indices.to_vec()))
    }

    /// Set the internal module roots (top-level module names) for a multi-file compilation.
    pub fn set_internal_module_roots(&mut self, roots: HashSet<String>) {
        self.internal_module_roots = roots;
    }

    /// Set the canonical ordinary source modules available to this compilation unit.
    pub fn set_source_module_paths(&mut self, paths: HashSet<Vec<String>>) {
        self.source_module_paths = paths;
    }

    /// Set the canonical source path of the module currently being emitted.
    pub fn set_current_source_module_path(&mut self, path: Option<Vec<String>>) {
        self.current_source_module_path = path;
    }

    /// Set the package identity of the compilation unit currently being emitted.
    pub fn set_current_package_identity(&mut self, identity: Option<String>) {
        self.current_package_identity = identity;
    }

    /// Configure whether anonymous union wrappers are addressed through the crate root.
    pub fn set_qualify_union_types_from_crate(&mut self, enabled: bool) {
        self.qualify_union_types_from_crate = enabled;
    }

    /// Add generated union wrapper definitions that should be emitted by this module.
    pub fn set_generated_union_types(&mut self, types: HashMap<String, IrType>) {
        self.generated_union_types = types;
    }

    /// Configure whether this module emits generated union wrapper definitions.
    pub fn set_emit_generated_union_definitions(&mut self, enabled: bool) {
        self.emit_generated_union_definitions = enabled;
    }

    /// Check if a top-level name is a known internal module root.
    pub(crate) fn is_internal_module_root(&self, name: &str) -> bool {
        self.internal_module_roots.contains(name)
    }

    /// Check if a full module path is known internally.
    pub(crate) fn is_internal_module_path(&self, segments: &[String]) -> bool {
        if let Some(first) = segments.first()
            && self.is_internal_module_root(first)
        {
            return true;
        }
        if segments.is_empty() {
            return false;
        }
        let joined = segments.join("_");
        self.internal_module_roots.contains(&joined)
    }

    /// Set external rust functions.
    pub fn set_external_rust_functions(&mut self, funcs: std::collections::HashSet<String>) {
        self.external_rust_functions = funcs;
    }

    /// Set whether serde is needed.
    pub(crate) fn set_needs_serde(&mut self, needs: bool) {
        *self.needs_serde.borrow_mut() = needs;
    }

    /// Create a Rust identifier for emission, using raw identifiers for keywords.
    ///
    /// This is the only safe way to emit segments like `r#async`:
    /// - `proc_macro2::Ident::new_raw("async", ..)` emits `r#async`
    /// - string-based escaping + `format_ident!("{}", "r#async")` relies on macro parsing quirks and is easy to misuse
    ///   (and `syn::Ident::new("r#async", ..)` is invalid and will panic).
    fn rust_ident(name: &str) -> proc_macro2::Ident {
        let span = proc_macro2::Span::call_site();
        if matches!(name, "self" | "Self" | "crate" | "super") {
            return proc_macro2::Ident::new(name, span);
        }
        if rust_keywords::is_keyword(name) {
            return proc_macro2::Ident::new_raw(name, span);
        }
        proc_macro2::Ident::new(name, span)
    }

    /// Create the emitted identifier for a linker-visible top-level Incan function.
    ///
    /// Lowering registered the source name and canonical identity together. Emission only projects that retained
    /// identity; it never decodes an emitted name or infers identity from source spelling.
    fn rust_function_ident(&self, name: &str) -> proc_macro2::Ident {
        if let Some(projection) = self.function_registry.emitted_projection(name) {
            return Self::rust_ident(&projection);
        }
        if let Some(physical_name) = self.function_registry.generated_physical_name(name) {
            return Self::rust_ident(physical_name);
        }
        Self::rust_ident(name)
    }

    /// Bind every source static spelling used by this IR module to its one compiler-owned projection.
    fn set_static_projections(&self, program: &IrProgram) -> Result<(), EmitError> {
        let mut projections = HashMap::new();
        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Static {
                    name,
                    provenance: IrStaticProvenance::Source(identity),
                    ..
                } => {
                    if !matches!(identity.kind, SemanticSourceTargetKind::Static)
                        || !matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
                    {
                        return Err(EmitError::InternalInvariant(format!(
                            "source static `{name}` carries a non-static or non-Incan canonical identity"
                        )));
                    }
                    let projection = encode_incan_symbol_identity(identity);
                    Self::insert_static_projection(&mut projections, name, &projection)?;
                    Self::insert_static_projection(&mut projections, &projection, &projection)?;
                }
                IrDeclKind::Static {
                    provenance: IrStaticProvenance::CompilerGenerated,
                    ..
                } => {}
                IrDeclKind::SymbolAlias {
                    name,
                    target_canonical: Some(identity),
                    ..
                } if matches!(identity.kind, SemanticSourceTargetKind::Static) => {
                    if !matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. }) {
                        return Err(EmitError::InternalInvariant(format!(
                            "source static alias `{name}` carries a non-Incan canonical identity"
                        )));
                    }
                    let projection = encode_incan_symbol_identity(identity);
                    Self::insert_static_projection(&mut projections, name, &projection)?;
                    Self::insert_static_projection(&mut projections, &projection, &projection)?;
                }
                IrDeclKind::Import { items, .. } => {
                    for item in items.iter().filter(|item| item.is_static) {
                        let Some(identity) = item.canonical.as_ref() else {
                            return Err(EmitError::InternalInvariant(format!(
                                "source static import `{}` has no compiler-owned canonical identity",
                                item.source_binding_name()
                            )));
                        };
                        if !matches!(identity.kind, SemanticSourceTargetKind::Static)
                            || !matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
                        {
                            return Err(EmitError::InternalInvariant(format!(
                                "source static import `{}` carries a non-static or non-Incan canonical identity",
                                item.source_binding_name()
                            )));
                        }
                        let projection = encode_incan_symbol_identity(identity);
                        Self::insert_static_projection(&mut projections, item.source_binding_name(), &projection)?;
                        Self::insert_static_projection(&mut projections, &projection, &projection)?;
                    }
                }
                _ => {}
            }
        }
        *self.static_projections.borrow_mut() = projections;
        Ok(())
    }

    /// Insert one static binding without allowing two canonical declarations to collapse onto the same local name.
    fn insert_static_projection(
        projections: &mut HashMap<String, String>,
        binding: &str,
        projection: &str,
    ) -> Result<(), EmitError> {
        if let Some(existing) = projections.get(binding) {
            if existing != projection {
                return Err(EmitError::InternalInvariant(format!(
                    "source static binding `{binding}` resolves to two canonical projections"
                )));
            }
            return Ok(());
        }
        projections.insert(binding.to_string(), projection.to_string());
        Ok(())
    }

    /// Create the exact Rust identifier for a source static reference.
    ///
    /// Absence is an IR invariant failure rather than permission to guess from the source spelling.
    fn rust_source_static_ident(&self, name: &str) -> Result<proc_macro2::Ident, EmitError> {
        let projection = self.static_projections.borrow().get(name).cloned().ok_or_else(|| {
            EmitError::InternalInvariant(format!(
                "source static reference `{name}` has no compiler-owned canonical projection"
            ))
        })?;
        Ok(Self::rust_ident(&projection))
    }

    /// Create the Rust identifier for a compiler-generated static helper.
    ///
    /// Generated helpers deliberately bypass source projections. This remains true if a synthetic spelling collides
    /// with a source static name.
    fn rust_generated_static_ident(name: &str) -> proc_macro2::Ident {
        let mut rendered = String::with_capacity(name.len().max(1));
        for ch in name.chars() {
            if ch.is_ascii_alphanumeric() {
                rendered.push(ch.to_ascii_uppercase());
            } else {
                rendered.push('_');
            }
        }
        if rendered.is_empty() {
            rendered.push('_');
        }
        proc_macro2::Ident::new(&rendered, proc_macro2::Span::call_site())
    }

    /// Create a Rust identifier from the provenance retained on a static reference.
    fn rust_static_reference_ident(
        &self,
        name: &str,
        reference_kind: super::expr::IrStaticReferenceKind,
    ) -> Result<proc_macro2::Ident, EmitError> {
        match reference_kind {
            super::expr::IrStaticReferenceKind::Source => self.rust_source_static_ident(name),
            super::expr::IrStaticReferenceKind::CompilerGenerated => Ok(Self::rust_generated_static_ident(name)),
        }
    }

    /// Create a Rust identifier from the provenance retained on a static declaration.
    fn rust_static_declaration_ident(
        &self,
        name: &str,
        provenance: &IrStaticProvenance,
    ) -> Result<proc_macro2::Ident, EmitError> {
        match provenance {
            IrStaticProvenance::Source(_) => self.rust_source_static_ident(name),
            IrStaticProvenance::CompilerGenerated => Ok(Self::rust_generated_static_ident(name)),
        }
    }

    /// RFC 023: Set the `rust.module()` Rust backing path for this program.
    ///
    /// When set, `@rust.extern` functions delegate to `<path>::<fn_name>()`.
    pub fn set_rust_module_path(&mut self, path: Option<String>) {
        self.rust_module_path = path;
    }

    /// Deprecated compatibility shim: generated unused/dead lint allows are no longer emitted.
    pub fn without_clippy_allows(self) -> Self {
        self
    }

    /// Deprecated compatibility shim: generated unused/dead lint allows are no longer emitted.
    pub fn set_add_clippy_allows(&mut self, enabled: bool) {
        let _ = enabled;
    }

    /// Enable strict generated Rust lint validation.
    pub fn set_strict_generated_lints(&mut self, enabled: bool) {
        self.emit_strict_generated_lint_denies = enabled;
    }

    /// Set whether public source items are treated as externally reachable during emission.
    pub fn set_preserve_public_items(&mut self, enabled: bool) {
        self.preserve_public_items = enabled;
    }

    /// Set whether value enums in this module should adopt the stdlib `OrdinalKey` trait.
    pub fn set_emit_std_ordinal_value_enum_impls(&mut self, enabled: bool) {
        self.emit_std_ordinal_value_enum_impls = enabled;
    }

    /// Set whether local newtypes should receive compiler-provided `TryFrom[str]` implementations.
    pub fn set_emit_std_string_try_from_newtype_impls(&mut self, enabled: bool) {
        self.emit_std_string_try_from_newtype_impls = enabled;
    }

    /// Set value-enum metadata loaded from `.incnlib` dependencies for consumer-side `OrdinalKey` impls.
    pub(crate) fn set_external_ordinal_value_enums(&mut self, enums: Vec<ExternalOrdinalValueEnum>) {
        self.external_ordinal_value_enums = enums;
    }

    /// Set user-authored key metadata loaded from `.incnlib` dependencies for consumer-side `OrdinalKey` impls.
    pub(crate) fn set_external_ordinal_custom_keys(&mut self, keys: Vec<ExternalOrdinalCustomKey>) {
        self.external_ordinal_custom_keys = keys;
    }

    /// Set public serialized value-enum identities for library emission.
    pub(crate) fn set_public_ordinal_type_identities(&mut self, identities: HashMap<String, String>) {
        self.public_ordinal_type_identities = identities;
    }

    /// Set private items that are called by compiler-generated code injected after IR emission.
    pub fn set_externally_reachable_items(&mut self, names: HashSet<String>) {
        self.externally_reachable_items = names;
    }

    /// Replace pre-emission usage facts for the program currently being emitted.
    pub(super) fn set_generated_use_analysis(&self, analysis: GeneratedUseAnalysis) {
        *self.generated_use_analysis.borrow_mut() = analysis;
    }

    /// True when a top-level declaration with `name` should be emitted.
    pub(super) fn should_emit_decl_name(&self, name: &str, visibility: &Visibility) -> bool {
        (self.preserve_public_items && !matches!(visibility, Visibility::Private))
            || self.generated_use_analysis.borrow().reachable_items.contains(name)
    }

    /// True when an import binding should be emitted because generated code references it.
    pub(super) fn should_emit_import_binding(&self, name: &str) -> bool {
        self.generated_use_analysis.borrow().used_imports.contains(name)
    }

    /// True when a Rust trait import should be emitted for extension-method lookup.
    pub(super) fn should_emit_extension_trait_import(&self, name: &str) -> bool {
        self.generated_use_analysis
            .borrow()
            .used_extension_trait_imports
            .contains(name)
    }

    /// True when a method should be emitted for a preserved public surface or an observed generated-use call.
    pub(super) fn should_emit_method(&self, target_type: &str, method_name: &str, visibility: &Visibility) -> bool {
        self.generated_use_analysis.borrow().should_retain_method(
            self.preserve_public_items,
            target_type,
            method_name,
            visibility,
        )
    }

    /// True when the compiler-provided enum `message()` helper should be emitted.
    ///
    /// Public enums keep the helper as part of their exported Rust-facing surface. Private and crate-visible helper
    /// enums emit it only when reachable generated code actually calls `.message()`, which avoids dead-code warnings
    /// without hiding them behind crate-level lint suppressions.
    pub(super) fn should_emit_enum_message_method(&self, enum_name: &str, visibility: &Visibility) -> bool {
        (self.preserve_public_items && matches!(visibility, Visibility::Public))
            || self
                .generated_use_analysis
                .borrow()
                .used_methods
                .contains(&(enum_name.to_string(), "message".to_string()))
    }

    /// Select the generated Rust constructor surface while preserving type-private Incan fields.
    ///
    /// Public models with only default-backed private fields receive a public-field bridge. Required private model
    /// fields have no public native constructor, while classes preserve the complete constructor inputs accepted by
    /// the Incan typechecker.
    pub(super) fn struct_constructor_surface(&self, s: &IrStruct) -> StructConstructorSurface {
        if s.fields.is_empty() {
            return StructConstructorSurface::Absent;
        }

        let analysis = self.generated_use_analysis.borrow();
        let has_type_private_fields = s.fields.iter().any(|field| field.is_type_private);
        let has_required_type_private_field = s
            .fields
            .iter()
            .any(|field| field.is_type_private && field.default.is_none());
        let constructor_is_used = analysis.used_constructors.contains(&s.name);

        if matches!(s.visibility, Visibility::Public) && has_type_private_fields {
            match s.kind {
                IrStructKind::Model if !has_required_type_private_field => {
                    return StructConstructorSurface::PublicBridge;
                }
                IrStructKind::Model => {
                    return if constructor_is_used {
                        StructConstructorSurface::PrivateAllFields
                    } else {
                        StructConstructorSurface::Absent
                    };
                }
                IrStructKind::Class => return StructConstructorSurface::PublicAllFields,
                IrStructKind::Newtype => {}
            }
        }

        if !constructor_is_used {
            return StructConstructorSurface::Absent;
        }

        match s.visibility {
            Visibility::Public => StructConstructorSurface::PublicAllFields,
            Visibility::Crate => StructConstructorSurface::CrateAllFields,
            Visibility::Private => StructConstructorSurface::PrivateAllFields,
        }
    }

    /// True when a generated private field needs a narrow `dead_code` expectation because Rust cannot see an
    /// Incan-level semantic use for it in the emitted program.
    pub(super) fn should_expect_private_field_dead_code(
        &self,
        struct_name: &str,
        field_name: &str,
        visibility: &Visibility,
    ) -> bool {
        matches!(visibility, Visibility::Private)
            && !self
                .generated_use_analysis
                .borrow()
                .read_fields
                .contains(&(struct_name.to_string(), field_name.to_string()))
    }

    /// Set whether to emit the Zen of Incan in main.
    pub fn set_emit_zen(&mut self, emit: bool) {
        self.emit_zen_in_main = emit;
    }

    /// Set type-to-module path mappings for qualifying route wrapper types.
    pub fn set_type_module_paths(&mut self, paths: HashMap<String, Vec<String>>, ambiguous: HashSet<String>) {
        self.type_module_paths = paths;
        self.ambiguous_type_names = ambiguous;
    }

    /// Set value-to-module path mappings for dependency expressions that must be emitted outside their defining
    /// module.
    pub fn set_value_module_paths(&mut self, paths: HashMap<String, Vec<String>>, ambiguous: HashSet<String>) {
        self.value_module_paths = paths;
        self.ambiguous_value_names = ambiguous;
    }

    /// Emit a qualified path for an item imported from dependency metadata.
    pub(in crate::backend::ir::emit) fn emit_dependency_item_path(
        &self,
        module_path: &[String],
        name: &str,
    ) -> Option<TokenStream> {
        let mut segments = vec![quote! { crate }];
        for segment in module_path {
            let ident = Self::rust_ident(segment);
            segments.push(quote! { #ident });
        }
        let ident = Self::rust_ident(name);
        segments.push(quote! { #ident });

        let mut iter = segments.into_iter();
        let first = iter.next()?;
        Some(iter.fold(first, |acc, segment| quote! { #acc :: #segment }))
    }

    /// Emit a dependency-qualified type path when a local type name is ambiguous.
    pub(in crate::backend::ir::emit) fn emit_dependency_type_path(&self, name: &str) -> Option<TokenStream> {
        if name.contains("::") || self.ambiguous_type_names.contains(name) {
            return None;
        }
        if let Some(module_path) = self.type_module_paths.get(name) {
            return self.emit_dependency_item_path(module_path, name);
        }

        // Crate-root anonymous unions are emitted before module-local imports. A split compiled provider can collect
        // a union whose payload is owned by another SDK component, so qualify the payload through the shared provider
        // facade instead of relying on a bare type name being in scope at the crate root.
        let module_paths = self.compiled_sdk_type_module_paths.get(name)?;
        if module_paths.len() != 1 {
            return None;
        }
        let module_path = module_paths.iter().next()?;
        let mut facade_path = vec!["__incan_std".to_string()];
        facade_path.extend(module_path.iter().cloned());
        self.emit_dependency_item_path(&facade_path, name)
    }

    /// Emit a checked public-provider type through its Rust-visible path when a crate-root wrapper cannot rely on
    /// imports.
    pub(in crate::backend::ir::emit) fn emit_public_dependency_type_path(&self, name: &str) -> Option<TokenStream> {
        if name.contains("::") || self.local_nominal_type_names.contains(name) {
            return None;
        }
        let path = self.public_dependency_type_paths.get(name)?;
        let mut segments = path.iter().map(|segment| Self::rust_ident(segment));
        let first = segments.next()?;
        let name = Self::rust_ident(name);
        let path = segments.fold(quote! { #first }, |path, segment| quote! { #path :: #segment });
        Some(quote! { #path :: #name })
    }

    /// Emit a dependency-qualified value path when a local value name is ambiguous.
    pub(in crate::backend::ir::emit) fn emit_dependency_value_path(&self, name: &str) -> Option<TokenStream> {
        if name.contains("::") || self.ambiguous_value_names.contains(name) {
            return None;
        }
        let module_path = self.value_module_paths.get(name)?;
        self.emit_dependency_item_path(module_path, name)
    }

    /// Set imported enum type names discovered during codegen setup.
    pub fn set_dependency_enum_types(&mut self, enum_type_names: HashSet<String>) {
        self.dependency_enum_types = enum_type_names;
    }

    /// Seed nominal declaration metadata from another lowered module.
    ///
    /// Multi-file emission creates one Rust module at a time, but constructor/default emission still needs the
    /// declared field list and default expressions for imported Incan types used by the current module.
    pub(crate) fn seed_nominal_metadata_from_program(&mut self, program: &IrProgram) {
        self.seed_nominal_metadata_from_program_inner(program, false);
    }

    /// Seed dependency metadata while avoiding ambiguous short names.
    ///
    /// Dependency modules may export the same model name from different source modules, such as `std.fs.IoError` and
    /// `std.io.IoError`. The IR currently stores constructor names as short names, so retaining field metadata for
    /// ambiguous imported types can make one module validate a constructor against another module's fields.
    pub(crate) fn seed_dependency_nominal_metadata_from_program(&mut self, program: &IrProgram) {
        self.seed_nominal_metadata_from_program_inner(program, true);
        self.register_source_dependency_constructor_reexports(program);
        // A full emitter may seed hundreds of checked provider modules. Resolving the complete public-reexport
        // graph after each module makes the fixed-point walk quadratic in that dependency surface. Delay the walk
        // until a consumer actually binds constructors, when all currently available seed data can be resolved in
        // one pass to the same fixed point.
        self.source_dependency_constructor_reexports_dirty = true;
    }

    /// Seed public dependency nominal metadata from `.incnlib` manifests.
    ///
    /// Package consumers do not have the provider's lowered IR available, but const validation and constructor emission
    /// need the same field metadata for public models/classes that source-module consumers receive from lowered
    /// dependency modules.
    pub(crate) fn seed_public_dependency_nominal_metadata(&mut self, index: &LibraryManifestIndex) {
        let mut counts = HashMap::<String, usize>::new();
        let mut public_type_paths = HashMap::<String, HashSet<Vec<String>>>::new();
        for library in index.known_libraries() {
            let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(&library) else {
                continue;
            };
            let mut manifest_nominal_names = manifest
                .exports
                .models
                .iter()
                .map(|model| model.name.clone())
                .chain(manifest.exports.classes.iter().map(|class| class.name.clone()))
                .chain(manifest.exports.enums.iter().map(|enum_| enum_.name.clone()))
                .chain(manifest.exports.newtypes.iter().map(|newtype| newtype.name.clone()))
                .collect::<Vec<_>>();
            manifest_nominal_names.sort();
            manifest_nominal_names.dedup();
            for name in manifest_nominal_names {
                public_type_paths.entry(name).or_default().insert(vec![library.clone()]);
            }
            if let Some(api) = manifest.contract_metadata.api.as_ref() {
                for module in &api.modules {
                    let mut provider_path = vec![library.clone()];
                    if !matches!(module.module_path.as_slice(), [root] if root == "lib" || root == "main") {
                        provider_path.extend(module.module_path.iter().cloned());
                    }
                    for declaration in module.declarations.iter().filter(|declaration| {
                        crate::frontend::api_metadata::checked_api_declaration_is_public_namespace_member(declaration)
                    }) {
                        let nominal_name = match declaration {
                            ApiDeclaration::Model(model) => Some(&model.name),
                            ApiDeclaration::Class(class) => Some(&class.name),
                            ApiDeclaration::Enum(enum_) => Some(&enum_.name),
                            ApiDeclaration::Newtype(newtype) => Some(&newtype.name),
                            _ => None,
                        };
                        if let Some(name) = nominal_name {
                            public_type_paths
                                .entry(name.clone())
                                .or_default()
                                .insert(provider_path.clone());
                        }
                    }
                }
            }
            let mut public_names = manifest
                .exports
                .models
                .iter()
                .map(|model| model.name.clone())
                .chain(manifest.exports.classes.iter().map(|class| class.name.clone()))
                .chain(manifest.exports.aliases.iter().map(|alias| alias.name.clone()))
                .collect::<Vec<_>>();
            public_names.sort();
            public_names.dedup();
            for public_name in public_names {
                if let Some(shape) = Self::manifest_constructor_shape_for_public_name(manifest, &public_name) {
                    self.pub_dependency_constructor_metadata.insert(
                        (library.clone(), vec![public_name.clone()]),
                        StructConstructorMetadata::from_manifest_fields(&library, shape.kind, &shape.fields),
                    );
                    *counts.entry(public_name).or_default() += 1;
                }
            }
            if let Some(api) = manifest.contract_metadata.api.as_ref() {
                for module in &api.modules {
                    for declaration in module.declarations.iter().filter(|declaration| {
                        crate::frontend::api_metadata::checked_api_declaration_is_public_namespace_member(declaration)
                    }) {
                        let Some(name) = Self::api_nominal_declaration_name(declaration) else {
                            continue;
                        };
                        let Some(shape) = Self::manifest_constructor_shape_for_api_declaration(api, declaration) else {
                            continue;
                        };
                        let mut public_paths = vec![module.module_path.clone()];
                        if module.module_path.len() > 1 {
                            public_paths.push(module.module_path[..module.module_path.len() - 1].to_vec());
                        }
                        for mut public_path in public_paths {
                            public_path.push(name.to_string());
                            self.pub_dependency_constructor_metadata.insert(
                                (library.clone(), public_path),
                                StructConstructorMetadata::from_manifest_fields(&library, shape.kind, &shape.fields),
                            );
                        }
                        *counts.entry(name.to_string()).or_default() += 1;
                    }
                }
            }
        }

        self.public_dependency_type_paths = public_type_paths
            .into_iter()
            .filter_map(|(name, paths)| {
                if paths.len() == 1 {
                    paths.into_iter().next().map(|path| (name, path))
                } else {
                    None
                }
            })
            .collect();

        for library in index.known_libraries() {
            let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = index.get(&library) else {
                continue;
            };
            let mut public_names = manifest
                .exports
                .models
                .iter()
                .map(|model| model.name.clone())
                .chain(manifest.exports.classes.iter().map(|class| class.name.clone()))
                .chain(manifest.exports.aliases.iter().map(|alias| alias.name.clone()))
                .collect::<Vec<_>>();
            public_names.sort();
            public_names.dedup();
            for public_name in public_names {
                if counts.get(&public_name).copied().unwrap_or_default() == 1
                    && let Some(shape) = Self::manifest_constructor_shape_for_public_name(manifest, &public_name)
                {
                    self.register_manifest_constructor_metadata(&library, &public_name, shape.kind, &shape.fields);
                }
            }
        }
    }

    /// Return the public nominal name represented by one checked API declaration.
    fn api_nominal_declaration_name(declaration: &ApiDeclaration) -> Option<&str> {
        match declaration {
            ApiDeclaration::Model(model) => Some(&model.name),
            ApiDeclaration::Class(class) => Some(&class.name),
            ApiDeclaration::Alias(alias) => Some(&alias.name),
            _ => None,
        }
    }

    /// Resolve constructor shape for a checked API declaration, following public aliases by exact source path.
    fn manifest_constructor_shape_for_api_declaration(
        api: &crate::frontend::api_metadata::CheckedApiMetadataPackage,
        declaration: &ApiDeclaration,
    ) -> Option<ManifestConstructorShape> {
        match declaration {
            ApiDeclaration::Model(model) => Some(ManifestConstructorShape {
                kind: IrStructKind::Model,
                fields: model_export_from_api(model).fields,
            }),
            ApiDeclaration::Class(class) => Some(ManifestConstructorShape {
                kind: IrStructKind::Class,
                fields: class_export_from_api(class).fields,
            }),
            ApiDeclaration::Alias(alias) => Self::api_declaration_for_target_path(api, &alias.target_path)
                .and_then(|target| Self::manifest_constructor_shape_for_api_declaration(api, target)),
            _ => None,
        }
    }

    /// Resolve the constructor shape behind one exact public export, including facade aliases backed by checked API
    /// metadata.
    fn manifest_constructor_shape_for_public_name(
        manifest: &LibraryManifest,
        public_name: &str,
    ) -> Option<ManifestConstructorShape> {
        if let Some(model) = manifest.exports.models.iter().find(|model| model.name == public_name) {
            return Some(ManifestConstructorShape {
                kind: IrStructKind::Model,
                fields: model.fields.clone(),
            });
        }
        if let Some(class) = manifest.exports.classes.iter().find(|class| class.name == public_name) {
            return Some(ManifestConstructorShape {
                kind: IrStructKind::Class,
                fields: class.fields.clone(),
            });
        }
        let target_path = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)
            .and_then(|entry| entry.target_path())
            .or_else(|| {
                manifest
                    .exports
                    .aliases
                    .iter()
                    .find(|alias| alias.name == public_name)
                    .map(|alias| alias.target_path.as_slice())
            })?;
        if let Some(api) = manifest.contract_metadata.api.as_ref()
            && let Some(declaration) = Self::api_declaration_for_target_path(api, target_path)
        {
            return match declaration {
                ApiDeclaration::Model(model) => Some(ManifestConstructorShape {
                    kind: IrStructKind::Model,
                    fields: model_export_from_api(model).fields,
                }),
                ApiDeclaration::Class(class) => Some(ManifestConstructorShape {
                    kind: IrStructKind::Class,
                    fields: class_export_from_api(class).fields,
                }),
                _ => None,
            };
        }
        let target_name = target_path.last()?;
        if let Some(model) = manifest.exports.models.iter().find(|model| &model.name == target_name) {
            return Some(ManifestConstructorShape {
                kind: IrStructKind::Model,
                fields: model.fields.clone(),
            });
        }
        manifest
            .exports
            .classes
            .iter()
            .find(|class| &class.name == target_name)
            .map(|class| ManifestConstructorShape {
                kind: IrStructKind::Class,
                fields: class.fields.clone(),
            })
    }

    /// Resolve one exact checked API declaration from its provider-local identity path.
    fn api_declaration_for_target_path<'api>(
        api: &'api crate::frontend::api_metadata::CheckedApiMetadataPackage,
        target_path: &[String],
    ) -> Option<&'api ApiDeclaration> {
        let name = target_path.last()?;
        let path = target_path.strip_prefix(&["crate".to_string()]).unwrap_or(target_path);
        let module_path = path.get(..path.len().saturating_sub(1))?;
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        module.declarations.iter().find(|declaration| match declaration {
            ApiDeclaration::Model(model) => model.name == *name,
            ApiDeclaration::Class(class) => class.name == *name,
            _ => false,
        })
    }

    /// Return canonical provider candidates for one ordinary source import in source-resolution order.
    ///
    /// Unqualified imports first search beside the current source module and then at the source root. Absolute and
    /// explicit-parent imports have one candidate. Matching candidates against the checked dependency registry keeps
    /// emission aligned with the module that source resolution actually loaded without selecting by declaration shape.
    fn source_import_module_candidates(
        program: &IrProgram,
        path: &[String],
        qualifier: IrImportQualifier,
    ) -> Vec<Vec<String>> {
        let current_module = program
            .source_module_name
            .as_deref()
            .map(|name| name.split('.').map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let (is_absolute, parent_levels) = match qualifier {
            IrImportQualifier::None => return Vec::new(),
            IrImportQualifier::Auto => (false, 0),
            IrImportQualifier::Crate => (true, 0),
            IrImportQualifier::Super(levels) => (false, levels),
        };
        logical_source_path_candidates(&current_module, path, is_absolute, parent_levels)
    }

    /// Record public source-import projections so facade exports retain their declaring provider's constructor ABI.
    fn register_source_dependency_constructor_reexports(&mut self, program: &IrProgram) {
        let Some(exporting_module) = program
            .source_module_name
            .as_deref()
            .map(|name| name.split('.').map(str::to_string).collect::<Vec<_>>())
        else {
            return;
        };
        for decl in &program.declarations {
            let IrDeclKind::Import {
                visibility: Visibility::Public,
                origin: IrImportOrigin::Standard,
                qualifier,
                path,
                items,
                ..
            } = &decl.kind
            else {
                continue;
            };
            if path
                .first()
                .is_some_and(|root| root == stdlib::STDLIB_ROOT || root == stdlib::INCAN_STD_NAMESPACE)
            {
                continue;
            }
            let target_module_candidates = Self::source_import_module_candidates(program, path, *qualifier);
            for item in items {
                let projection = SourceConstructorReexport {
                    exporting_module: exporting_module.clone(),
                    target_module_candidates: target_module_candidates.clone(),
                    target_name: item.name.clone(),
                    exported_name: item.alias.clone().unwrap_or_else(|| item.name.clone()),
                };
                if !self.source_dependency_constructor_reexports.contains(&projection) {
                    self.source_dependency_constructor_reexports.push(projection);
                }
            }
        }
    }

    /// Resolve every known source facade projection to its exact declaring constructor metadata.
    ///
    /// Repeating until no projection changes makes this independent of dependency registration order and preserves
    /// provider identity through multi-hop aliases without falling back to short names or constructor shape.
    fn propagate_source_dependency_constructor_reexports(&mut self) {
        if !std::mem::take(&mut self.source_dependency_constructor_reexports_dirty) {
            return;
        }
        loop {
            let additions = self
                .source_dependency_constructor_reexports
                .iter()
                .filter_map(|projection| {
                    let export_key = (projection.exporting_module.clone(), projection.exported_name.clone());
                    if self.source_dependency_constructor_metadata.contains_key(&export_key) {
                        return None;
                    }
                    projection
                        .target_module_candidates
                        .iter()
                        .find_map(|module_path| {
                            self.source_dependency_constructor_metadata
                                .get(&(module_path.clone(), projection.target_name.clone()))
                        })
                        .cloned()
                        .map(|metadata| (export_key, metadata))
                })
                .collect::<Vec<_>>();
            if additions.is_empty() {
                return;
            }
            for (key, metadata) in additions {
                self.source_dependency_constructor_metadata.insert(key, metadata);
            }
        }
    }

    /// Bind checked ordinary source dependency constructors to the exact local import names used by this program.
    ///
    /// The exact module/declaration key is resolved before the local alias is installed. This deliberately replaces
    /// short-name variants so sibling modules exporting the same class name cannot exchange constructor bridge ABIs.
    fn bind_source_dependency_constructor_metadata(&mut self, program: &IrProgram) {
        self.propagate_source_dependency_constructor_reexports();
        let bindings = program
            .declarations
            .iter()
            .filter_map(|decl| {
                let IrDeclKind::Import {
                    origin: IrImportOrigin::Standard,
                    qualifier,
                    path,
                    items,
                    ..
                } = &decl.kind
                else {
                    return None;
                };
                if path
                    .first()
                    .is_some_and(|root| root == stdlib::STDLIB_ROOT || root == stdlib::INCAN_STD_NAMESPACE)
                {
                    return None;
                }
                let module_candidates = Self::source_import_module_candidates(program, path, *qualifier);
                let metadata_by_identity = &self.source_dependency_constructor_metadata;
                Some(items.iter().filter_map(move |item| {
                    let exact = module_candidates.iter().find_map(|module_path| {
                        metadata_by_identity
                            .get(&(module_path.clone(), item.name.clone()))
                            .cloned()
                    });
                    let metadata = exact.or_else(|| {
                        let mut providers = metadata_by_identity
                            .iter()
                            .filter_map(|((module_path, declared_name), metadata)| {
                                (declared_name == &item.name && module_path.ends_with(path))
                                    .then_some((module_path.clone(), metadata.clone()))
                            })
                            .collect::<Vec<_>>();
                        providers.sort_by(|(left, _), (right, _)| left.cmp(right));
                        providers.dedup_by(|(left, _), (right, _)| left == right);
                        (providers.len() == 1).then(|| providers.remove(0).1)
                    });
                    metadata.map(|metadata| (item.alias.clone().unwrap_or_else(|| item.name.clone()), metadata))
                }))
            })
            .flatten()
            .collect::<Vec<_>>();
        for (binding, metadata) in bindings {
            self.struct_constructor_metadata.insert(binding, vec![metadata]);
        }
    }

    /// Bind exact public dependency constructor metadata to the local names introduced by this program's imports.
    fn bind_public_dependency_constructor_metadata(&mut self, program: &IrProgram) {
        let bindings = program
            .declarations
            .iter()
            .filter_map(|decl| {
                let IrDeclKind::Import {
                    origin: IrImportOrigin::PubLibrary { dependency_key },
                    path,
                    items,
                    ..
                } = &decl.kind
                else {
                    return None;
                };
                Some(items.iter().filter_map(|item| {
                    let mut public_path = path.iter().skip(1).cloned().collect::<Vec<_>>();
                    public_path.push(item.name.clone());
                    self.pub_dependency_constructor_metadata
                        .get(&(dependency_key.clone(), public_path))
                        .cloned()
                        .map(|metadata| (item.alias.clone().unwrap_or_else(|| item.name.clone()), metadata))
                }))
            })
            .flatten()
            .collect::<Vec<_>>();
        for (binding, metadata) in bindings {
            // An explicit import carries exact package/export identity. Replace any short-name fallback seeded before
            // this program rather than letting a same-shaped declaration from another dependency win by insertion
            // order.
            self.struct_constructor_metadata.insert(binding, vec![metadata]);
        }
    }

    /// Seed nominal method and enum metadata from one compiled SDK-provider manifest.
    ///
    /// Built-in stdlib consumers intentionally do not materialize provider source modules. The artifact manifest is
    /// therefore the sole metadata source for whether a receiver uses Incan call semantics and whether a member is an
    /// enum variant rather than a Rust field access.
    pub(crate) fn seed_sdk_provider_manifest_metadata(&mut self, manifest: &LibraryManifest) {
        let provider_crate = manifest.name.replace('-', "_");
        for entry in &manifest.contract_metadata.identity_graph.exports {
            if entry.kind != ExportIdentityKind::Function || entry.public_path.first() != Some(&manifest.name) {
                continue;
            }
            let Some(identity) = entry.canonical.as_ref().and_then(|canonical| canonical.hydrate()) else {
                continue;
            };
            let public_std_path = std::iter::once(stdlib::STDLIB_ROOT.to_string())
                .chain(entry.public_path.iter().skip(1).cloned())
                .collect::<Vec<_>>();
            let source_std_path = std::iter::once(stdlib::STDLIB_ROOT.to_string())
                .chain(entry.source_path.iter().cloned())
                .collect::<Vec<_>>();
            for path in [public_std_path, source_std_path] {
                if self.ambiguous_compiled_sdk_function_paths.contains(&path) {
                    continue;
                }
                match self.compiled_sdk_function_identities.get(&path) {
                    Some(existing) if existing != &identity => {
                        self.compiled_sdk_function_identities.remove(&path);
                        self.ambiguous_compiled_sdk_function_paths.insert(path);
                    }
                    Some(_) => {}
                    None => {
                        self.compiled_sdk_function_identities.insert(path, identity.clone());
                    }
                }
            }
        }
        self.seed_compiled_provider_export_metadata(
            &provider_crate,
            &manifest.exports.models,
            &manifest.exports.classes,
            &manifest.exports.enums,
            &manifest.exports.newtypes,
        );

        // The aggregate artifact's top-level export list describes its crate facade, while its checked API records
        // the public declarations of every `std.*` provider module. Consumers must use that artifact-owned API
        // instead of reopening the provider source modules.
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return;
        };
        self.compiled_sdk_module_paths
            .extend(api.modules.iter().map(|module| module.module_path.clone()));
        let mut models = Vec::new();
        let mut classes = Vec::new();
        let mut enums = Vec::new();
        let mut newtypes = Vec::new();
        let mut functions = Vec::new();
        for module in &api.modules {
            for declaration in &module.declarations {
                let nominal_name = match declaration {
                    ApiDeclaration::Model(model) => Some(model.name.as_str()),
                    ApiDeclaration::Class(class) => Some(class.name.as_str()),
                    ApiDeclaration::Enum(enum_) => Some(enum_.name.as_str()),
                    ApiDeclaration::Newtype(newtype) => Some(newtype.name.as_str()),
                    _ => None,
                };
                if let Some(name) = nominal_name {
                    self.compiled_sdk_type_module_paths
                        .entry(name.to_string())
                        .or_default()
                        .insert(module.module_path.clone());
                }
                match declaration {
                    ApiDeclaration::Model(model) => models.push(model_export_from_api(model)),
                    ApiDeclaration::Class(class) => classes.push(class_export_from_api(class)),
                    ApiDeclaration::Enum(enum_) => enums.push(enum_export_from_api(enum_)),
                    ApiDeclaration::Newtype(newtype) => newtypes.push(newtype_export_from_api(newtype)),
                    ApiDeclaration::Function(function) => functions.push(function_export_from_api(function)),
                    _ => {}
                }
            }
        }
        self.seed_compiled_provider_export_metadata(&provider_crate, &models, &classes, &enums, &newtypes);
        self.seed_compiled_provider_factory_metadata(&functions, &models, &classes);
    }

    /// Set provider module paths when a caller already derived them from the artifact entrypoint or manifest.
    pub(crate) fn set_compiled_sdk_module_paths(&mut self, paths: HashSet<Vec<String>>) {
        self.compiled_sdk_module_paths = paths;
    }

    /// Return whether a dependency module is supplied by a linked compiled SDK provider.
    pub(in crate::backend::ir::emit) fn is_compiled_sdk_module_path(&self, module: &[String]) -> bool {
        self.compiled_sdk_module_paths.contains(module)
    }

    /// Return whether an emitted `__incan_std.*` path is supplied by a linked compiled SDK provider.
    pub(in crate::backend::ir::emit) fn is_compiled_sdk_emission_path(&self, path: &[String]) -> bool {
        path.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE)
            && self.is_compiled_sdk_module_path(&path[1..])
    }

    /// Return whether a named type is owned by the direct crate-root projection of a compiled stdlib provider module.
    pub(in crate::backend::ir::emit) fn is_compiled_sdk_type_in_module(
        &self,
        type_name: &str,
        source_module: &str,
    ) -> bool {
        let Some(artifact_module) = source_module.strip_prefix("std.") else {
            return false;
        };
        let expected = artifact_module.split('.');
        self.compiled_sdk_type_module_paths
            .get(type_name)
            .is_some_and(|modules| {
                modules
                    .iter()
                    .any(|module| module.iter().map(String::as_str).eq(expected.clone()))
            })
    }

    /// Seed nominal metadata represented by one artifact export surface.
    fn seed_compiled_provider_export_metadata(
        &mut self,
        provider_crate: &str,
        models: &[crate::library_manifest::ModelExport],
        classes: &[crate::library_manifest::ClassExport],
        enums: &[crate::library_manifest::EnumExport],
        newtypes: &[NewtypeExport],
    ) {
        for model in models {
            self.register_manifest_constructor_metadata(
                provider_crate,
                &model.name,
                IrStructKind::Model,
                &model.fields,
            );
            self.register_manifest_method_metadata(&model.name, &model.methods, &model.type_params);
        }
        for class in classes {
            self.register_manifest_constructor_metadata(
                provider_crate,
                &class.name,
                IrStructKind::Class,
                &class.fields,
            );
            self.register_manifest_method_metadata(&class.name, &class.methods, &class.type_params);
        }
        for enum_ in enums {
            for variant in &enum_.variants {
                let fields = variant
                    .fields
                    .iter()
                    .map(Self::manifest_type_ref_to_ir_type)
                    .collect::<Vec<_>>();
                self.enum_variant_fields.insert(
                    (enum_.name.clone(), variant.name.clone()),
                    if fields.is_empty() {
                        VariantFields::Unit
                    } else {
                        VariantFields::Tuple(fields)
                    },
                );
            }
            for alias in &enum_.variant_aliases {
                self.enum_variant_aliases
                    .insert((enum_.name.clone(), alias.name.clone()), alias.target.clone());
            }
            self.register_manifest_method_metadata(&enum_.name, &enum_.methods, &enum_.type_params);
        }
        // Newtypes carry method bodies just like models and classes. Consumers do not lower the provider source, so
        // their artifact metadata must participate in the same Incan ownership planning as every other nominal type.
        for newtype in newtypes {
            if let IrType::Struct(backing_type) | IrType::NamedGeneric(backing_type, _) =
                Self::manifest_type_ref_to_ir_type(&newtype.underlying)
            {
                self.newtype_backing_type_names
                    .entry(backing_type)
                    .or_default()
                    .insert(newtype.name.clone());
            }
            self.register_manifest_method_metadata(&newtype.name, &newtype.methods, &newtype.type_params);
        }
    }

    /// Project factory method metadata onto its public constructor identity.
    fn seed_compiled_provider_factory_metadata(
        &mut self,
        functions: &[crate::library_manifest::FunctionExport],
        models: &[crate::library_manifest::ModelExport],
        classes: &[crate::library_manifest::ClassExport],
    ) {
        // Public factories such as `BytesIO()` can expose an otherwise-internal nominal return type. Call lowering
        // preserves the public factory identity at some consumer boundaries, so project the returned type's method
        // metadata onto that identity as well. This is manifest data, not a source-module fallback.
        for function in functions {
            let IrType::Struct(returned_name) = Self::manifest_type_ref_to_ir_type(&function.return_type) else {
                continue;
            };
            if let Some(model) = models.iter().find(|model| model.name == returned_name) {
                self.register_manifest_method_metadata(&function.name, &model.methods, &model.type_params);
            }
            if let Some(class) = classes.iter().find(|class| class.name == returned_name) {
                self.register_manifest_method_metadata(&function.name, &class.methods, &class.type_params);
            }
        }
    }

    /// Register method-call metadata reconstructed from one `.incnlib` nominal export.
    fn register_manifest_method_metadata(
        &mut self,
        owner: &str,
        methods: &[MethodExport],
        type_params: &[crate::library_manifest::TypeParamExport],
    ) {
        for method in methods {
            if let Some(identity) = method.canonical.as_ref().and_then(|canonical| canonical.hydrate()) {
                self.register_member_projection(owner, &method.name, identity);
            }
            let key = (owner.to_string(), method.name.clone());
            self.method_signatures.insert(
                key.clone(),
                FunctionSignature {
                    params: method
                        .params
                        .iter()
                        .map(|param| FunctionParam {
                            name: param.name.clone(),
                            ty: Self::manifest_type_ref_to_ir_type(&param.ty),
                            mutability: Mutability::Immutable,
                            is_self: false,
                            kind: match param.kind {
                                ParamKindExport::Normal => crate::frontend::ast::ParamKind::Normal,
                                ParamKindExport::RestPositional => crate::frontend::ast::ParamKind::RestPositional,
                                ParamKindExport::RestKeyword => crate::frontend::ast::ParamKind::RestKeyword,
                            },
                            // Call-site defaults are already lowered from the manifest when the call is resolved.
                            // This cache is for Rust ownership and variant emission only.
                            default: None,
                        })
                        .collect(),
                    return_type: Self::manifest_type_ref_to_ir_type(&method.return_type),
                },
            );
            self.method_signature_type_params
                .insert(key, type_params.iter().map(|param| param.name.clone()).collect());
        }
    }

    /// Seed nominal metadata, optionally skipping ambiguous dependency names.
    fn seed_nominal_metadata_from_program_inner(&mut self, program: &IrProgram, skip_ambiguous: bool) {
        for (owner, source_name, identity) in &program.member_projections {
            if !skip_ambiguous || !self.ambiguous_type_names.contains(owner) {
                self.register_member_projection(owner, source_name, identity.clone());
            }
        }
        let source_dependency_module_path = if skip_ambiguous {
            program
                .source_module_name
                .as_deref()
                .map(|name| name.split('.').map(str::to_string).collect::<Vec<_>>())
        } else {
            None
        };
        for decl in &program.declarations {
            match &decl.kind {
                IrDeclKind::Struct(s) => {
                    if let Some(module_path) = source_dependency_module_path.as_ref() {
                        self.source_dependency_constructor_metadata.insert(
                            (module_path.clone(), s.name.clone()),
                            StructConstructorMetadata::from_source_dependency(module_path, s),
                        );
                    }
                    // Constructor metadata is keyed by the short source type name and must therefore ignore ambiguous
                    // dependency declarations. A newtype carrier relationship is different: it is keyed by the exact
                    // raw Rust type, and `method_signature_for_receiver` refuses a non-unique result. Preserve those
                    // ownership facts even when the source wrapper name itself is ambiguous.
                    if s.kind == IrStructKind::Newtype
                        && let Some(IrType::Struct(backing_type) | IrType::NamedGeneric(backing_type, _)) =
                            s.fields.first().map(|field| &field.ty)
                    {
                        self.newtype_backing_type_names
                            .entry(backing_type.clone())
                            .or_default()
                            .insert(s.name.clone());
                    }
                    if skip_ambiguous && self.ambiguous_type_names.contains(&s.name) {
                        continue;
                    }
                    self.register_struct_constructor_metadata(s);
                    if !s.derives.is_empty() {
                        self.struct_derives.insert(s.name.clone(), s.derives.clone());
                    }
                    self.struct_field_names
                        .insert(s.name.clone(), s.fields.iter().map(|f| f.name.clone()).collect());
                    for field in &s.fields {
                        let key = (s.name.clone(), field.name.clone());
                        self.struct_field_types.insert(key.clone(), field.ty.clone());
                        self.struct_field_surface_type_names
                            .insert(key.clone(), field.surface_type_name.clone());
                        self.struct_field_aliases.insert(key.clone(), field.alias.clone());
                        self.struct_field_descriptions
                            .insert(key.clone(), field.description.clone());
                        if let Some(default) = &field.default {
                            self.struct_field_defaults.insert(key, default.clone());
                        }
                    }
                }
                IrDeclKind::Enum(e) => {
                    if skip_ambiguous && self.ambiguous_type_names.contains(&e.name) {
                        continue;
                    }
                    for v in &e.variants {
                        self.enum_variant_fields
                            .insert((e.name.clone(), v.name.clone()), v.fields.clone());
                    }
                    for alias in &e.variant_aliases {
                        self.enum_variant_aliases
                            .insert((e.name.clone(), alias.name.clone()), alias.target.clone());
                    }
                }
                IrDeclKind::TypeAlias {
                    name,
                    type_params,
                    ty,
                    is_rusttype,
                    ..
                } => {
                    if skip_ambiguous && self.ambiguous_type_names.contains(name) {
                        continue;
                    }
                    if type_params.is_empty() && !is_rusttype {
                        self.type_aliases.insert(name.clone(), ty.clone());
                    }
                    if *is_rusttype {
                        self.rusttype_alias_names.insert(name.clone());
                    }
                }
                IrDeclKind::Impl(i) => {
                    for method in &i.methods {
                        let params = method.params.iter().filter(|param| !param.is_self).cloned().collect();
                        let key = (i.target_type.clone(), method.name.clone());
                        self.method_signatures.insert(
                            key.clone(),
                            FunctionSignature {
                                params,
                                return_type: method.return_type.clone(),
                            },
                        );
                        self.method_signature_type_params
                            .insert(key, i.type_params.iter().map(|param| param.name.clone()).collect());
                    }
                }
                _ => {}
            }
        }
    }

    /// Register one struct's constructor metadata unless an equivalent field layout is already known.
    fn register_struct_constructor_metadata(&mut self, s: &IrStruct) {
        let metadata = StructConstructorMetadata::from_struct(s);
        self.register_constructor_metadata_variant(&s.name, metadata);
    }

    /// Register one constructor metadata variant under a source-visible binding.
    fn register_constructor_metadata_variant(&mut self, name: &str, metadata: StructConstructorMetadata) {
        let variants = self.struct_constructor_metadata.entry(name.to_string()).or_default();
        if !variants.iter().any(|existing| {
            existing.provider_identity == metadata.provider_identity
                && existing.fields == metadata.fields
                && existing.field_types == metadata.field_types
                && existing.default_fields == metadata.default_fields
                && existing.field_aliases == metadata.field_aliases
                && existing.type_private_fields == metadata.type_private_fields
                && existing.constructor_surface == metadata.constructor_surface
        }) {
            variants.push(metadata);
        }
    }

    /// Register constructor metadata reconstructed from a public dependency manifest.
    fn register_manifest_constructor_metadata(
        &mut self,
        library: &str,
        name: &str,
        kind: IrStructKind,
        fields: &[FieldExport],
    ) {
        let metadata = StructConstructorMetadata::from_manifest_fields(library, kind, fields);
        self.register_constructor_metadata_variant(name, metadata);
        self.struct_field_names.insert(
            name.to_string(),
            fields.iter().map(|field| field.name.clone()).collect(),
        );
        for field in fields {
            let key = (name.to_string(), field.name.clone());
            self.struct_field_types
                .insert(key.clone(), Self::manifest_type_ref_to_ir_type(&field.ty));
            self.struct_field_surface_type_names
                .insert(key.clone(), field.surface_type_name.clone());
            self.struct_field_aliases.insert(key.clone(), field.alias.clone());
            self.struct_field_descriptions.insert(key, field.description.clone());
        }
    }

    /// Convert manifest-safe field defaults into the IR required by artifact-backed constructors.
    fn manifest_default_to_ir_expr(library: &str, default: &ParamDefaultExport) -> Option<TypedExpr> {
        match default {
            ParamDefaultExport::Int(value) => Some(TypedExpr::new(IrExprKind::Int(*value), IrType::Int)),
            ParamDefaultExport::Float(value) => value
                .parse::<f64>()
                .ok()
                .map(|value| TypedExpr::new(IrExprKind::Float(value), IrType::Float)),
            ParamDefaultExport::Bool(value) => Some(TypedExpr::new(IrExprKind::Bool(*value), IrType::Bool)),
            ParamDefaultExport::String(value) => Some(TypedExpr::new(
                IrExprKind::Literal(IrLiteral::StaticStr(value.clone())),
                IrType::StaticStr,
            )),
            ParamDefaultExport::Bytes(value) => Some(TypedExpr::new(IrExprKind::Bytes(value.clone()), IrType::Bytes)),
            ParamDefaultExport::None => Some(TypedExpr::new(IrExprKind::None, IrType::Unit)),
            ParamDefaultExport::List(values) => Some(TypedExpr::new(
                IrExprKind::List(
                    values
                        .iter()
                        .map(|value| Self::manifest_default_to_ir_expr(library, value).map(IrListEntry::Element))
                        .collect::<Option<Vec<_>>>()?,
                ),
                IrType::List(Box::new(IrType::Unknown)),
            )),
            ParamDefaultExport::Dict(entries) => Some(TypedExpr::new(
                IrExprKind::Dict(
                    entries
                        .iter()
                        .map(|entry| {
                            Some(IrDictEntry::Pair(
                                Self::manifest_default_to_ir_expr(library, &entry.key)?,
                                Box::new(Self::manifest_default_to_ir_expr(library, &entry.value)?),
                            ))
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                IrType::Dict(Box::new(IrType::Unknown), Box::new(IrType::Unknown)),
            )),
            ParamDefaultExport::ConstRef(path) => Self::manifest_default_const_ref_to_ir_expr(library, path),
            ParamDefaultExport::Call { path, args, signature } => {
                let function_name = path.last()?.clone();
                let args = args
                    .iter()
                    .map(|arg| {
                        Some(IrCallArg {
                            name: arg.name.clone(),
                            kind: if arg.name.is_some() {
                                IrCallArgKind::Named
                            } else {
                                IrCallArgKind::Positional
                            },
                            expr: Self::manifest_default_to_ir_expr(library, &arg.value)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                let callable_signature = signature
                    .as_ref()
                    .map(|signature| Self::manifest_default_call_signature_to_ir(library, signature));
                let return_type = callable_signature
                    .as_ref()
                    .map(|signature| signature.return_type.clone())
                    .unwrap_or(IrType::Unknown);
                let mut canonical_path = vec!["pub".to_string(), library.to_string()];
                canonical_path.extend(path.iter().cloned());
                Some(TypedExpr::new(
                    IrExprKind::Call {
                        func: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: function_name,
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Unknown,
                        )),
                        type_args: Vec::new(),
                        args,
                        callable_signature,
                        canonical_path: Some(canonical_path),
                    },
                    return_type,
                ))
            }
            ParamDefaultExport::Unsupported => None,
        }
    }

    /// Convert a compiled-library constant default into an exact dependency-qualified value path.
    fn manifest_default_const_ref_to_ir_expr(library: &str, path: &[String]) -> Option<TypedExpr> {
        if path.is_empty() {
            return None;
        }
        let mut expr = TypedExpr::new(
            IrExprKind::Var {
                name: library.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::ExternalName,
            },
            IrType::Unknown,
        );
        for segment in path {
            expr = TypedExpr::new(
                IrExprKind::Field {
                    object: Box::new(expr),
                    field: segment.clone(),
                },
                IrType::Unknown,
            );
        }
        Some(expr)
    }

    /// Rebuild the checked callable surface retained for a compiled-library field default helper.
    fn manifest_default_call_signature_to_ir(
        library: &str,
        signature: &ParamDefaultCallSignatureExport,
    ) -> FunctionSignature {
        FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let kind = match param.kind {
                        ParamKindExport::Normal => crate::frontend::ast::ParamKind::Normal,
                        ParamKindExport::RestPositional => crate::frontend::ast::ParamKind::RestPositional,
                        ParamKindExport::RestKeyword => crate::frontend::ast::ParamKind::RestKeyword,
                    };
                    let base_ty = Self::manifest_type_ref_to_ir_type(&param.ty);
                    let ty = match kind {
                        crate::frontend::ast::ParamKind::Normal => base_ty,
                        crate::frontend::ast::ParamKind::RestPositional => IrType::List(Box::new(base_ty)),
                        crate::frontend::ast::ParamKind::RestKeyword => {
                            IrType::Dict(Box::new(IrType::String), Box::new(base_ty))
                        }
                    };
                    FunctionParam {
                        name: param.name.clone(),
                        ty,
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind,
                        default: Self::manifest_param_default_to_ir_expr(library, param)
                            .map(crate::backend::ir::decl::FunctionParamDefault::source),
                    }
                })
                .collect(),
            return_type: Self::manifest_type_ref_to_ir_type(&signature.return_type),
        }
    }

    /// Lower one nested callable-parameter default retained inside a field-default call signature.
    fn manifest_param_default_to_ir_expr(library: &str, param: &ParamExport) -> Option<TypedExpr> {
        param
            .default
            .as_ref()
            .filter(|default| default.is_materializable())
            .and_then(|default| Self::manifest_default_to_ir_expr(library, default))
    }

    /// Convert a public manifest type reference into the IR vocabulary used by emission metadata.
    fn manifest_type_ref_to_ir_type(ty: &TypeRef) -> IrType {
        Self::resolved_type_to_ir_type(&resolved_type_from_manifest_type_ref(ty))
    }

    /// Convert resolved frontend metadata into IR type metadata without requiring an AST lowering context.
    fn resolved_type_to_ir_type(ty: &ResolvedType) -> IrType {
        match ty {
            ResolvedType::Never => IrType::Unknown,
            ResolvedType::Int => IrType::Int,
            ResolvedType::Float => IrType::Float,
            ResolvedType::Numeric(id) => IrType::Numeric(*id),
            ResolvedType::Bool => IrType::Bool,
            ResolvedType::Str => IrType::String,
            ResolvedType::Bytes => IrType::Bytes,
            ResolvedType::FrozenStr => IrType::FrozenStr,
            ResolvedType::FrozenBytes => IrType::FrozenBytes,
            ResolvedType::FrozenList(inner) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenList).to_string(),
                vec![Self::resolved_type_to_ir_type(inner)],
            ),
            ResolvedType::FrozenSet(inner) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenSet).to_string(),
                vec![Self::resolved_type_to_ir_type(inner)],
            ),
            ResolvedType::FrozenDict(key, value) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenDict).to_string(),
                vec![
                    Self::resolved_type_to_ir_type(key),
                    Self::resolved_type_to_ir_type(value),
                ],
            ),
            ResolvedType::Unit => IrType::Unit,
            ResolvedType::Named(name) => IrType::Struct(name.clone()),
            ResolvedType::Generic(name, args) => {
                let args = args.iter().map(Self::resolved_type_to_ir_type).collect::<Vec<_>>();
                match collections::from_str(name) {
                    Some(CollectionTypeId::List) => {
                        IrType::List(Box::new(args.first().cloned().unwrap_or(IrType::Unknown)))
                    }
                    Some(CollectionTypeId::Dict) => IrType::Dict(
                        Box::new(args.first().cloned().unwrap_or(IrType::Unknown)),
                        Box::new(args.get(1).cloned().unwrap_or(IrType::Unknown)),
                    ),
                    Some(CollectionTypeId::Set) => {
                        IrType::Set(Box::new(args.first().cloned().unwrap_or(IrType::Unknown)))
                    }
                    Some(CollectionTypeId::Option) => {
                        IrType::Option(Box::new(args.first().cloned().unwrap_or(IrType::Unknown)))
                    }
                    Some(CollectionTypeId::Result) => IrType::Result(
                        Box::new(args.first().cloned().unwrap_or(IrType::Unknown)),
                        Box::new(args.get(1).cloned().unwrap_or(IrType::Unknown)),
                    ),
                    Some(CollectionTypeId::Tuple) => IrType::Tuple(args),
                    Some(id) => IrType::NamedGeneric(collections::as_str(id).to_string(), args),
                    None if name == IR_UNION_TYPE_NAME => Self::canonical_manifest_union_type(args),
                    None if args.is_empty() => IrType::Struct(name.clone()),
                    None => IrType::NamedGeneric(name.clone(), args),
                }
            }
            ResolvedType::Function(params, ret) => IrType::Function {
                params: params
                    .iter()
                    .map(|param| Self::resolved_type_to_ir_type(&param.ty))
                    .collect(),
                ret: Box::new(Self::resolved_type_to_ir_type(ret)),
            },
            ResolvedType::TypeToken(inner) => IrType::TypeToken(Box::new(Self::resolved_type_to_ir_type(inner))),
            ResolvedType::Tuple(items) => IrType::Tuple(items.iter().map(Self::resolved_type_to_ir_type).collect()),
            ResolvedType::TypeVar(name) => IrType::Generic(name.clone()),
            ResolvedType::SelfType => IrType::SelfType,
            ResolvedType::Ref(inner) => IrType::Ref(Box::new(Self::resolved_type_to_ir_type(inner))),
            ResolvedType::RefMut(inner) => IrType::RefMut(Box::new(Self::resolved_type_to_ir_type(inner))),
            ResolvedType::RustPath(path) => IrType::Struct(path.clone()),
            ResolvedType::CallSiteInfer | ResolvedType::Unknown => IrType::Unknown,
        }
    }

    /// Reconstruct the canonical anonymous-union shape used by AST lowering.
    ///
    /// Artifact metadata stores the source `Union[...]` spelling. Its generated Rust wrapper name is stable only
    /// after flattening, ordering, and optional-`None` normalization match the lowering path exactly.
    fn canonical_manifest_union_type(members: Vec<IrType>) -> IrType {
        let mut has_none = false;
        let mut flattened = Vec::new();
        for member in members {
            match member {
                IrType::Unit => has_none = true,
                IrType::NamedGeneric(name, nested) if name == IR_UNION_TYPE_NAME => flattened.extend(nested),
                member => flattened.push(member),
            }
        }
        flattened.sort_by_key(IrType::rust_name);
        flattened.dedup();
        if has_none {
            return match flattened.as_slice() {
                [] => IrType::Unit,
                [only] => IrType::Option(Box::new(only.clone())),
                _ => IrType::Option(Box::new(IrType::NamedGeneric(
                    IR_UNION_TYPE_NAME.to_string(),
                    flattened,
                ))),
            };
        }
        match flattened.as_slice() {
            [] => IrType::Unknown,
            [only] => only.clone(),
            _ => IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), flattened),
        }
    }

    /// Select the constructor metadata variant matching the named fields in one constructor expression.
    pub(super) fn struct_constructor_metadata_for_fields(
        &self,
        name: &str,
        fields: &[(String, TypedExpr)],
    ) -> Option<&StructConstructorMetadata> {
        let variants = self.struct_constructor_metadata.get(name)?;
        if variants.len() == 1 {
            return variants.first();
        }

        let provided = fields
            .iter()
            .filter_map(|(field, _)| (!field.is_empty()).then_some(field.as_str()))
            .collect::<HashSet<_>>();
        let candidates = variants
            .iter()
            .filter(|metadata| metadata.supports_named_fields(&provided))
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return candidates.first().copied();
        }

        let constructible = candidates
            .iter()
            .copied()
            .filter(|metadata| metadata.constructible_from(&provided))
            .collect::<Vec<_>>();
        if constructible.len() == 1 {
            return constructible.first().copied();
        }

        if let Some(current_fields) = self.struct_field_names.get(name)
            && let Some(metadata) = variants.iter().find(|metadata| &metadata.fields == current_fields)
        {
            return Some(metadata);
        }
        candidates.first().copied().or_else(|| variants.first())
    }

    /// Select constructor metadata by exact `pub::<dependency>` identity carried on a lowered canonical call path.
    pub(super) fn public_dependency_constructor_metadata_for_path(
        &self,
        path: &[String],
        fields: &[(String, TypedExpr)],
    ) -> Option<&StructConstructorMetadata> {
        if path.first().map(String::as_str) != Some("pub") {
            return None;
        }
        let dependency = path.get(1)?;
        let public_path = path.get(2..)?.to_vec();
        let metadata = self
            .pub_dependency_constructor_metadata
            .get(&(dependency.clone(), public_path))?;
        let provided = fields
            .iter()
            .filter_map(|(field, _)| (!field.is_empty()).then_some(field.as_str()))
            .collect::<HashSet<_>>();
        (metadata.supports_named_fields(&provided) && metadata.constructible_from(&provided)).then_some(metadata)
    }

    /// Return an Incan-owned method signature for a receiver type when typechecker call-site metadata is unavailable.
    ///
    /// A qualified nominal name is already an identity, so it must match the registry exactly. The one deliberate
    /// bridge is an exact Rust backing type recorded for a source newtype, whose source API supplies ownership facts
    /// while implementing that newtype. Falling back to a final segment would instead let unrelated Rust and Incan
    /// types collide: for example, an external `bevy::prelude::App` would acquire defaults from Incan `App`.
    pub(super) fn method_signature_for_receiver(
        &self,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<&FunctionSignature> {
        match receiver_ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                let (is_rust_import_alias, canonical_import_aliases) = {
                    let rust_import_paths = self.rust_import_paths.borrow();
                    let canonical_name = name.trim_start_matches("::");
                    (
                        rust_import_paths.contains_key(name),
                        rust_import_paths
                            .iter()
                            .filter(|(_, path)| path.join("::") == canonical_name)
                            .map(|(alias, _)| alias.clone())
                            .collect::<Vec<_>>(),
                    )
                };
                if !is_rust_import_alias
                    && let Some(signature) = self.method_signatures.get(&(name.clone(), method_name.to_string()))
                {
                    return Some(signature);
                }
                if name.contains("::") || is_rust_import_alias {
                    // A source newtype records its carrier using the local Rust-import alias, while method lowering
                    // can retain either that alias or its canonical Rust path. Relate only those two representations
                    // of the same import; a normal `rust::` alias must never acquire a source method with the same
                    // short name.
                    let mut backing_type_names = vec![name.clone()];
                    backing_type_names.extend(canonical_import_aliases);
                    let backing_newtypes = backing_type_names
                        .iter()
                        .filter_map(|backing_type| self.newtype_backing_type_names.get(backing_type))
                        .flat_map(|newtypes| newtypes.iter())
                        .collect::<HashSet<_>>();
                    let mut signatures = backing_newtypes.iter().filter_map(|newtype_name| {
                        self.method_signatures
                            .get(&((*newtype_name).clone(), method_name.to_string()))
                    });
                    let signature = signatures.next()?;
                    return signatures.next().is_none().then_some(signature);
                }
                name.rsplit("::").next().and_then(|short_name| {
                    self.method_signatures
                        .get(&(short_name.to_string(), method_name.to_string()))
                })
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => self.method_signature_for_receiver(inner, method_name),
            _ => None,
        }
    }

    /// Return the call-site signature applicable to a method receiver.
    ///
    /// Semantic call-site metadata can retain source defaults before backend lowering has established that a short
    /// nominal name is a direct Rust import. Rust methods cannot have source-language defaults, so retain their type
    /// shapes but clear defaults for those receivers. A source newtype carrier is the deliberate exception: its
    /// imported Rust alias implements a source-owned API and therefore still owns its declared defaults.
    pub(super) fn method_call_signature_for_receiver(
        &self,
        receiver_ty: &IrType,
        call_signature: Option<&FunctionSignature>,
    ) -> Option<FunctionSignature> {
        let mut call_signature = call_signature?.clone();
        let direct_rust_alias_without_newtype = match receiver_ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                let short_name = name.rsplit("::").next().unwrap_or(name);
                let rust_import_paths = self.rust_import_paths.borrow();
                let matching_import_aliases = rust_import_paths
                    .keys()
                    .filter(|alias| *alias == name || *alias == short_name)
                    .collect::<Vec<_>>();
                !matching_import_aliases.is_empty()
                    && !matching_import_aliases
                        .iter()
                        .any(|alias| self.newtype_backing_type_names.contains_key(*alias))
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => {
                return self.method_call_signature_for_receiver(inner, Some(&call_signature));
            }
            _ => false,
        };
        if direct_rust_alias_without_newtype {
            for param in &mut call_signature.params {
                param.default = None;
            }
        }
        Some(call_signature)
    }

    /// Return a method signature specialized through a concrete generic receiver target.
    ///
    /// Associated constructors such as `OrderedDict.from_items(...)` can be checked from the assignment target
    /// (`OrderedDict[String, Int]`) even when the callee expression itself still carries generic impl parameters
    /// (`K`, `V`). Specializing the raw impl signature lets aggregate literal emission materialize owned element
    /// shapes before Rust typechecking sees the generated call.
    pub(super) fn specialized_method_signature_for_receiver(
        &self,
        receiver_ty: &IrType,
        method_name: &str,
    ) -> Option<FunctionSignature> {
        let IrType::NamedGeneric(type_name, args) = receiver_ty else {
            return None;
        };
        let (signature_key, signature) = self
            .method_signatures
            .get_key_value(&(type_name.clone(), method_name.to_string()))
            .or_else(|| {
                type_name.rsplit("::").next().and_then(|short_name| {
                    self.method_signatures
                        .get_key_value(&(short_name.to_string(), method_name.to_string()))
                })
            })?;
        let type_params = self.method_signature_type_params.get(signature_key)?;
        if type_params.len() != args.len() {
            return None;
        }
        let subst: HashMap<&str, &IrType> = type_params
            .iter()
            .map(String::as_str)
            .zip(args.iter())
            .chain(std::iter::once(("Self", receiver_ty)))
            .collect();
        Some(FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let mut param = param.clone();
                    param.ty = Self::substitute_signature_type(&param.ty, &subst);
                    param
                })
                .collect(),
            return_type: Self::substitute_signature_type(&signature.return_type, &subst),
        })
    }

    /// Substitute generic placeholders inside a method signature type.
    fn substitute_signature_type(ty: &IrType, subst: &HashMap<&str, &IrType>) -> IrType {
        match ty {
            IrType::Generic(name) => subst.get(name.as_str()).copied().cloned().unwrap_or_else(|| ty.clone()),
            IrType::SelfType => subst.get("Self").copied().cloned().unwrap_or_else(|| ty.clone()),
            IrType::Struct(name) if Self::is_signature_placeholder_name(name) => {
                subst.get(name.as_str()).copied().cloned().unwrap_or_else(|| ty.clone())
            }
            IrType::List(inner) => IrType::List(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Dict(key, value) => IrType::Dict(
                Box::new(Self::substitute_signature_type(key, subst)),
                Box::new(Self::substitute_signature_type(value, subst)),
            ),
            IrType::Set(inner) => IrType::Set(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|item| Self::substitute_signature_type(item, subst))
                    .collect(),
            ),
            IrType::Option(inner) => IrType::Option(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::Result(ok, err) => IrType::Result(
                Box::new(Self::substitute_signature_type(ok, subst)),
                Box::new(Self::substitute_signature_type(err, subst)),
            ),
            IrType::NamedGeneric(name, args) => IrType::NamedGeneric(
                name.clone(),
                args.iter()
                    .map(|arg| Self::substitute_signature_type(arg, subst))
                    .collect(),
            ),
            IrType::Function { params, ret } => IrType::Function {
                params: params
                    .iter()
                    .map(|param| Self::substitute_signature_type(param, subst))
                    .collect(),
                ret: Box::new(Self::substitute_signature_type(ret, subst)),
            },
            IrType::TypeToken(inner) => IrType::TypeToken(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::ExternalUnion { library, union } => IrType::ExternalUnion {
                library: library.clone(),
                union: Box::new(Self::substitute_signature_type(union, subst)),
            },
            IrType::Ref(inner) => IrType::Ref(Box::new(Self::substitute_signature_type(inner, subst))),
            IrType::RefMut(inner) => IrType::RefMut(Box::new(Self::substitute_signature_type(inner, subst))),
            _ => ty.clone(),
        }
    }

    /// Specialize a generic signature by matching its return type against an expected result type.
    ///
    /// This covers associated constructors such as `OrdinalMap.from_keys(...) ?`: the callable signature still talks in
    /// terms of `Self`/`K`, while the surrounding assignment tells us the concrete `Result[OrdinalMap[str], E]` shape.
    pub(super) fn specialize_signature_by_result_target(
        signature: &FunctionSignature,
        target_ty: &IrType,
    ) -> Option<FunctionSignature> {
        let mut owned_subst = HashMap::<String, IrType>::new();
        if !Self::collect_result_target_substitutions(&signature.return_type, target_ty, &mut owned_subst)
            || owned_subst.is_empty()
        {
            return None;
        }
        let subst: HashMap<&str, &IrType> = owned_subst.iter().map(|(name, ty)| (name.as_str(), ty)).collect();
        Some(FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let mut param = param.clone();
                    param.ty = Self::substitute_signature_type(&param.ty, &subst);
                    param
                })
                .collect(),
            return_type: Self::substitute_signature_type(&signature.return_type, &subst),
        })
    }

    /// Collect generic substitutions by matching a signature return type against a concrete target.
    fn collect_result_target_substitutions(
        pattern: &IrType,
        actual: &IrType,
        subst: &mut HashMap<String, IrType>,
    ) -> bool {
        match (pattern, actual) {
            (IrType::Generic(name), actual) => Self::insert_result_target_substitution(name, actual, subst),
            (IrType::SelfType, actual) => Self::insert_result_target_substitution("Self", actual, subst),
            (IrType::Struct(name), actual) if Self::is_signature_placeholder_name(name) => {
                Self::insert_result_target_substitution(name, actual, subst)
            }
            (IrType::List(pattern), IrType::List(actual))
            | (IrType::Set(pattern), IrType::Set(actual))
            | (IrType::Option(pattern), IrType::Option(actual))
            | (IrType::Ref(pattern), IrType::Ref(actual))
            | (IrType::RefMut(pattern), IrType::RefMut(actual)) => {
                Self::collect_result_target_substitutions(pattern, actual, subst)
            }
            (IrType::Result(pattern_ok, pattern_err), IrType::Result(actual_ok, actual_err)) => {
                Self::collect_result_target_substitutions(pattern_ok, actual_ok, subst)
                    && Self::collect_result_target_substitutions(pattern_err, actual_err, subst)
            }
            (IrType::Dict(pattern_key, pattern_value), IrType::Dict(actual_key, actual_value)) => {
                Self::collect_result_target_substitutions(pattern_key, actual_key, subst)
                    && Self::collect_result_target_substitutions(pattern_value, actual_value, subst)
            }
            (IrType::Tuple(pattern_items), IrType::Tuple(actual_items))
                if pattern_items.len() == actual_items.len() =>
            {
                pattern_items
                    .iter()
                    .zip(actual_items.iter())
                    .all(|(pattern, actual)| Self::collect_result_target_substitutions(pattern, actual, subst))
            }
            (IrType::NamedGeneric(pattern_name, pattern_args), IrType::NamedGeneric(actual_name, actual_args))
                if pattern_name == actual_name && pattern_args.len() == actual_args.len() =>
            {
                pattern_args
                    .iter()
                    .zip(actual_args.iter())
                    .all(|(pattern, actual)| Self::collect_result_target_substitutions(pattern, actual, subst))
            }
            _ => pattern == actual,
        }
    }

    /// Insert one return-target substitution, rejecting conflicting generic bindings.
    fn insert_result_target_substitution(name: &str, actual: &IrType, subst: &mut HashMap<String, IrType>) -> bool {
        if let Some(existing) = subst.get(name) {
            existing == actual
        } else {
            subst.insert(name.to_string(), actual.clone());
            true
        }
    }

    /// Best-effort specialization for call-site signatures that still expose receiver generics.
    pub(super) fn specialize_signature_by_receiver_args(
        signature: &FunctionSignature,
        receiver_ty: &IrType,
    ) -> Option<FunctionSignature> {
        let IrType::NamedGeneric(_, args) = receiver_ty else {
            return None;
        };
        let mut generic_names = Vec::new();
        for param in &signature.params {
            Self::collect_signature_generics(&param.ty, &mut generic_names);
        }
        if generic_names.is_empty() || generic_names.len() > args.len() {
            return None;
        }
        let subst: HashMap<&str, &IrType> = generic_names.iter().map(String::as_str).zip(args.iter()).collect();
        Some(FunctionSignature {
            params: signature
                .params
                .iter()
                .map(|param| {
                    let mut param = param.clone();
                    param.ty = Self::substitute_signature_type(&param.ty, &subst);
                    param
                })
                .collect(),
            return_type: Self::substitute_signature_type(&signature.return_type, &subst),
        })
    }

    /// Collect generic placeholder names from a signature type in first-use order.
    fn collect_signature_generics(ty: &IrType, out: &mut Vec<String>) {
        match ty {
            IrType::Generic(name) if !out.contains(name) => out.push(name.clone()),
            IrType::Struct(name) if Self::is_signature_placeholder_name(name) && !out.contains(name) => {
                out.push(name.clone());
            }
            IrType::List(inner)
            | IrType::Set(inner)
            | IrType::Option(inner)
            | IrType::Ref(inner)
            | IrType::RefMut(inner)
            | IrType::TypeToken(inner) => {
                Self::collect_signature_generics(inner, out);
            }
            IrType::Dict(key, value) | IrType::Result(key, value) => {
                Self::collect_signature_generics(key, out);
                Self::collect_signature_generics(value, out);
            }
            IrType::Tuple(items) | IrType::NamedGeneric(_, items) => {
                for item in items {
                    Self::collect_signature_generics(item, out);
                }
            }
            IrType::ExternalUnion { union, .. } => Self::collect_signature_generics(union, out),
            IrType::Function { params, ret } => {
                for param in params {
                    Self::collect_signature_generics(param, out);
                }
                Self::collect_signature_generics(ret, out);
            }
            _ => {}
        }
    }

    /// Return whether a struct-shaped name is really a lowered generic placeholder.
    fn is_signature_placeholder_name(name: &str) -> bool {
        !name.is_empty() && name.len() <= 2 && name.chars().all(|ch| ch.is_ascii_uppercase())
    }

    /// True if `ty` is a user-defined Incan enum in IR, including imported enums.
    ///
    /// Named enums lower to [`IrType::Struct`] (see `lower_resolved_type`); [`IrType::Enum`] is also treated as enum.
    /// Imported enums are tracked separately because consumer modules only carry the short nominal type name after
    /// typechecking/lowering. Used by for-loop emission to iterate with `.iter().cloned()` so the loop variable is an
    /// owned `E`, matching the typechecker and `PartialEq` for both local and cross-module enum loops (#195, #372).
    pub(super) fn type_is_user_enum(&self, ty: &IrType) -> bool {
        match ty {
            IrType::Enum(_) => true,
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                self.enum_variant_fields.keys().any(|(enum_name, _)| enum_name == name)
                    || self.dependency_enum_types.contains(name)
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConstructorProviderIdentity, FunctionSignature, IrEmitter, StructConstructorMetadata, StructConstructorSurface,
    };
    use crate::backend::ir::decl::{
        IrDecl, IrDeclKind, IrImportItem, IrImportOrigin, IrImportQualifier, IrStaticProvenance, IrStruct,
        IrStructKind, StructField, Visibility,
    };
    use crate::backend::ir::expr::{IrExprKind, IrStaticReferenceKind, TypedExpr};
    use crate::backend::ir::stmt::{IrStmt, IrStmtKind};
    use crate::backend::ir::{FunctionRegistry, IrProgram, IrType};
    use crate::library_manifest::{
        CanonicalIdentityExport, ExportIdentity, ExportIdentityKind, ExportIdentityProjection, LibraryManifest,
    };
    use incan_semantics_core::{
        CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    };

    #[test]
    fn compiled_sdk_manifest_seeds_exact_stdlib_function_identity() {
        let registry = FunctionRegistry::new();
        let identity = CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: "incan_stdlib_core".to_string(),
                module_path: vec!["result".to_string()],
            },
            declaration_name: "map".to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(10, 20),
        };
        let mut manifest = LibraryManifest::new("incan_stdlib_core", "0.6.0");
        manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
            public_name: "map".to_string(),
            public_path: vec!["incan_stdlib_core".to_string(), "result_map".to_string()],
            source_path: vec!["result".to_string(), "map".to_string()],
            kind: ExportIdentityKind::Function,
            projection: ExportIdentityProjection::Direct,
            canonical: CanonicalIdentityExport::from_canonical("incan_stdlib_core", &identity),
        });

        let std_path = ["std".to_string(), "result".to_string(), "map".to_string()];
        let source_identity = CanonicalSymbolId::module_declaration(
            vec!["std".to_string(), "result".to_string()],
            "map",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(30, 40),
        );
        let mut canonical_registry = FunctionRegistry::new();
        canonical_registry.register_canonical_path_projection(
            &std_path,
            "map".to_string(),
            source_identity,
            Vec::new(),
            IrType::Unit,
        );
        let mut emitter = IrEmitter::new(&registry);
        emitter.set_canonical_function_registry(canonical_registry);

        emitter.seed_sdk_provider_manifest_metadata(&manifest);

        assert_eq!(emitter.canonical_stdlib_function_identity(&std_path), Some(&identity));
    }

    #[test]
    fn callback_reference_matches_imported_source_parameter_surface() {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);

        assert!(emitter.call_signature_type_matches(
            &IrType::RustDisplay("&mut egui::Ui".to_string()),
            &IrType::Struct("Ui".to_string()),
        ));
        assert!(emitter.call_signature_type_matches(
            &IrType::Struct("egui::Ui".to_string()),
            &IrType::Struct("Ui".to_string()),
        ));
        assert!(!emitter.call_signature_type_matches(
            &IrType::RustDisplay("&mut egui::Ui".to_string()),
            &IrType::Struct("Frame".to_string()),
        ));
    }

    #[test]
    fn qualified_rust_receiver_does_not_inherit_same_named_incan_method_signature() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter.method_signatures.insert(
            ("App".to_string(), "run".to_string()),
            FunctionSignature {
                params: Vec::new(),
                return_type: IrType::Unit,
            },
        );
        emitter
            .newtype_backing_type_names
            .entry("RustJsonValue".to_string())
            .or_default()
            .insert("JsonValue".to_string());
        emitter.rust_import_paths.borrow_mut().insert(
            "RustJsonValue".to_string(),
            vec!["incan_stdlib".to_string(), "json".to_string(), "JsonValue".to_string()],
        );
        emitter.method_signatures.insert(
            ("JsonValue".to_string(), "string".to_string()),
            FunctionSignature {
                params: vec![crate::backend::ir::decl::FunctionParam {
                    name: "value".to_string(),
                    ty: IrType::String,
                    mutability: crate::backend::ir::Mutability::Immutable,
                    is_self: false,
                    kind: crate::frontend::ast::ParamKind::Normal,
                    default: None,
                }],
                return_type: IrType::Struct("JsonValue".to_string()),
            },
        );

        assert!(
            emitter
                .method_signature_for_receiver(&IrType::Struct("App".to_string()), "run")
                .is_some(),
            "an Incan nominal receiver uses its own method metadata"
        );
        emitter.rust_import_paths.borrow_mut().insert(
            "App".to_string(),
            vec!["bevy".to_string(), "prelude".to_string(), "App".to_string()],
        );
        assert!(
            emitter
                .method_signature_for_receiver(&IrType::Struct("bevy::prelude::App".to_string()), "run")
                .is_none(),
            "qualified Rust receivers must not use an unrelated short-name Incan signature"
        );
        assert!(
            emitter
                .method_signature_for_receiver(&IrType::Struct("App".to_string()), "run")
                .is_none(),
            "a local Rust import alias must not use an unrelated source method signature"
        );
        assert!(
            emitter
                .method_signature_for_receiver(&IrType::Struct("incan_stdlib::json::JsonValue".to_string()), "string",)
                .is_some(),
            "a source newtype may supply call ownership facts through its exact Rust import identity"
        );
        assert!(
            emitter
                .method_signature_for_receiver(&IrType::Struct("RustJsonValue".to_string()), "string")
                .is_some(),
            "a source newtype may supply call ownership facts through its local Rust import alias"
        );

        let signature_with_source_default = FunctionSignature {
            params: vec![crate::backend::ir::decl::FunctionParam {
                name: "port".to_string(),
                ty: IrType::Int,
                mutability: crate::backend::ir::Mutability::Immutable,
                is_self: false,
                kind: crate::frontend::ast::ParamKind::Normal,
                default: Some(crate::backend::ir::decl::FunctionParamDefault::source(TypedExpr::new(
                    IrExprKind::Int(8080),
                    IrType::Int,
                ))),
            }],
            return_type: IrType::Unit,
        };
        let direct_alias_signature = emitter
            .method_call_signature_for_receiver(
                &IrType::Struct("App".to_string()),
                Some(&signature_with_source_default),
            )
            .ok_or("a direct Rust import alias receiver should produce a call signature")?;
        assert!(
            direct_alias_signature.params[0].default.is_none(),
            "a direct Rust import alias must not retain source-language defaults"
        );
        let qualified_chain_signature = emitter
            .method_call_signature_for_receiver(
                &IrType::Struct("std::web::App".to_string()),
                Some(&signature_with_source_default),
            )
            .ok_or("a source-qualified call-chain receiver should produce a call signature")?;
        assert!(
            qualified_chain_signature.params[0].default.is_none(),
            "a call-chain receiver retaining a source-qualified spelling must still respect its Rust import alias"
        );
        let newtype_carrier_signature = emitter
            .method_call_signature_for_receiver(
                &IrType::Struct("RustJsonValue".to_string()),
                Some(&signature_with_source_default),
            )
            .ok_or("a source newtype carrier receiver should produce a call signature")?;
        assert!(
            newtype_carrier_signature.params[0].default.is_some(),
            "a source newtype carrier retains its source-owned defaults"
        );
        Ok(())
    }

    fn checked_source_class(
        private_ty: IrType,
        private_default: TypedExpr,
        public_ty: IrType,
        trailing_ty: IrType,
        trailing_default: TypedExpr,
    ) -> IrStruct {
        IrStruct {
            kind: IrStructKind::Class,
            name: "Vault".to_string(),
            docstring: None,
            fields: vec![
                StructField {
                    name: "secret".to_string(),
                    ty: private_ty,
                    surface_type_name: None,
                    visibility: Visibility::Private,
                    is_type_private: true,
                    default: Some(private_default),
                    alias: None,
                    description: None,
                },
                StructField {
                    name: "label".to_string(),
                    ty: public_ty,
                    surface_type_name: None,
                    visibility: Visibility::Public,
                    is_type_private: false,
                    default: None,
                    alias: None,
                    description: None,
                },
                StructField {
                    name: "revision".to_string(),
                    ty: trailing_ty,
                    surface_type_name: None,
                    visibility: Visibility::Private,
                    is_type_private: true,
                    default: Some(trailing_default),
                    alias: None,
                    description: None,
                },
            ],
            derives: Vec::new(),
            visibility: Visibility::Public,
            type_params: Vec::new(),
            derive_rust_modules: Default::default(),
            lint_allows: Vec::new(),
        }
    }

    fn checked_source_model() -> IrStruct {
        let mut model = checked_source_class(
            IrType::String,
            TypedExpr::new(IrExprKind::String("sealed".to_string()), IrType::String),
            IrType::String,
            IrType::Int,
            TypedExpr::new(IrExprKind::Int(1), IrType::Int),
        );
        model.kind = IrStructKind::Model;
        model
    }

    fn source_import(path: &[&str], item_name: &str, alias: Option<&str>, visibility: Visibility) -> IrDecl {
        IrDecl::new(IrDeclKind::Import {
            visibility,
            origin: IrImportOrigin::Standard,
            qualifier: IrImportQualifier::Auto,
            path: path.iter().map(|segment| (*segment).to_string()).collect(),
            alias: None,
            items: vec![IrImportItem {
                name: item_name.to_string(),
                alias: alias.map(str::to_string),
                canonical: None,
                is_static: false,
                force_reexport: false,
                rust_trait_import: None,
            }],
        })
    }

    #[test]
    fn rust_ident_uses_raw_idents_for_keywords() {
        let ident = IrEmitter::rust_ident("async");
        let rendered = quote::quote! { #ident }.to_string();
        assert_eq!(rendered, "r#async");
    }

    #[test]
    fn rust_generated_static_ident_uses_uppercase_global_style() {
        let registry = crate::backend::ir::FunctionRegistry::new();
        let _emitter = IrEmitter::new(&registry);
        let ident = IrEmitter::rust_generated_static_ident("_active_sessions");
        let rendered = quote::quote! { #ident }.to_string();
        assert_eq!(rendered, "_ACTIVE_SESSIONS");
    }

    fn static_identity(module: &str, declaration_name: &str) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec![module.to_string()]),
            declaration_name: declaration_name.to_string(),
            kind: SemanticSourceTargetKind::Static,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, declaration_name.len()),
        }
    }

    #[test]
    fn source_static_with_non_static_identity_fails_closed() -> Result<(), String> {
        let mut identity = static_identity("fixture", "counter");
        identity.kind = SemanticSourceTargetKind::Function;
        let mut program = IrProgram::new();
        program.declarations.push(IrDecl::new(IrDeclKind::Static {
            visibility: Visibility::Public,
            name: "counter".to_string(),
            provenance: IrStaticProvenance::Source(identity),
            ty: IrType::Int,
            value: TypedExpr::new(IrExprKind::Int(0), IrType::Int),
        }));
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let error = match emitter.emit_program_tokens(&program) {
            Ok(_) => return Err("static with a function identity unexpectedly emitted".to_string()),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("source static `counter` carries a non-static or non-Incan canonical identity"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn source_static_reference_without_projection_fails_closed() -> Result<(), String> {
        let mut program = IrProgram::new();
        program.module_init.push(IrStmt::new(IrStmtKind::Expr(TypedExpr::new(
            IrExprKind::StaticRead {
                name: "missing".to_string(),
                reference_kind: IrStaticReferenceKind::Source,
            },
            IrType::Int,
        ))));
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let error = match emitter.emit_program_tokens(&program) {
            Ok(_) => return Err("source static reference without a projection unexpectedly emitted".to_string()),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("source static reference `missing` has no compiler-owned canonical projection"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn colliding_static_import_aliases_fail_closed() -> Result<(), String> {
        let mut program = IrProgram::new();
        for module in ["left", "right"] {
            program.declarations.push(IrDecl::new(IrDeclKind::Import {
                visibility: Visibility::Private,
                origin: IrImportOrigin::Standard,
                qualifier: IrImportQualifier::Auto,
                path: vec![module.to_string()],
                alias: None,
                items: vec![IrImportItem {
                    name: "counter".to_string(),
                    alias: Some("shared".to_string()),
                    canonical: Some(static_identity(module, "counter")),
                    is_static: true,
                    force_reexport: false,
                    rust_trait_import: None,
                }],
            }));
        }
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let error = match emitter.emit_program_tokens(&program) {
            Ok(_) => return Err("two static identities unexpectedly shared one emitted binding".to_string()),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("source static binding `shared` resolves to two canonical projections"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    /// Confirm default-backed private fields retain an exact canonical provider bridge.
    fn private_default_source_models_expose_public_field_provider_bridges_issue884() {
        let model = checked_source_model();
        let metadata =
            StructConstructorMetadata::from_source_dependency(&["pkg".to_string(), "vaults".to_string()], &model);
        assert_eq!(metadata.constructor_surface, StructConstructorSurface::PublicBridge);
        assert_eq!(
            metadata.provider_identity,
            Some(ConstructorProviderIdentity::SourceModule(vec![
                "pkg".to_string(),
                "vaults".to_string()
            ]))
        );
    }

    #[test]
    /// Confirm required private fields leave no native constructor surface.
    fn constructor_metadata_omits_required_private_fields_from_native_surfaces() {
        let mut model = checked_source_model();
        model.fields[0].default = None;

        let metadata =
            StructConstructorMetadata::from_source_dependency(&["pkg".to_string(), "vaults".to_string()], &model);

        assert_eq!(metadata.constructor_surface, StructConstructorSurface::Absent);
    }

    #[test]
    /// Confirm all-public models continue to use direct Rust struct construction in consumers.
    fn constructor_metadata_keeps_all_public_models_on_struct_literal_surface() {
        let mut model = checked_source_model();
        for field in &mut model.fields {
            field.is_type_private = false;
            field.visibility = Visibility::Public;
        }

        let metadata =
            StructConstructorMetadata::from_source_dependency(&["pkg".to_string(), "vaults".to_string()], &model);

        assert_eq!(
            metadata.constructor_surface,
            StructConstructorSurface::DirectStructLiteral
        );
    }

    #[test]
    /// Confirm a provider-private nominal field type cannot leak through a constructor signature.
    fn constructor_metadata_hides_required_private_nominal_field_types() {
        let mut model = checked_source_model();
        model.fields[0].ty = IrType::Struct("PrivateToken".to_string());
        model.fields[0].default = None;

        let metadata =
            StructConstructorMetadata::from_source_dependency(&["pkg".to_string(), "vaults".to_string()], &model);

        assert_eq!(metadata.constructor_surface, StructConstructorSurface::Absent);
    }

    #[test]
    /// Confirm class constructor metadata preserves private inputs independently of later field-access privacy.
    fn constructor_metadata_keeps_complete_private_class_input_surface_issue886() {
        let mut class = checked_source_class(
            IrType::String,
            TypedExpr::new(IrExprKind::String("sealed".to_string()), IrType::String),
            IrType::String,
            IrType::Int,
            TypedExpr::new(IrExprKind::Int(1), IrType::Int),
        );
        class.fields[0].default = None;

        let metadata =
            StructConstructorMetadata::from_source_dependency(&["pkg".to_string(), "vaults".to_string()], &class);

        assert_eq!(metadata.constructor_surface, StructConstructorSurface::PublicAllFields);
        assert_eq!(
            metadata.constructor_fields().map(String::as_str).collect::<Vec<_>>(),
            ["secret", "label", "revision"]
        );
    }

    #[test]
    /// Confirm compiled-library metadata filters private defaults from the bridge argument order.
    fn manifest_constructor_metadata_exports_only_public_fields_for_private_defaults() {
        use crate::library_manifest::{FieldExport, FieldVisibilityExport, ParamDefaultExport, TypeRef};

        let fields = vec![
            FieldExport {
                name: "secret".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    name: "bool".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Private,
                has_default: true,
                default: Some(ParamDefaultExport::Bool(true)),
                alias: None,
                description: None,
            },
            FieldExport {
                name: "label".to_string(),
                canonical: None,
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                surface_type_name: None,
                visibility: FieldVisibilityExport::Public,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
        ];

        let bridge = StructConstructorMetadata::from_manifest_fields("sealed", IrStructKind::Model, &fields);
        assert_eq!(bridge.constructor_surface, StructConstructorSurface::PublicBridge);
        assert_eq!(
            bridge.constructor_fields().map(String::as_str).collect::<Vec<_>>(),
            ["label"]
        );

        let mut required_private_fields = fields;
        required_private_fields[0].has_default = false;
        required_private_fields[0].default = None;
        let absent =
            StructConstructorMetadata::from_manifest_fields("sealed", IrStructKind::Model, &required_private_fields);
        assert_eq!(absent.constructor_surface, StructConstructorSurface::Absent);

        let class =
            StructConstructorMetadata::from_manifest_fields("sealed", IrStructKind::Class, &required_private_fields);
        assert_eq!(class.constructor_surface, StructConstructorSurface::PublicAllFields);
        assert_eq!(
            class.constructor_fields().map(String::as_str).collect::<Vec<_>>(),
            ["secret", "label"]
        );
    }

    #[test]
    fn runtime_reflection_omits_type_private_fields_issue884() {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter
            .struct_field_names
            .insert("Vault".to_string(), vec!["secret".to_string(), "label".to_string()]);
        emitter
            .struct_type_private_fields
            .insert(("Vault".to_string(), "secret".to_string()));

        assert_eq!(
            emitter.public_reflection_field_names("Vault"),
            Some(vec!["label".to_string()])
        );
    }

    #[test]
    fn ordinary_source_imports_bind_private_constructor_bridges_by_exact_module_identity_issue886()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);

        let mut text_provider = IrProgram::new();
        text_provider.source_module_name = Some("pkg.text_vaults".to_string());
        text_provider
            .declarations
            .push(IrDecl::new(IrDeclKind::Struct(checked_source_class(
                IrType::String,
                TypedExpr::new(IrExprKind::String("provider-default".to_string()), IrType::String),
                IrType::String,
                IrType::Int,
                TypedExpr::new(IrExprKind::Int(7), IrType::Int),
            ))));
        let mut number_provider = IrProgram::new();
        number_provider.source_module_name = Some("pkg.number_vaults".to_string());
        number_provider
            .declarations
            .push(IrDecl::new(IrDeclKind::Struct(checked_source_class(
                IrType::Int,
                TypedExpr::new(IrExprKind::Int(7), IrType::Int),
                IrType::Int,
                IrType::String,
                TypedExpr::new(IrExprKind::String("provider-default".to_string()), IrType::String),
            ))));
        let mut facade = IrProgram::new();
        facade.source_module_name = Some("pkg.vault_facade".to_string());
        facade.declarations = vec![source_import(
            &["text_vaults"],
            "Vault",
            Some("FacadeVault"),
            Visibility::Public,
        )];
        let mut public_api = IrProgram::new();
        public_api.source_module_name = Some("pkg.public_api".to_string());
        public_api.declarations = vec![source_import(
            &["vault_facade"],
            "FacadeVault",
            Some("ExportedVault"),
            Visibility::Public,
        )];
        // Seed the two-hop public facade before its declaring provider to prove fixed-point propagation does not
        // depend on the order in which checked source dependencies reach the emitter.
        emitter.seed_dependency_nominal_metadata_from_program(&public_api);
        emitter.seed_dependency_nominal_metadata_from_program(&facade);
        emitter.seed_dependency_nominal_metadata_from_program(&number_provider);
        emitter.seed_dependency_nominal_metadata_from_program(&text_provider);

        let mut consumer = IrProgram::new();
        // Root-program emission has no logical source module name. It must still bind a bare source import to the
        // sole canonical provider, while rejecting duplicate provider suffixes elsewhere.
        consumer.declarations = vec![
            source_import(&["text_vaults"], "Vault", None, Visibility::Private),
            source_import(
                &["pkg", "number_vaults"],
                "Vault",
                Some("NumberVault"),
                Visibility::Private,
            ),
            source_import(
                &["public_api"],
                "ExportedVault",
                Some("ConsumerVault"),
                Visibility::Private,
            ),
        ];
        emitter.bind_source_dependency_constructor_metadata(&consumer);

        let Some(text) = emitter
            .struct_constructor_metadata
            .get("Vault")
            .and_then(|variants| variants.first())
        else {
            return Err(std::io::Error::other("direct import did not bind exact text provider metadata").into());
        };
        let Some(number) = emitter
            .struct_constructor_metadata
            .get("NumberVault")
            .and_then(|variants| variants.first())
        else {
            return Err(std::io::Error::other("aliased import did not bind exact number provider metadata").into());
        };
        let Some(facade) = emitter
            .struct_constructor_metadata
            .get("ConsumerVault")
            .and_then(|variants| variants.first())
        else {
            return Err(std::io::Error::other("multi-hop facade import did not bind provider metadata").into());
        };
        assert_eq!(
            text.provider_identity.as_ref(),
            Some(&ConstructorProviderIdentity::SourceModule(vec![
                "pkg".to_string(),
                "text_vaults".to_string()
            ]))
        );
        assert_eq!(
            number.provider_identity.as_ref(),
            Some(&ConstructorProviderIdentity::SourceModule(vec![
                "pkg".to_string(),
                "number_vaults".to_string()
            ]))
        );
        assert_eq!(facade.provider_identity, text.provider_identity);
        assert_eq!(facade.fields, text.fields);
        assert_eq!(facade.default_fields, text.default_fields);
        assert_eq!(facade.constructor_surface, StructConstructorSurface::PublicAllFields);
        assert_eq!(text.fields, ["secret", "label", "revision"]);
        assert_eq!(number.fields, ["secret", "label", "revision"]);
        assert_eq!(text.field_types.get("label"), Some(&IrType::String));
        assert_eq!(number.field_types.get("label"), Some(&IrType::Int));
        assert_eq!(
            text.default_fields,
            std::collections::HashSet::from(["secret".to_string(), "revision".to_string()])
        );
        assert_eq!(
            number.default_fields,
            std::collections::HashSet::from(["secret".to_string(), "revision".to_string()])
        );
        assert_eq!(text.constructor_surface, StructConstructorSurface::PublicAllFields);
        assert_eq!(number.constructor_surface, StructConstructorSurface::PublicAllFields);
        Ok(())
    }
}
