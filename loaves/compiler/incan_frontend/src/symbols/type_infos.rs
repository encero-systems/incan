//! Type, class, model, newtype, enum, trait, capability, field, method and bound infos.

use super::*;

/// Class information
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub type_params: Vec<String>,
    pub extends: Option<String>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<TypeBoundInfo>,
    pub derives: Vec<String>,
    pub fields: HashMap<String, FieldInfo>,
    /// Constructor defaults keyed by field name, including inherited fields.
    ///
    /// Keeping these with the ordered class metadata lets compiled-library exports preserve the same constructor ABI
    /// as source lowering instead of trying to recover inherited defaults from the child's own AST fields. Boxed
    /// because default expressions are AST-heavy metadata and should not inflate every `SymbolKind` variant.
    pub field_defaults: Box<HashMap<String, crate::ast::Spanned<crate::ast::Expr>>>,
    /// Canonical defaults inherited from compiled-library parents.
    ///
    /// Synthetic AST is still retained for ordinary typechecking, but it cannot preserve a manifest call's checked
    /// signature or distinguish an already-canonical provider path from a child module's local path. This sidecar
    /// keeps the original checked metadata intact when a local child is exported again, including an `Unsupported`
    /// sentinel that preserves provider-owned default optionality when the expression cannot cross the manifest.
    /// Boxed with source defaults so canonical provider expressions do not inflate every symbol-table entry.
    pub field_default_metadata: Box<HashMap<String, crate::library_exports::CheckedParamDefault>>,
    /// Compiled dependency that owns each inherited field.
    ///
    /// Source-declared fields have no entry. The map is copied with inherited members so ownership survives source
    /// module boundaries instead of being reconstructed from the final subclass name.
    pub field_provider_libraries: Box<HashMap<String, String>>,
    pub field_order: Vec<String>,
    pub properties: HashMap<String, PropertyInfo>,
    pub methods: HashMap<String, MethodInfo>,
    pub method_overloads: HashMap<String, Vec<MethodInfo>>,
    pub method_aliases: HashMap<String, String>,
}

/// Model information
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub type_params: Vec<String>,
    pub traits: Vec<String>,
    pub trait_adoptions: Vec<TypeBoundInfo>,
    pub derives: Vec<String>,
    pub fields: HashMap<String, FieldInfo>,
    pub field_order: Vec<String>,
    pub properties: HashMap<String, PropertyInfo>,
    pub methods: HashMap<String, MethodInfo>,
    pub method_overloads: HashMap<String, Vec<MethodInfo>>,
    pub method_aliases: HashMap<String, String>,
}

/// Newtype information
#[derive(Debug, Clone)]
pub struct NewtypeInfo {
    pub type_params: Vec<String>,
    pub is_rusttype: bool,
    /// Set when this `rusttype` declares at least one `interop:` edge (used by later pipeline stages).
    pub has_interop: bool,
    pub underlying: ResolvedType,
    /// RFC 017 constrained primitive predicates carried by the declared underlying type.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
    /// Whether RFC 017 implicit coercion is permitted for this newtype.
    pub implicit_coercion_enabled: bool,
    /// Alias-to-target method rebinding map declared inside the type body (`alias = target`).
    ///
    /// Example: `send_now = try_send` is stored as `"send_now" -> "try_send"`.
    pub method_rebindings: HashMap<String, String>,
    /// Explicit traits adopted by this newtype/rusttype via `with`, using source-level trait names.
    pub traits: Vec<String>,
    /// Explicit traits adopted by this newtype/rusttype, preserving generic trait arguments when present.
    pub trait_adoptions: Vec<TypeBoundInfo>,
    /// Source-level `@derive(...)` names declared by this newtype.
    pub derives: Vec<String>,
    pub method_aliases: HashMap<String, String>,
    pub methods: HashMap<String, MethodInfo>,
    /// All newtype/rusttype method declarations grouped by name for trait-backed overload resolution.
    pub method_overloads: HashMap<String, Vec<MethodInfo>>,
}

/// One resolved constrained primitive predicate on a newtype underlying type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewtypePrimitiveConstraint {
    pub key: TypeConstraintKey,
    pub value: i64,
    pub repr: String,
}

/// Enum information
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub type_params: Vec<String>,
    /// Explicit traits adopted by this enum via `with`, using source-level trait names.
    pub traits: Vec<String>,
    /// Explicit traits adopted by this enum, preserving generic trait arguments when present.
    pub trait_adoptions: Vec<TypeBoundInfo>,
    pub variants: Vec<String>,
    /// Canonical declaration identities for variants and their same-enum aliases.
    ///
    /// Alias spellings point at the target variant's identity. Missing entries are explicitly unproven and must not
    /// be reconstructed from the enum/member spelling.
    pub variant_identities: HashMap<String, CanonicalSymbolId>,
    /// Positional payload fields for each canonical variant name.
    pub variant_fields: HashMap<String, Vec<ResolvedType>>,
    /// Variant alias name to canonical variant name.
    pub variant_aliases: HashMap<String, String>,
    pub value_enum: Option<ValueEnumInfo>,
    /// Names from `@derive(...)` (same vocabulary as models/classes).
    pub derives: Vec<String>,
    /// Inherent methods and associated functions declared in the enum body.
    pub methods: HashMap<String, MethodInfo>,
    /// All enum method declarations grouped by name for trait-backed overload resolution.
    pub method_overloads: HashMap<String, Vec<MethodInfo>>,
}

/// RFC 032 value enum metadata.
#[derive(Debug, Clone)]
pub struct ValueEnumInfo {
    pub value_type: ValueEnumBacking,
    pub values: HashMap<String, ValueEnumValue>,
}

/// Backing primitive kind for a value enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueEnumBacking {
    Str,
    Int,
}

impl ValueEnumBacking {
    /// Return the ordinary Incan primitive type represented by this backing kind.
    pub fn resolved_type(self) -> ResolvedType {
        match self {
            Self::Str => ResolvedType::Str,
            Self::Int => ResolvedType::Int,
        }
    }

    /// Return the surface spelling used in diagnostics for this backing kind.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
        }
    }
}

/// Literal value assigned to one value enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueEnumValue {
    Str(String),
    Int(i64),
}

impl ValueEnumValue {
    /// Return the raw value in a diagnostic-friendly display form.
    pub fn display_value(&self) -> String {
        match self {
            Self::Str(value) => format!("{value:?}"),
            Self::Int(value) => value.to_string(),
        }
    }
}

/// Trait information
#[derive(Debug, Clone)]
pub struct TraitInfo {
    pub type_params: Vec<String>,
    /// Direct supertraits from `with Trait, Other[T]` (RFC 042), after resolving type arguments.
    ///
    /// Each entry is `(trait_name, type_arguments)`; use an empty `type_arguments` list for a non-generic supertrait.
    pub supertraits: Vec<(String, Vec<ResolvedType>)>,
    pub methods: HashMap<String, MethodInfo>,
    pub method_aliases: HashMap<String, String>,
    pub properties: HashMap<String, PropertyInfo>,
    pub requires: Vec<(String, ResolvedType)>, // Required fields
}

/// A `capability` declaration's collected shape (RFC 104).
///
/// A capability names an authority to perform a side-effecting operation, not a value and not a type, so it carries no
/// `ResolvedType`: no expression in the language ever holds a capability. What it does carry is the authority's own
/// description, the typed dimensions a grant may constrain, and the other capabilities its implementation needs.
///
/// `requires` is deliberately unresolved at collection time. Capabilities may reference each other in any order within
/// a module, so resolving those references is a checking concern; collection records what was written and where, and
/// checking turns each into a symbol reference. Holding this capability never grants what it requires — that is the
/// invariant the separate list exists to preserve.
#[derive(Debug, Clone)]
pub struct CapabilityInfo {
    /// Prose from the `description` clause, when the declaration supplied a string literal.
    pub description: Option<String>,
    /// Typed scope dimensions from the `scope:` block, in declaration order.
    pub scope: Vec<(String, ResolvedType)>,
    /// Other capabilities this one needs, as written, in declaration order.
    pub requires: Vec<CapabilityRequirement>,
    pub is_public: bool,
}

/// One entry of a capability's `requires` list, before it is resolved to a capability symbol.
///
/// The span is kept so a later diagnostic can point at the reference the author wrote rather than at the enclosing
/// declaration.
#[derive(Debug, Clone)]
pub struct CapabilityRequirement {
    /// Dotted path exactly as written, split into segments — `host.http.request` becomes three segments.
    pub path: Vec<String>,
    /// Span of the reference itself.
    pub span: Span,
}

/// Module/import information
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub path: Vec<String>,
    pub is_python: bool,
}

/// Variant information
#[derive(Debug, Clone)]
pub struct VariantInfo {
    /// Canonical identity of the enum member declaration, when provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub enum_name: String,
    pub fields: Vec<ResolvedType>,
}

/// Field information
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// Canonical identity of the source field declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub ty: ResolvedType,
    /// Canonical Incan source spelling retained for reflection and documentation.
    ///
    /// This is presentation metadata only. Typechecking and lowering continue to use [`Self::ty`] as the semantic
    /// authority.
    pub surface_type_name: Option<String>,
    pub visibility: crate::ast::Visibility,
    /// Whether access is limited to methods declared on the owning nominal type.
    pub is_type_private: bool,
    pub owner: Option<String>,
    pub has_default: bool,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// Return whether a checked field uses the declaring nominal type as its access boundary.
pub fn field_is_type_private(field: &crate::ast::FieldDecl, private_fields_are_type_private: bool) -> bool {
    private_fields_are_type_private && matches!(field.visibility, crate::ast::Visibility::Private)
}

/// Return the user-facing field type name without making presentation metadata a second semantic authority.
///
/// Ordinary Incan types use registry-owned source spellings such as `dict`, `str` and `int`.
/// Rust-backed types retain the authored Incan spelling because their resolved representation carries
/// a canonical Rust path that must never leak through `FieldInfo.type_name`.
pub fn field_surface_type_name(source: &Type, resolved: &ResolvedType) -> String {
    if resolved_type_contains_rust_path(resolved) {
        source.to_string()
    } else {
        canonical_incan_type_name(resolved)
    }
}

/// Computed property information.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    /// Canonical identity of the source property declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub return_type: ResolvedType,
    pub visibility: crate::ast::Visibility,
    pub owner: Option<String>,
    /// False for abstract trait property requirements.
    pub has_body: bool,
}

/// Method information
#[derive(Debug, Clone)]
pub struct MethodInfo {
    /// Canonical identity of the source method declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub type_params: Vec<String>,
    pub type_param_bounds: HashMap<String, Vec<String>>,
    pub type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    pub trait_target: Option<TypeBoundInfo>,
    pub receiver: Option<Receiver>,
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub has_body: bool, // false for abstract methods (...)
    pub alias_of: Option<String>,
}

/// Resolved type-parameter bound metadata preserved for export/import paths.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeBoundInfo {
    pub name: String,
    pub source_name: Option<String>,
    pub type_args: Vec<ResolvedType>,
    pub module_path: Option<Vec<String>>,
    /// Compiler-resolved generic header attached to this exact adopted-trait implementation.
    pub implementation_type_params: Vec<ImplementationTypeParamInfo>,
    /// An `Eq` or `Hash` bound the compiler inferred because the callable's body hashes the type parameter (#1758),
    /// rather than one the source declared. A call is refused for it only when its type argument is known to lack the
    /// derive, since a type from another module may carry it through a `@rust.derive(...)` no manifest records.
    pub inferred: bool,
}

/// One implementation-header type parameter retained from checked library metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplementationTypeParamInfo {
    pub name: String,
    pub bounds: Vec<ImplementationTraitBoundInfo>,
}

/// One exact implementation requirement retained from checked library metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplementationTraitBoundInfo {
    pub trait_path: String,
    pub type_args: Vec<ResolvedType>,
    pub associated_types: Vec<(String, ResolvedType)>,
    pub origin: ImplementationTraitBoundOriginInfo,
}

/// Origin classification for an implementation-only bound before IR lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplementationTraitBoundOriginInfo {
    Standard,
    RustCapability,
    SourceCallable,
}
