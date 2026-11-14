//! Type, class, model, newtype, enum, trait, capability, field, method and bound infos.

use super::*;

/// Class information
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NewtypePrimitiveConstraint {
    pub key: TypeConstraintKey,
    pub value: i64,
    pub repr: String,
}

/// Enum information
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValueEnumInfo {
    pub value_type: ValueEnumBacking,
    pub values: HashMap<String, ValueEnumValue>,
}

/// Backing primitive kind for a value enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapabilityRequirement {
    /// Dotted path exactly as written, split into segments — `host.http.request` becomes three segments.
    pub path: Vec<String>,
    /// Span of the reference itself.
    pub span: Span,
}

/// Module/import information
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModuleInfo {
    pub path: Vec<String>,
    pub is_python: bool,
}

/// Variant information
#[derive(Debug, Clone, serde::Serialize)]
pub struct VariantInfo {
    /// Canonical identity of the enum member declaration, when provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub enum_name: String,
    pub fields: Vec<ResolvedType>,
}

/// Checked migration guidance for a declaration or member that used to be public.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct MadePrivateInfo {
    pub since: Option<String>,
    pub retire_in: Option<String>,
    pub message: Option<String>,
    pub by: Option<String>,
    pub issue: Option<String>,
    /// Checked source reference rendered for diagnostics; it is never an automatic rewrite.
    pub replacement: Option<String>,
}

impl MadePrivateInfo {
    /// Drop parser spans while preserving the lifecycle facts checked on a field declaration.
    pub fn from_field(metadata: &crate::ast::MadePrivateMetadata) -> Self {
        Self {
            since: metadata.since.as_ref().map(|value| value.node.clone()),
            retire_in: metadata.retire_in.as_ref().map(|value| value.node.clone()),
            message: metadata.message.as_ref().map(|value| value.node.clone()),
            by: metadata.by.as_ref().map(|value| value.node.clone()),
            issue: metadata.issue.as_ref().map(|value| value.node.clone()),
            replacement: metadata.replacement.as_ref().map(|path| path.node.segments.join(".")),
        }
    }
}

/// Read compiler-owned `@made_private(...)` facts from one declaration's decorators.
pub fn made_private_from_decorators(
    decorators: &[crate::ast::Spanned<crate::ast::Decorator>],
) -> Option<MadePrivateInfo> {
    use crate::ast::{DecoratorArg, DecoratorArgValue, Expr, Literal};

    let decorator = decorators.iter().find(|decorator| {
        incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
            == Some(incan_lang::lang::decorators::DecoratorId::MadePrivate)
    })?;
    let mut info = MadePrivateInfo::default();
    for argument in &decorator.node.args {
        let DecoratorArg::Named(name, DecoratorArgValue::Expr(value)) = argument else {
            continue;
        };
        let string = match &value.node {
            Expr::Literal(Literal::String(value)) => Some(value.clone()),
            _ => None,
        };
        match name.as_str() {
            "since" => info.since = string,
            "retire_in" => info.retire_in = string,
            "message" => info.message = string,
            "by" => info.by = string,
            "issue" => info.issue = string,
            "replacement" => info.replacement = made_private_reference(&value.node),
            _ => {}
        }
    }
    Some(info)
}

/// Render one checked-reference expression without treating its text as callable dispatch.
fn made_private_reference(expr: &crate::ast::Expr) -> Option<String> {
    match expr {
        crate::ast::Expr::Ident(name) => Some(name.clone()),
        crate::ast::Expr::Field(base, member) => {
            let mut path = made_private_reference(&base.node)?;
            path.push('.');
            path.push_str(member);
            Some(path)
        }
        _ => None,
    }
}

/// Field information
#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldInfo {
    /// Canonical identity of the source field declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
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
    pub made_private: Option<MadePrivateInfo>,
}

/// Derive the stored-record identity from a field's earliest declared name.
pub fn field_record_identity(field: &crate::ast::FieldDecl) -> Vec<u8> {
    let earliest_name = field
        .metadata
        .renamed_from
        .first()
        .map_or(field.name.as_str(), |earlier| earlier.node.as_str());
    incan_semantics_core::record_identity::record_member_identity(earliest_name).to_vec()
}

/// Return whether a field participates in the canonical stored hash, defaulting an absent key to true.
pub fn field_record_canonical(field: &crate::ast::FieldDecl) -> bool {
    field.metadata.canonical.as_ref().is_none_or(|canonical| canonical.node)
}

/// Copy a field's earlier names into checked metadata without retaining parser spans.
pub fn field_renamed_from(field: &crate::ast::FieldDecl) -> Vec<String> {
    field
        .metadata
        .renamed_from
        .iter()
        .map(|earlier| earlier.node.clone())
        .collect()
}

/// Copy inert rename labels into checked facts; source spans remain in the syntax tree for diagnostics.
pub fn field_renamed_from_lifecycle(
    field: &crate::ast::FieldDecl,
) -> Option<incan_semantics_core::body_ir::LifecycleMetadata> {
    let lifecycle = field.metadata.renamed_from_lifecycle.as_ref()?;
    Some(incan_semantics_core::body_ir::LifecycleMetadata {
        since: lifecycle.since.as_ref().map(|value| value.node.clone()),
        retire_in: lifecycle.retire_in.as_ref().map(|value| value.node.clone()),
        message: lifecycle.message.as_ref().map(|value| value.node.clone()),
        by: lifecycle.by.as_ref().map(|value| value.node.clone()),
        issue: lifecycle.issue.as_ref().map(|value| value.node.clone()),
    })
}

/// Recognize an empty collection default without evaluating user code or guessing a factory's result.
pub fn field_default_is_empty(field: &crate::ast::FieldDecl) -> bool {
    field.default.as_ref().is_some_and(|default| match &default.node {
        crate::ast::Expr::List(values) => values.is_empty(),
        crate::ast::Expr::Dict(values) => values.is_empty(),
        crate::ast::Expr::Tuple(values) | crate::ast::Expr::Set(values) => values.is_empty(),
        _ => false,
    })
}

/// Whether the unmarked members of a type, its fields and methods alike, are private to it (#884, #2570).
///
/// Those of every class are, and those of a model, enum or newtype marked `pub`. An unmarked member of a model, enum or
/// newtype without `pub` is visible wherever the type is, which is the module that declares it.
pub fn members_are_type_private(is_class: bool, type_visibility: crate::ast::Visibility) -> bool {
    is_class || matches!(type_visibility, crate::ast::Visibility::Public)
}

/// Return whether a checked field uses the declaring nominal type as its access boundary.
pub fn field_is_type_private(field: &crate::ast::FieldDecl, members_type_private: bool) -> bool {
    members_type_private && matches!(field.visibility, crate::ast::Visibility::Private)
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
        canonical_incan_type_name(&restore_qualified_surface_names(source, resolved))
    }
}

/// Restore declaration names at module-qualified source nodes before rendering reflection metadata.
///
/// Checked types retain canonical carrier keys so equal-spelled declarations from different modules stay distinct.
/// Those keys are semantic identities, not source spellings: field metadata presents the declaration name a direct
/// import would expose while preserving the normalized spelling of surrounding builtin types.
fn restore_qualified_surface_names(source: &Type, resolved: &ResolvedType) -> ResolvedType {
    match (source, resolved) {
        (Type::Dotted(segments), ResolvedType::Named(_)) => segments
            .last()
            .cloned()
            .map(ResolvedType::Named)
            .unwrap_or_else(|| resolved.clone()),
        (Type::DottedGeneric(segments, source_args), ResolvedType::Generic(base, resolved_args)) => {
            let base = segments.last().cloned().unwrap_or_else(|| base.clone());
            ResolvedType::Generic(base, restore_qualified_surface_arguments(source_args, resolved_args))
        }
        (Type::Generic(_, source_args), ResolvedType::Generic(base, resolved_args)) => ResolvedType::Generic(
            base.clone(),
            restore_qualified_surface_arguments(source_args, resolved_args),
        ),
        (Type::Function(source_params, source_return), ResolvedType::Function(resolved_params, resolved_return)) => {
            let params = source_params
                .iter()
                .zip(resolved_params)
                .map(|(source, resolved)| CallableParam {
                    ty: restore_qualified_surface_names(&source.node, &resolved.ty),
                    ..resolved.clone()
                })
                .collect();
            ResolvedType::Function(
                params,
                Box::new(restore_qualified_surface_names(&source_return.node, resolved_return)),
            )
        }
        (Type::Tuple(source_items), ResolvedType::Tuple(resolved_items)) => ResolvedType::Tuple(
            source_items
                .iter()
                .zip(resolved_items)
                .map(|(source, resolved)| restore_qualified_surface_names(&source.node, resolved))
                .collect(),
        ),
        (Type::Ref(source), ResolvedType::Ref(resolved)) => {
            ResolvedType::Ref(Box::new(restore_qualified_surface_names(&source.node, resolved)))
        }
        (Type::RefMut(source), ResolvedType::RefMut(resolved)) => {
            ResolvedType::RefMut(Box::new(restore_qualified_surface_names(&source.node, resolved)))
        }
        (Type::MutParam(source), resolved) => restore_qualified_surface_names(&source.node, resolved),
        _ => resolved.clone(),
    }
}

/// Restore qualified declaration names in paired generic arguments without changing unmatched checked arguments.
fn restore_qualified_surface_arguments(
    source: &[crate::ast::Spanned<Type>],
    resolved: &[ResolvedType],
) -> Vec<ResolvedType> {
    resolved
        .iter()
        .enumerate()
        .map(|(index, resolved)| {
            source.get(index).map_or_else(
                || resolved.clone(),
                |source| restore_qualified_surface_names(&source.node, resolved),
            )
        })
        .collect()
}

/// Computed property information.
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct MethodInfo {
    /// Canonical identity of the source method declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    /// Where the method may be called from (#2570).
    pub scope: MethodScope,
    /// Source name of the type that declares the method, which a subclass inheriting it keeps. Absent for a trait
    /// method and for a method without a source declaration.
    pub owner: Option<String>,
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
    pub made_private: Option<MadePrivateInfo>,
}

/// Where a method may be called from (#2570).
///
/// `pub def` makes a method public. An unmarked method follows the rule for unmarked fields
/// ([`members_are_type_private`]): it is private to a class, and to a model, enum or newtype marked `pub`, and visible
/// wherever any other type is. A method declared in a trait, or
/// one that implements an adopted trait's method, takes the trait's visibility instead: public for a `pub` trait, and
/// the trait's module for any other trait, which is the only module that can name it. A language hook is public by
/// rule. A method alias keeps its target's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MethodScope {
    /// Callable only from methods of the declaring type.
    Type,
    /// Callable anywhere in the declaring module.
    Module,
    /// Callable wherever the type is visible.
    Public,
}

impl MethodScope {
    /// The scope a method declaration's written visibility gives it, on a type whose unmarked members are private to it
    /// or not ([`members_are_type_private`]).
    pub fn written(visibility: crate::ast::Visibility, members_type_private: bool) -> Self {
        match visibility {
            crate::ast::Visibility::Public => Self::Public,
            crate::ast::Visibility::Private if members_type_private => Self::Type,
            crate::ast::Visibility::Private => Self::Public,
        }
    }

    /// The visibility a library publishes: only a public method is callable by another module.
    pub fn published_visibility(self) -> crate::ast::Visibility {
        match self {
            Self::Public => crate::ast::Visibility::Public,
            Self::Type | Self::Module => crate::ast::Visibility::Private,
        }
    }
}

/// Resolved type-parameter bound metadata preserved for export/import paths.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ImplementationTypeParamInfo {
    pub name: String,
    pub bounds: Vec<ImplementationTraitBoundInfo>,
}

/// One exact implementation requirement retained from checked library metadata.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ImplementationTraitBoundInfo {
    pub trait_path: String,
    pub type_args: Vec<ResolvedType>,
    pub associated_types: Vec<(String, ResolvedType)>,
    pub origin: ImplementationTraitBoundOriginInfo,
}

/// Origin classification for an implementation-only bound before IR lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ImplementationTraitBoundOriginInfo {
    Standard,
    RustCapability,
    SourceCallable,
}
