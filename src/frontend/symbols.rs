//! Symbol table and scope management for Incan
//!
//! Tracks all named entities (types, functions, variables, traits) and their scopes.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use crate::frontend::ast::{ParamKind, Receiver, Span, Type, TypeConstraintKey};
use incan_core::interop::RustItemMetadata;
use incan_core::lang::builtins::{self, BuiltinFnId};
use incan_core::lang::conventions;
use incan_core::lang::surface::constructors;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_core::lang::traits;
use incan_core::lang::traits::TraitId;
use incan_core::lang::types::collections;
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics;
use incan_core::lang::types::numerics::NumericTypeId;
use incan_core::lang::types::stringlike;
use incan_core::lang::types::stringlike::StringLikeId;
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

/// Unique identifier for symbols
pub type SymbolId = usize;

/// Result of registering one binding key in RFC 120's shared collision mechanism.
///
/// Registration is intentionally first-wins. A caller may still retain metadata for the rejected declaration, but
/// the active binding remains the first declaration until the caller reports the collision. This keeps invalid
/// programs deterministic and lets every construct preserve its existing diagnostic wording while sharing one
/// answer to the collision question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingRegistration<V> {
    /// This key was vacant and now owns `value`.
    Registered,
    /// This key was already active; `existing` remains registered.
    Collision { existing: V },
}

/// Register one binding key, preserving the first active binding on collision.
///
/// Symbol scopes, field aliases, trait instantiations, and public-library exports all call this function. Their keys
/// describe the relevant RFC 120 namespace or construct-specific subdomain; their diagnostics remain at the call
/// site, but no caller reimplements first-wins collision detection.
pub fn register_binding<K, V>(bindings: &mut HashMap<K, V>, key: K, value: V) -> BindingRegistration<V>
where
    K: Eq + Hash,
    V: Clone,
{
    match bindings.entry(key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(value);
            BindingRegistration::Registered
        }
        std::collections::hash_map::Entry::Occupied(entry) => BindingRegistration::Collision {
            existing: entry.get().clone(),
        },
    }
}

/// One name-registration collision produced by [`SymbolTable`].
#[derive(Debug, Clone)]
pub struct SymbolBindingCollision {
    pub name: String,
    pub namespace: SymbolNamespace,
    pub existing_span: Span,
    pub incoming_span: Span,
    pub existing_identity: Option<CanonicalSymbolId>,
    pub incoming_identity: Option<CanonicalSymbolId>,
    pub existing_is_import: bool,
    pub incoming_is_import: bool,
}

/// Collision key within one lexical scope.
///
/// A variant's owner distinguishes two enums' same-spelled members even though their convenience bindings coexist in
/// the module scope. Other members are already separated by their owning type's scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindingKey {
    namespace: SymbolNamespace,
    owner: Option<String>,
    name: String,
}

#[derive(Debug)]
struct BindingTransaction {
    previous: HashMap<String, Option<SymbolId>>,
    collision_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingDefinitionMode {
    RejectCollision,
    AllowExplicitShadow,
    PreserveExistingLookup,
}

#[derive(Debug, Clone)]
enum BindingIdentity {
    /// Mint an identity from this declaration site.
    Declaration,
    /// Preserve the resolved target identity; `None` remains explicitly unproven.
    Target(Option<CanonicalSymbolId>),
}

/// Classify one symbol-table definition into its RFC 120 declaration category, when the kind decides it.
///
/// `None` marks the shapes a [`SymbolKind`] alone cannot classify: overload sets (each overload owns a span-keyed
/// identity; a set-level one would name an arbitrary member), `Type(Builtin)` placeholders (a generic binder and an
/// imported-type stub share that representation, so the defining site must state the category), and module bindings
/// (module-path-namespace identities are deferred until a consumer needs them). A `Variable` defaults to `Local`;
/// parameter and receiver sites state their category explicitly via `SymbolTable::define_with_target_kind`.
fn default_identity_kind(kind: &SymbolKind) -> Option<SemanticSourceTargetKind> {
    match kind {
        SymbolKind::Variable(_) => Some(SemanticSourceTargetKind::Local),
        SymbolKind::Static(_) => Some(SemanticSourceTargetKind::Static),
        SymbolKind::Function(_) => Some(SemanticSourceTargetKind::Function),
        SymbolKind::FunctionOverloads(_) => None,
        SymbolKind::Type(TypeInfo::Class(_)) => Some(SemanticSourceTargetKind::Class),
        SymbolKind::Type(TypeInfo::Model(_)) => Some(SemanticSourceTargetKind::Model),
        SymbolKind::Type(TypeInfo::TypeAlias) => Some(SemanticSourceTargetKind::TypeAlias),
        SymbolKind::Type(TypeInfo::Newtype(info)) => Some(if info.is_rusttype {
            SemanticSourceTargetKind::Rusttype
        } else {
            SemanticSourceTargetKind::Newtype
        }),
        SymbolKind::Type(TypeInfo::Enum(_)) => Some(SemanticSourceTargetKind::Enum),
        SymbolKind::Type(TypeInfo::Builtin) => None,
        SymbolKind::Trait(_) => Some(SemanticSourceTargetKind::Trait),
        SymbolKind::Capability(_) => Some(SemanticSourceTargetKind::Capability),
        SymbolKind::Variant(_) => Some(SemanticSourceTargetKind::Variant),
        SymbolKind::Field(_) => Some(SemanticSourceTargetKind::Field),
        SymbolKind::Property(_) => Some(SemanticSourceTargetKind::Property),
        SymbolKind::Module(_) => None,
        SymbolKind::RustItem(_) => Some(SemanticSourceTargetKind::RustItem),
    }
}

/// Return which RFC 120 namespace an identity of this declaration category lives in.
///
/// Member declarations (fields, methods, properties, variants) are owned by their nominal type and reached
/// `.`-directed, so their identities carry the member namespace even where a bare lexical convenience binding exists
/// (a variant's bare spelling defers to real lexical bindings; the identity belongs to the member declaration).
/// Everything the scope table binds otherwise is ordinary lexical.
fn identity_namespace_for_kind(kind: &SemanticSourceTargetKind) -> SymbolNamespace {
    match kind {
        SemanticSourceTargetKind::Field
        | SemanticSourceTargetKind::Method
        | SemanticSourceTargetKind::Property
        | SemanticSourceTargetKind::Variant => SymbolNamespace::Member,
        SemanticSourceTargetKind::Module => SymbolNamespace::ModulePath,
        _ => SymbolNamespace::OrdinaryLexical,
    }
}

/// Return the canonical root identity for one compiler-owned builtin function.
///
/// This is shared by symbol registration and downstream checked-IR consumers so the builtin registry has one
/// identity projection. Callers must already hold a [`BuiltinFnId`]; this function never classifies source spelling.
pub(crate) fn canonical_builtin_function_identity(builtin: BuiltinFnId) -> Option<CanonicalSymbolId> {
    builtins::BUILTIN_FUNCTIONS
        .iter()
        .find(|entry| entry.id == builtin)
        .map(|entry| canonical_builtin_identity(entry.canonical))
}

/// Build one canonical identity from a compiler-owned builtin registry entry.
fn canonical_builtin_identity(canonical_name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Builtin,
        declaration_name: canonical_name.to_string(),
        kind: SemanticSourceTargetKind::Builtin,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    }
}

/// Canonical semantic name for anonymous union types (RFC 029).
pub const UNION_TYPE_NAME: &str = incan_core::lang::types::UNION_TYPE_NAME;

/// Separator used in generated Rust symbols for source overload implementations.
const OVERLOAD_EMITTED_NAME_SEPARATOR: &str = "_overload_";

/// Build the generated Rust symbol name for one overload implementation.
pub(crate) fn overload_emitted_name(source_name: &str, hash: u64) -> String {
    format!("{source_name}{OVERLOAD_EMITTED_NAME_SEPARATOR}{hash:016x}")
}

/// Build the deterministic Rust symbol name for one same-name source overload.
pub(crate) fn overloaded_function_emitted_name(source_name: &str, info: &FunctionInfo) -> String {
    let mut signature = String::new();
    signature.push_str(source_name);
    signature.push('(');
    for param in &info.params {
        signature.push_str(&param.ty.to_string());
        signature.push(';');
    }
    signature.push_str(")->");
    signature.push_str(&info.return_type.to_string());
    overload_emitted_name(source_name, stable_fnv1a(signature.as_bytes()))
}

/// Hash bytes with the FNV-1a variant used for deterministic generated symbol suffixes.
fn stable_fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Return the generated Rust symbol prefix shared by all overload implementations for one source binding.
pub(crate) fn overload_emitted_name_prefix(source_name: &str) -> String {
    format!("{source_name}{OVERLOAD_EMITTED_NAME_SEPARATOR}")
}

/// Return whether a generated Rust symbol names one overload implementation.
pub(crate) fn is_overload_emitted_name(name: &str) -> bool {
    let Some((source_name, hash)) = name.rsplit_once(OVERLOAD_EMITTED_NAME_SEPARATOR) else {
        return false;
    };
    !source_name.is_empty() && hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Symbol table managing all named entities
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
    scopes: Vec<Scope>,
    current_scope: usize,
    current_scope_binding_transaction: Option<BindingTransaction>,
    /// RFC 120 canonical identities, minted once per definition and keyed by [`SymbolId`].
    ///
    /// This side map is the compiler's single declaration-site assignment point: every named definition that the
    /// table can classify receives its identity here, at [`Self::define`] time, instead of each downstream consumer
    /// re-deriving one from module path plus spelling. A definition the table cannot classify (an overload set, a
    /// placeholder whose defining site did not state its kind) deliberately has no entry — absent means unproven,
    /// never "rebuild it from the name".
    identities: HashMap<SymbolId, CanonicalSymbolId>,
    /// Module path owning locally-declared symbols, used as the minted [`SymbolOrigin`].
    ///
    /// Empty until the typechecker learns the module path; identities minted before that carry an empty module
    /// origin, which `module_identity_for_path` renders as `<module>` — the same anonymous-module convention the
    /// fact store already uses.
    module_path: Vec<String>,
    /// Defining package for declarations checked as part of a compiled-library artifact.
    ///
    /// Source modules normally use [`SymbolOrigin::Module`]. A library producer sets this before collecting its
    /// modules so the identity embedded in its Rust symbols is exactly the package identity later hydrated by
    /// consumers from the `.incnlib` contract.
    package_identity: Option<String>,
    /// True only while [`Self::add_builtins`] runs, so [`Self::define`] leaves identity assignment to the explicit
    /// registry identities that loop attaches (builtin aliases must carry the canonical registry spelling, which the
    /// generic mint cannot know).
    minting_builtins: bool,
    /// Temporary lexical view used while collecting one dependency's checked interface.
    ///
    /// Dependency declarations are metadata, not active consumer source bindings. Keeping this map separate from the
    /// root scope makes them available while one interface is collected and makes them disappear atomically before
    /// consumer source is checked. A fresh map is used for each dependency so equal spellings from sibling modules
    /// cannot overwrite one another.
    dependency_interface_bindings: Option<HashMap<String, SymbolId>>,
    /// Historical symbols created inside dependency-interface views.
    ///
    /// The symbols remain inspectable after the temporary view closes, but they must not be exported as declarations
    /// owned by the consumer module even when older collection paths minted a consumer-shaped identity for them.
    dependency_interface_symbol_ids: HashSet<SymbolId>,
    /// Symbols defined through [`Self::define_import_binding`], marked so a later-arriving import-resolution proof
    /// can be attached to exactly the binding it proves (see [`Self::backfill_import_identity`]) and never to an
    /// unrelated same-spelled definition.
    import_bindings: HashSet<SymbolId>,
    /// Checked source path associated with each import or alias binding that resolves through a source import.
    ///
    /// This is keyed by symbol id so rejected collisions and later shadows cannot overwrite the active binding's
    /// meaning. Rust and Python imports deliberately have no source binding path here.
    import_binding_paths: HashMap<SymbolId, Vec<String>>,
    /// Registration key for every non-builtin symbol that participates in collision detection.
    binding_keys: HashMap<SymbolId, BindingKey>,
    /// Collisions awaiting typechecker-owned diagnostics.
    binding_collisions: Vec<SymbolBindingCollision>,
}

impl SymbolTable {
    /// Create a root module scope populated with the language's builtin symbols.
    pub fn new() -> Self {
        let mut table = Self {
            symbols: Vec::new(),
            scopes: vec![Scope::new(None, ScopeKind::Module)],
            current_scope: 0,
            current_scope_binding_transaction: None,
            identities: HashMap::new(),
            module_path: Vec::new(),
            package_identity: None,
            minting_builtins: false,
            dependency_interface_bindings: None,
            dependency_interface_symbol_ids: HashSet::new(),
            import_bindings: HashSet::new(),
            import_binding_paths: HashMap::new(),
            binding_keys: HashMap::new(),
            binding_collisions: Vec::new(),
        };

        // Add builtin types
        table.minting_builtins = true;
        table.add_builtins();
        table.minting_builtins = false;
        table
    }

    /// Record the module path that owns subsequently-defined local declarations.
    ///
    /// Called by the typechecker as soon as it knows its module identity, before user declarations are collected.
    /// Builtins are already defined by then and keep their [`SymbolOrigin::Builtin`] identities.
    pub fn set_module_path(&mut self, module_path: Vec<String>) {
        self.module_path = module_path;
    }

    /// Record that subsequently checked declarations are owned by one compiled package.
    pub fn set_package_identity(&mut self, package_identity: Option<String>) {
        self.package_identity = package_identity;
    }

    /// Return the origin that owns declarations in the currently checked module.
    fn declaration_origin(&self) -> SymbolOrigin {
        self.package_identity
            .as_ref()
            .map(|library| SymbolOrigin::Package {
                library: library.clone(),
                module_path: self.module_path.clone(),
            })
            .unwrap_or_else(|| SymbolOrigin::Module(self.module_path.clone()))
    }

    /// Return the RFC 120 canonical identity minted for a defined symbol, if the definition proved one.
    ///
    /// Absent means the definition was not classifiable at its site (overload set, unclassified placeholder,
    /// module binding) or was an import whose target resolution did not prove an identity. Consumers must treat
    /// absence as "unproven" and fail closed rather than reconstruct an identity from the symbol's spelling.
    pub fn identity_of(&self, id: SymbolId) -> Option<&CanonicalSymbolId> {
        self.identities.get(&id)
    }

    /// Return the canonical root identity for one builtin function registry entry.
    ///
    /// This deliberately bypasses lexical lookup: an ordinary builtin such as `len` may be shadowed locally, while
    /// an explicit `std.builtins.len` resolution still needs the compiler-owned builtin identity. Some builtin
    /// functions are recognized directly by the typechecker and do not need a physical root-scope symbol, but every
    /// closed registry id still has the same canonical registry identity.
    pub fn builtin_function_identity(&self, builtin: BuiltinFnId) -> Option<CanonicalSymbolId> {
        canonical_builtin_function_identity(builtin)
    }

    /// Populate the root scope with built-in type symbols.
    fn add_builtins(&mut self) {
        // Builtin types (from the canonical `incan_core::lang::types` registries).
        //
        // We define both canonical spellings and aliases so name lookup stays robust and we avoid
        // drift between the compiler and the language vocabulary registries. Each entry pairs the
        // defined spelling with its canonical registry spelling, so every alias records the one RFC 120
        // registry identity instead of minting a second identity per spelling.
        let mut builtin_types: Vec<(&'static str, &'static str)> = Vec::new();
        for t in numerics::NUMERIC_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for t in stringlike::STRING_LIKE_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for t in collections::COLLECTION_TYPES {
            builtin_types.push((t.canonical, t.canonical));
            builtin_types.extend(t.aliases.iter().map(|alias| (*alias, t.canonical)));
        }
        for t in surface_types::SURFACE_TYPES {
            // RFC 022: stdlib-scoped types must be explicitly imported (e.g. `from std.web import App`).
            // Only truly global surface types (Rust interop helpers) are injected here.
            if surface_types::is_global(t.item.id) {
                builtin_types.push((t.item.canonical, t.item.canonical));
                builtin_types.extend(t.item.aliases.iter().map(|alias| (*alias, t.item.canonical)));
            }
        }
        // Unit-ish types that are not yet modeled in `incan_core::lang::types`.
        builtin_types.push((conventions::UNIT_TYPE_NAME, conventions::UNIT_TYPE_NAME));
        builtin_types.push((conventions::NONE_TYPE_NAME, conventions::NONE_TYPE_NAME));
        builtin_types.push((UNION_TYPE_NAME, UNION_TYPE_NAME));

        // Deduplicate to avoid defining the same builtin twice.
        let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
        for (name, canonical) in builtin_types.into_iter().filter(|(n, _)| seen.insert(*n)) {
            let id = self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, canonical);
        }

        // Builtin traits
        for info in traits::TRAITS {
            let type_params = match info.id {
                TraitId::Awaitable => vec!["T".to_string()],
                _ => Vec::new(),
            };
            let id = self.define(Symbol {
                name: info.canonical.to_string(),
                kind: SymbolKind::Trait(TraitInfo {
                    type_params,
                    methods: HashMap::new(),
                    method_aliases: HashMap::new(),
                    properties: HashMap::new(),
                    requires: vec![],
                    supertraits: vec![],
                }),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, info.canonical);
        }

        // Builtin variants for Result and Option
        // Ok(T) and Err(E) for Result
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Ok).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Ok));
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Err).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Result).to_string(),
                fields: vec![ResolvedType::TypeVar("E".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Err));
        // Some(T) and None for Option
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::Some).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![ResolvedType::TypeVar("T".to_string())],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::Some));
        let id = self.define(Symbol {
            name: constructors::as_str(constructors::ConstructorId::None).to_string(),
            kind: SymbolKind::Variant(VariantInfo {
                identity: None,
                enum_name: collections::as_str(CollectionTypeId::Option).to_string(),
                fields: vec![],
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, constructors::as_str(constructors::ConstructorId::None));

        // Builtin functions
        for name in std::iter::once(builtins::as_str(BuiltinFnId::Print))
            .chain(builtins::aliases(BuiltinFnId::Print).iter().copied())
        {
            let id = self.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::Function(FunctionInfo {
                    params: vec![CallableParam::named("msg", ResolvedType::Str, ParamKind::Normal)],
                    return_type: ResolvedType::Unit,
                    is_async: false,
                    type_params: vec![],
                    type_param_bounds: HashMap::new(),
                    type_param_bound_details: HashMap::new(),
                    emitted_name: None,
                }),
                span: Span::default(),
                scope: 0,
            });
            self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Print));
        }
        let id = self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Len).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![CallableParam::named(
                    "collection",
                    ResolvedType::Unknown,
                    ParamKind::Normal,
                )],
                return_type: ResolvedType::Int,
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                emitted_name: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Len));
        // range() builtin - returns an iterator
        let id = self.define(Symbol {
            name: builtins::as_str(BuiltinFnId::Range).to_string(),
            kind: SymbolKind::Function(FunctionInfo {
                params: vec![CallableParam::named("n", ResolvedType::Int, ParamKind::Normal)],
                return_type: ResolvedType::Named("Range".to_string()), // Iterator-like
                is_async: false,
                type_params: vec![],
                type_param_bounds: HashMap::new(),
                type_param_bound_details: HashMap::new(),
                emitted_name: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        self.record_builtin_identity(id, builtins::as_str(BuiltinFnId::Range));
    }

    /// Enter a new scope
    pub fn enter_scope(&mut self, kind: ScopeKind) {
        let new_scope = Scope::new(Some(self.current_scope), kind);
        self.scopes.push(new_scope);
        self.current_scope = self.scopes.len() - 1;
    }

    /// Exit the current scope
    pub fn exit_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.current_scope = parent;
        }
    }

    /// Define a new symbol in the current scope through the shared RFC 120 registration mechanism.
    pub fn define(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a binding introduced through an explicit `let` or `mut` shadowing form.
    ///
    /// RFC 120 makes these forms declarations even when the spelling is already active in this scope. The new
    /// declaration replaces the active registration without producing a duplicate-binding diagnostic; later plain
    /// assignment and reference lookup therefore select the new binding and its fresh canonical identity.
    pub fn define_explicit_shadow(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::AllowExplicitShadow,
        )
    }

    /// Define a compiler-generated refinement of the currently-active binding.
    ///
    /// Branch narrowing changes the binding's type inside a nested scope but does not declare a new source object.
    /// The synthetic symbol therefore replaces the active registration while preserving the declaration identity it
    /// refines. An unresolved refinement retains the ordinary declaration-site fallback for recovery paths.
    pub fn define_refined_binding(&mut self, symbol: Symbol) -> SymbolId {
        let identity = self
            .lookup(&symbol.name)
            .and_then(|id| self.identities.get(&id))
            .cloned();
        let identity = identity.map_or(BindingIdentity::Declaration, |identity| {
            BindingIdentity::Target(Some(identity))
        });
        self.define_registered(symbol, identity, false, BindingDefinitionMode::AllowExplicitShadow)
    }

    /// Define a symbol whose identity kind the defining site states explicitly.
    ///
    /// The generic [`Self::define`] mint classifies a definition from its [`SymbolKind`], which cannot distinguish a
    /// parameter from a local (both are `Variable`) or a generic-binder placeholder from a builtin type (both are
    /// `Type(Builtin)`). Sites that know the RFC 120 declaration category pass it here so the minted identity records
    /// what was declared rather than how the table happens to represent it.
    pub fn define_with_target_kind(&mut self, symbol: Symbol, kind: SemanticSourceTargetKind) -> SymbolId {
        let identity = self.mint_identity_with_kind(&symbol, self.current_scope, kind);
        self.define_registered(
            symbol,
            BindingIdentity::Target(Some(identity)),
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define an import, alias, or re-export binding carrying its resolved target's identity.
    ///
    /// RFC 120: a binding created by an import is a binding to an *existing* canonical symbol — it must carry the
    /// declaring module's identity, never one minted from the importing module. `target_identity` is the identity
    /// import resolution proved (see `TypeChecker::dependency_member_identity`); `None` records the binding as
    /// unproven rather than inventing a consumer-module identity for it.
    pub fn define_import_binding(&mut self, symbol: Symbol, target_identity: Option<CanonicalSymbolId>) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            true,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a source alias carrying its target's identity without classifying the binding as an import.
    ///
    /// Aliases and imports both preserve a resolved declaration identity, but only syntactic imports participate in
    /// ambiguous-import diagnostics. An unresolved alias remains identity-less rather than minting an alias-site
    /// declaration identity.
    pub fn define_alias_binding(&mut self, symbol: Symbol, target_identity: Option<CanonicalSymbolId>) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a checked source import and retain the exact path accepted for its local binding.
    ///
    /// The binding path is distinct from canonical declaration identity: an import may resolve through a facade while
    /// its identity names the module that owns the declaration. Decorator and trait resolution need the accepted
    /// source path, so they consume this sidecar rather than reconstructing it from raw AST imports.
    pub fn define_import_binding_at_path(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_import_binding(symbol, target_identity);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Define a source alias that preserves both its target identity and its checked import binding path.
    pub fn define_alias_binding_at_path(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_alias_binding(symbol, target_identity);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Define an import whose represented target kind is sufficient to mint its canonical identity.
    ///
    /// Rust item metadata carries the declaring crate path directly, so Rust imports use this path instead of asking
    /// their collector to duplicate the table's identity construction. Source imports continue to pass the identity
    /// proven by module resolution to [`Self::define_import_binding`]; module placeholders remain explicitly unproven.
    pub fn define_import_binding_with_inferred_target(&mut self, symbol: Symbol) -> SymbolId {
        let target_identity = self.mint_identity(&symbol);
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            true,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define an inferred-identity import reached through a checked source import path.
    pub fn define_import_binding_with_inferred_target_at_path(
        &mut self,
        symbol: Symbol,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_import_binding_with_inferred_target(symbol);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Remove a symbol's minted identity after its definition stops naming one declaration.
    ///
    /// The one current case is a module function symbol converted in place into an overload set: the identity minted
    /// for the first declaration would otherwise keep naming an arbitrary member of the set. Overload declarations
    /// keep their own span-keyed identities on `FunctionBindingInfo`.
    pub fn clear_identity(&mut self, id: SymbolId) {
        self.identities.remove(&id);
    }

    /// Attach a later-proven import-resolution identity to the import binding it proves.
    ///
    /// Import materialization and identity proof do not always happen in one order, so a binding defined before its
    /// proof was recorded starts identity-less. This attaches the proof to the *current* binding for `name` only
    /// when that binding was defined as an import binding and still has no identity — a local declaration or
    /// overload set that has since shadowed the import keeps its own facts, and a binding that already carries an
    /// identity is never overwritten.
    pub fn backfill_import_identity(&mut self, name: &str, identity: &CanonicalSymbolId) {
        let Some(id) = self.lookup(name) else {
            return;
        };
        if self.import_bindings.contains(&id) && !self.identities.contains_key(&id) {
            self.identities.insert(id, identity.clone());
        }
    }

    /// Return whether a retained symbol is the binding ordinary lookup currently selects.
    ///
    /// Rejected definitions remain in the arena for diagnostics and identity evidence. Side tables keyed only by a
    /// spelling must therefore commit their metadata only when this returns `true`; otherwise they would become
    /// last-wins while lexical lookup remains first-wins.
    pub(crate) fn is_active_lookup_binding(&self, id: SymbolId) -> bool {
        self.get(id).is_some_and(|symbol| self.lookup(&symbol.name) == Some(id))
    }

    /// Return the checked source import path associated with the active binding for `name`.
    pub(crate) fn import_binding_path(&self, name: &str) -> Option<&[String]> {
        let id = self.lookup(name)?;
        self.import_binding_paths.get(&id).map(Vec::as_slice)
    }

    /// Iterate checked source import paths for active module-scope bindings.
    pub(crate) fn active_import_binding_paths(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.scopes[0].symbols.iter().filter_map(|(name, id)| {
            self.import_binding_paths
                .get(id)
                .map(|path| (name.as_str(), path.as_slice()))
        })
    }

    /// Iterate active top-level source bindings in definition order for the HIR handoff.
    ///
    /// The symbol arena preserves definition order while the root lookup map does not. Filtering through that map
    /// keeps first-wins collision semantics: rejected later definitions remain available for diagnostics but do not
    /// become HIR bindings. Builtins and dependency-interface declarations have no consumer source declaration.
    pub(crate) fn active_module_source_bindings(
        &self,
    ) -> impl Iterator<Item = (Span, &str, Option<&CanonicalSymbolId>)> + '_ {
        self.symbols.iter().enumerate().filter_map(|(id, symbol)| {
            if symbol.scope != 0
                || symbol.span == Span::default()
                || self.dependency_interface_symbol_ids.contains(&id)
                || self.scopes[0].symbols.get(&symbol.name) != Some(&id)
            {
                return None;
            }
            Some((symbol.span, symbol.name.as_str(), self.identities.get(&id)))
        })
    }

    /// Define a symbol without replacing an existing same-scope lookup binding.
    ///
    /// Enum variants need to remain available to whole-table consumers such as match exhaustiveness and qualified
    /// pattern resolution, but a variant named like an imported type must not steal the bare identifier from that type.
    pub fn define_preserving_existing_binding(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::PreserveExistingLookup,
        )
    }

    /// Define an alias that preserves both the target identity and any already-active bare lookup binding.
    pub fn define_alias_preserving_existing_binding(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
    ) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            false,
            BindingDefinitionMode::PreserveExistingLookup,
        )
    }

    /// Register and retain one symbol definition.
    ///
    /// `identity` decides whether this binding mints a declaration identity or preserves a resolved target identity.
    /// A missing target identity stays missing rather than becoming an alias-site declaration. Builtins
    /// occupy the fallback tier and deliberately bypass source-binding registration, so a source declaration such as
    /// `len` can replace that lookup spelling without a collision. `mode` records the source binding form: ordinary
    /// definitions reject collisions, explicit `let`/`mut` declarations replace an active registration, and member
    /// convenience bindings such as enum variants retain metadata without stealing an ordinary lexical lookup.
    fn define_registered(
        &mut self,
        mut symbol: Symbol,
        identity: BindingIdentity,
        is_import: bool,
        mode: BindingDefinitionMode,
    ) -> SymbolId {
        self.record_current_scope_binding_before_change(&symbol.name);
        symbol.scope = self.current_scope;
        let id = self.symbols.len();
        let identity = match identity {
            BindingIdentity::Declaration => self.mint_identity(&symbol),
            BindingIdentity::Target(identity) => identity,
        };
        if let Some(identity) = identity {
            self.identities.insert(id, identity);
        }
        if is_import {
            self.import_bindings.insert(id);
        }

        let name = symbol.name.clone();
        self.symbols.push(symbol);

        if self.minting_builtins {
            self.scopes[self.current_scope].symbols.insert(name, id);
            return id;
        }
        if let Some(bindings) = &mut self.dependency_interface_bindings {
            bindings.insert(name, id);
            self.dependency_interface_symbol_ids.insert(id);
            return id;
        }

        let key = self.binding_key(id);
        if key.namespace == SymbolNamespace::OrdinaryLexical
            && let Some(existing) = self.immutable_builtin_binding(&name)
        {
            self.binding_keys.insert(id, key.clone());
            self.record_binding_collision(existing, id, &key, is_import);
            return id;
        }
        if mode == BindingDefinitionMode::AllowExplicitShadow {
            self.scopes[self.current_scope]
                .binding_registrations
                .insert(key.clone(), id);
            self.binding_keys.insert(id, key);
            self.scopes[self.current_scope].symbols.insert(name, id);
            return id;
        }
        let registration = register_binding(
            &mut self.scopes[self.current_scope].binding_registrations,
            key.clone(),
            id,
        );
        self.binding_keys.insert(id, key.clone());
        match registration {
            BindingRegistration::Registered => {
                if mode == BindingDefinitionMode::PreserveExistingLookup {
                    self.scopes[self.current_scope].symbols.entry(name).or_insert(id);
                } else {
                    self.scopes[self.current_scope].symbols.insert(name, id);
                }
            }
            BindingRegistration::Collision { existing } => {
                self.record_binding_collision(existing, id, &key, is_import);
                // The first registration remains the active lookup binding. The rejected symbol and its identity are
                // retained for complete diagnostics and recovery analysis, but invalid source cannot silently change
                // what later references mean.
            }
        }
        id
    }

    /// Build the collision key for one already-retained symbol.
    fn binding_key(&self, id: SymbolId) -> BindingKey {
        let symbol = &self.symbols[id];
        let namespace = if self.import_bindings.contains(&id) {
            SymbolNamespace::OrdinaryLexical
        } else if let Some(identity) = self.identities.get(&id) {
            identity.namespace
        } else {
            default_identity_kind(&symbol.kind)
                .map(|kind| identity_namespace_for_kind(&kind))
                .unwrap_or(SymbolNamespace::OrdinaryLexical)
        };
        let owner = match &symbol.kind {
            SymbolKind::Variant(info) => Some(info.enum_name.clone()),
            _ => None,
        };
        BindingKey {
            namespace,
            owner,
            name: symbol.name.clone(),
        }
    }

    /// Return the root builtin binding when `name` is one of the immutable output-function spellings.
    ///
    /// Unlike ordinary builtins, this check is global across lexical scopes and applies before explicit-shadow mode:
    /// no ordinary lexical declaration or import binding may reuse `print` or `println`. Member names live in a
    /// separate namespace and do not replace either builtin.
    fn immutable_builtin_binding(&self, name: &str) -> Option<SymbolId> {
        (builtins::from_str(name) == Some(BuiltinFnId::Print))
            .then(|| self.scopes[0].symbols.get(name).copied())
            .flatten()
    }

    /// Retain both sides of one rejected registration for typechecker-owned diagnostics.
    fn record_binding_collision(&mut self, existing: SymbolId, incoming: SymbolId, key: &BindingKey, is_import: bool) {
        self.binding_collisions.push(SymbolBindingCollision {
            name: self.symbols[incoming].name.clone(),
            namespace: key.namespace,
            existing_span: self.symbols[existing].span,
            incoming_span: self.symbols[incoming].span,
            existing_identity: self.identities.get(&existing).cloned(),
            incoming_identity: self.identities.get(&incoming).cloned(),
            existing_is_import: self.import_bindings.contains(&existing),
            incoming_is_import: is_import,
        });
    }

    // ---- RFC 120 canonical identity minting ----

    /// Mint the canonical identity for one definition, when its symbol kind classifies it.
    ///
    /// Returns `None` while builtins are being registered (their loop attaches explicit registry identities so alias
    /// spellings share the canonical entry), for overload sets (each overload declaration owns a span-keyed identity
    /// of its own; a set-level identity would name an arbitrary member), and for definitions whose kind is a
    /// placeholder the site must classify via [`Self::define_with_target_kind`].
    fn mint_identity(&self, symbol: &Symbol) -> Option<CanonicalSymbolId> {
        if self.minting_builtins {
            return None;
        }
        let kind = default_identity_kind(&symbol.kind)?;
        Some(self.mint_identity_with_kind(symbol, symbol.scope, kind))
    }

    /// Build one canonical identity for a definition with an already-decided declaration category.
    ///
    /// A `rust::` item's identity is anchored to its crate path with a zero declaration span: it has no Incan
    /// declaration site, and the zero span keeps every module's import of one item structurally equal instead of
    /// splitting per import site. Everything else is owned by the current module and anchored at its declaration
    /// span, with the defining scope as discriminant for bindings that are not module-unique.
    fn mint_identity_with_kind(
        &self,
        symbol: &Symbol,
        scope: usize,
        kind: SemanticSourceTargetKind,
    ) -> CanonicalSymbolId {
        let mut kind = kind;
        let mut namespace = identity_namespace_for_kind(&kind);
        let (origin, declaration_name, declaration_span) = match &symbol.kind {
            SymbolKind::RustItem(info) => {
                if info.binding != RustImportBindingKind::FromImport {
                    namespace = SymbolNamespace::ModulePath;
                    kind = SemanticSourceTargetKind::Module;
                }
                (
                    SymbolOrigin::RustCrate(info.path.split("::").map(str::to_string).collect()),
                    info.path.rsplit("::").next().unwrap_or(&symbol.name).to_string(),
                    HirSourceSpan::new(0, 0),
                )
            }
            _ => (
                self.declaration_origin(),
                symbol.name.clone(),
                HirSourceSpan::new(symbol.span.start, symbol.span.end),
            ),
        };
        // A member is owned by its nominal type and distinguished by its declaration span, not by which table scope
        // its convenience binding happened to be defined in — so member identities never carry a scope discriminant
        // and compare equal however they are reached.
        let scope_discriminant =
            (namespace != SymbolNamespace::Member && scope != 0).then_some(ScopeDiscriminant(scope));
        CanonicalSymbolId {
            namespace,
            origin,
            declaration_name,
            kind,
            scope_discriminant,
            declaration_span,
        }
    }

    /// Iterate the identities of module-scope declarations this module owns, paired with their declaration spans.
    ///
    /// Yields only definitions whose identity origin is the current module and whose declaration span is real:
    /// builtins carry the registry origin and zero spans, and cross-module import bindings carry the declaring
    /// module's origin, so neither can masquerade as a local declaration. A binding to a *same-module* declaration
    /// (an `alias` targeting a local symbol) does pass the filter and yields its binding site paired with the
    /// target declaration's identity — which is the RFC 120 answer for that site: the alias is a second binding to
    /// the existing declaration, not a second declaration.
    pub fn local_declaration_identities(&self) -> impl Iterator<Item = (Span, &CanonicalSymbolId)> + '_ {
        self.symbols.iter().enumerate().filter_map(|(id, symbol)| {
            if symbol.scope != 0 || symbol.span == Span::default() || self.dependency_interface_symbol_ids.contains(&id)
            {
                return None;
            }
            let identity = self.identities.get(&id)?;
            let owned_here = match (&identity.origin, self.package_identity.as_deref()) {
                (SymbolOrigin::Module(path), None) => path == &self.module_path,
                (SymbolOrigin::Package { library, module_path }, Some(package)) => {
                    library == package && module_path == &self.module_path
                }
                _ => false,
            };
            if !owned_here {
                return None;
            }
            Some((symbol.span, identity))
        })
    }

    /// Mint the canonical identity of one module-level declaration owned by the current module.
    ///
    /// The table is the single minting authority for RFC 120 identities, so sites that record declaration facts
    /// outside the symbol table (function/partial bindings, method bindings) obtain their identity here rather than
    /// assembling one from module path plus spelling themselves.
    pub fn module_declaration_identity(
        &self,
        name: &str,
        kind: SemanticSourceTargetKind,
        span: Span,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: identity_namespace_for_kind(&kind),
            origin: self.declaration_origin(),
            declaration_name: name.to_string(),
            kind,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(span.start, span.end),
        }
    }

    /// Build the canonical identity of one compiler-proven source or package module path.
    ///
    /// Module namespaces have no declaration token in the importing file, so their provenance anchor is zero. Their
    /// structural owner is the checked module graph path: project and `std` modules retain a module origin, while
    /// `pub::` paths retain their package owner. Rust namespace imports use the dedicated Rust branch in the mint.
    pub fn module_path_identity(path: &[String]) -> Option<CanonicalSymbolId> {
        let declaration_name = path.last()?.clone();
        let origin = match path {
            [root, library, rest @ ..] if root == "pub" => SymbolOrigin::Package {
                library: library.clone(),
                module_path: rest.to_vec(),
            },
            _ => SymbolOrigin::Module(path.to_vec()),
        };
        Some(CanonicalSymbolId {
            namespace: SymbolNamespace::ModulePath,
            origin,
            declaration_name,
            kind: SemanticSourceTargetKind::Module,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 0),
        })
    }

    /// Mint the canonical identity of one member declaration owned by a nominal type in the current module.
    ///
    /// Members live in RFC 120's member namespace: they are reached `.`-directed from an owner type, never through
    /// the scope chain, so a member and a lexical binding sharing one spelling never compare equal. Two owners'
    /// same-named members stay distinct through their declaration spans.
    pub fn member_declaration_identity(
        &self,
        name: &str,
        kind: SemanticSourceTargetKind,
        span: Span,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::Member,
            origin: self.declaration_origin(),
            declaration_name: name.to_string(),
            kind,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(span.start, span.end),
        }
    }

    /// Attach the explicit registry identity for one builtin definition.
    ///
    /// Builtin alias spellings (`i64` for `int`, `println` for `print`) are separate table entries, but RFC 120's
    /// builtin fallback tier has one canonical entry per builtin — so every alias records the canonical registry
    /// spelling as its declaration name and all spellings compare equal.
    fn record_builtin_identity(&mut self, id: SymbolId, canonical_name: &str) {
        self.identities.insert(id, canonical_builtin_identity(canonical_name));
    }

    /// Look up a symbol by name in the current scope chain
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        let mut scope_idx = self.current_scope;
        loop {
            if scope_idx == 0
                && let Some(id) = self
                    .dependency_interface_bindings
                    .as_ref()
                    .and_then(|bindings| bindings.get(name))
            {
                return Some(*id);
            }
            if let Some(&id) = self.scopes[scope_idx].symbols.get(name) {
                return Some(id);
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Look up a symbol only in the current scope (no parent lookup)
    pub fn lookup_local(&self, name: &str) -> Option<SymbolId> {
        if self.current_scope == 0
            && let Some(id) = self
                .dependency_interface_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(name))
        {
            return Some(*id);
        }
        self.scopes[self.current_scope].symbols.get(name).copied()
    }

    /// Drain symbol-definition collisions for typechecker-owned diagnostics.
    pub(crate) fn take_binding_collisions(&mut self) -> Vec<SymbolBindingCollision> {
        std::mem::take(&mut self.binding_collisions)
    }

    /// Open a fresh temporary lookup view for one dependency interface.
    pub(crate) fn begin_dependency_interface_bindings(&mut self) {
        debug_assert!(self.dependency_interface_bindings.is_none());
        self.dependency_interface_bindings = Some(HashMap::new());
    }

    /// Drop the active dependency-interface view before another module or consumer source is checked.
    pub(crate) fn finish_dependency_interface_bindings(&mut self) {
        debug_assert!(self.dependency_interface_bindings.is_some());
        self.dependency_interface_bindings = None;
    }

    /// Start recording the previous value of each current-scope binding changed by subsequent definitions.
    pub(crate) fn begin_current_scope_binding_transaction(&mut self) {
        debug_assert!(self.current_scope_binding_transaction.is_none());
        self.current_scope_binding_transaction = Some(BindingTransaction {
            previous: HashMap::new(),
            collision_len: self.binding_collisions.len(),
        });
    }

    /// Finish the active binding transaction and return only the names it touched.
    pub(crate) fn finish_current_scope_binding_transaction(&mut self) -> HashMap<String, Option<SymbolId>> {
        let Some(transaction) = self.current_scope_binding_transaction.take() else {
            return HashMap::new();
        };
        // Dependency-import transactions temporarily materialize bindings only to collect checked public metadata.
        // Their collisions are not collisions in the consumer's lexical scope and must not leak into its diagnostics.
        self.binding_collisions.truncate(transaction.collision_len);
        transaction.previous
    }

    /// Record one binding before its first change in the active transaction.
    fn record_current_scope_binding_before_change(&mut self, name: &str) {
        let previous = if self.current_scope == 0 {
            self.dependency_interface_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(name).copied())
                .or_else(|| self.scopes[self.current_scope].symbols.get(name).copied())
        } else {
            self.scopes[self.current_scope].symbols.get(name).copied()
        };
        if let Some(transaction) = &mut self.current_scope_binding_transaction {
            transaction.previous.entry(name.to_string()).or_insert(previous);
        }
    }

    /// Restore or remove one name binding in the current scope without deleting historical symbol metadata.
    pub(crate) fn restore_current_scope_binding(&mut self, name: &str, binding: Option<SymbolId>) {
        if self.current_scope == 0
            && let Some(bindings) = &mut self.dependency_interface_bindings
        {
            if let Some(symbol_id) = binding {
                bindings.insert(name.to_string(), symbol_id);
            } else {
                bindings.remove(name);
            }
            return;
        }
        if let Some(current) = self.scopes[self.current_scope].symbols.get(name).copied()
            && let Some(key) = self.binding_keys.get(&current)
            && self.scopes[self.current_scope].binding_registrations.get(key) == Some(&current)
        {
            self.scopes[self.current_scope].binding_registrations.remove(key);
        }
        if let Some(symbol_id) = binding {
            self.scopes[self.current_scope]
                .symbols
                .insert(name.to_string(), symbol_id);
            if let Some(key) = self.binding_keys.get(&symbol_id).cloned() {
                self.scopes[self.current_scope]
                    .binding_registrations
                    .insert(key, symbol_id);
            }
        } else {
            self.scopes[self.current_scope].symbols.remove(name);
        }
    }

    /// Get a symbol by ID
    pub fn get(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id)
    }

    /// Get a mutable symbol by ID
    pub fn get_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(id)
    }

    /// All symbols in definition order (builtins first, then user declarations).
    ///
    /// Used for whole-program analyses such as supertrait graphs.
    pub(crate) fn all_symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Get the current scope kind
    pub fn current_scope_kind(&self) -> ScopeKind {
        self.scopes[self.current_scope].kind
    }

    /// Check if we're inside a function/method
    pub fn in_function(&self) -> bool {
        let mut scope_idx = self.current_scope;
        loop {
            match self.scopes[scope_idx].kind {
                ScopeKind::Function | ScopeKind::Method { .. } => return true,
                _ => {}
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        false
    }

    /// Get the current function's return type (if in a function)
    pub fn current_return_type(&self) -> Option<&ResolvedType> {
        let mut scope_idx = self.current_scope;
        loop {
            match &self.scopes[scope_idx].kind {
                ScopeKind::Function | ScopeKind::Method { .. } => {
                    return self.scopes[scope_idx].return_type.as_ref();
                }
                _ => {}
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Set the return type for the current function scope
    pub fn set_return_type(&mut self, ty: ResolvedType) {
        self.scopes[self.current_scope].return_type = Some(ty);
    }
}

/// Build the canonical identity for a source-owned type/object member.
///
/// This is shared by the live symbol table and source stdlib extraction so both paths retain the declaring module
/// and declaration span. Callers without exact Incan declaration provenance must leave member identity absent.
pub(crate) fn source_member_identity(
    module_path: &[String],
    name: &str,
    kind: SemanticSourceTargetKind,
    span: Span,
) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::Member,
        origin: SymbolOrigin::Module(module_path.to_vec()),
        declaration_name: name.to_string(),
        kind,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(span.start, span.end),
    }
}

/// A scope containing symbol definitions
#[derive(Debug)]
pub struct Scope {
    pub parent: Option<usize>,
    pub kind: ScopeKind,
    pub symbols: HashMap<String, SymbolId>,
    binding_registrations: HashMap<BindingKey, SymbolId>,
    pub return_type: Option<ResolvedType>,
}

impl Scope {
    /// Create an empty lexical scope with independent lookup and collision-registration state.
    pub fn new(parent: Option<usize>, kind: ScopeKind) -> Self {
        Self {
            parent,
            kind,
            symbols: HashMap::new(),
            binding_registrations: HashMap::new(),
            return_type: None,
        }
    }
}

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
    /// Optional extracted Rust semantic metadata (RFC 041).
    pub metadata: Option<RustItemMetadata>,
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
#[derive(Debug, Clone, PartialEq)]
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
        }
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
    /// as source lowering instead of trying to recover inherited defaults from the child's own AST fields.
    /// Boxed because default expressions are AST-heavy metadata and should not inflate every `SymbolKind` variant.
    pub field_defaults: Box<HashMap<String, crate::frontend::ast::Spanned<crate::frontend::ast::Expr>>>,
    /// Canonical defaults inherited from compiled-library parents.
    ///
    /// Synthetic AST is still retained for ordinary typechecking, but it cannot preserve a manifest call's checked
    /// signature or distinguish an already-canonical provider path from a child module's local path. This sidecar
    /// keeps the original checked metadata intact when a local child is exported again, including an `Unsupported`
    /// sentinel that preserves provider-owned default optionality when the expression cannot cross the manifest.
    /// Boxed with source defaults so canonical provider expressions do not inflate every symbol-table entry.
    pub field_default_metadata: Box<HashMap<String, crate::frontend::library_exports::CheckedParamDefault>>,
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
    pub visibility: crate::frontend::ast::Visibility,
    /// Whether access is limited to methods declared on the owning nominal type.
    pub is_type_private: bool,
    pub owner: Option<String>,
    pub has_default: bool,
    pub alias: Option<String>,
    pub description: Option<String>,
}

/// Return whether a checked field uses the declaring nominal type as its access boundary.
pub(crate) fn field_is_type_private(
    field: &crate::frontend::ast::FieldDecl,
    private_fields_are_type_private: bool,
) -> bool {
    private_fields_are_type_private && matches!(field.visibility, crate::frontend::ast::Visibility::Private)
}

/// Return the user-facing field type name without making presentation metadata a second semantic authority.
///
/// Ordinary Incan types use the resolved type's canonical spelling, which normalizes compatibility aliases such as
/// `Dict` to `dict`. Rust-backed types retain the authored Incan spelling because their resolved representation carries
/// a canonical Rust path that must never leak through `FieldInfo.type_name`.
pub(crate) fn field_surface_type_name(source: &Type, resolved: &ResolvedType) -> String {
    if resolved_type_contains_rust_path(resolved) {
        source.to_string()
    } else {
        canonical_incan_type_name(resolved)
    }
}

/// Render one resolved type with canonical Incan collection spellings while preserving its semantic shape.
fn canonical_incan_type_name(ty: &ResolvedType) -> String {
    let mut canonical = ty.clone();
    canonicalize_collection_type_names(&mut canonical);
    canonical.to_string()
}

/// Normalize compatibility aliases such as `List` recursively without changing nominal or Rust type identity.
fn canonicalize_collection_type_names(ty: &mut ResolvedType) {
    match ty {
        ResolvedType::Generic(name, args) => {
            if let Some(collection) = collections::from_str(name) {
                *name = match collection {
                    CollectionTypeId::List => "list",
                    CollectionTypeId::Dict => "dict",
                    CollectionTypeId::Set => "set",
                    CollectionTypeId::Tuple
                    | CollectionTypeId::Option
                    | CollectionTypeId::Result
                    | CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::Generator => collections::as_str(collection),
                }
                .to_string();
            }
            for arg in args {
                canonicalize_collection_type_names(arg);
            }
        }
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => canonicalize_collection_type_names(inner),
        ResolvedType::FrozenDict(key, value) => {
            canonicalize_collection_type_names(key);
            canonicalize_collection_type_names(value);
        }
        ResolvedType::Function(params, result) => {
            for param in params {
                canonicalize_collection_type_names(&mut param.ty);
            }
            canonicalize_collection_type_names(result);
        }
        ResolvedType::Tuple(args) => {
            for arg in args {
                canonicalize_collection_type_names(arg);
            }
        }
        ResolvedType::Never
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Bool
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes
        | ResolvedType::Unit
        | ResolvedType::Named(_)
        | ResolvedType::TypeVar(_)
        | ResolvedType::SelfType
        | ResolvedType::RustPath(_)
        | ResolvedType::CallSiteInfer
        | ResolvedType::Unknown => {}
    }
}

/// Return whether a resolved field type contains canonical Rust-path metadata at any nesting depth.
fn resolved_type_contains_rust_path(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::RustPath(_) => true,
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => resolved_type_contains_rust_path(inner),
        ResolvedType::FrozenDict(key, value) => {
            resolved_type_contains_rust_path(key) || resolved_type_contains_rust_path(value)
        }
        ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(resolved_type_contains_rust_path),
        ResolvedType::Function(params, result) => {
            params.iter().any(|param| resolved_type_contains_rust_path(&param.ty))
                || resolved_type_contains_rust_path(result)
        }
        _ => false,
    }
}

/// Computed property information.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    /// Canonical identity of the source property declaration, when declaration provenance is available.
    pub identity: Option<CanonicalSymbolId>,
    pub return_type: ResolvedType,
    pub visibility: crate::frontend::ast::Visibility,
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

/// Resolved type (after type checking)
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedType {
    /// Internal Rust never type (`!`).
    ///
    /// This is not currently source-spellable in Incan. It preserves diverging Rust callable results so the
    /// typechecker can apply bottom-type compatibility without treating `!` as a nominal Rust path.
    Never,
    /// Primitive types
    Int,
    Float,
    /// Exact-width numeric type introduced by RFC 009.
    Numeric(NumericTypeId),
    Bool,
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    FrozenList(Box<ResolvedType>),
    FrozenDict(Box<ResolvedType>, Box<ResolvedType>),
    FrozenSet(Box<ResolvedType>),
    /// Unit type
    Unit,
    /// Named type (class, model, newtype, enum)
    Named(String),
    /// Generic type with arguments
    Generic(String, Vec<ResolvedType>),
    /// Function type, including rest-parameter shape when known.
    Function(Vec<CallableParam>, Box<ResolvedType>),
    /// Value-level token for a source type, e.g. `Type[int]`.
    TypeToken(Box<ResolvedType>),
    /// Tuple type
    Tuple(Vec<ResolvedType>),
    /// Type variable (for generics)
    TypeVar(String),
    /// Self type (resolved to the implementing type in traits)
    SelfType,
    /// Internal reference type (borrowed `&T`).
    ///
    /// ## Notes
    /// - This is currently compiler-internal (not a user-spellable surface type).
    /// - It exists to model Rust interop semantics like `HashMap::get` returning `Option<&V>`.
    Ref(Box<ResolvedType>),
    /// Internal mutable reference type (borrowed `&mut T`).
    ///
    /// ## Notes
    /// - This is currently compiler-internal (not a user-spellable surface type).
    /// - It exists to preserve mutable Rust interop signatures through IR lowering.
    RefMut(Box<ResolvedType>),
    /// Rust import with a known canonical path (`crate::...` string), RFC 041.
    ///
    /// Lowers to backend `IrType::Unknown` until dedicated IR typing exists; provenance also lives on
    /// [`SymbolKind::RustItem`].
    RustPath(String),
    /// Call-site `_` placeholder in bracketed type arguments (RFC 054); resolved away before lowering.
    CallSiteInfer,
    /// Unknown/error type
    Unknown,
}

impl ResolvedType {
    /// Check if this is a Result type
    pub fn is_result(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result)
        )
    }

    /// Check if this is an Option type
    pub fn is_option(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option)
        )
    }

    /// Check if this is an anonymous union type.
    pub fn is_union(&self) -> bool {
        matches!(self, ResolvedType::Generic(name, _) if name == UNION_TYPE_NAME)
    }

    /// Get the normalized member list from `Union[...]`.
    pub fn union_members(&self) -> Option<&[ResolvedType]> {
        match self {
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => Some(args.as_slice()),
            _ => None,
        }
    }

    /// Get the Ok type from Result[T, E]
    pub fn result_ok_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Get the Err type from `Result[T, E]`.
    pub fn result_err_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && args.len() >= 2 =>
            {
                Some(&args[1])
            }
            _ => None,
        }
    }

    /// Get the inner type from `Option[T]`.
    pub fn option_inner_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Get the yielded element type from `Generator[T]`.
    pub fn generator_element_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Generator) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Return the canonical owned Incan `Iterator[T]` item type.
    pub(crate) fn iterator_item_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if traits::from_qualified_str(name) == Some(TraitId::Iterator) && args.len() == 1 =>
            {
                args.first()
            }
            _ => None,
        }
    }

    /// Return the item type accepted by the direct `zip(left, right)` builtin.
    ///
    /// The backend can currently adapt lists, frozen lists, and canonical source-owned iterators without broadening
    /// this builtin to every value accepted by ordinary `for` iteration. Unknown types remain accepted for diagnostic
    /// recovery; all other unsupported operands are rejected by the builtin call checker.
    pub(crate) fn builtin_zip_item_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Unknown => Some(self),
            ResolvedType::FrozenList(inner) => Some(inner),
            ResolvedType::Generic(name, args)
                if (name == surface_types::as_str(SurfaceTypeId::Vec)
                    || matches!(
                        collections::from_str(name),
                        Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                    ))
                    && args.len() == 1 =>
            {
                args.first()
            }
            _ => self.iterator_item_type(),
        }
    }
}

impl std::fmt::Display for ResolvedType {
    /// Format a resolved type using user-facing Incan type syntax.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvedType::Never => write!(f, "!"),
            ResolvedType::Int => write!(f, "int"),
            ResolvedType::Float => write!(f, "float"),
            ResolvedType::Numeric(id) => write!(f, "{}", numerics::as_str(*id)),
            ResolvedType::Bool => write!(f, "bool"),
            ResolvedType::Str => write!(f, "str"),
            ResolvedType::Bytes => write!(f, "bytes"),
            ResolvedType::FrozenStr => write!(f, "FrozenStr"),
            ResolvedType::FrozenBytes => write!(f, "FrozenBytes"),
            ResolvedType::FrozenList(elem) => write!(f, "FrozenList[{}]", elem),
            ResolvedType::FrozenDict(k, v) => write!(f, "FrozenDict[{}, {}]", k, v),
            ResolvedType::FrozenSet(elem) => write!(f, "FrozenSet[{}]", elem),
            ResolvedType::Unit => write!(f, "Unit"),
            ResolvedType::Named(name) => write!(f, "{}", name),
            ResolvedType::Generic(name, args) => {
                write!(f, "{}[", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, "]")
            }
            ResolvedType::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match p.kind {
                        ParamKind::Normal => write!(f, "{}", p.ty)?,
                        ParamKind::RestPositional => write!(f, "*{}", p.ty)?,
                        ParamKind::RestKeyword => write!(f, "**{}", p.ty)?,
                    }
                }
                write!(f, ") -> {}", ret)
            }
            ResolvedType::TypeToken(inner) => write!(f, "Type[{}]", inner),
            ResolvedType::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            ResolvedType::TypeVar(name) => write!(f, "{}", name),
            ResolvedType::SelfType => write!(f, "Self"),
            ResolvedType::Ref(inner) => write!(f, "&{}", inner),
            ResolvedType::RefMut(inner) => write!(f, "&mut {}", inner),
            ResolvedType::RustPath(path) => write!(f, "rust::{}", path),
            ResolvedType::CallSiteInfer => write!(f, "_"),
            ResolvedType::Unknown => write!(f, "?"),
        }
    }
}

/// Construct the canonical semantic form for an anonymous union.
///
/// This flattens nested unions, removes duplicates, sorts members by display for deterministic equality, and rewrites
/// `None`/`Unit`-containing unions through `Option[...]` as required by RFC 029.
pub fn union_ty(members: Vec<ResolvedType>) -> ResolvedType {
    let mut flattened = Vec::new();
    let mut contains_none = false;

    for member in members {
        match member {
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => flattened.extend(args),
            ResolvedType::Unit => contains_none = true,
            other => flattened.push(other),
        }
    }

    flattened.sort_by_key(|member| member.to_string());
    flattened.dedup();

    let inner = match flattened.as_slice() {
        [] => ResolvedType::Unit,
        [single] => single.clone(),
        _ => ResolvedType::Generic(UNION_TYPE_NAME.to_string(), flattened),
    };

    if contains_none {
        ResolvedType::Generic(collections::as_str(CollectionTypeId::Option).to_string(), vec![inner])
    } else {
        inner
    }
}

/// Convert AST Type to ResolvedType
/// Normalize type name to canonical form (uppercase for built-in generics)
fn normalize_type_name(name: &str) -> String {
    // Generic base normalization: prefer the canonical spelling from `incan_core` for all builtin
    // collection/generic-base types (and their aliases).
    if let Some(id) = collections::from_str(name) {
        return collections::as_str(id).to_string();
    }
    name.to_string()
}

/// Resolve `a::b::c` in type position when `a` is a `rust::` import binding (module or item).
fn resolve_qualified_rust_type_path(segments: &[String], symbols: &SymbolTable) -> ResolvedType {
    if segments.len() < 2 {
        return ResolvedType::Unknown;
    }
    let Some(root) = segments.first() else {
        return ResolvedType::Unknown;
    };
    let Some(id) = symbols.lookup(root) else {
        return ResolvedType::Unknown;
    };
    let Some(sym) = symbols.get(id) else {
        return ResolvedType::Unknown;
    };
    let SymbolKind::RustItem(info) = &sym.kind else {
        return ResolvedType::Unknown;
    };
    let mut path = info.path.clone();
    for seg in segments.iter().skip(1) {
        path.push_str("::");
        path.push_str(seg);
    }
    ResolvedType::RustPath(path)
}

/// Resolve an AST type annotation into the canonical semantic type representation.
pub fn resolve_type(ty: &Type, symbols: &SymbolTable) -> ResolvedType {
    resolve_type_with_rust_arg_renderer(ty, symbols, &render_resolved_type_as_rust_arg, &|_| {})
}

/// Resolve an AST type while allowing the typechecker to preserve provider identity inside opaque Rust applications.
pub(crate) fn resolve_type_with_rust_arg_renderer<F, G>(
    ty: &Type,
    symbols: &SymbolTable,
    render_rust_arg: &F,
    qualify_structured_rust_arg: &G,
) -> ResolvedType
where
    F: Fn(&ResolvedType) -> String,
    G: Fn(&mut ResolvedType),
{
    match ty {
        Type::Qualified(segments) => resolve_qualified_rust_type_path(segments, symbols),
        Type::Dotted(_) => ResolvedType::Unknown,
        Type::Simple(name) => {
            if let Some(id) = numerics::from_str(name.as_str()) {
                return match name.as_str() {
                    "int" => ResolvedType::Int,
                    "float" => ResolvedType::Float,
                    "bool" => ResolvedType::Bool,
                    _ => match id {
                        NumericTypeId::Bool => ResolvedType::Bool,
                        _ => ResolvedType::Numeric(id),
                    },
                };
            }
            if let Some(id) = stringlike::from_str(name.as_str()) {
                return match id {
                    StringLikeId::Str => ResolvedType::Str,
                    StringLikeId::Bytes => ResolvedType::Bytes,
                    StringLikeId::FrozenStr => ResolvedType::FrozenStr,
                    StringLikeId::FrozenBytes => ResolvedType::FrozenBytes,
                    // We currently treat f-strings as a regular string type at the type level.
                    StringLikeId::FString => ResolvedType::Str,
                };
            }
            if let Some(id) = collections::from_str(name.as_str()) {
                // `List`/`Dict`/... can appear in type position without parameters (e.g. `Tuple` as "any tuple").
                // Preserve it as a named type, but normalize to the canonical spelling from `incan_core`.
                return ResolvedType::Named(collections::as_str(id).to_string());
            }

            match name.as_str() {
                conventions::UNIT_TYPE_NAME | conventions::NONE_TYPE_NAME => ResolvedType::Unit,
                _ => {
                    if let Some(id) = symbols.lookup(name)
                        && let Some(sym) = symbols.get(id)
                        && let SymbolKind::RustItem(info) = &sym.kind
                    {
                        return match info.binding {
                            RustImportBindingKind::CrateRoot => ResolvedType::Unknown,
                            RustImportBindingKind::RootedPath | RustImportBindingKind::FromImport => {
                                ResolvedType::RustPath(info.path.clone())
                            }
                        };
                    }
                    // Check if it's a known type
                    if symbols.lookup(name).is_some() {
                        ResolvedType::Named(name.clone())
                    } else {
                        // Could be a type variable
                        ResolvedType::TypeVar(name.clone())
                    }
                }
            }
        }
        Type::ConstrainedPrimitive(name, _) => {
            let base = Type::Simple(name.clone());
            resolve_type_with_rust_arg_renderer(&base, symbols, render_rust_arg, qualify_structured_rust_arg)
        }
        Type::Generic(name, args) => {
            let mut resolved_args: Vec<_> = args
                .iter()
                .map(|arg| {
                    resolve_type_with_rust_arg_renderer(
                        &arg.node,
                        symbols,
                        render_rust_arg,
                        qualify_structured_rust_arg,
                    )
                })
                .collect();
            if let Some(id) = symbols.lookup(name)
                && let Some(symbol) = symbols.get(id)
                && let SymbolKind::RustItem(info) = &symbol.kind
                && !matches!(info.binding, RustImportBindingKind::CrateRoot)
            {
                if rust_generic_preserves_nominal_ir(info) {
                    for arg in &mut resolved_args {
                        qualify_structured_rust_arg(arg);
                    }
                } else {
                    let rendered_args = resolved_args.iter().map(render_rust_arg).collect::<Vec<_>>().join(", ");
                    return ResolvedType::RustPath(format!("{}<{rendered_args}>", info.path));
                }
            }
            if name == "Type" {
                return ResolvedType::TypeToken(Box::new(
                    resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown),
                ));
            }
            // Normalize type name for built-in generics (aliases → canonical spellings).
            let id = collections::from_str(name.as_str());
            let normalized_name = id
                .map(|id| collections::as_str(id).to_string())
                .unwrap_or_else(|| normalize_type_name(name));

            if normalized_name == UNION_TYPE_NAME {
                return union_ty(resolved_args);
            }

            match id {
                Some(CollectionTypeId::FrozenList) => {
                    let elem = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenList(Box::new(elem))
                }
                Some(CollectionTypeId::FrozenSet) => {
                    let elem = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenSet(Box::new(elem))
                }
                Some(CollectionTypeId::FrozenDict) => {
                    let k = resolved_args.first().cloned().unwrap_or(ResolvedType::Unknown);
                    let v = resolved_args.get(1).cloned().unwrap_or(ResolvedType::Unknown);
                    ResolvedType::FrozenDict(Box::new(k), Box::new(v))
                }
                _ => ResolvedType::Generic(normalized_name, resolved_args),
            }
        }
        Type::DottedGeneric(segments, args) => ResolvedType::Generic(
            segments.join("."),
            args.iter()
                .map(|arg| {
                    resolve_type_with_rust_arg_renderer(
                        &arg.node,
                        symbols,
                        render_rust_arg,
                        qualify_structured_rust_arg,
                    )
                })
                .collect(),
        ),
        Type::IntLiteral(value) => ResolvedType::TypeVar(value.repr.clone()),
        Type::Function(params, ret) => {
            let resolved_params: Vec<_> = params
                .iter()
                .map(|param| {
                    CallableParam::positional(resolve_type_with_rust_arg_renderer(
                        &param.node,
                        symbols,
                        render_rust_arg,
                        qualify_structured_rust_arg,
                    ))
                })
                .collect();
            let resolved_ret =
                resolve_type_with_rust_arg_renderer(&ret.node, symbols, render_rust_arg, qualify_structured_rust_arg);
            ResolvedType::Function(resolved_params, Box::new(resolved_ret))
        }
        Type::Ref(inner) => ResolvedType::Ref(Box::new(resolve_type_with_rust_arg_renderer(
            &inner.node,
            symbols,
            render_rust_arg,
            qualify_structured_rust_arg,
        ))),
        Type::RefMut(inner) => ResolvedType::RefMut(Box::new(resolve_type_with_rust_arg_renderer(
            &inner.node,
            symbols,
            render_rust_arg,
            qualify_structured_rust_arg,
        ))),
        Type::Unit => ResolvedType::Unit,
        Type::Tuple(elems) => {
            let resolved_elems: Vec<_> = elems
                .iter()
                .map(|element| {
                    resolve_type_with_rust_arg_renderer(
                        &element.node,
                        symbols,
                        render_rust_arg,
                        qualify_structured_rust_arg,
                    )
                })
                .collect();
            ResolvedType::Tuple(resolved_elems)
        }
        Type::SelfType => ResolvedType::SelfType,
        Type::Infer => ResolvedType::CallSiteInfer,
    }
}

/// Return whether one compiler-owned Rust generic keeps its structured semantic IR representation.
///
/// `Box[T]` participates in receiver/method semantics that consume `ResolvedType::Generic`. Other Rust applications,
/// including standard-library types such as `PhantomData[T]`, retain their complete canonical display for declaration
/// emission across compiled-library boundaries.
fn rust_generic_preserves_nominal_ir(info: &RustItemInfo) -> bool {
    matches!(
        info.path.as_str(),
        "std::boxed::Box" | "alloc::boxed::Box" | "rust::std::boxed::Box" | "rust::alloc::boxed::Box"
    )
}

/// Render one checked type as a Rust generic argument without retaining source-only collection spellings.
pub(crate) fn render_resolved_type_as_rust_arg(ty: &ResolvedType) -> String {
    match ty {
        ResolvedType::Never => "!".to_string(),
        ResolvedType::Int => "i64".to_string(),
        ResolvedType::Float => "f64".to_string(),
        ResolvedType::Numeric(id) => numerics::rust_name(*id).to_string(),
        ResolvedType::Bool => "bool".to_string(),
        ResolvedType::Str => "String".to_string(),
        ResolvedType::Bytes => "Vec<u8>".to_string(),
        ResolvedType::FrozenStr => "incan_stdlib::frozen::FrozenStr".to_string(),
        ResolvedType::FrozenBytes => "incan_stdlib::frozen::FrozenBytes".to_string(),
        ResolvedType::FrozenList(inner) => format!(
            "incan_stdlib::frozen::FrozenList<{}>",
            render_resolved_type_as_rust_arg(inner)
        ),
        ResolvedType::FrozenDict(key, value) => format!(
            "incan_stdlib::frozen::FrozenDict<{}, {}>",
            render_resolved_type_as_rust_arg(key),
            render_resolved_type_as_rust_arg(value)
        ),
        ResolvedType::FrozenSet(inner) => format!(
            "incan_stdlib::frozen::FrozenSet<{}>",
            render_resolved_type_as_rust_arg(inner)
        ),
        ResolvedType::Unit => "()".to_string(),
        ResolvedType::Named(name) => name
            .strip_prefix("pub::")
            .map_or_else(|| name.clone(), ToString::to_string),
        ResolvedType::TypeVar(name) => name.clone(),
        ResolvedType::Generic(name, args) => {
            if collections::from_str(name.as_str()) == Some(CollectionTypeId::Tuple) {
                return render_rust_tuple(args);
            }
            let base = match collections::from_str(name.as_str()) {
                Some(CollectionTypeId::List) => "Vec",
                Some(CollectionTypeId::Dict) => "std::collections::HashMap",
                Some(CollectionTypeId::Set) => "std::collections::HashSet",
                Some(CollectionTypeId::Option) => "Option",
                Some(CollectionTypeId::Result) => "Result",
                Some(CollectionTypeId::FrozenList) => "incan_stdlib::frozen::FrozenList",
                Some(CollectionTypeId::FrozenDict) => "incan_stdlib::frozen::FrozenDict",
                Some(CollectionTypeId::FrozenSet) => "incan_stdlib::frozen::FrozenSet",
                Some(CollectionTypeId::Generator) => "incan_stdlib::iter::Generator",
                Some(CollectionTypeId::Tuple) | None => name,
            };
            let rendered_args = args
                .iter()
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}<{rendered_args}>")
        }
        ResolvedType::Function(params, ret) => {
            let params = params
                .iter()
                .map(|param| render_resolved_type_as_rust_arg(&param.ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn({params}) -> {}", render_resolved_type_as_rust_arg(ret))
        }
        ResolvedType::TypeToken(inner) => {
            format!(
                "incan_stdlib::reflection::TypeToken<{}>",
                render_resolved_type_as_rust_arg(inner)
            )
        }
        ResolvedType::Tuple(items) => render_rust_tuple(items),
        ResolvedType::SelfType => "Self".to_string(),
        ResolvedType::Ref(inner) => format!("&{}", render_resolved_type_as_rust_arg(inner)),
        ResolvedType::RefMut(inner) => format!("&mut {}", render_resolved_type_as_rust_arg(inner)),
        ResolvedType::RustPath(path) => path.clone(),
        ResolvedType::CallSiteInfer | ResolvedType::Unknown => "_".to_string(),
    }
}

/// Render one checked tuple using Rust's single-element trailing-comma rule.
fn render_rust_tuple(items: &[ResolvedType]) -> String {
    let rendered = items.iter().map(render_resolved_type_as_rust_arg).collect::<Vec<_>>();
    match rendered.as_slice() {
        [only] => format!("({only},)"),
        _ => format!("({})", rendered.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Span, Spanned, Type};

    /// Builtins that are checker-recognized without a physical root symbol still receive the one registry identity.
    #[test]
    fn every_builtin_function_registry_entry_has_a_canonical_identity() -> Result<(), String> {
        let table = SymbolTable::new();
        for entry in builtins::BUILTIN_FUNCTIONS {
            let identity = table
                .builtin_function_identity(entry.id)
                .ok_or_else(|| format!("missing canonical identity for builtin `{}`", entry.canonical))?;
            assert_eq!(identity.namespace, SymbolNamespace::OrdinaryLexical);
            assert_eq!(identity.origin, SymbolOrigin::Builtin);
            assert_eq!(identity.declaration_name, entry.canonical);
            assert_eq!(identity.kind, SemanticSourceTargetKind::Builtin);
            assert_eq!(identity.scope_discriminant, None);
            assert_eq!(identity.declaration_span, HirSourceSpan::new(0, 0));
        }
        Ok(())
    }

    #[test]
    fn compiled_package_declarations_are_minted_in_the_package_origin() -> Result<(), String> {
        let mut table = SymbolTable::new();
        table.set_package_identity(Some("incan_stdlib_system".to_string()));
        table.set_module_path(vec!["fs".to_string(), "path".to_string()]);
        let id = table.define_with_target_kind(
            Symbol {
                name: "helper".to_string(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: ResolvedType::Int,
                    is_mutable: false,
                    is_used: false,
                }),
                span: Span::new(10, 20),
                scope: 0,
            },
            SemanticSourceTargetKind::Function,
        );
        let identity = table.identity_of(id).ok_or("missing package-owned identity")?;

        assert_eq!(
            identity.origin,
            SymbolOrigin::Package {
                library: "incan_stdlib_system".to_string(),
                module_path: vec!["fs".to_string(), "path".to_string()],
            }
        );
        assert_eq!(identity.declaration_name, "helper");
        assert_eq!(identity.kind, SemanticSourceTargetKind::Function);
        Ok(())
    }

    #[test]
    fn shared_binding_registration_preserves_the_first_site_and_reports_the_collision() {
        let mut bindings = HashMap::new();
        let first = Span::new(4, 8);
        let second = Span::new(20, 24);

        assert_eq!(
            register_binding(&mut bindings, "name".to_string(), first),
            BindingRegistration::Registered
        );
        assert_eq!(
            register_binding(&mut bindings, "name".to_string(), second),
            BindingRegistration::Collision { existing: first }
        );
        assert_eq!(bindings.get("name"), Some(&first));
    }

    #[test]
    fn dependency_interface_bindings_are_temporary_and_transactional() -> Result<(), String> {
        let mut table = SymbolTable::new();
        let builtin_len = table.lookup("len").ok_or("missing builtin len")?;
        let builtin_len_identity = table
            .builtin_function_identity(BuiltinFnId::Len)
            .ok_or("missing canonical builtin len identity")?;
        table.begin_dependency_interface_bindings();
        let external = table.define_import_binding(
            Symbol {
                name: "External".to_string(),
                kind: SymbolKind::Type(TypeInfo::TypeAlias),
                span: Span::new(4, 12),
                scope: 0,
            },
            None,
        );
        assert_eq!(table.lookup("External"), Some(external));

        let interface_len = table.define_import_binding(
            Symbol {
                name: "len".to_string(),
                kind: SymbolKind::Module(ModuleInfo {
                    path: vec!["dependency".to_string()],
                    is_python: false,
                }),
                span: Span::new(20, 23),
                scope: 0,
            },
            None,
        );
        assert_eq!(
            table.lookup("len"),
            Some(interface_len),
            "the active dependency interface must resolve before root builtins"
        );
        assert_eq!(
            table.builtin_function_identity(BuiltinFnId::Len),
            Some(builtin_len_identity),
            "explicit builtin resolution must bypass the active dependency view"
        );

        table.begin_current_scope_binding_transaction();
        let replacement = table.define_import_binding(
            Symbol {
                name: "External".to_string(),
                kind: SymbolKind::Type(TypeInfo::Builtin),
                span: Span::new(30, 38),
                scope: 0,
            },
            None,
        );
        assert_eq!(table.lookup("External"), Some(replacement));
        let previous = table.finish_current_scope_binding_transaction();
        table.restore_current_scope_binding("External", previous.get("External").copied().flatten());
        assert_eq!(
            table.lookup("External"),
            Some(external),
            "the dependency-local import transaction must restore the interface view"
        );

        table.finish_dependency_interface_bindings();
        assert_eq!(table.lookup("External"), None);
        assert_eq!(table.lookup("len"), Some(builtin_len));
        assert!(
            table.get(external).is_some(),
            "historical interface metadata remains inspectable"
        );
        Ok(())
    }

    #[test]
    fn test_overload_emitted_name_validation_requires_generated_hash_suffix() {
        let emitted = overload_emitted_name("cast", 0xd28281f54a5b9ea6);

        assert!(is_overload_emitted_name(&emitted));
        assert!(!is_overload_emitted_name("cast_overload_suffix"));
        assert!(is_overload_emitted_name("__incan_overload_d28281f54a5b9ea6"));
    }

    #[test]
    fn test_scope_lookup() {
        let mut table = SymbolTable::new();

        // Define in global scope
        table.define(Symbol {
            name: "x".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        // Enter a new scope
        table.enter_scope(ScopeKind::Function);

        // Should still find x
        assert!(table.lookup("x").is_some());

        // Define y in inner scope
        table.define(Symbol {
            name: "y".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        assert!(table.lookup("y").is_some());

        // Exit scope
        table.exit_scope();

        // x still visible, y not
        assert!(table.lookup("x").is_some());
        assert!(table.lookup("y").is_none());
    }

    #[test]
    fn test_result_type_helpers() {
        let result_type = ResolvedType::Generic(
            "Result".to_string(),
            vec![ResolvedType::Int, ResolvedType::Named("AppError".to_string())],
        );

        assert!(result_type.is_result());
        assert_eq!(result_type.result_ok_type(), Some(&ResolvedType::Int));
        assert_eq!(
            result_type.result_err_type(),
            Some(&ResolvedType::Named("AppError".to_string()))
        );
    }

    #[test]
    fn test_function_type_resolution() {
        let symbols = SymbolTable::new();

        // The parser desugars Callable[(), int] → Type::Function([], int).
        // Verify that resolve_type handles the desugared form correctly.

        // () -> int (zero params)
        let fn_zero = Type::Function(
            vec![],
            Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_zero, &symbols);
        assert_eq!(ty, ResolvedType::Function(vec![], Box::new(ResolvedType::Int)));

        // (int) -> int (single param)
        let fn_single = Type::Function(
            vec![Spanned::new(Type::Simple("int".to_string()), Span::default())],
            Box::new(Spanned::new(Type::Simple("int".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_single, &symbols);
        assert_eq!(
            ty,
            ResolvedType::Function(
                vec![CallableParam::positional(ResolvedType::Int)],
                Box::new(ResolvedType::Int),
            )
        );

        // (int, str) -> bool (multi param)
        let fn_multi = Type::Function(
            vec![
                Spanned::new(Type::Simple("int".to_string()), Span::default()),
                Spanned::new(Type::Simple("str".to_string()), Span::default()),
            ],
            Box::new(Spanned::new(Type::Simple("bool".to_string()), Span::default())),
        );
        let ty = resolve_type(&fn_multi, &symbols);
        assert_eq!(
            ty,
            ResolvedType::Function(
                vec![
                    CallableParam::positional(ResolvedType::Int),
                    CallableParam::positional(ResolvedType::Str),
                ],
                Box::new(ResolvedType::Bool),
            )
        );
    }

    #[test]
    fn resolve_type_preserves_existing_int_float_bool_names() {
        let symbols = SymbolTable::new();

        assert_eq!(
            resolve_type(&Type::Simple("int".to_string()), &symbols),
            ResolvedType::Int
        );
        assert_eq!(
            resolve_type(&Type::Simple("float".to_string()), &symbols),
            ResolvedType::Float
        );
        assert_eq!(
            resolve_type(&Type::Simple("bool".to_string()), &symbols),
            ResolvedType::Bool
        );
    }

    #[test]
    fn resolve_type_maps_exact_width_and_alias_numeric_names() {
        let symbols = SymbolTable::new();

        assert_eq!(
            resolve_type(&Type::Simple("i64".to_string()), &symbols),
            ResolvedType::Numeric(NumericTypeId::I64)
        );
        assert_eq!(
            resolve_type(&Type::Simple("integer".to_string()), &symbols),
            ResolvedType::Numeric(NumericTypeId::I32)
        );
        assert_eq!(
            resolve_type(&Type::Simple("byte".to_string()), &symbols),
            ResolvedType::Numeric(NumericTypeId::U8)
        );
        assert_eq!(
            resolve_type(&Type::Simple("real".to_string()), &symbols),
            ResolvedType::Numeric(NumericTypeId::F32)
        );
        assert_eq!(
            resolve_type(&Type::Simple("double".to_string()), &symbols),
            ResolvedType::Numeric(NumericTypeId::F64)
        );
    }

    #[test]
    fn resolve_type_qualified_rust_module_item() {
        let mut table = SymbolTable::new();
        table.define(Symbol {
            name: "proto_type".to_string(),
            kind: SymbolKind::RustItem(RustItemInfo {
                crate_name: "substrait".to_string(),
                path: "substrait::proto::type".to_string(),
                binding: RustImportBindingKind::FromImport,
                metadata: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        let ty = Type::Qualified(vec!["proto_type".to_string(), "Binary".to_string()]);
        let r = resolve_type(&ty, &table);
        assert_eq!(r, ResolvedType::RustPath("substrait::proto::type::Binary".to_string()));
    }

    #[test]
    fn resolve_type_canonicalizes_generic_rust_import_and_nested_collection() {
        let mut table = SymbolTable::new();
        for (name, path) in [
            ("RustEnvelope", "rust_shadow::Envelope"),
            ("RustToken", "rust_shadow::Token"),
        ] {
            table.define(Symbol {
                name: name.to_string(),
                kind: SymbolKind::RustItem(RustItemInfo {
                    crate_name: "rust_shadow".to_string(),
                    path: path.to_string(),
                    binding: RustImportBindingKind::FromImport,
                    metadata: None,
                }),
                span: Span::default(),
                scope: 0,
            });
        }
        let ty = Type::Generic(
            "RustEnvelope".to_string(),
            vec![Spanned::new(
                Type::Generic(
                    "list".to_string(),
                    vec![Spanned::new(Type::Simple("RustToken".to_string()), Span::default())],
                ),
                Span::default(),
            )],
        );

        assert_eq!(
            resolve_type(&ty, &table),
            ResolvedType::RustPath("rust_shadow::Envelope<Vec<rust_shadow::Token>>".to_string())
        );
    }

    #[test]
    fn resolve_type_retains_non_nominal_standard_generic_path() {
        let mut table = SymbolTable::new();
        table.define(Symbol {
            name: "PhantomData".to_string(),
            kind: SymbolKind::RustItem(RustItemInfo {
                crate_name: "std".to_string(),
                path: "std::marker::PhantomData".to_string(),
                binding: RustImportBindingKind::FromImport,
                metadata: None,
            }),
            span: Span::default(),
            scope: 0,
        });
        table.define(Symbol {
            name: "Payload".to_string(),
            kind: SymbolKind::Type(TypeInfo::TypeAlias),
            span: Span::default(),
            scope: 0,
        });
        let ty = Type::Generic(
            "PhantomData".to_string(),
            vec![Spanned::new(Type::Simple("Payload".to_string()), Span::default())],
        );

        assert_eq!(
            resolve_type(&ty, &table),
            ResolvedType::RustPath("std::marker::PhantomData<Payload>".to_string())
        );
    }

    #[test]
    fn field_surface_type_name_uses_canonical_incan_collection_spelling() {
        let source = Type::Generic(
            "Dict".to_string(),
            vec![
                Spanned::new(Type::Simple("str".to_string()), Span::default()),
                Spanned::new(Type::Simple("TelemetryValue".to_string()), Span::default()),
            ],
        );
        let resolved = ResolvedType::Generic(
            "dict".to_string(),
            vec![ResolvedType::Str, ResolvedType::Named("TelemetryValue".to_string())],
        );

        assert_eq!(field_surface_type_name(&source, &resolved), "dict[str, TelemetryValue]");

        let source = Type::Generic(
            "List".to_string(),
            vec![Spanned::new(Type::Simple("int".to_string()), Span::default())],
        );
        let resolved = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Int]);

        assert_eq!(field_surface_type_name(&source, &resolved), "list[int]");
    }

    #[test]
    fn field_surface_type_name_preserves_authored_spelling_for_nested_rust_types() {
        let source = Type::Generic(
            "Box".to_string(),
            vec![Spanned::new(
                Type::Generic(
                    "RustEnvelope".to_string(),
                    vec![Spanned::new(
                        Type::Generic(
                            "list".to_string(),
                            vec![Spanned::new(Type::Simple("RustToken".to_string()), Span::default())],
                        ),
                        Span::default(),
                    )],
                ),
                Span::default(),
            )],
        );
        let resolved = ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::RustPath(
                "rust_shadow::Envelope<Vec<rust_shadow::Token>>".to_string(),
            )],
        );

        assert_eq!(
            field_surface_type_name(&source, &resolved),
            "Box[RustEnvelope[list[RustToken]]]"
        );
    }

    #[test]
    fn resolved_iterator_item_type_uses_canonical_trait_identity_issue950_953() {
        let qualified = ResolvedType::Generic(
            "stdlib_core::__incan_std::derives::collection::Iterator".to_string(),
            vec![ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str])],
        );
        let similarly_named = ResolvedType::Generic("RecordIterator".to_string(), vec![ResolvedType::Int]);

        assert_eq!(
            qualified.iterator_item_type(),
            Some(&ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str]))
        );
        assert_eq!(similarly_named.iterator_item_type(), None);
    }
}
