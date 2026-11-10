//! Registry artifacts and definitions, class layouts and field defaults, and newtype construction.

use super::*;

/// Checked RFC 113 registry data that later stages consume without re-parsing decorator expressions.
#[derive(Debug, Default, Clone)]
pub struct RegistryArtifacts {
    /// Registry definitions keyed by their module-static binding name.
    pub definitions: HashMap<String, RegistryDefinitionInfo>,
    /// Canonical registry definitions imported into this module, keyed by the local import name.
    ///
    /// These are dependency facts, not duplicate local definitions. A checked `@describe` may use one only after the
    /// importer resolves the public static binding to the defining module's `Registry.define(...)` contract.
    pub imported_definitions: HashMap<String, ImportedRegistryDefinitionInfo>,
    /// Declaration descriptions in source order.
    pub descriptions: Vec<RegistryDescriptionInfo>,
    /// Explicit compilation-unit and package entries in source order.
    pub explicit_entries: Vec<RegistryExplicitEntryInfo>,
    /// The standard `Registry` class and `RegistryEntry` type, read from the standard package owning `std.registry`
    /// when this module defines, imports or enters a registry. Lowering selects the registry's reserved methods here.
    pub standard: Option<StandardRegistryTypes>,
}

/// The standard package's registry declarations a module's registry facts are checked against.
#[derive(Debug, Clone)]
pub struct StandardRegistryTypes {
    /// The `Registry` class's identity.
    pub registry: CanonicalSymbolId,
    /// The `Registry` class, with its methods.
    pub registry_class: ClassInfo,
    /// The `RegistryEntry` type's identity.
    pub entry: CanonicalSymbolId,
}

/// Type and subject contract declared by one `Registry.define(...)` static.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryDefinitionInfo {
    pub key_type: ResolvedType,
    pub descriptor_type: ResolvedType,
    pub subjects: Vec<SemanticRegistrySubjectKind>,
    /// Whether dependency consumers may inspect this registry through a published package artifact.
    pub is_public: bool,
}

/// A public registry definition imported from the source module that owns its `Registry.define(...)` declaration.
///
/// This fact carries the defining contract across an import boundary without inserting a second definition into the
/// consumer's [`RegistryArtifacts::definitions`] map. `owner_module_path` and `owner_binding` are the canonical
/// registry identity used by package metadata; `local_name` is intentionally only the consumer's spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRegistryDefinitionInfo {
    /// Contract checked from the defining static declaration.
    pub definition: RegistryDefinitionInfo,
    /// Canonical source module that owns the registry static.
    pub owner_module_path: Vec<String>,
    /// Canonical static binding in the owning source module.
    pub owner_binding: String,
}

/// Provenance of the canonical registry binding selected for one checked description.
///
/// Runtime lowering still uses the source-local `registry_name` to access the imported static. Metadata and semantic
/// facts use this reference so an imported alias cannot invent a second registry identity in the consumer module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegistryDescriptionRegistry {
    /// The described registry is declared by a static in this module.
    Local {
        /// Module-local static binding that owns the `Registry.define(...)` call.
        binding: String,
    },
    /// The described registry is a public static imported from its defining source module.
    Imported {
        /// Canonical source module containing the defining static.
        module_path: Vec<String>,
        /// Static binding in the canonical source module.
        binding: String,
        /// Public visibility checked on the canonical definition.
        public: bool,
    },
}

/// One checked `@describe` declaration attachment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryDescriptionInfo {
    /// Source-local binding that lowering reads to register the runtime descriptor.
    pub registry_name: String,
    /// Canonical definition selected for this description.
    pub registry: RegistryDescriptionRegistry,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    pub declaration_name: String,
    pub declaration_span: crate::ast::Span,
    /// Exact decorator attachment that produced this fact.
    ///
    /// Backend lowering uses this to require the frontend-approved artifact for the concrete syntax it materializes;
    /// it must not treat a raw `@describe` expression as semantic authority.
    pub decorator_span: crate::ast::Span,
    /// Source range of the checked key materialization expression.
    pub key_span: crate::ast::Span,
    /// Source range of the checked descriptor materialization expression.
    pub descriptor_span: crate::ast::Span,
}

/// One checked explicit compilation-unit or package registry entry.
///
/// The entry binding is a real source value. Its structural facts remain distinct from declaration decorators because
/// its subject is the defining compilation unit or package rather than the binding declaration itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryExplicitEntryInfo {
    pub registry_name: String,
    pub key: SemanticRegistryValue,
    pub descriptor: SemanticRegistryValue,
    pub subject_kind: SemanticRegistrySubjectKind,
    /// Exact source method approved for the declaration-only registry entry call.
    pub entry_method_identity: CanonicalSymbolId,
    /// Exact source constructor approved for the explicit subject expression.
    pub subject_constructor_identity: CanonicalSymbolId,
    /// Exact source-owned materializer selected by the frontend for backend substitution.
    pub checked_constructor_identity: CanonicalSymbolId,
    pub entry_name: String,
    pub declaration_span: crate::ast::Span,
    pub key_span: crate::ast::Span,
    pub subject_span: crate::ast::Span,
    pub descriptor_span: crate::ast::Span,
}

/// Typechecker-owned class layout consumed by backend struct lowering.
#[derive(Debug, Clone)]
pub struct ClassLayoutInfo {
    /// Whether the source class participates in the compiled library's public ABI.
    pub is_public: bool,
    /// Generic parameters that must remain unqualified inside Rust type displays.
    pub type_params: Vec<String>,
    /// Flattened fields in constructor ABI order: oldest ancestor first, then local fields.
    pub fields: Vec<ClassFieldLayoutInfo>,
    /// Ordered field names that violated the checked class-layout invariant.
    ///
    /// Lowering fails closed when this is non-empty instead of silently emitting a smaller Rust struct.
    pub missing_fields: Vec<String>,
    /// Defaults whose checked provider semantics cannot be materialized in a flattened consumer subclass.
    ///
    /// Typechecking diagnoses these at the class declaration. Lowering also fails closed if a caller attempts to use
    /// artifacts from an unsuccessful typecheck.
    pub unmaterializable_defaults: Vec<String>,
}

/// One checked class field retained across the frontend/backend boundary.
#[derive(Debug, Clone)]
pub struct ClassFieldLayoutInfo {
    /// Source-visible field name.
    pub name: String,
    /// Compiler-derived stored-record identity from the field's earliest declared name.
    pub record_identity: Vec<u8>,
    /// Whether this field participates in the canonical stored hash.
    pub record_canonical: bool,
    /// Earlier declared names in chronological order, earliest first.
    pub renamed_from: Vec<String>,
    /// Descriptive rename facts retained from the checked declaration.
    pub renamed_from_lifecycle: Option<incan_semantics_core::body_ir::LifecycleMetadata>,
    /// Whether the checked default is a statically empty collection literal.
    pub default_is_empty: bool,
    /// Fully resolved field type selected by the typechecker.
    pub ty: ResolvedType,
    /// Source-level Incan spelling retained solely for reflection and documentation.
    pub surface_type_name: Option<String>,
    /// Checked source visibility, including visibility reconstructed from compiled manifests.
    pub visibility: Visibility,
    /// Checked initializer plan, retaining whether the expression belongs to this source unit or a compiled provider.
    pub default: Option<ClassFieldDefaultInfo>,
    /// Canonical source field alias used for construction and access.
    pub alias: Option<String>,
    /// Source-authored field description preserved for generated reflection metadata.
    pub description: Option<String>,
}

/// Checked origin and value for one class-field default.
#[derive(Debug, Clone)]
pub enum ClassFieldDefaultInfo {
    /// Source expression owned by the current package, including defaults inherited through source classes.
    Source(Spanned<Expr>),
    /// Manifest-safe expression owned by a compiled dependency.
    PublicDependency {
        /// Dependency key used to qualify constants and helper calls during consumer lowering.
        library: String,
        /// Canonical provider-owned default expression.
        value: CheckedParamDefault,
    },
}

/// One compiler-checked newtype construction plan shared by lowering and generated bridges.
#[derive(Debug, Clone, PartialEq)]
pub struct NewtypeConstructionInfo {
    /// Derives lowering adds automatically, beyond `Copy` for a `Copy` underlying type: `Debug` unless the underlying
    /// type is known to lack it, and `Clone` when the underlying type implements it (#1754).
    ///
    /// The checker's derive relation decides both, so a newtype implements what the checker says it does.
    pub automatic_derives: Vec<String>,
    /// Declared type parameters in source order.
    pub type_params: Vec<String>,
    /// Resolved wrapped value type, including references to the declared type parameters.
    pub underlying: ResolvedType,
    /// Canonical checked constructor selected by the typechecker, when present.
    pub checked_constructor: Option<String>,
    /// Exact declaration identity of `checked_constructor`, when source provenance is available.
    pub checked_constructor_identity: Option<CanonicalSymbolId>,
    /// Checked hook signature available to consumers even when its declaration span belongs to another module.
    pub checked_constructor_binding: Option<FunctionBindingInfo>,
    /// Compiler-generated constrained-primitive predicates used when no checked constructor exists.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
    /// Whether ordinary implicit construction from the underlying value is allowed.
    pub implicit_coercion_enabled: bool,
    /// Whether the typechecker proved the newtype can participate in `TryFrom[str]` composition.
    pub supports_string_conversion: bool,
}
