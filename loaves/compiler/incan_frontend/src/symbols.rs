//! Symbol table and scope management for Incan
//!
//! Tracks all named entities (types, functions, variables, traits) and their scopes.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

use crate::ast::{ParamKind, Receiver, Span, Type, TypeConstraintKey};
use crate::nominal_inventory::InventoryRevision;
use incan_lang::interop::RustItemMetadata;
use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::conventions;
use incan_lang::lang::surface::constructors;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_lang::lang::traits;
use incan_lang::lang::traits::TraitId;
use incan_lang::lang::types::collections;
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use incan_lang::lang::types::numerics::NumericTypeId;
use incan_lang::lang::types::stringlike;
use incan_lang::lang::types::stringlike::StringLikeId;
use incan_lang::lang::types::{BuiltinTypeName, builtin_type_name};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

mod definitions;
mod lookup;
mod resolved_types;
mod symbol_kinds;
mod table_setup;
mod type_infos;
mod type_resolution;

pub use resolved_types::*;
pub use symbol_kinds::*;
pub use type_infos::*;
pub use type_resolution::*;

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
pub fn canonical_builtin_function_identity(builtin: BuiltinFnId) -> Option<CanonicalSymbolId> {
    builtins::BUILTIN_FUNCTIONS
        .iter()
        .find(|entry| entry.id == builtin)
        .map(|entry| canonical_builtin_identity(entry.canonical))
}

/// Project the canonical identity of a builtin `Option` or `Result` constructor, the same identity symbol registration
/// records for it, so checked-IR consumers can name a constructor the checker selected without reading source
/// spelling. Source bindings and aliases cannot authorize a synthetic Option pattern through this finite registry.
pub(crate) fn canonical_builtin_constructor_identity(constructor: constructors::ConstructorId) -> CanonicalSymbolId {
    canonical_builtin_identity(constructors::as_str(constructor))
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

/// Build the canonical target of a member selected from a compiler-owned typed surface registry.
///
/// Builtin members have no source declaration span. The owner-qualified declaration key preserves their owner
/// across checked-IR boundaries; callers must select the owner and member semantically, never from source spelling
/// alone.
pub(crate) fn canonical_builtin_member_identity(owner: &str, member: &str) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::Member,
        origin: SymbolOrigin::Builtin,
        declaration_name: format!("{owner}.{member}"),
        kind: SemanticSourceTargetKind::Method,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    }
}

/// Canonical semantic name for anonymous union types (RFC 029).
pub const UNION_TYPE_NAME: &str = incan_lang::lang::types::UNION_TYPE_NAME;

/// Prefix of the checker's spelling of a union member nominal that names its declaring module (#1796).
const MODULE_QUALIFIED_NOMINAL_PREFIX: &str = "mod::";

/// Spell a nominal type by the module that declares it, as a union member names it when several modules of one check
/// declare that name (#1796).
///
/// A union member that names a model, class, enum or newtype is the declaration the name resolves to where the union
/// is written, but a bare spelling cannot tell two modules' `Product` apart. The spelling is
/// `mod::<dotted module path>::<declaration name>`, with an empty module path for a module checked without one; it
/// never comes from source, and it displays as the dotted path users write.
pub fn module_qualified_nominal_name(module_path: &[String], declaration_name: &str) -> String {
    format!(
        "{MODULE_QUALIFIED_NOMINAL_PREFIX}{}::{declaration_name}",
        module_path.join(".")
    )
}

/// Split a [`module_qualified_nominal_name`] spelling into its declaring module path and declaration name.
///
/// Returns `None` for every other spelling.
pub fn split_module_qualified_nominal_name(name: &str) -> Option<(Vec<String>, &str)> {
    let (module, declaration_name) = name.strip_prefix(MODULE_QUALIFIED_NOMINAL_PREFIX)?.rsplit_once("::")?;
    if declaration_name.is_empty() {
        return None;
    }
    let module_path = if module.is_empty() {
        Vec::new()
    } else {
        module.split('.').map(str::to_string).collect()
    };
    Some((module_path, declaration_name))
}

/// Separator used in generated Rust symbols for source overload implementations.
const OVERLOAD_EMITTED_NAME_SEPARATOR: &str = "_overload_";

/// Build the generated Rust symbol name for one overload implementation.
pub fn overload_emitted_name(source_name: &str, hash: u64) -> String {
    format!("{source_name}{OVERLOAD_EMITTED_NAME_SEPARATOR}{hash:016x}")
}

/// Build the deterministic Rust symbol name for one same-name source overload.
pub fn overloaded_function_emitted_name(source_name: &str, info: &FunctionInfo) -> String {
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

/// An existing lexical scope suspended between declaration preparation and statement checking. The owning symbol
/// table retains its bindings and canonical identities; consuming the handle never creates or rebinds a symbol.
#[must_use]
pub(crate) struct PreparedScope(usize);

/// Symbol table managing all named entities
#[derive(Debug)]
pub struct SymbolTable {
    /// Mutation witness for module types and their lexical shadows, independent of ordinary local values.
    nominal_revision: InventoryRevision,
    /// Module type spellings whose shadowing can change a nominal or alias contribution.
    nominal_binding_names: HashSet<String>,
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

impl Default for SymbolTable {
    /// Construct a usable root scope with builtin symbols, matching [`SymbolTable::new`]. An empty scope vector is
    /// not a valid table: lexical lookup and registration require the current root scope to exist.
    fn default() -> Self {
        Self::new()
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

#[cfg(test)]
mod tests;
