//! Symbols, symbol kinds, and variable, static, function and callable parameter infos.

use super::*;

/// Kind of scope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Module,
    Function,
    Method { receiver: Option<Receiver> },
    Class,
    Model,
    Trait,
    Block,
}

/// A symbol in the symbol table
#[derive(Debug, Clone, serde::Serialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub scope: usize,
}

/// How a `rust::...` import binding relates to Rust’s module/type namespace (RFC 041).
///
/// Incan does not run the Rust type checker here; this classification is derived from import syntax only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RustImportBindingKind {
    /// `import rust::crate_name` — binds the crate root as a namespace (not a concrete type).
    CrateRoot,
    /// `import rust::crate_name::a::b::...` with at least one path segment after the crate name.
    RootedPath,
    /// `from rust::... import item` — binds a single imported Rust item.
    FromImport,
}

/// Provenance for a symbol that refers into a Rust dependency via `rust::` (RFC 041).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RustItemInfo {
    /// Crate name (first segment after `rust::` in the import source).
    pub crate_name: String,
    /// Canonical path used for diagnostics and future lowering: `crate::module::Item` (same string the import
    /// collector already built, joined with `::`).
    pub path: String,
    pub binding: RustImportBindingKind,
    /// Optional extracted Rust semantic metadata (RFC 041), shared with the lookup store and imported aliases.
    #[serde(serialize_with = "serialize_optional_rust_metadata")]
    pub metadata: Option<Arc<RustItemMetadata>>,
}

/// Serialize shared Rust metadata by value so the dependency-interface receipt does not require Serde's `rc` feature.
fn serialize_optional_rust_metadata<S>(
    metadata: &Option<Arc<RustItemMetadata>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serde::Serialize::serialize(&metadata.as_deref(), serializer)
}

/// Kind of symbol
#[derive(Debug, Clone, serde::Serialize)]
pub enum SymbolKind {
    /// Variable/binding
    Variable(VariableInfo),
    /// Module static storage cell.
    Static(StaticInfo),
    /// Function
    Function(FunctionInfo),
    /// Top-level same-name function overloads.
    FunctionOverloads(Vec<FunctionOverloadInfo>),
    /// Type (class, model, newtype, enum, builtin)
    Type(TypeInfo),
    /// Trait
    Trait(TraitInfo),
    /// Module/import
    Module(ModuleInfo),
    /// Enum variant
    Variant(VariantInfo),
    /// Field
    Field(FieldInfo),
    /// Computed property
    Property(PropertyInfo),
    /// Rust dependency import (`import rust::...` / `from rust::... import ...`, RFC 005 / RFC 041).
    RustItem(RustItemInfo),
    /// `capability` declaration naming an ambient runtime authority (RFC 104).
    Capability(CapabilityInfo),
}

/// Variable information
#[derive(Debug, Clone, serde::Serialize)]
pub struct VariableInfo {
    pub ty: ResolvedType,
    pub is_mutable: bool,
    pub is_used: bool,
}

/// Module static storage metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StaticInfo {
    pub ty: ResolvedType,
    pub is_public: bool,
    pub is_imported: bool,
    pub is_used: bool,
}

/// Function information
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionInfo {
    pub params: Vec<CallableParam>,
    pub return_type: ResolvedType,
    pub is_async: bool,
    pub type_params: Vec<String>,
    /// Explicit source-declared bounds per type parameter (RFC 023), keyed by type parameter name.
    pub type_param_bounds: HashMap<String, Vec<String>>,
    /// Resolved source-declared bounds, preserving generic type arguments such as `T with Serialize[F]`.
    pub type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    /// Rust function name emitted for this source callable when overloads require name disambiguation.
    pub emitted_name: Option<String>,
}

/// One top-level overload candidate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FunctionOverloadInfo {
    pub info: FunctionInfo,
    pub span: Span,
    /// Canonical declaration identity of this overload candidate.
    ///
    /// An overload-set symbol cannot carry one declaration identity because each candidate has its own source
    /// declaration. Keeping the identity beside the candidate preserves that distinction through aliases, imports,
    /// compiled-library manifests, and call resolution. `None` is an explicitly unproven external candidate.
    pub identity: Option<CanonicalSymbolId>,
}

/// Callable parameter metadata preserved after type resolution.
///
/// RFC 038 requires callable values to retain rest-parameter shape instead of collapsing to a flat list of types. The
/// optional `name` lets explicit `Callable[...]` types keep unnamed fixed parameters while declarations and methods
/// preserve names for keyword binding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallableParam {
    pub name: Option<String>,
    pub ty: ResolvedType,
    /// Authored type spelling retained only when the annotation contains a transparent type alias.
    ///
    /// Semantic compatibility, overload selection, hashing and lowering use [`Self::ty`]. This sidecar is
    /// presentation provenance for diagnostics and tooling and must never create alias identity.
    pub surface_type_name: Option<String>,
    pub kind: ParamKind,
    pub has_default: bool,
    /// This parameter receives a construction-time-captured partial preset when its caller omits it.
    ///
    /// It remains callable by name, but positional calls bind only non-preset parameters so the residual positional
    /// surface stays stable. This flag is meaningful only for a local `partial` expression; module partial
    /// declarations retain their established full-signature metadata without it.
    pub is_partial_preset: bool,
    /// The callable type marks this parameter `mut`: the callable's changes to the argument are visible to the caller.
    ///
    /// The `(mut T, ...) -> R` spelling, a `def` parameter declared `mut` whose changes reach the caller, a closure
    /// checked against a marked shape, and a `mut self` method's receiver in the callable shape its decorators see set
    /// the marker (#1790). The marker decides how the argument is passed, so two callable types are compatible only
    /// when their markers agree.
    pub is_mut: bool,
}

impl PartialEq for CallableParam {
    /// Compare only callable semantics; authored alias spelling is display provenance.
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.ty == other.ty
            && self.kind == other.kind
            && self.has_default == other.has_default
            && self.is_partial_preset == other.is_partial_preset
            && self.is_mut == other.is_mut
    }
}

impl Eq for CallableParam {}

impl std::hash::Hash for CallableParam {
    /// Hash only callable semantics so an alias spelling cannot create a distinct function type or overload key.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.ty.hash(state);
        self.kind.hash(state);
        self.has_default.hash(state);
        self.is_partial_preset.hash(state);
        self.is_mut.hash(state);
    }
}

impl CallableParam {
    /// Build metadata for a source-declared callable parameter.
    pub fn named(name: impl Into<String>, ty: ResolvedType, kind: ParamKind) -> Self {
        Self {
            name: Some(name.into()),
            ty,
            surface_type_name: None,
            kind,
            has_default: false,
            is_partial_preset: false,
            is_mut: false,
        }
    }

    /// Build metadata for a source-declared callable parameter with default-value information.
    pub fn named_with_default(name: impl Into<String>, ty: ResolvedType, kind: ParamKind, has_default: bool) -> Self {
        Self {
            name: Some(name.into()),
            ty,
            surface_type_name: None,
            kind,
            has_default,
            is_partial_preset: false,
            is_mut: false,
        }
    }

    /// Build metadata for an unnamed fixed parameter in a function type.
    pub fn positional(ty: ResolvedType) -> Self {
        Self {
            name: None,
            ty,
            surface_type_name: None,
            kind: ParamKind::Normal,
            has_default: false,
            is_partial_preset: false,
            is_mut: false,
        }
    }

    /// Return this parameter with the callable type's `mut` marker set as given.
    pub fn with_mut(mut self, is_mut: bool) -> Self {
        self.is_mut = is_mut;
        self
    }

    /// Return this parameter with its authored alias-bearing type spelling retained for presentation.
    pub fn with_surface_type_name(mut self, surface_type_name: Option<String>) -> Self {
        self.surface_type_name = surface_type_name;
        self
    }

    /// Return the source name when the callable metadata has one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Type information
#[derive(Debug, Clone, serde::Serialize)]
pub enum TypeInfo {
    Builtin,
    Class(ClassInfo),
    Model(ModelInfo),
    TypeAlias,
    Newtype(NewtypeInfo),
    Enum(EnumInfo),
}
