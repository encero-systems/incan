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
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub scope: usize,
}

/// How a `rust::...` import binding relates to Rust’s module/type namespace (RFC 041).
///
/// Incan does not run the Rust type checker here; this classification is derived from import syntax only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustImportBindingKind {
    /// `import rust::crate_name` — binds the crate root as a namespace (not a concrete type).
    CrateRoot,
    /// `import rust::crate_name::a::b::...` with at least one path segment after the crate name.
    RootedPath,
    /// `from rust::... import item` — binds a single imported Rust item.
    FromImport,
}

/// Provenance for a symbol that refers into a Rust dependency via `rust::` (RFC 041).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustItemInfo {
    /// Crate name (first segment after `rust::` in the import source).
    pub crate_name: String,
    /// Canonical path used for diagnostics and future lowering: `crate::module::Item` (same string the import
    /// collector already built, joined with `::`).
    pub path: String,
    pub binding: RustImportBindingKind,
    /// Optional extracted Rust semantic metadata (RFC 041), shared with the lookup store and imported aliases.
    pub metadata: Option<Arc<RustItemMetadata>>,
}

/// Kind of symbol
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct VariableInfo {
    pub ty: ResolvedType,
    pub is_mutable: bool,
    pub is_used: bool,
}

/// Module static storage metadata.
#[derive(Debug, Clone)]
pub struct StaticInfo {
    pub ty: ResolvedType,
    pub is_public: bool,
    pub is_imported: bool,
    pub is_used: bool,
}

/// Function information
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallableParam {
    pub name: Option<String>,
    pub ty: ResolvedType,
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

impl CallableParam {
    /// Build metadata for a source-declared callable parameter.
    pub fn named(name: impl Into<String>, ty: ResolvedType, kind: ParamKind) -> Self {
        Self {
            name: Some(name.into()),
            ty,
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

    /// Return the source name when the callable metadata has one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Type information
#[derive(Debug, Clone)]
pub enum TypeInfo {
    Builtin,
    Class(ClassInfo),
    Model(ModelInfo),
    TypeAlias,
    Newtype(NewtypeInfo),
    Enum(EnumInfo),
}
