//! Type checker for the Incan programming language.
//!
//! Validates types, mutability, trait conformance, and error-handling semantics for a parsed Incan program. The checker
//! runs in two passes over the AST and populates a [`SymbolTable`] with resolved type information.
//!
//! ## Notes
//!
//! - **Two-pass model**: The first pass ([`TypeChecker::check_program`]) collects all type and function declarations
//!   into the symbol table. The second pass validates bodies, expressions, and cross-references.
//! - **Import handling**: Call [`TypeChecker::import_module`] or use [`TypeChecker::check_with_imports`] to
//!   pre-populate symbols from dependency modules before checking the main module.
//! - **Error accumulation**: Errors are collected (not fatal) so the checker can report as many issues as possible in a
//!   single run.
//!
//! ## What is validated
//!
//! - All referenced types and symbols are known
//! - Type compatibility at assignments, calls, returns
//! - Mutability rules (`mut` vs immutable bindings)
//! - `?` operator only on `Result` types with compatible error types
//! - Trait conformance for `with` clauses and `@derive` decorators
//! - Newtype method signatures
//! - Match exhaustiveness for enums, `Result`, and `Option`
//!
//! ## Examples
//!
//! ```ignore
//! use incan_frontend::{parser, typechecker};
//!
//! let source = r#"
//! def greet(name: str) -> str:
//!     return f"Hello, {name}!"
//! "#;
//!
//! let ast = parser::parse(source)?;
//! let mut checker = typechecker::TypeChecker::new();
//! checker.check_program(&ast)?;
//! ```
//!
//! ## See also
//!
//! - [`symbols`](super::symbols) – symbol table and scope management
//! - [`diagnostics`](super::diagnostics) – error types and pretty printing

mod body_check;
mod body_ir_classes;
mod capturing_callables;
mod check_decl;
mod check_work;
pub(crate) use body_check::{BodyIrOracleInputs, CheckedBodyInputError, CheckedBodyView};
pub use check_work::CheckWork;
mod check_expr;
mod check_stmt;
mod collect;
mod collection_annotations;
mod const_eval;
mod declaration_views;
mod decorated_method_receivers;
mod derive_contract;
mod derive_requirements;
mod diagnostic_values;
mod for_item_taking;
mod hash_key_inference;
mod helpers;
mod iteration_borrows;
mod json_member_requirements;
mod lifetime_contracts;
mod mut_arguments;
mod mut_marker;
mod native_newtype_facts;
mod native_storage;
mod nominal_identities;
mod nominal_type_param_bounds;
mod open_native_owners;
mod reachability;
mod receiver_change;
mod reserved_names;
mod source_signatures;
pub use declaration_views::{CheckEnvironmentView, CheckedBindingView, DeclarationView, VisibilityPoint};
mod trait_bound_relations;
mod type_info;
pub(crate) use type_info::{ClassLayoutInfo, RustCheckedCall};
pub(crate) use validate_rust_module::is_trivial_body;
mod union_member_identity;
mod validate_rust_module;
mod web_routes;

pub use const_eval::ConstValue;
pub use type_info::{
    BodyIrIterationHooksInfo, BodyIrPropertyBindingInfo, BodyIrProtocolCallInfo, ClassFieldDefaultInfo,
    semantic_type_from_resolved,
};
pub use type_info::{
    CAbiInteropArtifacts, CAbiOutputSlot, CAbiSpanKind, CBindingBuffer, CBindingDescriptor, CBindingEnum,
    CBindingEnumVariant, CBindingFacade, CBindingFunctionCall, CBindingOutcome, CBindingParameter, CBindingRawCall,
    CBindingRawCallOwner, CBindingResource, CBindingStruct, CBindingStructField, CBindingSymbol, CBindingType,
    COutputMode, CResourceAccess, CapabilityDeclarationInfo, CheckedImportBindings, CheckedSourceBinding,
    ComputedPropertyAccessInfo, DecoratedFunctionBindingInfo, DecoratedMethodBindingInfo, ErrorMessageDisplay,
    FixedUnpackPlan, FunctionBindingInfo, IdentKind, ImportedRegistryDefinitionInfo, MethodDecoratorReceiverRole,
    MethodDecoratorReceiverSlot, PartialProjectionInfo, PartialProjectionPreset, PartialProjectionTargetKind,
    ProtocolIterationInfo, ProviderOperationDeclarationInfo, QualifiedTypeReferenceInfo, RegistryArtifacts,
    RegistryDefinitionInfo, RegistryDescriptionRegistry, RegistryExplicitEntryInfo, ResolvedMethodCall,
    ResolvedMethodDispatch, ResolvedOperatorCall, ResolvedOperatorKind, RustArgCoercionInfo, RustArgCoercionKind,
    SourceTargetInfo, StandardRegistryTypes, TypeCheckInfo, ValidatedNewtypeCoercionInfo, ValidatedNewtypeCoercionMode,
    ValidatedNewtypeCoercionStep, c_binding_descriptor_identity,
};

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::ast::*;
use crate::diagnostics::{CompileError, ErrorKind, errors};
use crate::loaf_index::{LoafIdentity, LoafIndex};
use crate::module::{
    ExportedSymbol, canonicalize_library_declaration_module_path, canonicalize_source_module_segments,
    exported_symbols, logical_source_import_candidates,
};
use crate::nominal_inventory::{InventoryRevision, NominalInventory};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::surface_semantics::SurfaceContext;
use crate::symbols::*;
use helpers::{collection_name, collection_type_id, render_resolved_type_as_rust_arg, stringlike_type_id};
use incan_lang::interop::{
    RUST_NEVER_TYPE_DISPLAY, RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustPayloadCarrier,
    RustTypeShape, render_rust_type_shape_path, rust_display_is_owned_string, split_top_level_rust_args,
    strip_rust_borrow_lifetimes,
};
use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::c_abi;
use incan_lang::lang::conventions;
use incan_lang::lang::decorators::{self as core_decorators, DecoratorId};
use incan_lang::lang::derives::{self as builtin_derives, DeriveId};
use incan_lang::lang::errors as runtime_errors;
use incan_lang::lang::standard_packages::StandardPackageNamespacePolicy;
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::functions::SurfaceFnId;
use incan_lang::lang::surface::types as surface_types;
use incan_lang::lang::surface::types::{SurfaceTypeId, SurfaceTypeKind};
use incan_lang::lang::trait_capabilities;
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics::{self, NumericTypeId};
use incan_lang::lang::types::stringlike::StringLikeId;
use incan_lang::numeric_values::numeric_type_losslessly_widens_to;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};

/// The kind of loop enclosing a `break` or `continue`, which decides whether `break` may yield a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopContextKind {
    /// A statement-form loop (`for`, `while`, or `loop:`) where `break` cannot yield a value.
    Statement,
    /// An expression-form `loop:` whose `break` statements must unify to one result type.
    Expression,
}

/// Semantic state for the innermost loop currently being type-checked.
///
/// Each active loop records whether it is statement- or expression-oriented, any contextual expected type for
/// `break <value>`, and the concrete break result types seen while checking the body.
#[derive(Debug, Clone)]
pub struct LoopContext {
    /// Whether this loop is a statement loop or a value-producing `loop:` expression.
    pub kind: LoopContextKind,
    /// Type that outer context expects this loop expression to produce, when known.
    pub expected_break_ty: Option<ResolvedType>,
    /// Types observed from `break` statements that contribute to the loop result.
    pub break_types: Vec<(ResolvedType, Span)>,
    /// Symbol-table scope of the loop body: a binding held by an outer scope outlives each pass of the loop.
    pub scope: usize,
    /// How many statement blocks surround the loop; its body is the next one, so an assignment in a block deeper than
    /// this runs in every pass of the loop that reaches it.
    pub block_depth: usize,
    /// The loop's original-source range and expansion context; exits compare this complete identity.
    pub span: Span,
}

/// Resolved target for a source-level `type Alias = Target` declaration.
#[derive(Debug, Clone)]
pub struct TypeAliasTarget {
    pub type_params: Vec<String>,
    pub target: ResolvedType,
    /// Nominal leaves accepted in the alias's declaring scope. Importing the alias retains these owners even when
    /// the consumer never imports the member types themselves.
    pub nominal_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
}

/// Source member categories registered through the RFC 120 member-name authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberBindingKind {
    Field,
    Method,
    Property,
    MethodAlias,
    MethodPartial,
    Variant,
    VariantAlias,
}

/// Receiver surface that owns a source member spelling.
///
/// Type-owned methods have no instance receiver and are selected only through a type expression. Fields, properties,
/// and receiver-bearing methods are selected through an instance. Keeping those surfaces distinct lets a model expose
/// `TimeDelta.days(value)` while retaining `delta.days` as ordinary stored data without weakening collision checks
/// within either surface. Within the instance surface, Rust and Incan also distinguish a stored field selection
/// (`value.name`) from an authored method call (`value.name()`); the collision registry preserves that distinction
/// explicitly when it encounters those two declaration kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MemberBindingSurface {
    Type,
    Instance,
}

impl MemberBindingSurface {
    /// Classify one authored method from its checked receiver shape.
    const fn for_method(method: &MethodDecl) -> Self {
        if method.receiver.is_some() {
            Self::Instance
        } else {
            Self::Type
        }
    }

    /// Return whether checked method metadata belongs to this receiver surface.
    const fn accepts_method(self, method: &MethodInfo) -> bool {
        match self {
            Self::Type => method.receiver.is_none(),
            Self::Instance => method.receiver.is_some(),
        }
    }
}

impl MemberBindingKind {
    /// Return the source-facing category label used in duplicate-member diagnostics.
    const fn label(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::Method => "method",
            Self::Property => "property",
            Self::MethodAlias => "method alias",
            Self::MethodPartial => "method partial",
            Self::Variant => "variant",
            Self::VariantAlias => "variant alias",
        }
    }

    /// Map declaration-owning member categories to their canonical semantic kind.
    const fn declaration_kind(self) -> Option<SemanticSourceTargetKind> {
        match self {
            Self::Field => Some(SemanticSourceTargetKind::Field),
            Self::Method => Some(SemanticSourceTargetKind::Method),
            Self::Property => Some(SemanticSourceTargetKind::Property),
            Self::Variant => Some(SemanticSourceTargetKind::Variant),
            Self::MethodAlias | Self::MethodPartial | Self::VariantAlias => None,
        }
    }
}

/// One imported dependency member as import resolution proved it: the declaration it binds and the name the
/// declaring module binds that declaration under.
///
/// `declared_name` is the written item name for a direct import and the last re-export hop's target name for a facade
/// chain. It differs from `identity.declaration_name` exactly when the declaring module binds one declaration under
/// a second name (`pub scale_alias = alias scale`); a facade's `… import calculate as facade_calculate` is not that,
/// so a consumer of `facade_calculate` resolves to `declared_name == "calculate"` (#1710).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDependencyMember {
    /// Canonical identity of the declaration the import binds.
    pub identity: CanonicalSymbolId,
    /// The name the declaring module binds `identity` under.
    pub declared_name: String,
}

impl ResolvedDependencyMember {
    /// Pair an identity with the name the walk looked it up by in the module that answered.
    fn declared_as(identity: CanonicalSymbolId, declared_name: &str) -> Self {
        Self {
            identity,
            declared_name: declared_name.to_string(),
        }
    }
}

/// Canonical identity for one type or trait exported through a compiled public-library dependency.
///
/// Local aliases and provider-qualified internal signature spellings map to this value. Admitted artifacts compare
/// by their exact selected instance and canonical declaration. Legacy metadata without canonical evidence retains
/// its provider-local source path; a consumer route is used only by metadata-only compatibility adapters.
#[derive(Debug, Clone)]
pub struct PublicLibraryTypeIdentity {
    dependency_key: String,
    source_path: Vec<String>,
    /// Declaration identity retained from the admitted manifest, including provider-only signature types.
    canonical: Option<CanonicalSymbolId>,
    /// Exact admitted artifact instance; aliases and bridge routes do not participate in this identity.
    selected_provider: Option<LoafIdentity>,
}

impl PartialEq for PublicLibraryTypeIdentity {
    /// Compare exact declaring instances independently of consumer aliases, retaining legacy source-path distinction.
    fn eq(&self, other: &Self) -> bool {
        match (&self.selected_provider, &other.selected_provider) {
            (Some(left), Some(right)) => {
                left == right
                    && match (&self.canonical, &other.canonical) {
                        (Some(left), Some(right)) => left == right,
                        (None, None) => self.source_path == other.source_path,
                        _ => false,
                    }
            }
            (None, None) => self.dependency_key == other.dependency_key && self.source_path == other.source_path,
            _ => false,
        }
    }
}

impl Eq for PublicLibraryTypeIdentity {}

impl std::hash::Hash for PublicLibraryTypeIdentity {
    /// Hash the same selected instance and canonical or legacy source declaration used by equality.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        if let Some(provider) = &self.selected_provider {
            std::hash::Hash::hash(&true, state);
            std::hash::Hash::hash(&provider.stable_key(), state);
            if let Some(canonical) = &self.canonical {
                std::hash::Hash::hash(&true, state);
                std::hash::Hash::hash(canonical, state);
            } else {
                std::hash::Hash::hash(&false, state);
                std::hash::Hash::hash(&self.source_path, state);
            }
        } else {
            std::hash::Hash::hash(&false, state);
            std::hash::Hash::hash(&self.dependency_key, state);
            std::hash::Hash::hash(&self.source_path, state);
        }
    }
}

impl PublicLibraryTypeIdentity {
    /// Build one provider-qualified identity from the active dependency key and manifest source path.
    pub fn new(dependency_key: &str, source_path: &[String]) -> Self {
        Self {
            dependency_key: dependency_key.to_string(),
            source_path: source_path.to_vec(),
            canonical: None,
            selected_provider: None,
        }
    }

    /// Retain the declaration identity already supplied by this checked provider type binding.
    fn with_canonical(mut self, canonical: Option<CanonicalSymbolId>) -> Self {
        self.canonical = canonical;
        self
    }
}

const PUBLIC_LIBRARY_NAMESPACE: &str = "pub";
const PUBLIC_LIBRARY_TYPE_PREFIX: &str = "pub::";
type RustTypeIdentity = (String, Option<String>, Vec<ResolvedType>, Vec<Option<ResolvedType>>);

/// Encode one provider-owned public type as an internal `ResolvedType` spelling without adding a new enum variant.
///
/// The encoded spelling remains source-unspellable because `pub::` is import syntax, but it preserves enough context
/// for type compatibility and can be lowered to the provider-qualified Rust nominal path.
pub fn canonical_public_library_type_name(dependency_key: &str, public_name: &str) -> String {
    format!("{PUBLIC_LIBRARY_TYPE_PREFIX}{dependency_key}::{public_name}")
}

/// Decode an internal provider-qualified public type spelling into its dependency key and public export name.
pub fn split_canonical_public_library_type_name(name: &str) -> Option<(&str, &str)> {
    let remainder = name.strip_prefix(PUBLIC_LIBRARY_TYPE_PREFIX)?;
    let (dependency_key, public_name) = remainder.split_once("::")?;
    (!dependency_key.is_empty() && !public_name.is_empty()).then_some((dependency_key, public_name))
}

/// Declaration context that determines how `yield` expressions are interpreted.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum YieldContext {
    /// `yield` is not valid in the current body.
    #[default]
    Disallowed,
    /// `yield` belongs to `std.testing.fixture` lifecycle semantics.
    Fixture,
    /// `yield value` produces one item of the active generator's element type.
    Generator { element_ty: ResolvedType },
}

struct TypeCompatibilityDepthGuard<'a> {
    depth: &'a Cell<usize>,
}

/// Source-local slot state before a checked C call establishes the exact native storage contract.
///
/// This state never reaches lowering. Once a raw symbol accepts the slot, a typed [`CAbiOutputSlot`] fact replaces it
/// in [`TypeCheckInfo`].
#[derive(Debug, Clone)]
pub struct PendingCAbiOutputSlot {
    /// Closure frame that owns the slot's storage; captures require an unimplemented lifetime contract.
    pub closure_depth: usize,
    pub constructor_span: Span,
    pub mode: COutputMode,
    pub declared_type: Option<String>,
    pub initial_type: Option<ResolvedType>,
    pub bound: bool,
}

/// Source-local ownership state for one opaque checked C typed span carrier.
///
/// This state is deliberately frontend-only. The accepted constructor and bridge operations are retained as
/// [`CAbiInteropArtifacts`] facts; this record only prevents the source local that carries them from escaping or
/// being consumed twice while the body is typechecked.
#[derive(Debug, Clone, Copy)]
pub struct CAbiSpanLocal {
    /// Exact element representation and mutability selected by its direct constructor.
    pub kind: CAbiSpanKind,
    /// Ordinary local declaration span, retained so lexical shadowing cannot reuse a stale name-state entry.
    pub binding_span: Span,
}

/// Result-local relationship between one raw C call and the slots it may initialize or invalidate.
#[derive(Debug, Clone)]
pub struct CAbiRawCallResult {
    pub binding: String,
    pub symbol: String,
    /// Ordinary local name that received this raw call result.
    pub local_name: Option<String>,
    /// Source span of the ordinary local that received this raw-call result.
    ///
    /// Keeping the declaration span makes outcome facts follow regular lexical shadowing rather than a bare local
    /// spelling.
    pub local_symbol_span: Option<Span>,
    pub slots_by_parameter: HashMap<String, String>,
}

/// A Rust associated call whose owner type arguments the call itself left open (#1720).
///
/// `HashMap.new()` on `rust::std::collections::HashMap` names neither `K` nor `V`, and nothing at the call fixes
/// them: no explicit `[str, int]`, no expected type, no argument that mentions them. A local bound to the value can
/// still have them fixed by its first use (#2481); the checker keeps this record until it knows whether such a use
/// exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRustGenericCall {
    /// The receiver as the source spells it (`HashMap`), for the diagnostic.
    pub owner: String,
    /// The associated function called (`new`).
    pub method: String,
    /// The owner's type parameters the call left open, in declaration order (`K`, `V`).
    pub type_params: Vec<String>,
    /// The call's native result with those parameters open, once callee retention has computed it. A local bound to
    /// the call holds it as its native carrier until a use fixes the parameters.
    pub native: Option<ResolvedType>,
    /// The native result's nominal item (`std::collections::HashMap`), which selects the local's native methods.
    pub native_item: Option<String>,
}

/// A local bound to an [`OpenRustGenericCall`], watched until its first use fixes the call's open parameters (#1720).
///
/// The first read must be a use that determines them, and the binding takes the type that use fixes at once. When
/// the statement holding a first read that fixed nothing ends, the call is refused for its open parameters (#2481). A
/// binding nothing reads is refused when its declaring block ends, naming the binding to annotate.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRustGenericBinding {
    /// The local's name.
    pub name: String,
    /// Identity of the statement block that declared the binding, so the block's end reports only its own.
    pub block: usize,
    /// Span of the call expression the binding received.
    pub span: Span,
    /// The call whose open parameters the binding carries.
    pub call: OpenRustGenericCall,
    /// The declaring assignment, checked again against the type the first use fixes.
    pub assignment: crate::ast::AssignmentStmt,
    /// The declaring statement's span, which keys the binding's checked type.
    pub statement: Span,
    /// The binding's canonical identity: only a read of this binding can fix it.
    pub identity: Option<CanonicalSymbolId>,
    /// The statement block of the first read, once something read the binding.
    pub read_in: Option<usize>,
}

impl Drop for TypeCompatibilityDepthGuard<'_> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

/// The callable whose signature and body annotations the checker is currently resolving.
///
/// Retained only to word the unknown-type-name diagnostic: the suggestion appends the unknown name to the callable's
/// declared type parameters (#1373). It carries no semantic authority and is never exported.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnotationOwner {
    /// Source-visible callable name.
    name: String,
    /// Type parameters the callable already declares, in declaration order.
    declared_type_params: Vec<String>,
}

/// A callable's original type templates, retained before imports, receiver arguments, or partial application specialize
/// them. Type-parameter positions never grant a declaration-owned nominal identity to a caller's substitution.
struct DeclarationCallableTypes {
    /// Parameter templates in declaration order, including their source binding labels.
    params: Vec<CallableParam>,
    /// Original return template before call-site substitution.
    return_type: ResolvedType,
}

/// Exact mutation witnesses for every input to the nominal scope, without query-time inventory scans.
#[derive(PartialEq, Eq)]
struct NominalScopeInputs {
    /// Active module types and lexical shadows, versioned by their symbol-table owner.
    symbols: InventoryRevision,
    /// Alias targets, including mutations that retain the same lookup binding ID.
    aliases: InventoryRevision,
    /// Public nominal owners, including equal-size canonical replacements.
    public: InventoryRevision,
    /// Immutable declaration scopes and carrier spellings are append-only until the program reset clears this cache.
    declarations: usize,
    /// Number of admitted canonical carrier spellings.
    carriers: usize,
}

/// One current nominal scope, shared with immutable declaration snapshots only when their inputs are identical.
struct CheckingNominalScope {
    /// Private bindings; only successful checking can transfer this still-valid snapshot into published facts.
    identities: Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>,
    /// Mutation-owned inputs that admitted these identities.
    inputs: NominalScopeInputs,
}

/// Collect and check a program while retaining accepted semantic facts for Body IR and analysis consumers.
pub struct TypeChecker {
    /// Test-only cold oracle bypasses local table extraction while retaining the original semantic operations.
    #[cfg(test)]
    legacy_body_checking: bool,
    /// Verification-only declaration activations; a statement producer may not run inside any of them.
    #[cfg(test)]
    active_declaration_checks: Vec<Span>,
    /// Per-body complete compatibility snapshots for cold-oracle comparisons, absent from production.
    #[cfg(test)]
    body_local_observations: Vec<(Span, String)>,
    /// Verification-only enclosing writers needed to reconstruct complete snapshots inside nominal declarations.
    #[cfg(test)]
    body_enclosing_observations: Vec<body_check::BodyLocal>,
    /// Actual stage entries for the current activation, reset before collecting one program.
    check_work: CheckWork,
    /// Invocation-owned local outputs waiting for explicit finalization; never an engine cache.
    body_checks: Vec<body_check::BodyCheckInput>,
    /// Completed invocation-owned body values exposed to the record adapter.
    checked_bodies: Vec<std::sync::Arc<body_check::CheckedBodyView>>,
    /// Locally settled open-value diagnostics rendered at the legacy final stage.
    body_open_diagnostics: Vec<CompileError>,
    /// Completed declaration/default/initializer outputs, separate from callable statement outputs.
    checked_declarations: Vec<std::sync::Arc<body_check::CheckedBodyView>>,
    /// Nested producer diagnostic intervals used only while capturing independently owned outputs.
    diagnostic_owner_ranges: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    /// Symbol table populated during the first pass.
    pub symbols: SymbolTable,
    /// Accumulated compile errors (non-fatal).
    pub errors: Vec<CompileError>,
    /// Accumulated non-fatal diagnostics (warnings/lints).
    ///
    /// These are produced during typechecking but do not cause `check_*` to fail.
    pub warnings: Vec<CompileError>,
    /// The `mut` bindings of the body being checked, by name, which a Rust or C boundary that needs a mutable argument
    /// admits. Each function and method body starts with an empty set and restores the enclosing one when it ends.
    pub mutable_bindings: HashSet<String>,
    /// Caller-visible `mut` parameters: which callables declare them, which the checked bodies change, and the call
    /// arguments waiting to be decided once the module's bodies are known (#1773, `INCAN-T0117`).
    pub(crate) mut_params: mut_arguments::MutParamFacts,
    /// The method whose body is being checked while its receiver is a plain `self`, so a write through `self`
    /// inside it can be refused with the declaration to change (#1723). `None` outside a method body, and inside a
    /// `mut self` method.
    current_immutable_self_method: Option<String>,
    /// Iterator bindings consumed by terminal RFC 088 methods in the current local checking flow.
    pub consumed_iterator_bindings: HashMap<String, Span>,
    /// `dict.get(key)` calls whose value type cannot be copied, with that type's name; refused at the end of checking
    /// unless the lookup turned out to be only read (see `check_expr/dict_lookups.rs`).
    pending_uncopyable_dict_lookups: Vec<(usize, Span, String)>,
    /// Locals bound directly to a module static (`live = counts`), which read the static's storage like the static
    /// itself does, with the name of the static each one aliases.
    static_alias_bindings: HashMap<SymbolId, String>,
    /// Declaration spans of the read-only bindings a field or element write may not go through (#1561): a local
    /// declared without `mut`, by `let` or by a first plain assignment, and a parameter not marked `mut`. Loop and
    /// pattern bindings are not recorded; a write through one follows the place it binds.
    read_only_binding_spans: HashSet<crate::ast::Span>,
    /// How many closure bodies enclose the expression being checked.
    closure_depth: usize,
    /// Spans of the expressions being checked as the receiver of `copy_utf8(max_bytes=...)`, the one position a scoped
    /// C text view may be read in (RFC 116).
    scoped_c_string_view_receivers: HashSet<crate::ast::Span>,
    /// Direct argument expressions of the checked raw call currently being validated.
    c_abi_raw_argument_spans: HashSet<crate::ast::Span>,
    /// Calls that returned a scoped C text view in the statement being checked and are neither bound to a local nor
    /// the receiver of `copy_utf8`; each one left at the end of the statement escapes (RFC 116).
    unbound_scoped_c_string_views: HashSet<crate::ast::Span>,
    /// Locals bound to a scoped C text view, by declaration span, with the closure depth they were bound at: a read
    /// from a deeper closure captures the view (RFC 116).
    scoped_c_string_view_bindings: HashMap<crate::ast::Span, usize>,
    /// Resource bindings transferred to an owning C ABI parameter in the current local checking flow.
    pub transferred_c_resource_bindings: HashMap<String, Span>,
    /// Lists whose items a `for` loop of the current body takes, and that body's `for` pattern bindings (#1844).
    for_item_taking: for_item_taking::ForItemTaking,
    /// Closures that capture local values, and the parameters and returns of this module that hold them (#1561).
    capturing_callables: capturing_callables::CapturingCallables,
    /// The standard package the program being checked is, when it is one; its `std.*` roots are its own modules.
    own_standard_package: Option<&'static StandardPackageNamespacePolicy>,
    /// Checked span constructors waiting for the enclosing direct assignment to name their only legal owner.
    pub unbound_c_abi_span_constructors: HashMap<crate::ast::Span, CAbiSpanKind>,
    /// Opaque checked typed span carriers keyed by their direct source local.
    pub c_abi_span_bindings: HashMap<String, CAbiSpanLocal>,
    /// Checked mutable spans already consumed by their finish bridge in the active local flow.
    pub consumed_c_abi_span_bindings: HashMap<crate::ast::Span, Span>,
    /// Local output handles created by `c.out[...]()` or `c.inout(...)` before a raw symbol binds them.
    pub pending_c_abi_output_slots: HashMap<String, PendingCAbiOutputSlot>,
    /// Constructor facts waiting for their enclosing ordinary assignment to supply a local slot name.
    pub unbound_c_abi_output_slot_constructors: HashMap<crate::ast::Span, PendingCAbiOutputSlot>,
    /// Checked raw-call output relationships waiting for their enclosing ordinary assignment to supply a result name.
    pub unbound_c_abi_raw_call_results: HashMap<crate::ast::Span, CAbiRawCallResult>,
    /// Raw C result bindings whose enum outcome may establish branch-local output validity.
    pub c_abi_raw_call_results: Vec<CAbiRawCallResult>,
    /// Rust associated calls in the current statement whose owner type arguments were left open, keyed by call
    /// span, waiting for the statement to say whether a local receives the value (#1720). When the statement ends,
    /// one left nested somewhere else is refused unless its callee was retained.
    open_rust_generic_calls: HashMap<crate::ast::Span, OpenRustGenericCall>,
    /// Locals bound to open Rust associated calls that no use has fixed yet (#1720, #2481).
    open_rust_generic_bindings: Vec<OpenRustGenericBinding>,
    /// Equally ranked trait callees of a native method call, by call span, kept for that call's refusal when no later
    /// selection step chooses a callee (#2481).
    tied_rust_trait_callees: HashMap<crate::ast::Span, Vec<String>>,
    /// Statement-block identities used to scope pending generic-call bindings.
    statement_blocks: StatementBlockState,
    /// Slot identities currently readable in the active control-flow path.
    pub available_c_abi_output_slots: HashSet<String>,
    /// Slot identities already consumed by `take()` in the active checking flow.
    pub consumed_c_abi_output_slots: HashMap<String, Span>,
    /// Current function's error type for `?` operator compatibility.
    pub current_return_error_type: Option<ResolvedType>,
    /// Active declaration-level interpretation for `yield` expressions.
    pub current_yield_context: YieldContext,
    /// Whether the body currently being checked belongs to an `async def` or async method.
    pub in_async_body: bool,
    /// Expression span currently being typechecked as an `await` operand.
    pub await_operand_span: Option<crate::ast::Span>,
    /// Nesting depth for expressions being checked as call arguments.
    pub call_argument_depth: usize,
    /// Nesting depth of explicit `unsafe:` acknowledgment blocks.
    pub unsafe_depth: usize,
    /// Expression spans where type-like identifiers are valid namespace/type owners.
    pub type_receiver_spans: Vec<crate::ast::Span>,
    /// Expression spans where type-like identifiers are valid value-level `Type[T]` tokens.
    pub type_token_value_spans: Vec<crate::ast::Span>,
    /// The target span of the local partial expression being checked, where a generic function may be named as a value
    /// so the partial can instantiate it (RFC 084).
    pub(in crate::typechecker) generic_partial_target_span: Option<crate::ast::Span>,
    /// The generic function a local partial's target named at [`Self::generic_partial_target_span`], taken by the
    /// partial expression check to instantiate it.
    pub(in crate::typechecker) generic_partial_target: Option<check_expr::GenericPartialTarget>,
    /// Stack of active loop contexts, innermost last.
    pub loop_stack: Vec<LoopContext>,
    /// Direct list places whose storage active statement loops still borrow.
    iteration_borrows: Vec<iteration_borrows::IterationBorrow>,
    /// Active trait @requires context for default method bodies.
    pub current_trait_requires: Option<HashMap<String, ResolvedType>>,
    /// Active trait computed-property contract for default member bodies.
    pub current_trait_properties: Option<HashMap<String, PropertyInfo>>,
    /// Active trait name for default method diagnostics.
    pub current_trait_name: Option<String>,
    /// Active nominal owner while checking a method body.
    pub current_method_owner: Option<String>,
    /// Active `@classmethod` owner type exposed to the method body as `cls`.
    pub current_classmethod_self_ty: Option<ResolvedType>,
    /// Named function currently checked as the owner of a direct C raw call.
    pub current_c_abi_raw_call_owner: Option<CBindingRawCallOwner>,
    /// Public function declarations already warned for direct checked C calls in the active program check.
    pub warned_public_c_abi_raw_call_owners: HashSet<(String, crate::ast::Span)>,
    /// In-scope generic type-parameter trait bounds, preserving generic arguments for RFC 025 dispatch.
    pub current_type_param_bound_details: Vec<HashMap<String, Vec<TypeBoundInfo>>>,
    /// Declared plain trait bounds of the bounded nominals visible to the checked module, keyed by the declaring
    /// module and declaration name: `(type parameter, bounds)` in declaration order (#1280).
    pub(in crate::typechecker) nominal_type_param_bounds: nominal_type_param_bounds::NominalBoundTable,
    /// Deduplicate missing-`@requires` diagnostics within a single trait default method body.
    pub current_trait_missing_requires_emitted: Option<HashSet<String>>,
    /// Collected module-level const declarations (for rich const-eval + cycle detection).
    pub const_decls: HashMap<String, (ConstDecl, Span)>,
    /// Collected module-level static declarations in source order.
    pub static_decls: Vec<(StaticDecl, Span)>,
    /// Collected module-level function declarations for static dependency analysis.
    pub local_function_decls: HashMap<String, FunctionDecl>,
    /// Names of the declarations the program being checked marks `pub`.
    ///
    /// A public library carries a parameter default that constructs a model or class to its consumers only when they
    /// can construct that type themselves, which needs its declaration to be public.
    current_module_public_declarations: HashSet<String>,
    /// What method-decorator receiver planning (#1790) gathers while bodies are checked.
    receiver_plan_inputs: decorated_method_receivers::ReceiverPlanInputs,
    /// Exact factory call currently collecting partial type arguments for Body IR. Ordinary calls and synthesized
    /// probes never publish into the decorator snapshot; the enclosing value is restored after factory checking.
    body_ir_decorator_factory_span: Option<Span>,
    /// Function symbols collected in the current module pass, keyed by source name.
    ///
    /// The checker imports dependency modules into one ambient symbol table. Same-name overload grouping is
    /// module-local; this map avoids global current-scope identity checks.
    pub current_module_function_symbols: HashMap<String, SymbolId>,
    /// Transparent source type aliases, keyed by their local type name.
    pub type_aliases: NominalInventory<TypeAliasTarget>,
    /// Member declarations rejected by the source-ordered shared member registry.
    ///
    /// Collection and semantic checking both consult this set so a later colliding declaration cannot overwrite the
    /// first active member or produce a second, contradictory collision through the lexical symbol table.
    rejected_member_bindings: HashSet<crate::ast::Span>,
    /// Previous type-alias bindings changed while dependency imports are collected transactionally.
    dependency_import_type_alias_transaction: Option<HashMap<String, Option<TypeAliasTarget>>>,
    /// Whether source annotation names should be validated during the semantic check pass.
    validate_source_type_names: bool,
    /// Source annotation diagnostics already emitted in the current program check, keyed by spelling and span:
    /// unbound names, and bare builtin collection annotations (#1717, #1749), which the collection pass and the body
    /// check can both meet.
    unknown_source_type_names_emitted: HashSet<(String, crate::ast::Span)>,
    /// Type names the program under check declares itself, recorded before its declarations are collected.
    ///
    /// A program may take a builtin collection spelling (`Result`, `Tuple`) for its own type; an annotation that
    /// names such a type is an ordinary nominal, and the collection pass can meet the annotation before it meets the
    /// declaration, so the bare-collection refusal consults this set rather than the symbol table alone (#1749).
    source_type_declaration_names: HashSet<String>,
    /// The callable whose signature and body annotations are being checked, when one is active.
    ///
    /// An unknown type name inside a callable is most likely a type parameter that was never declared, so the
    /// unknown-symbol diagnostic names this callable and its declared parameters in its suggestion (#1373).
    annotation_owner: Option<AnnotationOwner>,
    /// Declaration-order index for each local static binding.
    pub static_decl_positions: HashMap<String, usize>,
    /// Whether the active expression is the initializer of a `RegistryEntry[K, T]` module static.
    ///
    /// RFC 113 admits `Registry.entry(...)` only in this declaration context. Keeping the context explicit prevents a
    /// runtime call from masquerading as a complete compiler-checked catalog entry.
    pub checking_registry_entry_static_initializer: bool,
    /// Whether the active expression is a callable parameter default before an invocation frame exists.
    ///
    /// Method defaults are checked inside their enclosing nominal declaration scope, where fields and properties are
    /// otherwise visible as bare names. The flag keeps those instance-only symbols out of default lookup while
    /// preserving module declarations and active generic type parameters.
    pub checking_callable_default: bool,
    /// Const evaluation state machine.
    pub const_eval_state: HashMap<String, const_eval::ConstEvalState>,
    /// Cached const evaluation results.
    pub const_eval_cache: HashMap<String, const_eval::ConstEvalResult>,
    /// Reusable typechecker output for downstream stages.
    pub type_info: TypeCheckInfo,
    /// Current source-order boundary for borrowed declaration/environment readers.
    visibility_point: VisibilityPoint,
    /// Public exports for imported dependency modules, keyed by module name.
    pub dependency_exports: HashMap<String, Vec<ExportedSymbol>>,
    /// Exact source-module member symbols collected from dependencies, keyed by canonical module name.
    ///
    /// Direct source imports use this map before falling back to ambient symbol lookup so `from module import name as
    /// alias` binds `module.name`, not an unrelated same-name symbol imported earlier from a sibling dependency.
    pub dependency_member_symbols: HashMap<String, HashMap<String, SymbolKind>>,
    /// Exact transparent type-alias targets exported by dependency modules.
    ///
    /// Alias expansion is source semantics, so targets are keyed by owner module and are copied into the consumer's
    /// local alias map only when an explicit source import creates that binding.
    pub dependency_member_type_aliases: HashMap<String, HashMap<String, TypeAliasTarget>>,
    /// Checked `Registry.define(...)` contracts exported by dependency source modules, keyed by canonical module name.
    ///
    /// This cache is deliberately separate from imported static types. A `Registry[K, T]` type alone is not proof of
    /// a canonical registry definition; only an owned `Registry.define(...)` declaration contributes a fact here.
    pub dependency_registry_definitions: HashMap<String, DependencyRegistryDefinitions>,
    /// Exact source-module partial projection metadata collected from dependencies, keyed by canonical module name.
    ///
    /// Source imports must preserve the callable projection, not only the callable signature, so const evaluation and
    /// lowering see the same partial preset semantics on both sides of an import or facade boundary.
    pub dependency_member_partial_projections: HashMap<String, HashMap<String, PartialProjectionInfo>>,
    /// Module-owned direct dependency members captured while that module is being imported.
    ///
    /// Dependency re-export refresh runs after all modules have been imported, when the ambient symbol table may
    /// contain unrelated same-name declarations from sibling modules. Direct members must therefore come from this
    /// per-module snapshot rather than a later global `lookup_symbol(...)`.
    pub dependency_direct_member_symbols: HashMap<String, HashMap<String, SymbolKind>>,
    /// Direct dependency-owned type-alias targets before facade re-export projection.
    pub dependency_direct_member_type_aliases: HashMap<String, HashMap<String, TypeAliasTarget>>,
    /// Canonical identities retained by direct dependency member bindings, including source aliases.
    ///
    /// A public `alias` binding carries its target declaration's identity, so reconstructing identity from the alias
    /// spelling and span would invent a second declaration. This cache preserves the symbol table's authoritative
    /// answer across the dependency boundary.
    pub dependency_direct_member_identities: HashMap<String, HashMap<String, CanonicalSymbolId>>,
    /// Nominal bindings captured in a source declaration's own scope before its signature enters a consumer.
    /// Contextual empty values use these bindings rather than the caller's same-spelled declarations. A declaration
    /// and its members share one immutable snapshot instead of retaining a full map for every member.
    declaration_nominal_scopes: HashMap<CanonicalSymbolId, Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>>,
    /// Carrier contributions of retained declaration scopes, indexed once per immutable scope admission. The owning
    /// scopes are append-only, so this inventory contains one spelling per retained canonical nominal, not
    /// generations.
    retained_nominal_carriers: std::collections::BTreeMap<CanonicalSymbolId, String>,
    /// Private value-composition snapshot after declaration collection; lexical alias shadows invalidate it.
    checking_nominal_scope: RefCell<Option<CheckingNominalScope>>,
    /// Original callable signatures before receiver or trait arguments replace their declaration-owned binders.
    declaration_callables: HashMap<CanonicalSymbolId, DeclarationCallableTypes>,
    /// Re-export links a dependency module publishes, keyed by module cache key then exported name.
    ///
    /// A facade does not own what it re-exports, so an identity must follow this link to the declaring module instead
    /// of naming the facade. Stored as the written import path plus the original name, resolved with the facade's own
    /// module path as the resolution base.
    pub dependency_member_reexports: HashMap<String, HashMap<String, (ImportPath, String)>>,
    /// Public-library re-export links omitted by the frozen source-member cache; only private Body IR call checking
    /// follows these links. Direct source declarations and ordinary import resolution remain authoritative.
    body_ir_dependency_pub_reexports: HashMap<String, HashMap<String, (ImportPath, String)>>,
    /// True module path segments for a registered dependency, keyed by its flattened cache name.
    ///
    /// The cache name is the underscore-joined spelling that also names the emitted Rust module, so it is not
    /// injective: the path `pkg.helpers` and a module literally named `pkg_helpers` flatten to one string. An origin
    /// built from whichever candidate matched would therefore reflect the spelling that asked rather than the module
    /// that answered, and two spellings of one module would yield two identities. Registering the real segments
    /// alongside the flat name — the same thing `IrCodegen::add_module_with_path_segments` does for emission — lets an
    /// identity name the module that actually answered.
    pub dependency_module_path_segments: HashMap<String, Vec<String>>,
    /// Which modules of the current check declare each nominal type name, so a union member of a name several of them
    /// declare names its declaration (#1796).
    nominal_declarations: union_member_identity::NominalDeclarationContext,
    /// Module-owned direct partial projection metadata captured while that module is being imported.
    pub dependency_direct_member_partial_projections: HashMap<String, HashMap<String, PartialProjectionInfo>>,
    /// RFC 024 derivable-module metadata from imported source modules, keyed by module path.
    pub dependency_derivable_modules: HashMap<String, Vec<String>>,
    /// RFC 024/user-module trait metadata from imported source modules, keyed by module-qualified trait name.
    pub dependency_module_traits: HashMap<String, TraitInfo>,
    /// RFC 024 trait-level Rust derive metadata from imported source modules, keyed by module-qualified trait name.
    pub dependency_trait_rust_derive_paths: HashMap<String, Vec<String>>,
    /// Exact Rust derive macro paths applied directly to local concrete declarations.
    ///
    /// `@derive(...)` keeps source-trait adoption metadata in [`TypeInfo`]. Direct RFC 043 `@rust.derive(...)`
    /// bypasses that source-language trait surface, but its inspected expansion still has to participate in Rust
    /// generic-bound projection. Keeping the canonical macro paths separately avoids pretending that a passthrough
    /// Rust derive is an Incan trait adoption.
    pub local_rust_derive_paths: HashMap<String, Vec<String>>,
    /// `@rust.derive(...)` facts for every nominal type declared in the module being checked, keyed by type name.
    ///
    /// The derive relation reads it for builtin derives spelled through `@rust.derive`, and an entry's presence marks
    /// the declaration as local, whose derive list is complete.
    pub(in crate::typechecker) local_derive_facts: HashMap<String, derive_requirements::LocalDeriveFacts>,
    /// Hash-key requirements inferred during collection and the bounds exported after checking (#1758).
    hash_key_inference: HashKeyInferenceState,
    /// The compiled Loaves this program imports from: its dependencies and the standard packages (DD-0007).
    pub loaves: Arc<LoafIndex>,
    /// Builtin traits' standard declarations by trait name, read once from the standard package owning each.
    pub(in crate::typechecker) standard_traits: HashMap<String, TraitInfo>,
    /// Internal semantic type cache for dependency exports referenced transitively by imported signatures.
    ///
    /// These entries are intentionally **not** source-visible names: they exist so values returned from imported
    /// functions/methods can still participate in method lookup and trait compatibility even when the consumer did not
    /// explicitly import every referenced carrier type.
    pub transitive_pub_types: HashMap<String, Vec<TypeInfo>>,
    /// Canonical nominal identities for local aliases and provider-qualified signature spellings from `pub::` imports.
    ///
    /// Identity is deliberately separate from [`ResolvedType`]: source diagnostics retain local spellings while
    /// compatibility can still distinguish identical short names owned by separate dependencies.
    pub public_library_type_identities: NominalInventory<PublicLibraryTypeIdentity>,
    /// Internal semantic trait cache for dependency exports referenced transitively by imported signatures.
    ///
    /// This keeps trait/supertrait compatibility available for imported function signatures without making those trait
    /// names ambient in user source.
    pub transitive_pub_traits: HashMap<String, Vec<TraitInfo>>,
    /// Tracks which `pub::` libraries have already seeded the internal transitive semantic caches for this checker
    /// run.
    pub cached_pub_libraries: HashSet<String>,
    /// Physical type routes projected once from checked foreign leaf identities for each imported provider.
    pub foreign_pub_type_remappings: HashMap<String, HashMap<String, String>>,
    /// Per imported provider and checked API module: the type names the module's declarations spell whose meaning
    /// the package root's public names do not carry, mapped to the declaration's provider-qualified key.
    ///
    /// A published signature or field names a type as its declaring module spells it, through the module's own
    /// declarations and imports, private ones included. The root's public names cover the spellings that name the same
    /// declaration at the root; these maps cover the rest, such as a type only a submodule publishes or a root name
    /// that names another declaration.
    pub(in crate::typechecker) pub_library_declaration_scopes:
        HashMap<String, HashMap<Vec<String>, HashMap<String, String>>>,
    /// Whether a manual [`Self::import_module`] call prepared dependency-only semantics for the next program check.
    dependency_semantics_pending: bool,
    /// Module path for the program being checked (if known).
    pub current_module_path: Option<Vec<String>>,
    /// Source import aliases with declaration origins known from the dependency import graph.
    pub source_import_targets: HashMap<String, SourceTargetInfo>,
    /// Declared Rust crate names from `loaf.toml [rust-dependencies]` (RFC 023 / RFC 013).
    ///
    /// Used to validate that `rust.module()` paths reference known crates. When `None`, crate validation is skipped
    /// (e.g. single-file mode without a manifest).
    pub declared_crate_names: Option<HashSet<String>>,
    /// Local names bound to `std.testing` marker decorators via imports.
    ///
    /// These names are disallowed in runtime call expressions; markers are decorator-only semantics consumed by the
    /// test runner.
    pub testing_marker_import_bindings: HashSet<String>,
    /// Local names bound to stdlib surface helpers that still need compiler-known call typing.
    ///
    /// The stored symbol id must remain the active lookup binding; later local declarations or user imports with the
    /// same name shadow these helper semantics.
    pub surface_function_import_bindings: HashMap<String, (SurfaceFnId, SymbolId)>,
    /// Local names bound to stdlib surface types that still need compiler-known constructor typing.
    ///
    /// The stored symbol id must remain the active lookup binding; later local declarations or user imports with the
    /// same name shadow these constructor semantics.
    pub surface_type_import_bindings: HashMap<String, (SurfaceTypeId, SymbolId)>,
    /// Type names the program being checked declares at module scope (models, classes, enums, newtypes, traits, type
    /// aliases), recorded before collection so every annotation sees them.
    ///
    /// A module declaration is the program's own type: a stdlib surface type of the same name is reached only by
    /// importing it, so it never shadows the declaration, even from a signature collected before the declaration
    /// itself (#1795).
    pub module_declared_type_names: HashSet<String>,
    /// Fixture function names collected before body checking so dependency metadata is order-independent.
    pub testing_fixture_names: HashSet<String>,
    /// Checked `std.testing` marker contract supplied by the active provider projection.
    ///
    /// This is populated while imports are collected. Keeping it with the checker prevents declaration checking from
    /// reopening provider source after the compiled manifest has become the semantic authority.
    pub testing_marker_semantics: Option<crate::testing_markers::TestingMarkerSemantics>,
    /// Unified import-driven activation and strategy context for soft-keyword/decorator semantics.
    pub surface_context: SurfaceContext,
    /// RFC 042: transitive supertrait closure for each trait, keyed by trait name.
    ///
    /// Values list every reachable supertrait `(name, type_arguments)` after applying generic substitution along the
    /// chain. Filled at the end of the collection pass (before the second typecheck pass runs).
    pub supertrait_closure: HashMap<String, Vec<(String, Vec<ResolvedType>)>>,
    /// Trait `with` bounds queued during collection and resolved once all declarations are registered (RFC 042).
    ///
    /// Ensures supertrait names are not mistaken for free type parameters when the supertrait is declared later in the
    /// same module.
    pub pending_trait_supertraits: Vec<(SymbolId, Vec<Spanned<TraitBound>>)>,
    /// Recursion guards and native fact caches owned by this checker, never shared across checks.
    native_queries: NativeQueryState,
}

/// Hash-key requirements retained from collection through checked signature publication.
#[derive(Default)]
struct HashKeyInferenceState {
    /// Type parameters each generic function or method hashes, by declaration identity, for this module and imports.
    type_params: HashMap<CanonicalSymbolId, BTreeSet<String>>,
    /// This module's inferred hashed parameters, written into exported signatures once checking finishes.
    local_requirements: Vec<(hash_key_inference::HashKeyCallable, BTreeSet<String>)>,
    /// Additional bounds by function and type parameter, exported beside the function's declared source bounds.
    function_bounds: HashMap<String, HashMap<String, Vec<TypeBoundInfo>>>,
}

/// Identity of the active statement block and the serial that mints nested blocks.
#[derive(Default)]
struct StatementBlockState {
    /// A pending binding belongs to exactly the block that declared it.
    current: usize,
    /// Monotonically increasing identity source within one checker.
    serial: usize,
}

/// Per-checker recursion guards, observed Rust facts and memoized native signature/protocol selections.
#[derive(Default)]
struct NativeQueryState {
    /// Current recursive compatibility-check depth.
    ///
    /// Recursive source aliases and Rust facade identities can route compatibility back through the same structural
    /// surfaces. Keep that from becoming a process-level stack overflow; the guard returns `false` once a cycle is
    /// deep enough that the checker cannot prove compatibility.
    type_compatibility_depth: Cell<usize>,
    /// How many types deep the current compatibility check is inside another type (a type argument, a tuple element,
    /// a callable parameter or return, a referenced type, or a member of a union value).
    ///
    /// Lossless numeric widening converts one value, so it applies only at depth zero; see
    /// [`TypeChecker::nested_types_compatible`].
    nested_type_compatibility_depth: Cell<usize>,
    /// Successful canonical Rust ABI lookups made by this checker, including inferred receiver and return types.
    rust_metadata_queries: RefCell<BTreeSet<String>>,
    /// Native signature types already resolved by this checker, keyed by metadata display and owner path.
    ///
    /// Resolution depends only on the Rust facts and path normalization, which are fixed for a checker's lifetime,
    /// except for a display naming a nominal carrier key: that one resolves through the active module's bindings, so
    /// it is never cached. Without this cache, an owner's type arguments are re-derived from their displays at every
    /// level of generic nesting, which doubles the work per level.
    native_signature_types: RefCell<HashMap<(String, String), ResolvedType>>,
    /// Native iteration protocol calls already selected by this checker, keyed by owner, trait, method and the
    /// source type parameters in scope. Selection scans every implementation of the protocol trait.
    native_iteration_calls: RefCell<HashMap<(String, String, String, String), Option<type_info::RustCheckedCall>>>,
    /// Actual complete-type lookup requests observed by this isolated checker in demand regression tests.
    #[cfg(test)]
    rust_complete_type_lookup_requests: Cell<usize>,
}

/// Dependency-owned registry contracts and their canonical source-module identity.
///
/// Dependency map keys are compatibility lookup keys that join path segments with underscores. They cannot safely
/// reconstruct a source identity because a legal segment can itself contain an underscore, so the owner path travels
/// beside the definition rather than being recovered from the cache key.
#[derive(Debug, Clone)]
pub struct DependencyRegistryDefinitions {
    /// Exact source module that declares the cached registry statics.
    owner_module_path: Vec<String>,
    /// Definitions keyed by their owner-module static binding.
    definitions: HashMap<String, RegistryDefinitionInfo>,
}

impl TypeChecker {
    /// Create an empty typechecker with fresh symbol, diagnostic, import, and lowering-metadata state.
    ///
    /// Native queries, statement-block tracking and hash-key inference each initialize their own empty state.
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            legacy_body_checking: false,
            #[cfg(test)]
            active_declaration_checks: Vec::new(),
            #[cfg(test)]
            body_local_observations: Vec::new(),
            #[cfg(test)]
            body_enclosing_observations: Vec::new(),
            check_work: CheckWork::default(),
            body_checks: Vec::new(),
            checked_bodies: Vec::new(),
            checked_declarations: Vec::new(),
            diagnostic_owner_ranges: Vec::new(),
            body_open_diagnostics: Vec::new(),
            symbols: SymbolTable::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            mutable_bindings: HashSet::new(),
            mut_params: mut_arguments::MutParamFacts::default(),
            current_immutable_self_method: None,
            consumed_iterator_bindings: HashMap::new(),
            pending_uncopyable_dict_lookups: Vec::new(),
            static_alias_bindings: HashMap::new(),
            read_only_binding_spans: HashSet::new(),
            closure_depth: 0,
            scoped_c_string_view_receivers: HashSet::new(),
            c_abi_raw_argument_spans: HashSet::new(),
            unbound_scoped_c_string_views: HashSet::new(),
            scoped_c_string_view_bindings: HashMap::new(),
            transferred_c_resource_bindings: HashMap::new(),
            for_item_taking: for_item_taking::ForItemTaking::default(),
            capturing_callables: capturing_callables::CapturingCallables::default(),
            own_standard_package: None,
            unbound_c_abi_span_constructors: HashMap::new(),
            c_abi_span_bindings: HashMap::new(),
            consumed_c_abi_span_bindings: HashMap::new(),
            pending_c_abi_output_slots: HashMap::new(),
            unbound_c_abi_output_slot_constructors: HashMap::new(),
            unbound_c_abi_raw_call_results: HashMap::new(),
            open_rust_generic_calls: HashMap::new(),
            open_rust_generic_bindings: Vec::new(),
            tied_rust_trait_callees: HashMap::new(),
            statement_blocks: StatementBlockState::default(),
            c_abi_raw_call_results: Vec::new(),
            available_c_abi_output_slots: HashSet::new(),
            consumed_c_abi_output_slots: HashMap::new(),
            current_return_error_type: None,
            current_yield_context: YieldContext::Disallowed,
            in_async_body: false,
            await_operand_span: None,
            call_argument_depth: 0,
            unsafe_depth: 0,
            type_receiver_spans: Vec::new(),
            type_token_value_spans: Vec::new(),
            generic_partial_target_span: None,
            generic_partial_target: None,
            loop_stack: Vec::new(),
            iteration_borrows: Vec::new(),
            current_trait_requires: None,
            current_trait_properties: None,
            current_trait_name: None,
            current_method_owner: None,
            current_classmethod_self_ty: None,
            current_c_abi_raw_call_owner: None,
            warned_public_c_abi_raw_call_owners: HashSet::new(),
            current_type_param_bound_details: Vec::new(),
            nominal_type_param_bounds: HashMap::new(),
            current_trait_missing_requires_emitted: None,
            const_decls: HashMap::new(),
            static_decls: Vec::new(),
            local_function_decls: HashMap::new(),
            current_module_public_declarations: HashSet::new(),
            receiver_plan_inputs: Default::default(),
            body_ir_decorator_factory_span: None,
            current_module_function_symbols: HashMap::new(),
            type_aliases: NominalInventory::default(),
            rejected_member_bindings: HashSet::new(),
            dependency_import_type_alias_transaction: None,
            validate_source_type_names: false,
            unknown_source_type_names_emitted: HashSet::new(),
            source_type_declaration_names: HashSet::new(),
            annotation_owner: None,
            static_decl_positions: HashMap::new(),
            checking_registry_entry_static_initializer: false,
            checking_callable_default: false,
            const_eval_state: HashMap::new(),
            const_eval_cache: HashMap::new(),
            type_info: TypeCheckInfo::default(),
            visibility_point: VisibilityPoint::default(),
            dependency_exports: HashMap::new(),
            dependency_member_symbols: HashMap::new(),
            dependency_member_type_aliases: HashMap::new(),
            dependency_registry_definitions: HashMap::new(),
            dependency_member_partial_projections: HashMap::new(),
            dependency_direct_member_symbols: HashMap::new(),
            dependency_direct_member_type_aliases: HashMap::new(),
            dependency_direct_member_identities: HashMap::new(),
            declaration_nominal_scopes: HashMap::new(),
            retained_nominal_carriers: std::collections::BTreeMap::new(),
            checking_nominal_scope: RefCell::new(None),
            declaration_callables: HashMap::new(),
            dependency_member_reexports: HashMap::new(),
            body_ir_dependency_pub_reexports: HashMap::new(),
            dependency_module_path_segments: HashMap::new(),
            nominal_declarations: union_member_identity::NominalDeclarationContext::default(),
            dependency_direct_member_partial_projections: HashMap::new(),
            dependency_derivable_modules: HashMap::new(),
            dependency_module_traits: HashMap::new(),
            dependency_trait_rust_derive_paths: HashMap::new(),
            local_rust_derive_paths: HashMap::new(),
            local_derive_facts: HashMap::new(),
            hash_key_inference: HashKeyInferenceState::default(),
            loaves: Arc::new(LoafIndex::default()),
            standard_traits: HashMap::new(),
            transitive_pub_types: HashMap::new(),
            public_library_type_identities: NominalInventory::default(),
            transitive_pub_traits: HashMap::new(),
            cached_pub_libraries: HashSet::new(),
            foreign_pub_type_remappings: HashMap::new(),
            pub_library_declaration_scopes: HashMap::new(),
            dependency_semantics_pending: false,
            current_module_path: None,
            source_import_targets: HashMap::new(),
            declared_crate_names: None,
            testing_marker_import_bindings: HashSet::new(),
            surface_function_import_bindings: HashMap::new(),
            surface_type_import_bindings: HashMap::new(),
            module_declared_type_names: HashSet::new(),
            testing_fixture_names: HashSet::new(),
            testing_marker_semantics: None,
            surface_context: SurfaceContext::default(),
            supertrait_closure: HashMap::new(),
            pending_trait_supertraits: Vec::new(),
            native_queries: NativeQueryState::default(),
        }
    }

    /// Reject a source binding that would replace a protected builtin spelling.
    ///
    /// The registry resolves canonical and alias spellings to one stable builtin identity, while the protected policy
    /// deliberately selects only the identities that must remain globally available.
    ///
    /// Returns whether this call emitted the protected-binding diagnostic, allowing callers with further name-collision
    /// recovery to avoid obscuring the primary error.
    pub(in crate::typechecker) fn validate_protected_builtin_binding(&mut self, name: &str, span: Span) -> bool {
        let Some(builtin) = builtins::protected_binding_builtin(name) else {
            return false;
        };
        self.errors.push(errors::protected_builtin_binding(name, builtin, span));
        true
    }

    /// Push a loop's complete checked identity before checking its body, including generated expansion context.
    ///
    /// Statement loops pass `None` for `expected_break_ty`; expression loops forward whatever result type the
    /// surrounding context expects.
    pub fn push_loop_context(&mut self, kind: LoopContextKind, expected_break_ty: Option<ResolvedType>, span: Span) {
        self.loop_stack.push(LoopContext {
            kind,
            expected_break_ty,
            break_types: Vec::new(),
            scope: self.symbols.current_scope_index(),
            block_depth: self.item_taking_block_depth(),
            span,
        });
    }

    /// Pop the innermost loop context once the corresponding body has been checked.
    pub fn pop_loop_context(&mut self) -> Option<LoopContext> {
        self.loop_stack.pop()
    }

    /// Borrow the innermost active loop so `break` checking can append inferred break types.
    pub fn current_loop_context_mut(&mut self) -> Option<&mut LoopContext> {
        self.loop_stack.last_mut()
    }

    /// Return whether this call expression is inside the active `await` operand.
    pub fn is_in_await_operand(&self, span: Span) -> bool {
        self.await_operand_span
            .is_some_and(|operand| operand.start <= span.start && span.end <= operand.end)
    }

    /// Emit the missing-await warning for a direct async call when the current expression context should report it.
    pub fn warn_if_unawaited_async_call(&mut self, callable: &str, span: Span) {
        if !self.is_in_await_operand(span) && self.call_argument_depth == 0 {
            self.errors.push(errors::async_call_without_await(callable, span));
        }
    }

    /// Resolve the final type of a `loop:` expression from the `break` types observed in its body.
    ///
    /// The `break` values unify as [`Self::unify_branch_value_types`] states, against the outer expected type when
    /// one exists; a loop with no `break` has no value and is refused.
    pub fn resolve_loop_break_result_type(
        &mut self,
        loop_span: Span,
        expected_break_ty: Option<&ResolvedType>,
        break_types: &[(ResolvedType, Span)],
    ) -> ResolvedType {
        if break_types.is_empty() {
            self.errors.push(errors::loop_expression_requires_break(loop_span));
            return ResolvedType::Unknown;
        }
        self.unify_branch_value_types(break_types, expected_break_ty)
            .unwrap_or(ResolvedType::Unknown)
    }

    /// Return the one type the values of the branches of a value-producing construct unify to: the `break` values of
    /// a `loop:` expression, or the arms of a `match` expression.
    ///
    /// With `expected`, the type of the place the construct is written to, every value must be assignable to it and
    /// the construct yields it. Without it, each value either is assignable to the type unified so far, or the type
    /// unified so far is assignable to it and it becomes the result, so `i8` and `int` values unify to `int`. A
    /// `Result` side or `Option` payload one value leaves open takes the type another value gives it, whichever value
    /// comes first (#1561): `None` and `Some(5)` unify to `Option[int]`, and `Err("x")` and `Ok(1)` to
    /// `Result[int, str]`. A side no value fixes stays open, and the place the construct is written to settles it.
    /// A value that fits neither way is refused with a type mismatch at its span and the construct yields `Unknown`.
    /// An `Unknown` value has already been refused or is not yet known and a `Never` value does not complete, so
    /// neither takes part; `None` means no value took part.
    ///
    /// Every value whose type is not the result is recorded as written to a place of the result type
    /// ([`Self::record_value_destination_if_compatible`]), so lowering widens a narrower numeric value and wraps an
    /// `Option` payload in that branch itself, and its type is recorded with the open parts the result fills, so the
    /// `None` or `Err(...)` of one branch is built with the payload or side another branch gives. The branches of the
    /// generated construct then share one type, as Rust requires of the arms of a `match` and the values of a `loop`.
    pub(in crate::typechecker) fn unify_branch_value_types(
        &mut self,
        values: &[(ResolvedType, Span)],
        expected: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut participating = values
            .iter()
            .filter(|(ty, _)| !matches!(ty, ResolvedType::Unknown | ResolvedType::Never));
        let result_ty = match expected {
            Some(expected) => {
                if let Some((ty, span)) = participating.find(|(ty, _)| !self.types_compatible(ty, expected)) {
                    self.errors
                        .push(errors::type_mismatch(&expected.to_string(), &ty.to_string(), *span));
                    return Some(ResolvedType::Unknown);
                }
                expected.clone()
            }
            None => {
                let (first_ty, _) = participating.next()?;
                let mut result_ty = first_ty.clone();
                for (ty, span) in participating {
                    if self.types_compatible(ty, &result_ty) {
                        check_expr::fill_open_result_parts(&mut result_ty, ty, false);
                        continue;
                    }
                    if self.types_compatible(&result_ty, ty) {
                        let mut widened = ty.clone();
                        check_expr::fill_open_result_parts(&mut widened, &result_ty, false);
                        result_ty = widened;
                        continue;
                    }
                    self.errors
                        .push(errors::type_mismatch(&result_ty.to_string(), &ty.to_string(), *span));
                    return Some(ResolvedType::Unknown);
                }
                result_ty
            }
        };
        for (ty, span) in values {
            if *ty != result_ty {
                self.record_value_destination_if_compatible(*span, ty, &result_ty);
                let mut filled = ty.clone();
                check_expr::fill_open_result_parts(&mut filled, &result_ty, false);
                if filled != *ty {
                    self.record_expr_type(*span, filled);
                }
            }
        }
        Some(result_ty)
    }
    /// Return canonical Rust metadata queries successfully resolved by this checker.
    ///
    /// Library publication uses these paths in addition to explicit imports so inferred return receivers and other
    /// transitive ABI lookups survive the source-to-Loaf boundary. Failed speculative lookups are not requirements.
    pub fn observed_rust_metadata_queries(&self) -> BTreeSet<String> {
        self.native_queries.rust_metadata_queries.borrow().clone()
    }

    /// Retain a successful lookup's nominal query spelling without turning missing speculative paths into demands.
    fn record_rust_metadata_query(&self, canonical_path: &str) {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        if let Some(lookup_path) = Self::rust_metadata_lookup_path(canonical_path) {
            let mut queries = self.native_queries.rust_metadata_queries.borrow_mut();
            if !queries.contains(lookup_path) {
                queries.insert(lookup_path.to_string());
            }
        }
    }

    /// Resolve Rust metadata through the selected shipped ABI or inspection authority and retain successful demand.
    pub fn rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let metadata = self.lookup_rust_item_metadata_for_path(canonical_path)?;
        self.record_rust_metadata_query(canonical_path);
        Some(metadata)
    }

    /// Return Rust item metadata from the shipped ABI of the Loaves the checker reaches.
    fn lookup_rust_item_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        let lookup_path = Self::rust_metadata_lookup_path(canonical_path)?;
        self.loaves.rust_abi_item(lookup_path)
    }

    /// Return Rust item metadata from the shipped ABI of the Loaves the checker reaches.
    pub fn rust_item_metadata_for_method_call(
        &self,
        canonical_path: &str,
        _method: &str,
    ) -> Option<Arc<RustItemMetadata>> {
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Return complete type metadata from the shipped ABI of the Loaves the checker reaches.
    pub fn rust_item_metadata_for_complete_type(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        #[cfg(test)]
        self.native_queries
            .rust_complete_type_lookup_requests
            .set(self.native_queries.rust_complete_type_lookup_requests.get() + 1);
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Split top-level generic arguments from a Rust display type.
    fn split_top_level_generic_args(args: &str) -> Vec<&str> {
        split_top_level_rust_args(args)
    }

    /// Normalize a Rust metadata lookup path down to the nominal item path.
    ///
    /// rust-analyzer metadata is keyed by the item path (`foo::Bar`), not by instantiated spellings like
    /// `foo::Bar<T>` or placeholder displays like `{unknown}`. This strips outer generic instantiation from a Rust
    /// path while rejecting obviously non-item spellings before hitting the metadata cache/extractor.
    fn rust_metadata_lookup_path(canonical_path: &str) -> Option<&str> {
        let trimmed = canonical_path.trim();
        if trimmed == "[]" {
            return Some(trimmed);
        }
        if trimmed.is_empty() || trimmed == "{unknown}" {
            return None;
        }
        let had_generics = trimmed.contains('<');
        let base = trimmed.split_once('<').map_or(trimmed, |(base, _)| base);
        if had_generics && !base.contains("::") {
            return None;
        }
        if base.is_empty() || base.contains(['{', '}', '(', ')', '[', ']', ',', ' ']) {
            return None;
        }
        Some(base)
    }

    /// Strip the synthetic `rust::` namespace prefix used in Incan source paths.
    ///
    /// Canonical Rust metadata item keys are crate-rooted paths (`crate_name::...`), so compatibility checks and
    /// metadata lookups should normalize away the language surface namespace first.
    fn normalize_rust_namespace_path(path: &str) -> &str {
        path.strip_prefix("rust::").unwrap_or(path)
    }

    /// Return the inspected Rust owner path for a synthetic `rust::Type.method` call display.
    ///
    /// Method diagnostics retain the callable spelling, but metadata signatures resolve `crate::`, `self::`, and
    /// `super::` relative to the receiver type's module, never relative to the synthetic method segment.
    pub fn rust_method_owner_path(callable_display: &str) -> &str {
        let path = Self::normalize_rust_namespace_path(callable_display);
        path.rsplit_once('.').map_or(path, |(owner, _method)| owner)
    }

    /// Split a Rust display into its nominal path and the Incan-visible generic type arguments.
    ///
    /// Rust lifetimes are erased at the language boundary, while nested type arguments keep their normal display
    /// form and are resolved for nominal comparison. The returned base is also normalized from Incan's synthetic
    /// `rust::` namespace to the inspector's crate-rooted path.
    fn rust_path_base_and_args(&self, path: &str) -> (String, Vec<ResolvedType>) {
        let trimmed = path.trim();
        if let Some(start) = trimmed.find('<')
            && trimmed.ends_with('>')
        {
            let base = Self::normalize_rust_namespace_path(trimmed[..start].trim()).to_string();
            let inner = &trimmed[start + 1..trimmed.len() - 1];
            let args = Self::split_top_level_generic_args(inner)
                .into_iter()
                .filter(|arg| !Self::rust_generic_arg_is_lifetime(arg))
                .map(|arg| self.resolved_type_from_rust_display(arg))
                .collect();
            return (base, args);
        }
        (Self::normalize_rust_namespace_path(trimmed).to_string(), Vec::new())
    }

    /// Return whether a Rust generic argument is a lifetime, which Incan deliberately does not model as a type.
    ///
    /// Rust metadata may qualify an inferred lifetime through the owner module (for example, `crate::'_`). Preserve
    /// real type arguments while eliding both plain and qualified lifetime placeholders before nominal comparison.
    pub(crate) fn rust_generic_arg_is_lifetime(arg: &str) -> bool {
        let candidate = arg.trim().rsplit("::").next().unwrap_or(arg.trim());
        let Some(name) = candidate.strip_prefix('\'') else {
            return false;
        };
        !name.is_empty() && name.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    }

    /// Rewrite Rust's crate-relative type displays against the inspected callable or type path that produced them.
    ///
    /// rust-analyzer reports signatures from inside a Rust crate using displays such as
    /// `crate::ScalarFunctionImplementation`. Incan source, imports, and generated Rust refer to the dependency by its
    /// crate name (`datafusion_expr::ScalarFunctionImplementation`), so boundary typing must canonicalize those
    /// displays before compatibility checks or contextual argument checking run.
    pub fn rust_display_for_owner_path(&self, rust_ty: &str, owner_path: &str) -> String {
        let owner = Self::normalize_rust_namespace_path(owner_path);
        let owner_definition = self
            .rust_definition_for_path(owner)
            .unwrap_or_else(|| owner.to_string());
        let owner_segments = owner_definition.split("::").collect::<Vec<_>>();
        let Some(crate_name) = owner_segments.first().filter(|segment| !segment.is_empty()) else {
            return rust_ty.to_string();
        };
        let replacement = format!("{crate_name}::");
        let mut rendered = String::with_capacity(rust_ty.len() + replacement.len());
        let mut remaining = rust_ty;
        while let Some(idx) = remaining.find("crate::") {
            let (before, after) = remaining.split_at(idx);
            rendered.push_str(before);
            let prev = rendered.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
                rendered.push_str("crate::");
            } else if after["crate::".len()..].starts_with('[') {
                // rust-analyzer can render a slice nested in a generic callback bound as `crate::[T]`. A slice is
                // not a crate item, so preserve its Rust spelling instead of producing the invalid `dep::[T]`.
            } else {
                rendered.push_str(replacement.as_str());
            }
            remaining = &after["crate::".len()..];
        }
        rendered.push_str(remaining);

        let owner_module = &owner_segments[..owner_segments.len().saturating_sub(1)];
        let self_replacement = owner_module.join("::");
        let mut self_expanded = String::with_capacity(rendered.len() + self_replacement.len());
        let mut remaining = rendered.as_str();
        while let Some(idx) = remaining.find("self::") {
            let (before, after) = remaining.split_at(idx);
            self_expanded.push_str(before);
            let prev = self_expanded.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) || self_replacement.is_empty() {
                self_expanded.push_str("self::");
            } else {
                self_expanded.push_str(self_replacement.as_str());
                self_expanded.push_str("::");
            }
            remaining = &after["self::".len()..];
        }
        self_expanded.push_str(remaining);

        let owner_module_prefix = (!owner_module.is_empty()).then(|| format!("{}::", owner_module.join("::")));
        let mut super_expanded = String::with_capacity(self_expanded.len() + owner_definition.len());
        let mut remaining = self_expanded.as_str();
        while let Some(idx) = remaining.find("super::") {
            let (before, after) = remaining.split_at(idx);
            super_expanded.push_str(before);
            let mut rest = after;
            let mut parent_count = 0usize;
            while let Some(next) = rest.strip_prefix("super::") {
                parent_count += 1;
                rest = next;
            }
            // rust-analyzer can qualify a relative path with the method owner's module, for example
            // `datafusion::execution::context::super::options::...`. The prefix is context for the relative
            // segment, not part of the resulting type path, so remove it before replacing `super` with its parent.
            if let Some(prefix) = owner_module_prefix.as_deref()
                && super_expanded.ends_with(prefix)
            {
                super_expanded.truncate(super_expanded.len() - prefix.len());
            }
            let prev = super_expanded.chars().next_back();
            if prev.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric()) || parent_count >= owner_module.len() {
                super_expanded.push_str(&after[..after.len() - rest.len()]);
            } else {
                super_expanded.push_str(owner_module[..owner_module.len() - parent_count].join("::").as_str());
                super_expanded.push_str("::");
            }
            remaining = rest;
        }
        super_expanded.push_str(remaining);
        super_expanded
    }

    /// Return already-attached or cache-only metadata for one canonical Rust identity path.
    fn known_rust_metadata_for_path(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        let canonical_path = Self::normalize_rust_namespace_path(canonical_path);
        if let Some(metadata) = self.symbols.all_symbols().iter().find_map(|sym| {
            let SymbolKind::RustItem(info) = &sym.kind else {
                return None;
            };
            if Self::normalize_rust_namespace_path(info.path.as_str()) != canonical_path {
                return None;
            }
            info.metadata.clone()
        }) {
            return Some(metadata);
        }
        self.rust_item_metadata_for_path(canonical_path)
    }

    /// Return the Rust definition metadata for a canonical path.
    fn rust_definition_for_path(&self, canonical_path: &str) -> Option<String> {
        self.known_rust_metadata_for_path(canonical_path)
            .and_then(|metadata| metadata.definition_path.clone())
    }

    /// Resolve inspected default type arguments without inventing defaults for metadata that omitted them.
    fn rust_type_param_defaults(&self, metadata: Option<&RustItemMetadata>) -> Vec<Option<ResolvedType>> {
        let Some(RustItemMetadata {
            kind: RustItemKind::Type(type_info),
            ..
        }) = metadata
        else {
            return Vec::new();
        };
        type_info
            .type_param_defaults
            .iter()
            .map(|default| {
                default
                    .as_deref()
                    .map(|default| self.resolved_type_from_rust_display(default))
            })
            .collect()
    }

    /// Extract the cheap Rust identity already known to the checker for compatibility checks.
    ///
    /// This must stay metadata-light. `types_compatible(...)` calls it frequently, so it may only use symbol-local
    /// metadata already attached during import collection plus cache-only Rust ABI reads. Fresh
    /// rustc extraction from this path would leak a heavy metadata-loading concern into ordinary semantic
    /// checks. Transparent rusttypes and Rust aliases use their retained native targets before identity comparison.
    fn rust_identity_for_type(&self, ty: &ResolvedType) -> Option<RustTypeIdentity> {
        if let Some(target) = self.rusttype_native_target(ty) {
            return self.rust_identity_for_type(&target);
        }
        match ty {
            ResolvedType::RustPath(path) => {
                let (base, args) = self.rust_path_base_and_args(path);
                let metadata = self.known_rust_metadata_for_path(base.as_str());
                if metadata.as_ref().is_some_and(
                    |metadata| matches!(&metadata.kind, RustItemKind::Type(info) if info.alias_target.is_some()),
                ) {
                    let displays = args.iter().map(render_resolved_type_as_rust_arg).collect::<Vec<_>>();
                    let arguments = displays.iter().map(String::as_str).collect::<Vec<_>>();
                    if let Some(target) = self.rust_alias_expansion(&base, &arguments)
                        && target != *path
                    {
                        return self.rust_identity_for_type(&ResolvedType::RustPath(target));
                    }
                }
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((base, definition, args, defaults))
            }
            ResolvedType::Named(name) => {
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let metadata = info
                    .metadata
                    .clone()
                    .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    Vec::new(),
                    defaults,
                ))
            }
            ResolvedType::Generic(name, args) => {
                if name.contains("::") {
                    let arguments = args
                        .iter()
                        .map(render_resolved_type_as_rust_arg)
                        .collect::<Vec<_>>()
                        .join(", ");
                    return self.rust_identity_for_type(&ResolvedType::RustPath(format!("{name}<{arguments}>")));
                }
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                let metadata = info
                    .metadata
                    .clone()
                    .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
                let definition = metadata.as_ref().and_then(|metadata| metadata.definition_path.clone());
                let defaults = self.rust_type_param_defaults(metadata.as_deref());
                Some((
                    Self::normalize_rust_namespace_path(info.path.as_str()).to_string(),
                    definition,
                    args.clone(),
                    defaults,
                ))
            }
            _ => None,
        }
    }

    /// Resolve a bare Rust display name through imported Rust items when the local scope makes that identity unique.
    ///
    /// Rust metadata can report field and payload displays relative to the Rust module that owns the item, for example
    /// a field displayed as `Payload` while Incan imported the same Rust type as `DemoPayload`. Source aliases are
    /// display projections; the Rust boundary still needs the canonical Rust identity so local/import/reexport
    /// paths do not diverge.
    fn unique_imported_rust_item_path_for_display_tail(&self, display_tail: &str) -> Option<String> {
        if display_tail.is_empty() {
            return None;
        }
        // A qualified display names an item outright. Match it against the imported items by ancestral identity so
        // metadata's `alloc::boxed::Box` finds a source-level `from rust::std::boxed import Box`, and hand back the
        // import's own spelling so later nominal checks line up with the symbol the source actually bound.
        if display_tail.contains("::") {
            let wanted = incan_lang::interop::ancestral_rust_path(display_tail);
            let mut matches = HashSet::new();
            for symbol in self.symbols.all_symbols() {
                let SymbolKind::RustItem(info) = &symbol.kind else {
                    continue;
                };
                if matches!(info.binding, RustImportBindingKind::CrateRoot) {
                    continue;
                }
                let path = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
                if incan_lang::interop::ancestral_rust_path(path.as_str()) == wanted {
                    matches.insert(path);
                }
            }
            let mut iter = matches.into_iter();
            let first = iter.next()?;
            return if iter.next().is_none() { Some(first) } else { None };
        }
        let mut matches = HashSet::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            if matches!(info.binding, RustImportBindingKind::CrateRoot) {
                continue;
            }
            let path = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
            if path.rsplit("::").next() == Some(display_tail) {
                matches.insert(path);
                continue;
            }
            if let Some(metadata) = info.metadata.as_ref() {
                let canonical_path = Self::normalize_rust_namespace_path(metadata.canonical_path.as_str()).to_string();
                if canonical_path.rsplit("::").next() == Some(display_tail) {
                    matches.insert(canonical_path);
                }
            }
        }
        let mut iter = matches.into_iter();
        let first = iter.next()?;
        if iter.next().is_none() { Some(first) } else { None }
    }

    /// Resolve a source-level rusttype backing type to its canonical Rust path spelling.
    pub fn rust_path_for_rusttype_underlying(&self, ty: &ResolvedType) -> Option<String> {
        if let ResolvedType::RustPath(path) = ty {
            return Some(path.clone());
        }

        let (base, _definition, args, _defaults) = self.rust_identity_for_type(ty)?;
        if args.is_empty() {
            return Some(base);
        }

        let rendered_args = args
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{base}<{rendered_args}>"))
    }

    /// Bind a nominal declaration to the exact artifact already admitted for this consumer dependency route.
    ///
    /// `source_path` is the path the caller reached the type through, which for a re-export is the hop it forwards
    /// through. When the library's identity graph publishes the declaration `canonical` names, the identity carries
    /// the declaration's own source path instead, so every public path of one type yields one identity.
    fn admitted_public_type_identity(
        &self,
        library: &str,
        source_path: &[String],
        canonical: Option<CanonicalSymbolId>,
    ) -> PublicLibraryTypeIdentity {
        let declaration_path = match (&canonical, self.loaves.manifest(library)) {
            (Some(canonical), Some(manifest)) => manifest
                .contract_metadata
                .identity_graph
                .declaration_source_path(canonical),
            _ => None,
        };
        let mut identity =
            PublicLibraryTypeIdentity::new(library, declaration_path.unwrap_or(source_path)).with_canonical(canonical);
        // The declaring Loaf of a re-exported type may be another Loaf than the one it was imported through.
        identity.selected_provider = match &identity.canonical {
            Some(canonical) => self.loaves.declaring_loaf(canonical),
            None => self.loaves.entry(library),
        }
        .map(|entry| entry.artifact.identity.clone());
        identity
    }

    /// Retain foreign nominal origins from accepted import bindings, without looking up source names again.
    pub fn checked_nominal_type_origins(
        &self,
    ) -> std::collections::BTreeMap<String, crate::library_manifest::NominalTypeOriginExport> {
        self.public_library_type_identities
            .iter()
            .filter_map(|(name, identity)| {
                let provider = identity.selected_provider.as_ref()?;
                let canonical = identity.canonical.as_ref()?;
                let incan_semantics_core::SymbolOrigin::Package { library, .. } = &canonical.origin else {
                    return None;
                };
                if library != &provider.name {
                    return None;
                }
                Some((
                    name.clone(),
                    crate::library_manifest::NominalTypeOriginExport {
                        provider: provider.clone(),
                        canonical: crate::library_manifest::CanonicalIdentityExport::from_canonical(
                            library, canonical,
                        )?,
                    },
                ))
            })
            .collect()
    }

    /// Return the canonical public-library identity and generic arguments carried by one named type or trait spelling.
    fn public_library_type_identity_for_type<'a>(
        &'a self,
        ty: &'a ResolvedType,
    ) -> Option<(&'a PublicLibraryTypeIdentity, &'a [ResolvedType])> {
        match ty {
            ResolvedType::Named(name) => self
                .public_library_type_identities
                .get(name)
                .map(|identity| (identity, &[][..])),
            ResolvedType::Generic(name, args) => self
                .public_library_type_identities
                .get(name)
                .map(|identity| (identity, args.as_slice())),
            _ => None,
        }
    }

    /// Compare provider-qualified public-library identities before ordinary nominal spelling equality.
    ///
    /// Returning `None` leaves non-provider types and distinct trait-related declarations on the ordinary compatibility
    /// path, where adoption and supertrait relations are checked. Equal provider identities compare their generic
    /// arguments recursively; distinct nominal owners remain incompatible.
    fn public_library_type_identities_compatible(
        &self,
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> Option<bool> {
        let (actual_identity, actual_args) = self.public_library_type_identity_for_type(actual)?;
        let (expected_identity, expected_args) = self.public_library_type_identity_for_type(expected)?;
        if actual_identity != expected_identity {
            if [actual_identity, expected_identity].iter().any(|identity| {
                identity
                    .canonical
                    .as_ref()
                    .is_some_and(|canonical| canonical.kind == SemanticSourceTargetKind::Trait)
            }) {
                return None;
            }
            return Some(false);
        }
        if actual_args.len() != expected_args.len() {
            return Some(false);
        }
        Some(
            actual_args
                .iter()
                .zip(expected_args)
                .all(|(actual_arg, expected_arg)| self.types_compatible(actual_arg, expected_arg)),
        )
    }

    /// Return whether two Rust type identities describe the same boundary type.
    fn rust_type_identities_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> Option<bool> {
        if let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = expected
            && let Some(matches) = self.rust_type_identities_compatible(actual, inner)
        {
            return Some(matches);
        }
        let (actual_path, actual_def, actual_args, actual_defaults) = self.rust_identity_for_type(actual)?;
        let (expected_path, expected_def, expected_args, expected_defaults) = self.rust_identity_for_type(expected)?;
        let same_base = actual_path == expected_path;
        let same_definition =
            actual_def.is_some() && expected_def.is_some() && actual_def.as_ref() == expected_def.as_ref();
        let actual_resolves_to_expected = actual_def.as_deref() == Some(expected_path.as_str());
        let expected_resolves_to_actual = expected_def.as_deref() == Some(actual_path.as_str());
        if !same_base && !same_definition && !actual_resolves_to_expected && !expected_resolves_to_actual {
            // Without concrete definition metadata for both sides, cross-crate Rust paths can still be equivalent
            // through re-exports. Keep the Rust-path boundary permissive instead of reporting a false mismatch.
            if actual_def.is_none() || expected_def.is_none() {
                return None;
            }
            return Some(false);
        }
        let actual_args = self.rust_type_args_with_defaults(actual_args, actual_defaults);
        let expected_args = self.rust_type_args_with_defaults(expected_args, expected_defaults);
        Some(self.rust_type_args_compatible(actual_args.as_slice(), expected_args.as_slice()))
    }

    /// Fill only a contiguous trailing run of inspected Rust default type arguments.
    fn rust_type_args_with_defaults(
        &self,
        mut args: Vec<ResolvedType>,
        defaults: Vec<Option<ResolvedType>>,
    ) -> Vec<ResolvedType> {
        for default in defaults.into_iter().skip(args.len()) {
            let Some(default) = default else {
                break;
            };
            args.push(default);
        }
        args
    }

    /// Return whether Rust generic type arguments are compatible.
    fn rust_type_args_compatible(&self, actual_args: &[ResolvedType], expected_args: &[ResolvedType]) -> bool {
        if actual_args.len() != expected_args.len() {
            return (actual_args.is_empty() && expected_args.iter().all(Self::rust_type_arg_is_unknown_placeholder))
                || (expected_args.is_empty() && actual_args.iter().all(Self::rust_type_arg_is_unknown_placeholder));
        }
        actual_args.iter().zip(expected_args.iter()).all(|(actual, expected)| {
            Self::rust_type_arg_is_unknown_placeholder(actual)
                || Self::rust_type_arg_is_unknown_placeholder(expected)
                || self.types_compatible(actual, expected)
        })
    }

    /// Return whether a Rust type argument is an unknown placeholder.
    fn rust_type_arg_is_unknown_placeholder(arg: &ResolvedType) -> bool {
        match arg {
            ResolvedType::Unknown => true,
            ResolvedType::RustPath(path) => {
                matches!(
                    path.trim().as_bytes(),
                    [b'?'] | [b'{', b'u', b'n', b'k', b'n', b'o', b'w', b'n', b'}']
                )
            }
            _ => false,
        }
    }

    /// Whether a Rust signature parameter is the implicit receiver (`self`/`&self`/`&mut self`).
    pub fn rust_param_is_receiver(param: &RustParam) -> bool {
        if param.name.as_deref() == Some("self") {
            return true;
        }
        let normalized = param.type_display.replace(' ', "");
        matches!(
            normalized.as_str(),
            "self" | "&self" | "&mutself" | "Self" | "&Self" | "&mutSelf"
        )
    }

    /// Whether a Rust function signature starts with an implicit receiver parameter.
    pub fn rust_signature_has_receiver(sig: &RustFunctionSig) -> bool {
        sig.params.first().is_some_and(Self::rust_param_is_receiver)
    }

    /// Build a Rust function type from a signature whose displays may use `crate::` relative to `rust_path`.
    pub fn resolved_function_type_from_rust_sig_for_owner_path(
        &self,
        sig: &RustFunctionSig,
        drop_receiver: bool,
        rust_path: &str,
    ) -> ResolvedType {
        let skip = usize::from(drop_receiver && Self::rust_signature_has_receiver(sig));
        let params = sig
            .params
            .iter()
            .skip(skip)
            .map(|p| {
                CallableParam::positional(
                    self.resolved_param_type_from_rust_display_for_owner_path(p.type_display.as_str(), rust_path),
                )
            })
            .collect();
        let ret_display = self.rust_display_for_owner_path(sig.return_type.as_str(), rust_path);
        let ret = self.resolved_type_from_rust_display(ret_display.as_str());
        ResolvedType::Function(params, Box::new(ret))
    }

    /// Replace Rust metadata's `Self` placeholder with one concrete receiver type at a call site.
    fn substitute_rust_self_type_with_resolved(ty: ResolvedType, receiver: &ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::RustPath(path) if path == "Self" => receiver.clone(),
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(Self::substitute_rust_self_type_with_resolved(
                *inner, receiver,
            ))),
            ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name,
                args.into_iter()
                    .map(|arg| Self::substitute_rust_self_type_with_resolved(arg, receiver))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::substitute_rust_self_type_with_resolved(item, receiver))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(Self::substitute_rust_self_type_with_resolved(*key, receiver)),
                Box::new(Self::substitute_rust_self_type_with_resolved(*value, receiver)),
            ),
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .into_iter()
                    .map(|param| CallableParam {
                        ty: Self::substitute_rust_self_type_with_resolved(param.ty, receiver),
                        ..param
                    })
                    .collect(),
                Box::new(Self::substitute_rust_self_type_with_resolved(*ret, receiver)),
            ),
            other => other,
        }
    }

    /// Replace Rust metadata's `Self` placeholder with the concrete receiver path at a call site.
    fn substitute_rust_self_type(ty: ResolvedType, rust_path: &str) -> ResolvedType {
        Self::substitute_rust_self_type_with_resolved(ty, &ResolvedType::RustPath(rust_path.to_string()))
    }

    /// Build a Rust function type and resolve `Self` return or parameter positions against `rust_path`.
    pub fn resolved_function_type_from_rust_sig_for_path(
        &self,
        sig: &RustFunctionSig,
        drop_receiver: bool,
        rust_path: &str,
    ) -> ResolvedType {
        Self::substitute_rust_self_type(
            self.resolved_function_type_from_rust_sig_for_owner_path(sig, drop_receiver, rust_path),
            rust_path,
        )
    }

    /// Render `path` with generic arguments as `path<A, B, ...>` for embedding in [`ResolvedType::RustPath`].
    ///
    /// When `args` is empty, returns `path` unchanged (no angle brackets).
    fn render_rust_shape_path(path: &str, args: &[RustTypeShape]) -> String {
        render_rust_type_shape_path(path, args)
    }

    /// Detect whether a Rust display type starts with `&T` or `&mut T`.
    ///
    /// Returns the mutability flag plus the remaining inner type spelling so [`Self::resolved_type_from_rust_display`]
    /// can preserve borrow semantics for Rust-backed values instead of collapsing them into plain path types.
    /// Rust permits a tuple or slice delimiter immediately after `mut`; identifier continuations stay part of a name.
    fn rust_display_borrow_kind(display: &str) -> Option<(bool, &str)> {
        let after_amp = display.trim().strip_prefix('&')?.trim_start();
        if let Some(inner) = after_amp.strip_prefix("mut")
            && inner
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, '(' | '[' | '&' | '*'))
        {
            return Some((true, inner.trim_start()));
        }
        Some((false, after_amp))
    }

    /// Remove trailing callable marker bounds such as `+ Send + Sync` from a Rust callable-bound display.
    fn trim_top_level_rust_bound_suffix(text: &str) -> &str {
        let mut angle = 0usize;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        for (idx, ch) in text.char_indices() {
            match ch {
                '<' => angle += 1,
                '>' => angle = angle.saturating_sub(1),
                '(' => paren += 1,
                ')' => paren = paren.saturating_sub(1),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                '+' if angle == 0 && paren == 0 && bracket == 0 => return text[..idx].trim(),
                _ => {}
            }
        }
        text.trim()
    }

    /// Return the body of a Rust callable bound display such as `impl FnMut(A) -> B`.
    pub(in crate::typechecker) fn rust_callable_bound_body(display: &str) -> Option<&str> {
        let trimmed = Self::trim_top_level_rust_bound_suffix(display.trim());
        for prefix in ["impl ", "dyn "] {
            if let Some(rest) = trimmed.strip_prefix(prefix)
                && matches!(rest, r if r.starts_with("Fn") || r.starts_with("std::ops::Fn") || r.starts_with("core::ops::Fn"))
            {
                return Some(rest.trim());
            }
        }
        for compact_prefix in ["impl", "dyn"] {
            if let Some(rest) = trimmed.strip_prefix(compact_prefix)
                && matches!(rest, r if r.starts_with("Fn") || r.starts_with("std::ops::Fn") || r.starts_with("core::ops::Fn"))
            {
                return Some(rest.trim());
            }
        }
        if trimmed.starts_with("Fn") || trimmed.starts_with("std::ops::Fn") || trimmed.starts_with("core::ops::Fn") {
            Some(trimmed)
        } else {
            None
        }
    }

    /// Return the byte index of the matching close paren for an opening paren at the start of `text`.
    pub(in crate::typechecker) fn rust_callable_args_end(text: &str) -> Option<usize> {
        let mut depth = 0usize;
        for (idx, ch) in text.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(idx);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Resolve a safe Rust function pointer or callable bound display into an Incan function type.
    ///
    /// Rust generic callback parameters often surface as `impl FnMut(&mut T, &U)` or through a generic bound that is
    /// normalized to that display. Treating those as ordinary type variables lets by-value Incan function values reach
    /// rustc and fail there. This resolver preserves the callback input borrow shape so the frontend can reject
    /// incompatible named callbacks before emission.
    fn resolved_function_type_from_rust_callable_bound_display(&self, rust_ty: &str) -> Option<ResolvedType> {
        let after_name = if let Some(pointer) = rust_ty.trim().strip_prefix("fn") {
            pointer.trim_start()
        } else {
            let body = Self::rust_callable_bound_body(rust_ty)?;
            let body = body
                .strip_prefix("std::ops::")
                .or_else(|| body.strip_prefix("core::ops::"))
                .unwrap_or(body);
            ["FnOnce", "FnMut", "Fn"]
                .into_iter()
                .find_map(|name| body.strip_prefix(name))?
                .trim_start()
        };
        if !after_name.starts_with('(') {
            return None;
        }
        let args_end = Self::rust_callable_args_end(after_name)?;
        let args_inner = &after_name[1..args_end];
        let params = Self::split_top_level_generic_args(args_inner)
            .into_iter()
            .map(|arg| CallableParam::positional(self.resolved_param_type_from_rust_display(arg)))
            .collect::<Vec<_>>();
        let after_args = after_name[args_end + 1..].trim_start();
        let ret = if let Some(ret_display) = after_args.strip_prefix("->") {
            let ret_display = Self::trim_top_level_rust_bound_suffix(ret_display);
            self.resolved_type_from_rust_display(ret_display)
        } else {
            ResolvedType::Unit
        };
        Some(ResolvedType::Function(params, Box::new(ret)))
    }

    /// Remove Rust lifetime labels that decorate borrowed display types.
    ///
    /// rust-analyzer commonly prints borrowed method parameters as `&'h str` or `&'a [u8]`. The typechecker only needs
    /// the ownership shape and payload type, so erase the lifetime after `&` before whitespace normalization turns it
    /// into an unparseable token such as `&'hstr`.
    fn strip_borrow_lifetimes(rust_ty: &str) -> String {
        strip_rust_borrow_lifetimes(rust_ty)
    }

    /// Return Rust display text with lifetime parameters removed.
    fn rust_display_without_lifetimes(rust_ty: &str) -> String {
        Self::strip_borrow_lifetimes(rust_ty)
            .replace("'static ", "")
            .trim_start_matches("::")
            .to_string()
    }

    /// Return compact Rust display text for comparison. Bring a Rust type display into the one normal form every
    /// lookup and comparison in the checker uses.
    ///
    /// Lifetimes and formatting whitespace are removed while preserving keyword separation around `as` and after `dyn`,
    /// `impl` or `mut`. These spaces keep qualified associated types, trait objects and exclusive borrows parseable.
    /// Every route that compacts a display must use this normalization; different parameter and argument normalization
    /// previously made `Arc<dyn TableProvider>` fail to match itself (#1229).
    fn compact_rust_display(rust_ty: &str) -> String {
        let display = Self::rust_display_without_lifetimes(rust_ty);
        let display = display.as_str();
        let mut out = String::with_capacity(display.len());
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String| {
            if !word.is_empty() {
                if word == "as" && !out.ends_with('#') {
                    out.push(' ');
                }
                out.push_str(word);
                word.clear();
            }
        };
        let mut chars = display.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == ' ' {
                let keep = matches!(word.as_str(), "dyn" | "impl" | "mut" | "as")
                    && chars
                        .peek()
                        .is_some_and(|next| next.is_alphanumeric() || *next == '_' || *next == ':');
                flush(&mut word, &mut out);
                if keep {
                    out.push(' ');
                }
                continue;
            }
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                flush(&mut word, &mut out);
                out.push(ch);
            }
        }
        flush(&mut word, &mut out);
        out
    }

    /// Expand a Rust type alias applied to `args` into its target display, or `None` when `base` names no alias.
    ///
    /// The alias item records its target with the alias's own parameters; substituting the written arguments gives the
    /// aliased type exactly, including arms the alias spelling omits. Shipped caches and check-time extraction expose
    /// the same alias contract; a dependency without a shipped cache must retain its target too.
    fn rust_alias_expansion(&self, base: &str, args: &[&str]) -> Option<String> {
        if !base.contains("::") {
            return None;
        }
        let lookup_path = Self::rust_metadata_lookup_path(base)?;
        let metadata = self.known_rust_metadata_for_path(lookup_path)?;
        self.record_rust_metadata_query(base);
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let target = info.alias_target.as_deref()?;
        if info.type_params.len() != args.len() {
            return None;
        }
        Some(Self::substitute_rust_display_params(&target, &info.type_params, args))
    }

    /// Replace each binder of `display` that names one of `params` with the matching argument display, through
    /// [`TypeChecker::substitute_rust_binders`].
    fn substitute_rust_display_params(display: &str, params: &[String], args: &[&str]) -> String {
        let binder = |word: &str| {
            params
                .iter()
                .position(|param| param == word)
                .and_then(|index| args.get(index).copied())
        };
        Self::substitute_rust_binders(display, binder, None)
    }

    /// The one binder-substitution tokenizer for Rust type displays.
    ///
    /// Each identifier that `binder` maps is replaced by its display, unless `::` or `'` precedes it: a path segment
    /// (`a::T`) or a lifetime (`'a`) is not a binder, while the owner of a projection (`T::Item`) is. `Self` becomes
    /// `receiver` when the caller passes one.
    pub(in crate::typechecker) fn substitute_rust_binders<'a>(
        display: &str,
        binder: impl Fn(&str) -> Option<&'a str>,
        receiver: Option<&'a str>,
    ) -> String {
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        let mut out = String::with_capacity(display.len());
        let mut rest = display;
        while let Some(start) = rest.find(is_word) {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let end = tail.find(|ch: char| !is_word(ch)).unwrap_or(tail.len());
            let word = &tail[..end];
            let replacement = match (receiver, word) {
                _ if out.ends_with("::") || out.ends_with('\'') => None,
                (Some(receiver), "Self") => Some(receiver),
                _ => binder(word),
            };
            out.push_str(replacement.unwrap_or(word));
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    }

    /// Split a Rust generic display type into base and arguments.
    fn rust_generic_base_and_args(normalized: &str) -> Option<(&str, Vec<&str>)> {
        let start = normalized.find('<')?;
        if !normalized.ends_with('>') {
            return None;
        }
        let base = normalized[..start].trim();
        let inner = &normalized[start + 1..normalized.len() - 1];
        Some((base, Self::split_top_level_generic_args(inner)))
    }

    /// Classify collection vocabulary only when a qualified storage owner has a canonical native collection identity.
    /// A foreign ADT named `Vec` remains nominal; its spelling grants neither list storage nor list method lookup.
    fn rust_collection_id_from_display_base(base: &str) -> Option<CollectionTypeId> {
        let collection = incan_lang::lang::types::collections::from_rust_display_base(base)?;
        let native_owner = match collection {
            CollectionTypeId::List => {
                incan_lang::interop::ancestral_rust_path(base) == incan_lang::interop::ancestral_rust_path("Vec")
            }
            CollectionTypeId::Dict | CollectionTypeId::Set => {
                incan_lang::interop::RustCollectionFamily::for_canonical_path(base).is_some()
            }
            _ => true,
        };
        (!base.contains("::") || native_owner).then_some(collection)
    }

    /// Return the resolved Rust display type for a structural parameter.
    fn resolved_structural_rust_param_display<F>(&self, normalized: &str, mut resolve_arg: F) -> Option<ResolvedType>
    where
        F: FnMut(&Self, &str) -> ResolvedType,
    {
        let (base, arg_displays) = Self::rust_generic_base_and_args(normalized)?;
        let collection_id = Self::rust_collection_id_from_display_base(base)?;
        let args = arg_displays
            .into_iter()
            .map(|arg| resolve_arg(self, arg))
            .collect::<Vec<_>>();
        Self::resolved_structural_rust_arguments(collection_id, args).ok()
    }

    /// Construct the source view of a canonical native collection from already resolved arguments. A rejected arity
    /// returns the original arguments so native inference can retain its nominal owner without copying or rendering
    /// checked source identities back through Rust display text.
    fn resolved_structural_rust_arguments(
        collection_id: CollectionTypeId,
        mut args: Vec<ResolvedType>,
    ) -> Result<ResolvedType, Vec<ResolvedType>> {
        match collection_id {
            CollectionTypeId::List if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::List).to_string(),
                args,
            )),
            CollectionTypeId::Dict if args.len() == 2 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Dict).to_string(),
                args,
            )),
            CollectionTypeId::Set if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Set).to_string(),
                args,
            )),
            CollectionTypeId::Option if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Option).to_string(),
                args,
            )),
            CollectionTypeId::Result if args.len() <= 2 => {
                let ok = args.first().cloned().unwrap_or(ResolvedType::Unknown);
                let err = args.get(1).cloned().unwrap_or(ResolvedType::Unknown);
                Ok(ResolvedType::Generic(
                    collection_name(CollectionTypeId::Result).to_string(),
                    vec![ok, err],
                ))
            }
            CollectionTypeId::Tuple => Ok(ResolvedType::Tuple(args)),
            CollectionTypeId::FrozenList if args.len() == 1 => Ok(ResolvedType::FrozenList(Box::new(args.remove(0)))),
            CollectionTypeId::FrozenSet if args.len() == 1 => Ok(ResolvedType::FrozenSet(Box::new(args.remove(0)))),
            CollectionTypeId::FrozenDict if args.len() == 2 => {
                let value = args.pop().unwrap_or(ResolvedType::Unknown);
                let key = args.pop().unwrap_or(ResolvedType::Unknown);
                Ok(ResolvedType::FrozenDict(Box::new(key), Box::new(value)))
            }
            CollectionTypeId::Generator if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Generator).to_string(),
                args,
            )),
            _ => Err(args),
        }
    }

    /// Map structured Rust metadata [`RustTypeShape`] into a [`ResolvedType`] for field access and pattern typing.
    ///
    /// `Option`/`Result` become [`ResolvedType::Generic`] with constructor names `Option` and `Result`. Concrete paths
    /// are rendered through the ordinary Rust-display resolver so a structured metadata path receives the same
    /// primitive classification as its textual fallback (notably owned Rust strings).
    pub fn resolved_type_from_rust_shape(&self, shape: &RustTypeShape) -> ResolvedType {
        match shape {
            RustTypeShape::Never => ResolvedType::Never,
            RustTypeShape::Bool => ResolvedType::Bool,
            RustTypeShape::Float => ResolvedType::Float,
            RustTypeShape::Int => ResolvedType::Int,
            RustTypeShape::Str => ResolvedType::Str,
            RustTypeShape::Bytes => ResolvedType::Bytes,
            RustTypeShape::Unit => ResolvedType::Unit,
            RustTypeShape::Option(inner) => {
                ResolvedType::Generic("Option".to_string(), vec![self.resolved_type_from_rust_shape(inner)])
            }
            RustTypeShape::Result(ok, err) => ResolvedType::Generic(
                "Result".to_string(),
                vec![
                    self.resolved_type_from_rust_shape(ok),
                    self.resolved_type_from_rust_shape(err),
                ],
            ),
            RustTypeShape::Tuple(items) => ResolvedType::Tuple(
                items
                    .iter()
                    .map(|item| self.resolved_type_from_rust_shape(item))
                    .collect(),
            ),
            RustTypeShape::Ref(inner) => ResolvedType::Ref(Box::new(self.resolved_type_from_rust_shape(inner))),
            RustTypeShape::RustPath { path, args } => {
                // `String` has an implementation allocator argument in some rustc metadata views. That argument
                // does not change the language-level string value contract, so retain `str` semantics from the
                // canonical nominal path instead of rendering it as an opaque generic Rust display.
                if rust_display_is_owned_string(path) {
                    return ResolvedType::Str;
                }
                self.resolved_type_from_rust_display(Self::render_rust_shape_path(path, args).as_str())
            }
            RustTypeShape::TypeParam(name) => ResolvedType::TypeVar(name.clone()),
            RustTypeShape::Unknown => ResolvedType::Unknown,
        }
    }

    /// Return checked Rust metadata for one enum variant constructor's callable parameters.
    ///
    /// Rust enum variants are callable constructors at the source surface, but they are not ordinary inherent
    /// functions. Named fields retain their metadata names and declaration order for ordinary argument binding;
    /// tuple payloads remain positional. The same payload contract directs downstream ownership planning.
    pub fn rust_variant_callable_params(&self, rust_path: &str, variant: &str) -> Option<Vec<CallableParam>> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let variant = info.variants.iter().find(|candidate| candidate.name == variant)?;
        Some(
            variant
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let ty = self.resolved_type_from_rust_shape(field);
                    match variant.field_names.get(index) {
                        Some(name) => CallableParam::named(name, ty, ParamKind::Normal),
                        None => CallableParam::positional(ty),
                    }
                })
                .collect(),
        )
    }

    /// Return the storage carrier of each payload field of one Rust enum variant from its metadata.
    ///
    /// The callable parameters above are the semantic payload types; this is the parallel fact lowering needs to
    /// emit the Rust constructor, such as the `Box<T>` prost writes around a recursive `oneof` payload.
    pub fn rust_variant_payload_carriers(&self, rust_path: &str, variant: &str) -> Option<Vec<RustPayloadCarrier>> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let variant = info.variants.iter().find(|candidate| candidate.name == variant)?;
        Some(
            (0..variant.fields.len())
                .map(|index| variant.field_carrier(index))
                .collect(),
        )
    }

    /// Resolve a Rust-origin method signature from cached metadata.
    pub fn rust_method_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        if let RustItemKind::Type(info) = &metadata.kind {
            return info
                .methods
                .iter()
                .find(|m| m.name == method)
                .map(|m| m.signature.clone());
        }
        None
    }

    /// Resolve a Rust-origin associated function signature (must not take `self`) from cached metadata.
    pub fn rust_associated_function_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let sig = self.rust_method_signature(rust_path, method)?;
        if Self::rust_signature_has_receiver(&sig) {
            return None;
        }
        Some(sig)
    }

    /// Convert a Rust display type string into a conservative [`ResolvedType`].
    ///
    /// RFC 041: intentionally best-effort. Rust's diverging `!` return is preserved as [`ResolvedType::Never`]. Common
    /// primitive spellings and `Option` / `Result` wrappers are recognized, including namespaced aliases whose
    /// trailing segment is `Option` or `Result`. Nested generics, lifetimes, and crate paths otherwise become
    /// [`ResolvedType::RustPath`] (or [`ResolvedType::Unknown`] when empty); lowering relies on rustc for fidelity.
    /// Native scalar names precede source import lookup, and metadata-selected transparent aliases retain their
    /// target carrier rather than inventing a nominal layout for a public primitive path.
    ///
    /// ## `Result<T, E>` parsing
    ///
    /// `Result<…>` uses top-level generic splitting, so nested generic or tuple commas stay inside the appropriate
    /// argument. Prefer precise typing from Incan surfaces over relying on this heuristic for arbitrary Rust paths.
    pub fn resolved_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        let normalized = Self::compact_rust_display(display.as_str());
        if normalized == RUST_NEVER_TYPE_DISPLAY {
            return ResolvedType::Never;
        }
        if let Some(source) = self.resolved_source_from_rust_carrier(&normalized) {
            return source;
        }
        // Native scalar names are metadata vocabulary, independent of same-spelled source imports.
        if let Some(id) = numerics::from_str(&normalized)
            && numerics::as_str(id) == normalized
        {
            return match id {
                NumericTypeId::I64 => ResolvedType::Int,
                NumericTypeId::F64 => ResolvedType::Float,
                NumericTypeId::Bool => ResolvedType::Bool,
                id => ResolvedType::Numeric(id),
            };
        }
        // Public primitive re-exports are transparent aliases. A nominal alias keeps its public spelling: its expansion
        // can include default arguments that no public path names.
        if !normalized.contains('<')
            && let Some(expanded) = self.rust_alias_expansion(&normalized, &[])
            && expanded != normalized
            && expanded.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return self.resolved_type_from_rust_display(&expanded);
        }
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some(Symbol {
            kind: SymbolKind::RustItem(info),
            ..
        }) = self.lookup_symbol(normalized.as_str())
            && !matches!(info.binding, RustImportBindingKind::CrateRoot)
        {
            return ResolvedType::RustPath(info.path.clone());
        }
        self.resolved_nested_type_from_rust_display(display, normalized)
    }

    /// Recover an admitted source declaration when Rust returns its native carrier. Published layouts are indexed
    /// by canonical identity even when the consumer imports only a function; no lexical type binding is introduced.
    fn resolved_source_from_rust_carrier(&self, normalized: &str) -> Option<ResolvedType> {
        if !normalized.starts_with(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return None;
        }
        if let Some((_, name, _)) = self.symbols.active_module_source_bindings().find(|(_, name, _)| {
            self.symbols
                .module_type_identity(name)
                .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                .as_deref()
                == Some(normalized)
        }) {
            return Some(ResolvedType::Named(name.to_owned()));
        }
        self.transitive_pub_types
            .contains_key(normalized)
            .then(|| ResolvedType::Named(normalized.to_owned()))
    }

    /// Resolve borrowed, structural and nominal Rust spellings after scalar and callable forms have been admitted.
    /// Nested arguments reenter the same conversion so retained source carriers keep their identity.
    fn resolved_nested_type_from_rust_display(&self, display: String, normalized: String) -> ResolvedType {
        match normalized.as_str() {
            "&str" => return ResolvedType::Str,
            "&[u8]" => return ResolvedType::Bytes,
            _ => {}
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_ty = self.resolved_type_from_rust_display(inner);
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if rust_display_is_owned_string(normalized.as_str()) {
            return ResolvedType::Str;
        }
        if let Some(inner) = display.strip_prefix('(').and_then(|inner| inner.strip_suffix(')'))
            && inner.contains(',')
        {
            return ResolvedType::Tuple(
                Self::split_top_level_generic_args(inner)
                    .into_iter()
                    .map(|element| self.resolved_type_from_rust_display(element))
                    .collect(),
            );
        }
        match normalized.as_str() {
            "{unknown}" => ResolvedType::Unknown,
            "str" | "&str" => ResolvedType::Str,
            "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" | "&[u8]" => ResolvedType::Bytes,
            "()" => ResolvedType::Unit,
            _ if normalized.ends_with('>') => self.resolved_parameterized_rust_display(display, normalized),
            _ if !normalized.is_empty() => {
                if self.lookup_type_info(normalized.as_str()).is_some() {
                    ResolvedType::Named(normalized)
                } else if let Some(path) = self.unique_imported_rust_item_path_for_display_tail(normalized.as_str()) {
                    ResolvedType::RustPath(path)
                } else {
                    ResolvedType::RustPath(normalized)
                }
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Resolve parameterized Rust displays using wrapper aliases, structural facts and source nominal declarations.
    ///
    /// Consume the already normalized strings so the opaque-path fallback keeps the original display without another
    /// copy.
    fn resolved_parameterized_rust_display(&self, display: String, normalized: String) -> ResolvedType {
        if let Some((base, args)) = Self::rust_generic_base_and_args(display.as_str()) {
            let tail = base.rsplit("::").next().unwrap_or(base);
            match collection_type_id(tail) {
                Some(CollectionTypeId::Option) => {
                    let inner = args.first().copied().unwrap_or("");
                    return ResolvedType::Generic(
                        collection_name(CollectionTypeId::Option).to_string(),
                        vec![self.resolved_type_from_rust_display(inner)],
                    );
                }
                Some(CollectionTypeId::Result) => {
                    // A `Result` alias (`std::thread::Result<T>`, `std::io::Result<T>`) fixes the error arm its
                    // display omits; the alias item's recorded target names it.
                    if args.len() != 2
                        && let Some(expanded) = self.rust_alias_expansion(base, &args)
                    {
                        return self.resolved_type_from_rust_display(expanded.as_str());
                    }
                    let ok_ty = args
                        .first()
                        .map(|p| self.resolved_type_from_rust_display(p))
                        .unwrap_or(ResolvedType::Unknown);
                    // Without alias metadata (`datafusion_common::error::Result<T>` outside the inspected
                    // graph) keep the success path semantic and degrade only the
                    // missing error arm.
                    let err_ty = args
                        .get(1)
                        .map(|p| self.resolved_type_from_rust_display(p))
                        .unwrap_or(ResolvedType::Unknown);
                    return ResolvedType::Generic(
                        collection_name(CollectionTypeId::Result).to_string(),
                        vec![ok_ty, err_ty],
                    );
                }
                _ => {}
            }
            if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
                checker.resolved_type_from_rust_display(arg)
            }) {
                return structural;
            }
            if let Some(source) = self.resolved_source_generic_from_rust_display(base, &args) {
                return source;
            }
        }
        if self.lookup_type_info(normalized.as_str()).is_some() {
            ResolvedType::Named(normalized)
        } else {
            ResolvedType::RustPath(display)
        }
    }

    /// Name a source model, class, enum or newtype instance that a Rust display spells with angle brackets
    /// (`Node<T>`), so the checked type keeps the declaration instead of an opaque Rust path. An argument that names a
    /// type parameter of the enclosing declaration stays that parameter.
    fn resolved_source_generic_from_rust_display(&self, base: &str, args: &[&str]) -> Option<ResolvedType> {
        if !matches!(
            self.lookup_semantic_type_info(base),
            Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        let arguments = args
            .iter()
            .map(|argument| {
                let parameter = ResolvedType::Named(argument.trim().to_owned());
                if self.active_type_param_name(&parameter).is_some() {
                    parameter
                } else {
                    self.resolved_type_from_rust_display(argument)
                }
            })
            .collect();
        Some(ResolvedType::Generic(base.to_owned(), arguments))
    }

    /// Return a Rust generic parameter display when rust-analyzer reports a by-value generic boundary.
    ///
    /// Plain type parameters appear as `T` or `U`. Anonymous `impl Trait` parameters arrive as `impl Buf` in the
    /// checker's normal form, or with the whitespace erased (`implBuf`) when a caller compacted the display itself;
    /// both still carry by-value shape and must not be treated as borrowed Rust boundary targets.
    pub fn rust_display_type_var_name(normalized: &str) -> Option<&str> {
        if let Some(tail) = normalized.strip_prefix("impl").map(str::trim_start)
            && !tail.is_empty()
            && (tail.contains("::") || tail.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()))
        {
            return Some(normalized);
        }
        if normalized.len() == 1 && normalized.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
            Some(normalized)
        } else {
            None
        }
    }

    /// Convert a Rust parameter display type into a [`ResolvedType`] while preserving borrow shape.
    ///
    /// `resolved_type_from_rust_display()` intentionally maps borrowed scalar-like returns such as `&str` and `&[u8]`
    /// into owned Incan value types. Parameters need the opposite treatment: the callable signature must remember the
    /// borrowed Rust boundary so emission can pass `&arg` instead of moving an owned `String` or `Vec<u8>`.
    pub fn resolved_param_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        let normalized = Self::compact_rust_display(&display);
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some(name) = Self::rust_display_type_var_name(normalized.as_str()) {
            return ResolvedType::TypeVar(name.to_string());
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_normalized = Self::compact_rust_display(inner);
            let inner_ty = match inner {
                "str" => ResolvedType::Str,
                "[u8]" => ResolvedType::Bytes,
                _ if inner_normalized.starts_with('[') && inner_normalized.ends_with(']') => {
                    let elem = &inner_normalized[1..inner_normalized.len() - 1];
                    ResolvedType::Generic(
                        collection_name(CollectionTypeId::List).to_string(),
                        vec![self.resolved_type_from_rust_display(elem)],
                    )
                }
                _ => self.resolved_type_from_rust_display(inner_normalized.as_str()),
            };
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
            checker.resolved_param_type_from_rust_display(arg)
        }) {
            return structural;
        }
        self.resolved_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust parameter display into a resolved type after expanding crate-relative paths for its owner.
    pub fn resolved_param_type_from_rust_display_for_owner_path(
        &self,
        rust_ty: &str,
        owner_path: &str,
    ) -> ResolvedType {
        let display = self.rust_display_for_owner_path(rust_ty, owner_path);
        self.resolved_param_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust parameter display type into the typed target carried by Rust-boundary coercion metadata.
    ///
    /// This preserves the semantic difference between slice borrow targets such as `&str`/`&[u8]` and borrowed owned
    /// Rust targets such as `&String`/`&Vec<u8>`. Ordinary parameter typing still maps Rust scalar displays onto Incan
    /// value types, but coercion metadata is a backend contract: lowering and emission must be able to choose borrow
    /// versus materialize-then-borrow behavior from this typed target without re-decoding Rust display strings.
    pub fn resolved_rust_boundary_target_from_param_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_ty = match inner {
                "str" => ResolvedType::Str,
                "[u8]" => ResolvedType::Bytes,
                "String" | "std::string::String" | "alloc::string::String" => ResolvedType::RustPath(inner.to_string()),
                "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" => ResolvedType::RustPath(inner.to_string()),
                _ => self.resolved_type_from_rust_display(inner),
            };
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
            checker.resolved_rust_boundary_target_from_param_display(arg)
        }) {
            return structural;
        }
        self.resolved_param_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust boundary target after expanding crate-relative paths for its owner.
    pub fn resolved_rust_boundary_target_from_param_display_for_owner_path(
        &self,
        rust_ty: &str,
        owner_path: &str,
    ) -> ResolvedType {
        let display = self.rust_display_for_owner_path(rust_ty, owner_path);
        self.resolved_rust_boundary_target_from_param_display(display.as_str())
    }

    /// Set the declared Rust crate names from `loaf.toml [rust-dependencies]`.
    ///
    /// When set, `rust.module()` path validation will check that the first segment of the path is either a facet
    /// or a crate declared here.
    pub fn set_declared_crate_names(&mut self, names: HashSet<String>) {
        self.declared_crate_names = Some(names);
    }

    /// Set the compiled Loaves this program imports from: its dependencies and the standard packages (DD-0007).
    pub fn set_loaves(&mut self, loaves: Arc<LoafIndex>) {
        self.loaves = loaves;
    }

    /// Mark the program being checked as the standard package `policy` names.
    pub fn set_own_standard_package(&mut self, policy: &'static StandardPackageNamespacePolicy) {
        self.own_standard_package = Some(policy);
    }

    /// Record the module path owning this checker's declarations.
    ///
    /// Also forwarded to the symbol table, whose RFC 120 identity minting uses it as the [`SymbolOrigin`] of every
    /// locally-declared symbol; `None` resets to the anonymous-module origin.
    pub fn set_current_module_path(&mut self, path: Option<Vec<String>>) {
        self.symbols.set_module_path(path.clone().unwrap_or_default());
        self.current_module_path = path;
    }

    /// Set the defining package for declarations checked for a compiled-library artifact.
    pub fn set_current_package_identity(&mut self, package_identity: Option<String>) {
        self.symbols.set_package_identity(package_identity);
    }

    /// Return accumulated non-fatal diagnostics (warnings/lints) from the last check.
    pub fn warnings(&self) -> &[CompileError] {
        &self.warnings
    }

    /// Return accumulated type information for reuse by later stages (lowering/codegen).
    pub fn type_info(&self) -> &TypeCheckInfo {
        &self.type_info
    }

    /// Hand the accumulated type information to a later stage once checking is over. The opt-in Body IR oracle also
    /// retains indexed immutable owner references; checking and settlement are never repeated.
    pub fn into_type_info(mut self) -> TypeCheckInfo {
        if std::env::var_os("INCAN_BODY_IR_FREE_ORACLE").is_some() {
            self.type_info.body_ir_oracle_inputs = Some(self.body_ir_oracle_inputs());
        }
        self.type_info
    }

    /// Return the checked source path for one active import-derived binding.
    pub fn import_binding_path(&self, local_name: &str) -> Option<&[String]> {
        self.symbols.import_binding_path(local_name)
    }

    /// Record an expression's resolved type and the identities of its completed native storage. Native defaults
    /// must contribute payload identities at the same structural positions that storage publication retains.
    pub fn record_expr_type(&mut self, span: Span, ty: ResolvedType) {
        let mut identities = std::collections::BTreeMap::new();
        let storage = self.rust_storage_resolved(&ty, &self.native_call_type_parameters());
        self.collect_expression_type_identities(&storage, &mut Vec::new(), &mut identities);
        if identities.is_empty() {
            self.type_info.expressions.expression_type_identities.remove(&span);
        } else {
            self.type_info
                .expressions
                .expression_type_identities
                .insert(span, identities);
        }
        self.refuse_unbounded_nominal_expression_type(&ty, span);
        self.type_info.expressions.expr_types.insert(span, ty);
    }

    /// Project named leaves through their checked module, provider or native binding. Structured native displays
    /// recurse through the same type conversion as signatures, so Result payload identities survive native values. A
    /// `rusttype` value retains its inspected native item identity; its source declaration identity does not authorize
    /// a distinct nominal payload.
    fn collect_expression_type_identities(
        &self,
        ty: &ResolvedType,
        path: &mut Vec<usize>,
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        match ty {
            ResolvedType::RustPath(display) => self.collect_rust_expression_type_identities(display, path, identities),
            ResolvedType::Named(name) => {
                if matches!(self.lookup_semantic_type_info(name), Some(TypeInfo::Newtype(info)) if info.is_rusttype)
                    && let Some(native) = self.rust_canonical_path_for_receiver_type(ty)
                {
                    let public = self.type_info.rust.item_public_paths.get(&native).unwrap_or(&native);
                    if let Some(identity) = incan_semantics_core::body_ir::rust_item_identity(public) {
                        identities.insert(path.clone(), identity);
                    }
                    return;
                }
                // Resolved nominal types keep their accepted module/provider binding even when a local value
                // shadows the source alias. Provider-only return types may have no lexical symbol at all.
                let identity = match self.public_library_type_identities.get(name) {
                    Some(binding) => binding.canonical.clone(),
                    None => self
                        .symbols
                        .module_type_identity(name)
                        .or_else(|| self.type_info.declarations.named_type_identities.get(name))
                        .cloned(),
                };
                if let Some(identity) = identity {
                    identities.insert(path.clone(), identity);
                } else if incan_semantics_core::body_ir::rust_item_identity(name).is_some() {
                    self.collect_native_expression_leaf_identity(name, path, identities);
                }
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for (index, ty) in args.iter().enumerate() {
                    path.push(index);
                    self.collect_expression_type_identities(ty, path, identities);
                    path.pop();
                }
            }
            ResolvedType::Ref(ty)
            | ResolvedType::RefMut(ty)
            | ResolvedType::TypeToken(ty)
            | ResolvedType::FrozenList(ty)
            | ResolvedType::FrozenSet(ty) => {
                path.push(0);
                self.collect_expression_type_identities(ty, path, identities);
                path.pop();
            }
            ResolvedType::FrozenDict(key, value) => {
                for (index, ty) in [key, value].into_iter().enumerate() {
                    path.push(index);
                    self.collect_expression_type_identities(ty, path, identities);
                    path.pop();
                }
            }
            // Function signatures and type variables require their own binder environment; this expression-value
            // projection does not invent one after checking. Concrete source annotations retain their own facts.
            _ => {}
        }
    }

    /// Retain native leaf identities at checking, using the same signature type walk as native call carriers.
    /// Opaque generic owners contribute their argument leaves; an exact native item query authorizes a nominal
    /// leaf. Unsupported displays confer no identity, and registered runtime builtins keep their existing authority.
    fn collect_rust_expression_type_identities(
        &self,
        display: &str,
        path: &mut Vec<usize>,
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        let resolved = self.native_rust_signature_type(display, "");
        if resolved != ResolvedType::RustPath(display.to_owned()) {
            self.collect_expression_type_identities(&resolved, path, identities);
            return;
        }
        if let Some((base, arguments)) = Self::rust_generic_base_and_args(display) {
            for (index, argument) in arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .enumerate()
            {
                path.push(index);
                self.collect_expression_type_identities(
                    &self.native_rust_signature_type(argument, base),
                    path,
                    identities,
                );
                path.pop();
            }
            return;
        }
        self.collect_native_expression_leaf_identity(display, path, identities);
    }

    /// Retain a completed native leaf only through its registered runtime owner or inspected Rust declaration.
    /// Both structural default arguments and diagnostic native paths use this single identity admission rule.
    fn collect_native_expression_leaf_identity(
        &self,
        display: &str,
        path: &[usize],
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        if let Some(builtin) = incan_semantics_core::RuntimeBuiltinType::from_carrier_path(display) {
            let identity = self
                .type_info
                .body_ir_runtime_types
                .values()
                .find(|identity| {
                    incan_semantics_core::RuntimeBuiltinType::from_identity(identity).as_ref() == Some(&builtin)
                })
                .cloned()
                .or_else(|| incan_semantics_core::body_ir::rust_item_identity(display));
            if let Some(identity) = identity {
                identities.insert(path.to_vec(), identity);
            }
            return;
        }
        let Some(metadata) = self.rust_item_metadata_for_path(display) else {
            return;
        };
        if matches!(metadata.kind, RustItemKind::Type(_))
            && let Some(identity) = incan_semantics_core::body_ir::rust_item_identity(&metadata.canonical_path)
        {
            identities.insert(path.to_vec(), identity);
        }
    }

    /// Record `destination` as the type the value at `value_span` is written to, when the value's type `value_ty` is
    /// compatible with it and the destination is an `Option` (after alias expansion) or the value is numeric (#1858,
    /// RFC 009).
    ///
    /// The write sites that record this (field and index assignment, a model or class constructor field, `return`,
    /// `yield`, and a `match` arm or `break` value of the construct's type) accept a value of the `Option`'s payload
    /// type, and a numeric value whose type losslessly widens to the numeric type or union member the destination
    /// holds. Lowering wraps that value in the `Some` layers the destination adds and widens it. A value already of an
    /// `Option` destination's type is recorded too; lowering leaves it as it is.
    pub(in crate::typechecker) fn record_value_destination_if_compatible(
        &mut self,
        value_span: Span,
        value_ty: &ResolvedType,
        destination: &ResolvedType,
    ) {
        let destination = self.expand_type_aliases(destination.clone());
        let materializes = !matches!(destination, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            && self.borrowed_source_value_materializes(value_ty);
        let adapts = destination.is_option()
            || (numeric_type_id_for_compat(value_ty).is_some() && *value_ty != destination)
            || materializes;
        if !adapts || !self.types_compatible(value_ty, &destination) {
            return;
        }
        self.type_info
            .expressions
            .value_destination_types
            .insert(value_span, destination);
    }

    /// Whether `borrowed` is a shared reference to a source model, or to a class that derives `Clone`: the language
    /// copies such values on demand, so a write site or call slot that needs an owned value takes a clone of it. A
    /// type imported from Rust keeps its own copy and clone semantics and is never materialized here.
    pub(in crate::typechecker) fn borrowed_source_value_materializes(&self, borrowed: &ResolvedType) -> bool {
        let ResolvedType::Ref(inner) = borrowed else {
            return false;
        };
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = inner.as_ref() else {
            return false;
        };
        if self.rust_canonical_path_for_nominal_receiver(name, None).is_some() {
            return false;
        }
        match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(_)) => true,
            Some(TypeInfo::Class(_)) => self.type_implements_trait(name, builtin_traits::as_str(TraitId::Clone)),
            _ => false,
        }
    }

    /// Record the final checked type selected for one assignment binding.
    pub fn record_assignment_binding_type(&mut self, span: Span, ty: ResolvedType) {
        self.type_info.expressions.assignment_binding_types.insert(span, ty);
    }

    /// Record a source declaration target proven by normal typechecking.
    pub fn record_source_target(
        &mut self,
        span: Span,
        module_path: Vec<String>,
        name: impl Into<String>,
        kind: impl Into<String>,
    ) {
        self.type_info.expressions.source_targets.insert(
            span,
            SourceTargetInfo {
                module_path,
                name: name.into(),
                kind: kind.into(),
            },
        );
    }

    /// Export the RFC 120 identities and active source bindings into lowering-facing artifacts.
    ///
    /// Runs once after declaration collection. Body-local symbols are exported separately after their owner is checked.
    /// The legacy identity-only map keeps local declaration facts available to existing consumers. The
    /// checked-binding map additionally preserves imports and aliases with their target identities and all bindings
    /// introduced by a single declaration, so HIR never has to reconstruct source resolution from spellings.
    fn export_module_declaration_bindings(&mut self) {
        for (span, identity) in self.symbols.local_declaration_identities() {
            self.type_info
                .declarations
                .declaration_identities
                .insert(span, identity.clone());
        }
        self.type_info.declarations.hir_bindings_by_span.clear();
        let bindings = self
            .symbols
            .active_module_source_bindings()
            .map(|(span, name, canonical)| {
                (
                    span,
                    CheckedSourceBinding {
                        local_name: name.to_string(),
                        canonical: canonical.cloned(),
                    },
                )
            })
            .collect::<Vec<_>>();
        for (span, binding) in bindings {
            self.type_info
                .declarations
                .hir_bindings_by_span
                .entry(span)
                .or_default()
                .push(binding);
        }
    }

    /// Retain body-owned lexical identities after body checking without republishing the module header.
    fn export_nested_declaration_identities(&mut self) {
        for (span, identity) in self.symbols.nested_declaration_identities() {
            self.type_info
                .declarations
                .nested_declaration_identities
                .insert(span, identity.clone());
        }
    }

    /// Snapshot active checked source-import paths for consumers that do not retain the symbol table.
    fn export_checked_import_bindings(&mut self) {
        self.type_info.import_bindings = CheckedImportBindings::from_paths(
            self.symbols
                .active_import_binding_paths()
                .map(|(name, path)| (name.to_string(), path.to_vec())),
        );
    }

    /// Retain one compiler-proven ordinary function call for checked C bridge/facade projection.
    ///
    /// The general semantic source-target map remains the authority for all codegraph targets. This narrow retained
    /// view merely records which active callable made a resolved function call so C-ABI tooling can identify a
    /// public facade without reparsing source or generated Rust.
    pub fn record_c_abi_function_call_target(&mut self, span: Span, target: SourceTargetInfo) {
        let Some(caller) = self.current_c_abi_raw_call_owner.clone() else {
            return;
        };
        self.type_info
            .c_abi
            .function_calls
            .push(CBindingFunctionCall { span, caller, target });
    }

    /// Return the canonical declaration origin for a source binding, if it is part of the supported codegraph target
    /// set.
    pub fn source_target_for_symbol(&self, name: &str, kind: &SymbolKind) -> Option<SourceTargetInfo> {
        if let Some(target) = self.source_import_targets.get(name) {
            return Some(target.clone());
        }
        if matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
            && !self.current_module_function_symbols.contains_key(name)
        {
            return None;
        }
        let current_module_path = self.current_module_path.clone()?;
        let target_kind = Self::source_target_kind_for_symbol(kind)?;
        Some(SourceTargetInfo {
            module_path: current_module_path,
            name: name.to_string(),
            kind: target_kind.to_string(),
        })
    }

    /// Return the codegraph declaration kind for source symbol targets supported by the current semantic artifacts.
    pub fn source_target_kind_for_symbol(kind: &SymbolKind) -> Option<&'static str> {
        match kind {
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) => Some("function"),
            SymbolKind::Type(info) => Self::source_target_kind_for_type_info(info),
            SymbolKind::Capability(_) => Some("capability"),
            _ => None,
        }
    }

    /// Return the codegraph declaration kind for type symbols supported by the current semantic artifacts.
    pub fn source_target_kind_for_type_info(info: &TypeInfo) -> Option<&'static str> {
        match info {
            TypeInfo::Model(_) => Some("model"),
            TypeInfo::Class(_) => Some("class"),
            TypeInfo::Newtype(info) => Some(if info.is_rusttype { "rusttype" } else { "newtype" }),
            TypeInfo::Enum(_) => Some("enum"),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Record a typechecker-proven unpack binding plan for backend lowering.
    pub fn record_fixed_unpack_plan(&mut self, span: Span, plan: FixedUnpackPlan) {
        self.type_info.calls.fixed_unpack_plans.insert(span, plan);
    }

    /// Look up a type by name and return its [`TypeInfo`], if known.
    ///
    /// ## Parameters
    /// - `name`: The type name to look up.
    ///
    /// ## Returns
    /// - `Some(&TypeInfo)`: If `name` resolves to a type symbol.
    /// - `None`: If the symbol is missing or not a type.
    ///
    /// ## Notes
    /// - This helper exists to flatten the common pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    ///   - `match sym.kind { SymbolKind::Type(info) => ... }`
    pub fn lookup_type_info(&self, name: &str) -> Option<&TypeInfo> {
        let Some(id) = self.symbols.lookup(name) else {
            return self.module_qualified_nominal_type_info(name);
        };
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a symbol by name and return a reference to it, if found.
    ///
    /// Collapses the common two-step pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    pub fn lookup_symbol(&self, name: &str) -> Option<&Symbol> {
        self.check_environment().lookup(name).map(|binding| binding.symbol)
    }

    /// Return the generic-parameter name for a type-like placeholder visible in the current scope.
    ///
    /// During declaration collection, generic parameters are recorded as `TypeVar`, but inside function / method bodies
    /// they are introduced into the symbol table as scoped `Type(Builtin)` placeholders so annotations keep resolving.
    /// For operator and method permissiveness (RFC 023 additive backend inference), both representations should behave
    /// as "typevar-like" generic placeholders.
    pub fn generic_placeholder_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        match ty {
            ResolvedType::TypeVar(name) => Some(name.as_str()),
            ResolvedType::Named(name) => {
                let sym = self.lookup_symbol(name)?;
                match &sym.kind {
                    SymbolKind::Type(TypeInfo::Builtin) if sym.scope > 0 => Some(name.as_str()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether `ty` is a generic placeholder (`TypeVar`) or an in-scope type-parameter placeholder.
    pub fn is_generic_placeholder_type(&self, ty: &ResolvedType) -> bool {
        self.generic_placeholder_name(ty).is_some()
    }

    /// Return declared type parameters and explicit trait adoptions for a semantic type.
    fn semantic_type_params_and_adoptions(&self, type_name: &str) -> Option<(&[String], &[TypeBoundInfo])> {
        match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => Some((&model.type_params, &model.trait_adoptions)),
            TypeInfo::Class(class) => Some((&class.type_params, &class.trait_adoptions)),
            TypeInfo::Enum(enum_info) => Some((&enum_info.type_params, &enum_info.trait_adoptions)),
            TypeInfo::Newtype(newtype) => Some((&newtype.type_params, &newtype.trait_adoptions)),
            _ => None,
        }
    }

    /// Infer the concrete instantiation of `trait_name` for `type_name`, if the type adopts that trait.
    ///
    /// Explicit `with Trait[...]` arguments are authoritative. The older positional mapping remains as a fallback for
    /// nullary adoption metadata and derives.
    pub fn instantiated_trait_args_for_type(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        trait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let (type_params, adopted) = self.semantic_type_params_and_adoptions(type_name)?;
        let concrete_subst = crate::resolved_type_subst::type_param_subst_map(type_params, concrete_type_args);

        for adopted_trait in adopted {
            let Some(adopted_info) = self.lookup_trait_adoption_info(adopted_trait) else {
                continue;
            };
            let direct_args: Vec<ResolvedType> = if adopted_trait.type_args.is_empty() {
                concrete_type_args
                    .iter()
                    .take(adopted_info.type_params.len())
                    .cloned()
                    .collect()
            } else {
                adopted_trait
                    .type_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &concrete_subst))
                    .collect()
            };
            if direct_args.len() != adopted_info.type_params.len() {
                continue;
            }

            if self.trait_name_matches(&adopted_trait.name, trait_name) {
                return Some(direct_args);
            }

            let closure = self.semantic_supertrait_closure(&adopted_trait.name);
            let subst = crate::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
            for (supertrait_name, supertrait_args) in closure {
                if !self.trait_name_matches(&supertrait_name, trait_name) {
                    continue;
                }
                let instantiated = supertrait_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                return Some(instantiated);
            }
        }
        None
    }

    /// Whether `supertrait_name` is `subtrait_name` itself or appears in its RFC 042 transitive supertrait closure.
    fn trait_is_supertrait_of(&self, subtrait_name: &str, supertrait_name: &str) -> bool {
        if self.trait_name_matches(subtrait_name, supertrait_name) {
            return true;
        }
        self.semantic_supertrait_closure(subtrait_name)
            .iter()
            .any(|(name, _)| self.trait_name_matches(name, supertrait_name))
    }

    /// Return whether two visible trait spellings refer to the same source trait. Standard imports and retained
    /// public Loaf paths compare through the owning manifest's declaration identity before their source paths.
    fn trait_name_matches(&self, candidate: &str, expected: &str) -> bool {
        if candidate == expected {
            return true;
        }
        let foreign_path = |name: &str| {
            if name.starts_with("::") {
                return Some(name.trim_start_matches("::").to_string());
            }
            self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
                SymbolKind::RustItem(info) => Some(info.path.trim_start_matches("::").to_string()),
                _ => None,
            })
        };
        match (foreign_path(candidate), foreign_path(expected)) {
            (Some(candidate), Some(expected)) => return candidate == expected,
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        match (
            self.resolve_bound_trait_path(candidate),
            self.resolve_bound_trait_path(expected),
        ) {
            (Some(candidate), Some(expected)) => {
                match (
                    self.loaf_member_identity(&candidate.0, &candidate.1)
                        .or_else(|| self.package_trait_declaration(&candidate.0, &candidate.1)),
                    self.loaf_member_identity(&expected.0, &expected.1)
                        .or_else(|| self.package_trait_declaration(&expected.0, &expected.1)),
                ) {
                    (Some(candidate), Some(expected)) => candidate == expected,
                    _ => candidate == expected,
                }
            }
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => {
                candidate.rsplit('.').next().unwrap_or(candidate) == expected.rsplit('.').next().unwrap_or(expected)
            }
        }
    }

    /// Return whether a compiler-owned derive provides the visible trait bound, including imported aliases.
    fn builtin_derive_satisfies_trait(&self, derive: &str, trait_name: &str) -> bool {
        let Some(derive_id) = incan_lang::lang::derives::from_str(derive) else {
            return false;
        };
        let canonical_derive = incan_lang::lang::derives::as_str(derive_id);
        if canonical_derive == trait_name && self.import_binding_path(trait_name).is_none() {
            return self.lookup_semantic_trait_info(trait_name).is_some();
        }
        let Some((module_path, source_trait)) = self.resolve_bound_trait_path(trait_name) else {
            return false;
        };
        source_trait == canonical_derive
            && stdlib::trait_method_module_segments(canonical_derive)
                .is_some_and(|builtin_module_path| builtin_module_path == module_path)
            && self.lookup_semantic_trait_info(trait_name).is_some()
    }

    /// Instantiate `supertrait_name`'s type arguments when `subtrait_name` is known with `subtrait_args`.
    ///
    /// Used for trait-to-supertrait compatibility: a value typed `Subtrait[A1,...]` may appear where
    /// `Supertrait[B1,...]` is required when the supertrait edge is satisfied after substitution.
    fn instantiated_supertrait_args(
        &self,
        subtrait_name: &str,
        subtrait_args: &[ResolvedType],
        supertrait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let sub_info = self.lookup_semantic_trait_info(subtrait_name)?;
        if subtrait_args.len() != sub_info.type_params.len() {
            return None;
        }
        let subst = crate::resolved_type_subst::type_param_subst_map(&sub_info.type_params, subtrait_args);
        if self.trait_name_matches(subtrait_name, supertrait_name) {
            return Some(subtrait_args.to_vec());
        }
        let closure = self.semantic_supertrait_closure(subtrait_name);
        for (name, args) in &closure {
            if !self.trait_name_matches(name, supertrait_name) {
                continue;
            }
            let instantiated: Vec<ResolvedType> = args
                .iter()
                .map(|a| crate::resolved_type_subst::substitute_resolved_type(a, &subst))
                .collect();
            return Some(instantiated);
        }
        None
    }

    /// Whether a concrete named type declares adoption of `trait_name` (RFC 042: includes transitive supertraits).
    ///
    /// Also treats `@derive(T)` as implementing trait `T` when `T` is a visible trait symbol (e.g. `Clone`).
    pub fn type_implements_trait(&self, type_name: &str, trait_name: &str) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return false;
        };
        let (adopted, derives) = match info {
            TypeInfo::Model(m) => (m.trait_adoptions.as_slice(), Some(m.derives.as_slice())),
            TypeInfo::Class(c) => (c.trait_adoptions.as_slice(), Some(c.derives.as_slice())),
            TypeInfo::Enum(e) => (e.trait_adoptions.as_slice(), Some(e.derives.as_slice())),
            TypeInfo::Newtype(n) => (n.trait_adoptions.as_slice(), Some(n.derives.as_slice())),
            _ => return false,
        };
        // The trait the bound names, as a module and a trait name: an adoption that records the module it came from is
        // compared by that identity, so a type declared in another module satisfies `serde.json.Serialize` whatever the
        // declaring module called the trait (`json.Serialize` through its own import, #1887).
        let bound_identity = self.resolve_bound_trait_path(trait_name);
        let package_trait = self.package_trait_identity_of_type(type_name, trait_name);
        // A `pub::` package trait is one declaration under every spelling a consumer gives it (an import alias, a
        // package alias export, a module binding), so an adoption and a bound that name it under different spellings
        // meet through the package's checked identity of each (#1561).
        let bound_package_trait = bound_identity
            .as_ref()
            .and_then(|(module_path, name)| self.package_trait_declaration(module_path, name));
        for t in adopted {
            if self.trait_name_matches(&t.name, trait_name)
                || t.source_name
                    .as_deref()
                    .is_some_and(|source_name| self.trait_name_matches(source_name, trait_name))
                || adoption_names_bound_identity(t, bound_identity.as_ref())
                || package_trait
                    .as_ref()
                    .is_some_and(|(module_path, name)| adoption_names_declaration(t, module_path, name))
                || bound_package_trait
                    .as_ref()
                    .is_some_and(|bound| self.adopted_package_trait(t).as_ref() == Some(bound))
            {
                return true;
            }
            if self
                .semantic_supertrait_closure(&t.name)
                .iter()
                .any(|(name, _)| self.trait_name_matches(name, trait_name))
            {
                return true;
            }
        }
        if let Some(derives) = derives
            && derives
                .iter()
                .any(|derive| self.builtin_derive_satisfies_trait(derive, trait_name))
        {
            return true;
        }
        false
    }

    /// Return the declaring module and name of the trait `trait_name` binds when it and the type `type_name` come from
    /// the same compiled package, in the package's own module paths, which its types' adoptions record.
    ///
    /// A consumer binds the trait under its own spelling (`Picker as Chooser`, a package alias `pub Chooser =
    /// Picker`, or `t.Picker` through `import pub::pickers as t`) and under the package's public namespace, while the
    /// package's types recorded their adoptions in the package's modules (#1561). A type the consumer reaches through
    /// the package's namespace rather than an import of its own (`t.Label(...)`) is named by its public spelling.
    fn package_trait_identity_of_type(&self, type_name: &str, trait_name: &str) -> Option<(Vec<String>, String)> {
        let package_of = |name: &str| {
            let identity = self
                .symbols
                .lookup(name)
                .and_then(|id| self.symbols.identity_of(id))
                .or_else(|| self.public_library_type_identities.get(name)?.canonical.as_ref())
                .cloned()
                .or_else(|| {
                    let (module_path, declaration) = self.resolve_bound_trait_path(name)?;
                    self.package_trait_declaration(&module_path, &declaration)
                })?;
            match identity.origin {
                SymbolOrigin::Package { library, module_path } => {
                    Some((identity.kind, identity.declaration_name, library, module_path))
                }
                _ => None,
            }
        };
        let (trait_kind, declaration_name, trait_library, trait_module) = package_of(trait_name)?;
        let (_, _, type_library, _) = package_of(type_name)?;
        (trait_kind == SemanticSourceTargetKind::Trait && trait_library == type_library)
            .then_some((trait_module, declaration_name))
    }

    /// Return the checked identity of a trait in an admitted package or standard module, as its identity graph
    /// publishes it. Standard and public-package paths select the same declaring identity; an alias export retains
    /// its target. A canonical physical module path can also select that exact published declaration without
    /// requiring a consumer import. An unowned module or a non-trait member grants no adoption.
    fn package_trait_declaration(&self, module_path: &[String], name: &str) -> Option<CanonicalSymbolId> {
        let (manifest, rest) = match module_path {
            [root, library, rest @ ..] if root == PUBLIC_LIBRARY_NAMESPACE => (self.loaves.manifest(library)?, rest),
            [root, rest @ ..] if root == "std" => (self.loaves.owner_of(module_path)?.artifact.manifest.as_ref(), rest),
            _ => return None,
        };
        let public_path = std::iter::once(manifest.name.clone())
            .chain(rest.iter().cloned())
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>();
        manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_path(&public_path)
            .or_else(|| {
                manifest
                    .contract_metadata
                    .identity_graph
                    .exports
                    .iter()
                    .find_map(|export| {
                        let identity = export.canonical.as_ref()?.hydrate()?;
                        matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
                        if library == &manifest.name && module_path.as_slice() == rest)
                        .then_some(identity)
                        .filter(|identity| identity.declaration_name == name)
                    })
            })
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
    }

    /// Return the checked identity of the `pub::` package trait an adoption names, from the package module and the
    /// source name it recorded where the adopting type was declared.
    fn adopted_package_trait(&self, adoption: &TypeBoundInfo) -> Option<CanonicalSymbolId> {
        let module_path = adoption.module_path.as_deref()?;
        let name = adoption
            .source_name
            .as_deref()
            .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
        self.package_trait_declaration(module_path, name)
    }

    /// Explicit `with Trait[...]` entries plus trait-like `@derive` entries and the automatic `Clone` of a `model`,
    /// `class` or `enum`, for method lookup on one of those.
    ///
    /// Every model, class and enum derives `Clone` without `@derive(Clone)`, so `value.clone()` resolves through the
    /// builtin trait whether or not the derive is spelled (#1870).
    pub fn trait_adoptions_for_type_methods(
        &self,
        adopted: &[TypeBoundInfo],
        derives: &[String],
    ) -> Vec<TypeBoundInfo> {
        let automatic_clone = builtin_derives::as_str(DeriveId::Clone);
        self.with_derive_trait_adoptions(adopted, derives.iter().map(String::as_str).chain([automatic_clone]))
    }

    /// Explicit `with Trait[...]` entries of a newtype plus trait-like `@derive` entries and its automatic `Clone`, for
    /// method lookup on it.
    ///
    /// A newtype derives `Clone` automatically when its underlying type implements it (#1754), so `value.clone()`
    /// resolves whenever the derive relation says the newtype is `Clone` (#1870).
    pub(in crate::typechecker) fn newtype_trait_adoptions_for_type_methods(
        &self,
        name: &str,
        info: &NewtypeInfo,
    ) -> Vec<TypeBoundInfo> {
        let clone = builtin_derives::as_str(DeriveId::Clone);
        let automatic_clone = (!info.is_rusttype
            && self.derive_support(&ResolvedType::Named(name.to_string()), DeriveId::Clone)
                == derive_requirements::DeriveSupport::Supported)
            .then_some(clone);
        self.with_derive_trait_adoptions(
            &info.trait_adoptions,
            info.derives.iter().map(String::as_str).chain(automatic_clone),
        )
    }

    /// Append to `adopted` each derive in `derives` that names a trait in scope, once, as a trait adoption.
    fn with_derive_trait_adoptions<'a>(
        &self,
        adopted: &[TypeBoundInfo],
        derives: impl Iterator<Item = &'a str>,
    ) -> Vec<TypeBoundInfo> {
        let mut out = adopted.to_vec();
        for derive in derives {
            if self.lookup_semantic_trait_info(derive).is_some() && !out.iter().any(|adoption| adoption.name == derive)
            {
                out.push(TypeBoundInfo {
                    name: derive.to_string(),
                    source_name: None,
                    type_args: Vec::new(),
                    module_path: None,
                    implementation_type_params: Vec::new(),
                    inferred: false,
                });
            }
        }
        out
    }

    /// Look up a variable binding by name (in any scope) and return its [`VariableInfo`].
    ///
    /// Returns `None` if the symbol is missing or isn't a variable.
    pub fn lookup_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a module static binding by name (in any scope) and return its [`StaticInfo`].
    pub fn lookup_static_info(&self, name: &str) -> Option<&StaticInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Static(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a variable by walking the whole enclosing scope chain, not just the innermost scope.
    ///
    /// This is what plain assignment resolves against. `scopes_and_name_resolution.md` states the contract
    /// directly: "if `x` already exists in any enclosing scope, it is a **reassignment** (so `x` must be mutable)",
    /// and only `let`/`mut` introduce a binding that may shadow an outer one.
    ///
    /// [`Self::lookup_local_variable_info`] answers a different question — "is this name bound *here*" — and is
    /// still the right lookup for checks that are genuinely about the innermost scope. Using it for assignment is
    /// what let a plain assignment inside any block silently create a fresh immutable binding instead of
    /// reassigning the outer one, with no diagnostic even when the types disagreed (#1072).
    pub fn lookup_variable_info_in_scope_chain(&self, name: &str) -> Option<&VariableInfo> {
        let id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a variable binding by name in the current scope only.
    ///
    /// Returns `None` if the symbol is missing, is only present in an enclosing scope, or is not a variable.
    pub fn lookup_local_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a trait by name and return its [`TraitInfo`], if known.
    ///
    /// Returns `None` if the symbol is missing or isn't a trait.
    pub fn lookup_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }

    /// Look up semantic type metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This is intentionally narrower than [`Self::lookup_type_info`]: it is for internal semantic reasoning such as
    /// method lookup and trait compatibility, not for making unimported provider types available in user source.
    pub fn lookup_semantic_type_info(&self, name: &str) -> Option<&TypeInfo> {
        if let Some(info) = self.lookup_type_info(name) {
            return Some(info);
        }
        // A value may shadow the source spelling after an annotation has already selected a nominal type. Retain
        // that accepted type's exact artifact/declaration identity rather than resolving the value as a type again.
        if let Some(identity) = self.public_library_type_identities.get(name)
            && let Some(info) = self
                .public_library_type_identities
                .iter()
                .filter(|(binding, candidate)| {
                    split_canonical_public_library_type_name(binding).is_some() && *candidate == identity
                })
                .find_map(|(binding, _)| self.transitive_pub_types.get(binding).and_then(|infos| infos.first()))
        {
            return Some(info);
        }
        // A project's own transitively-reachable type wins over a stdlib stub of the same name. Both are fallbacks
        // for a name the current module does not declare, but only one of them is a type this program actually
        // built: a user class called `Registry` resolved to `incan_stdlib_core`'s `Registry` and reported its own
        // `pub` field private, because the stub was consulted first.
        //
        // Requiring a single entry is what makes that safe for a bare name, where two dependencies may each export a
        // `Registry` and choosing one silently would resolve a user's type to a stranger's declaration. A
        // provider-qualified key cannot be ambiguous that way -- it already names one dependency and one export --
        // so repeated entries under it are the same declaration reached again, once per hop of the facade chain that
        // republishes it. Counting those repeats as ambiguity left a package model with no resolvable owner, so a
        // method it gets from an adopted trait resolved through no trait at all and emitted a plain Rust method call
        // that nothing in scope provides.
        if let Some(infos) = self.transitive_pub_types.get(name)
            && let Some(first) = infos.first()
            && (infos.len() == 1 || split_canonical_public_library_type_name(name).is_some())
        {
            return Some(first);
        }
        // Only a carrier key can equal a nominal's carrier key; any other name skips the symbol scan.
        if !name.starts_with(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return None;
        }
        for (id, symbol) in self.symbols.all_symbols().iter().enumerate() {
            let SymbolKind::Type(info) = &symbol.kind else { continue };
            if symbol.scope == 0
                && self.symbols.is_active_lookup_binding(id)
                && self
                    .symbols
                    .identity_of(id)
                    .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                    .as_deref()
                    == Some(name)
            {
                return Some(info);
            }
        }
        None
    }

    /// Look up semantic trait metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This keeps imported trait contracts available for internal compatibility checks without widening source-visible
    /// name resolution. A name the module binds to a declared type or a type parameter names that type: a stdlib or
    /// dependency trait that shares the name, and that the module did not import, is not what the name spells
    /// (`class Index` beside the stdlib's `Index` trait).
    pub fn lookup_semantic_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        if let Some(symbol) = self.lookup_symbol(name) {
            match &symbol.kind {
                SymbolKind::Trait(info) => return Some(info),
                SymbolKind::Type(TypeInfo::Builtin) if symbol.scope == 0 => {}
                SymbolKind::Type(_) => return None,
                _ => {}
            }
        }
        let infos = self.transitive_pub_traits.get(name)?;
        (infos.len() == 1).then(|| &infos[0])
    }

    /// Look up the exact trait metadata named by a collected adoption.
    ///
    /// Dependency types retain the spelling used in their declaring module (for example `json.Serialize`) together
    /// with the provider module and source trait name. That qualified spelling is not a consumer binding, so exact
    /// provider metadata is consulted after ordinary semantic lookup rather than leaking the dependency's import into
    /// the consumer scope. It is consulted first when the spelling names another trait here (see
    /// [`Self::adoption_spelling_names_another_trait`]).
    pub fn lookup_trait_adoption_info(&self, adoption: &TypeBoundInfo) -> Option<&TraitInfo> {
        let exact = || {
            let module_path = adoption.module_path.as_ref()?;
            let source_name = adoption
                .source_name
                .as_deref()
                .or_else(|| adoption.name.rsplit('.').next())?;
            self.dependency_module_traits
                .get(&format!("{}.{}", module_path.join("."), source_name))
        };
        if self.adoption_spelling_names_another_trait(adoption)
            && let Some(info) = exact()
        {
            return Some(info);
        }
        self.lookup_semantic_trait_info(&adoption.name).or_else(exact)
    }

    /// Whether the bare name a standard-library trait adoption of another module's type spells names another trait,
    /// or none, in this module (#1561).
    ///
    /// An adoption keeps the spelling of the module that declares the adopting type. A type imported from another
    /// module that adopts `std.derives.comparison.Ord` spells `Ord` there, and in a module that does not import `Ord`
    /// that name is the builtin's methodless stub, so the defaults the type gets from the trait (`__ge__`, `__ne__`)
    /// were not found on it. A standard-library trait reaches a module through an import of the standard library, so a
    /// bare name that no such import binds does not name it here, except in the standard library's own source. The
    /// adopted trait's exact metadata is then read by its module path.
    fn adoption_spelling_names_another_trait(&self, adoption: &TypeBoundInfo) -> bool {
        let under_stdlib = adoption
            .module_path
            .as_ref()
            .and_then(|path| path.first())
            .is_some_and(|root| root == stdlib::STDLIB_ROOT);
        if !under_stdlib || adoption.name.contains('.') || self.checks_standard_library_source() {
            return false;
        }
        !self
            .import_binding_path(&adoption.name)
            .and_then(|bound| bound.first())
            .is_some_and(|root| root == stdlib::STDLIB_ROOT)
    }

    /// The name a method dispatched through a collected adoption names the adopted trait by in this module (#1561).
    ///
    /// An adoption keeps the spelling of the module that declares the adopting type, which may be an import alias
    /// (`from std.derives.comparison import Ord as Ordered`, then `enum Level with Ordered`). Where that spelling names
    /// another trait, or none, here (see [`Self::adoption_spelling_names_another_trait`]), a call of the trait's method
    /// on the imported type names the trait by its declaration's own name beside its module path, as the recorded
    /// dispatch is read in this module: `Level.High.__gt__(Level.Low)` named `comparison::Ordered`, which no module of
    /// the standard library declares (E0433 against the compiled SDK).
    pub(in crate::typechecker) fn adoption_trait_name_here(&self, adoption: &TypeBoundInfo) -> String {
        match adoption.source_name.as_deref() {
            Some(declaration_name) if self.adoption_spelling_names_another_trait(adoption) => {
                declaration_name.to_string()
            }
            _ => adoption.name.clone(),
        }
    }

    /// Return the transitive supertrait closure for one trait using visible symbols first, then cached `pub::`
    /// semantic metadata.
    ///
    /// The returned `(trait_name, type_arguments)` pairs preserve generic substitution along the supertrait chain just
    /// like the local collection-phase closure used for in-module traits.
    pub fn semantic_supertrait_closure(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        if let Some(closure) = self.supertrait_closure.get(trait_name) {
            return closure.clone();
        }
        self.expand_semantic_supertraits_transitively(trait_name)
    }

    /// Expand transitive supertraits for imported trait metadata that never entered the local declaration collector.
    fn expand_semantic_supertraits_transitively(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut work = Vec::new();
        let Some(root) = self.lookup_semantic_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());

        while let Some((supertrait_name, supertrait_args)) = work.pop() {
            let key = format!(
                "{supertrait_name}<{}>",
                supertrait_args
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((supertrait_name.clone(), supertrait_args.clone()));
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let subst =
                crate::resolved_type_subst::type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
            for (nested_name, nested_args) in &supertrait_info.supertraits {
                let mapped = nested_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                work.push((nested_name.clone(), mapped));
            }
        }

        result
    }

    /// Record each source trait adoption clause's checked type arguments by the clause's span.
    ///
    /// An owner's collected adoptions list its `with` clauses first and in source order, followed by derive-implied
    /// adoptions, so the clauses pair with that prefix. A clause whose paired adoption names a different trait or a
    /// different number of arguments records nothing.
    fn record_trait_adoption_arguments_for_lowering(&mut self, program: &Program) {
        let mut recorded = Vec::new();
        for declaration in &program.declarations {
            let (name, clauses) = match &declaration.node {
                Declaration::Model(model) => (&model.name, &model.traits),
                Declaration::Class(class) => (&class.name, &class.traits),
                Declaration::Newtype(newtype) => (&newtype.name, &newtype.traits),
                Declaration::Enum(value) => (&value.name, &value.traits),
                _ => continue,
            };
            let adoptions = match self.lookup_type_info(name) {
                Some(TypeInfo::Model(info)) => &info.trait_adoptions,
                Some(TypeInfo::Class(info)) => &info.trait_adoptions,
                Some(TypeInfo::Newtype(info)) => &info.trait_adoptions,
                Some(TypeInfo::Enum(info)) => &info.trait_adoptions,
                _ => continue,
            };
            for (clause, adoption) in clauses.iter().zip(adoptions) {
                if clause.node.type_args.is_empty()
                    || adoption.name != clause.node.name
                    || adoption.type_args.len() != clause.node.type_args.len()
                {
                    continue;
                }
                recorded.push((clause.span, adoption.type_args.clone()));
            }
        }
        self.type_info.traits.adoption_type_args = recorded.into_iter().collect();
    }

    /// Retain bare source adoptions' implicit arguments for Body IR without changing legacy's explicit snapshot.
    /// Clauses pair with the checked owner's source-ordered adoption prefix, and repeated trait names stay unproven
    /// rather than borrowing another clause's instantiation. The existing semantic helper owns positional mapping and
    /// exact arity; no lowering consumer reconstructs it from parameter spellings.
    fn record_body_ir_bare_adoption_arguments(&mut self, program: &Program) {
        let mut recorded = HashMap::new();
        for declaration in &program.declarations {
            let (name, clauses) = match &declaration.node {
                Declaration::Model(model) => (&model.name, &model.traits),
                Declaration::Class(class) => (&class.name, &class.traits),
                Declaration::Newtype(newtype) => (&newtype.name, &newtype.traits),
                Declaration::Enum(value) => (&value.name, &value.traits),
                _ => continue,
            };
            // ---- Declared owner parameter witnesses ----
            let Some((parameters, adoptions)) = self.semantic_type_params_and_adoptions(name) else {
                continue;
            };
            let owner_arguments: Vec<_> = parameters
                .iter()
                .map(|name| ResolvedType::TypeVar(name.clone()))
                .collect();

            // ---- Bare clauses at one checked instantiation ----
            for (clause, adoption) in clauses.iter().zip(adoptions) {
                if !clause.node.type_args.is_empty() || adoption.name != clause.node.name {
                    continue;
                }
                if adoptions
                    .iter()
                    .filter(|candidate| self.trait_name_matches(&candidate.name, &adoption.name))
                    .count()
                    != 1
                {
                    continue;
                }
                let Some(arguments) = self.instantiated_trait_args_for_type(name, &owner_arguments, &adoption.name)
                else {
                    continue;
                };
                if !arguments.is_empty() {
                    recorded.insert(clause.span, arguments);
                }
            }
        }

        // ---- Body IR-only snapshot ----
        self.type_info.traits.body_ir_adoption_type_args = recorded;
    }

    /// Snapshot declaration metadata into [`TypeCheckInfo`] for backend lowering.
    ///
    /// This records all visible trait and method-alias symbols (local and imported), not just declarations in the
    /// current module, so lowering can resolve supertrait graphs, generic trait arity, and same-type method aliases
    /// across module boundaries. Newtype construction retains the selected hook's checked signature and identity
    /// so imported validation calls use the declaring module's contract.
    fn record_trait_metadata_for_lowering(&mut self) {
        self.type_info.traits.direct_supertraits.clear();
        self.type_info.traits.type_params.clear();
        self.type_info.traits.method_identities.clear();
        self.type_info.traits.default_method_identities.clear();
        self.type_info.declarations.type_method_rebindings.clear();
        self.type_info.declarations.newtype_construction.clear();
        self.type_info.rust.type_aliases.clear();
        self.type_info.derivations.trait_rust_derive_paths = self.dependency_trait_rust_derive_paths.clone();
        let mut method_rebindings = Vec::new();
        let mut newtype_construction = Vec::new();
        let mut adopted_traits = Vec::new();
        for sym in self.symbols.all_symbols() {
            match &sym.kind {
                SymbolKind::Trait(info) => {
                    self.type_info
                        .traits
                        .direct_supertraits
                        .insert(sym.name.clone(), info.supertraits.clone());
                    self.type_info
                        .traits
                        .type_params
                        .insert(sym.name.clone(), info.type_params.clone());
                    for (method_name, method) in &info.methods {
                        self.type_info.traits.record_method(&sym.name, method_name, method);
                    }
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                }
                SymbolKind::Type(TypeInfo::Model(info)) => {
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Class(info)) => {
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Newtype(info)) => {
                    let mut aliases = info.method_rebindings.clone();
                    aliases.extend(info.method_aliases.clone());
                    method_rebindings.push((sym.name.clone(), aliases));
                    if !info.is_rusttype {
                        newtype_construction.push((sym.name.clone(), info.clone()));
                    } else {
                        let alias = ResolvedType::Generic(
                            sym.name.clone(),
                            info.type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
                        );
                        if let Some(template) = self.rusttype_native_target(&alias) {
                            let template = self.rust_storage_resolved(&template, &info.type_params);
                            self.type_info
                                .rust
                                .type_aliases
                                .insert(sym.name.clone(), (info.type_params.clone(), template));
                        }
                    }
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Enum(info)) => {
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                _ => {}
            }
        }
        self.record_standard_trait_metadata_for_lowering();
        for (qualified_trait_name, info) in &self.dependency_module_traits {
            for (method_name, method) in &info.methods {
                self.type_info
                    .traits
                    .record_method(qualified_trait_name, method_name, method);
            }
        }
        for adoption in adopted_traits {
            let Some(info) = self.lookup_trait_adoption_info(&adoption).cloned() else {
                continue;
            };
            for (method_name, method) in &info.methods {
                self.type_info.traits.record_method(&adoption.name, method_name, method);
            }
        }
        self.record_standard_trait_methods();
        for (type_name, aliases) in method_rebindings {
            if !aliases.is_empty() {
                self.type_info
                    .declarations
                    .type_method_rebindings
                    .insert(type_name, aliases);
            }
        }
        self.record_newtype_construction_metadata(newtype_construction);
    }

    /// Consume the staged newtype snapshots after trait methods and rebindings establish their construction contracts.
    fn record_newtype_construction_metadata(&mut self, newtypes: Vec<(String, NewtypeInfo)>) {
        let string_conversion = trait_capabilities::string_try_from();
        for (name, info) in newtypes {
            let concrete_type = if info.type_params.is_empty() {
                ResolvedType::Named(name.clone())
            } else {
                ResolvedType::Generic(
                    name.clone(),
                    info.type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
                )
            };
            let supports_string_conversion = self
                .temporary_trait_capability_supports_type(string_conversion, &concrete_type)
                .unwrap_or(false);
            let checked_constructor = self.validated_newtype_ctor_name(&name, &info);
            let checked_constructor_binding = checked_constructor
                .as_deref()
                .and_then(|constructor| info.methods.get(constructor))
                .map(|method| Self::newtype_constructor_binding(method, concrete_type));
            let checked_constructor_identity = checked_constructor_binding
                .as_ref()
                .and_then(|binding| binding.identity.clone());
            let automatic_derives = self.newtype_automatic_derive_names(&name, &info);
            // The selection itself is not retained; whether native derives cover every `@rust.derive` is.
            let mut derives = info.derives;
            Self::append_implied_derives(&mut derives);
            if self.append_native_rust_derives(&name, &mut derives) {
                self.type_info
                    .declarations
                    .rust_native_derive_types
                    .insert(name.clone());
            }
            self.type_info.declarations.newtype_construction.insert(
                name,
                crate::typechecker::type_info::NewtypeConstructionInfo {
                    automatic_derives,
                    type_params: info.type_params,
                    underlying: info.underlying,
                    checked_constructor,
                    checked_constructor_identity,
                    checked_constructor_binding,
                    constraints: info.constraints,
                    implicit_coercion_enabled: info.implicit_coercion_enabled,
                    supports_string_conversion,
                },
            );
        }
    }

    /// Retain loaded standard supertraits under their declaring module, without exposing their bare names in source.
    /// An imported Ord's Eq slots must survive even when the consumer imports no Eq or declares its own Eq.
    fn record_standard_trait_metadata_for_lowering(&mut self) {
        for (name, info) in &self.standard_traits {
            let Some(module) = info
                .methods
                .values()
                .filter_map(|method| method.identity.as_ref())
                .find_map(|identity| self.standard_declaring_module(identity))
            else {
                continue;
            };
            let key = format!("{}.{name}", module.join("."));
            self.type_info
                .traits
                .direct_supertraits
                .insert(key.clone(), info.supertraits.clone());
            self.type_info
                .traits
                .type_params
                .insert(key.clone(), info.type_params.clone());
            for (method_name, method) in &info.methods {
                self.type_info.traits.record_method(&key, method_name, method);
            }
        }
    }

    /// Record which visible trait slots the standard packages declare, after every slot is recorded.
    ///
    /// Aliases and facade imports keep a slot's provider identity, so the identity's origin decides; a slot of an
    /// ordinary package dependency stays eligible for project trait implementations.
    fn record_standard_trait_methods(&mut self) {
        let loaves = &self.loaves;
        self.type_info.traits.standard_method_identities = self
            .type_info
            .traits
            .method_identities
            .values()
            .filter(|identity| match &identity.origin {
                SymbolOrigin::Package { library, .. } => loaves.is_standard_package(library),
                _ => false,
            })
            .cloned()
            .collect();
    }

    /// Snapshot checked class layouts for backend lowering.
    ///
    /// The symbol table already contains inherited fields from source and compiled-library parents. Persisting that
    /// flattened view makes it the semantic authority for lowering and avoids requiring imported classes to have a
    /// synthetic consumer-side AST declaration.
    fn record_class_layouts_for_lowering(&mut self, program: &Program) {
        let mut layouts = HashMap::new();
        for declaration in &program.declarations {
            let Declaration::Class(class) = &declaration.node else {
                continue;
            };
            let Some(TypeInfo::Class(info)) = self.lookup_type_info(&class.name).cloned() else {
                continue;
            };
            let mut fields = Vec::with_capacity(info.field_order.len());
            let mut missing_fields = Vec::new();
            let mut unmaterializable_defaults = Vec::new();
            for name in &info.field_order {
                let Some(field) = info.fields.get(name) else {
                    missing_fields.push(name.clone());
                    continue;
                };
                let provider_library = info.field_provider_libraries.get(name).cloned();
                let mut canonical_ty = field.ty.clone();
                self.canonicalize_public_library_nominals(&mut canonical_ty);
                if let Err(message) = crate::rust_type_display::validate_emission_display(&canonical_ty) {
                    self.errors.push(
                        CompileError::type_error(
                            format!(
                                "Checked class '{}' has invalid Rust type metadata for field '{}': {}",
                                class.name, name, message
                            ),
                            declaration.span,
                        )
                        .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
                    );
                }
                if let Some(provider) = provider_library.as_deref()
                    && let Err(message) =
                        crate::rust_type_display::qualify_for_manifest(&canonical_ty, Some(provider), &info.type_params)
                {
                    self.errors.push(
                        CompileError::type_error(
                            format!(
                                "Checked class '{}' cannot republish Rust type metadata for field '{}': {}",
                                class.name, name, message
                            ),
                            declaration.span,
                        )
                        .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
                    );
                }
                let default = self.checked_class_field_default(
                    &class.name,
                    &info,
                    name,
                    provider_library.as_deref(),
                    declaration.span,
                    &mut unmaterializable_defaults,
                );
                if field.has_default && default.is_none() {
                    unmaterializable_defaults.push(name.clone());
                    self.errors.push(CompileError::type_error(
                        format!(
                            "Checked class '{}' has no lowering plan for default field '{}'",
                            class.name, name
                        ),
                        declaration.span,
                    ));
                }
                fields.push(type_info::ClassFieldLayoutInfo {
                    name: name.clone(),
                    ty: canonical_ty,
                    surface_type_name: field.surface_type_name.clone(),
                    visibility: field.visibility,
                    default,
                    alias: field.alias.clone(),
                    description: field.description.clone(),
                });
            }
            unmaterializable_defaults.sort();
            unmaterializable_defaults.dedup();
            layouts.insert(
                class.name.clone(),
                type_info::ClassLayoutInfo {
                    is_public: matches!(class.visibility, Visibility::Public),
                    type_params: info.type_params.clone(),
                    fields,
                    missing_fields,
                    unmaterializable_defaults,
                },
            );
        }
        self.type_info.declarations.class_layouts = layouts;
    }

    /// Resolve a field's local or inherited default, preserving provider failures before layout completeness checks.
    fn checked_class_field_default(
        &mut self,
        class_name: &str,
        info: &ClassInfo,
        name: &str,
        provider: Option<&str>,
        span: Span,
        unmaterializable_defaults: &mut Vec<String>,
    ) -> Option<type_info::ClassFieldDefaultInfo> {
        let Some(value) = info.field_default_metadata.get(name) else {
            return info
                .field_defaults
                .get(name)
                .cloned()
                .map(type_info::ClassFieldDefaultInfo::Source);
        };
        if let Some(provider) = provider
            && let Err(message) =
                crate::library_exports::qualify_checked_default_for_manifest(value, provider, &info.type_params)
        {
            unmaterializable_defaults.push(name.to_owned());
            self.errors.push(
                CompileError::type_error(
                    format!(
                        "Checked class '{class_name}' cannot republish default metadata for field '{name}': {message}"
                    ),
                    span,
                )
                .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
            );
        }
        match (provider, value) {
            (Some(library), crate::library_exports::CheckedParamDefault::Unsupported) => {
                unmaterializable_defaults.push(name.to_owned());
                self.errors.push(
                    CompileError::type_error(
                        format!("Class '{class_name}' cannot inherit compiled default for field '{name}' because the provider artifact cannot materialize that expression"), span,
                    ).with_hint(format!("Override '{name}' in '{class_name}' with a local default expression")),
                );
                Some(type_info::ClassFieldDefaultInfo::PublicDependency {
                    library: library.to_owned(),
                    value: value.clone(),
                })
            }
            (Some(library), _) => Some(type_info::ClassFieldDefaultInfo::PublicDependency {
                library: library.to_owned(),
                value: value.clone(),
            }),
            (None, _) => {
                unmaterializable_defaults.push(name.to_owned());
                self.errors.push(
                    CompileError::type_error(
                        format!("Checked class '{class_name}' lost the dependency identity for inherited default field '{name}'"), span,
                    ).with_hint("Re-import the compiled parent through its declared package dependency"),
                );
                None
            }
        }
    }

    /// Snapshot checked model field visibility for backend lowering.
    ///
    /// The parser preserves authored modifiers and applies the compatible public-model default, while the symbol table
    /// is the frontend's effective semantic view. Keeping only that checked view downstream prevents lowering from
    /// independently reconstructing privacy from syntax.
    fn record_model_field_visibilities_for_lowering(&mut self, program: &Program) {
        let mut model_field_visibilities = HashMap::new();
        let mut model_field_types = HashMap::new();
        let mut model_type_private_fields = HashSet::new();
        for declaration in &program.declarations {
            let Declaration::Model(model) = &declaration.node else {
                continue;
            };
            let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) else {
                continue;
            };
            for field in &model.fields {
                if let Some(checked) = info.fields.get(&field.node.name) {
                    model_field_types.insert(field.span, checked.ty.clone());
                }
            }
            model_field_visibilities.insert(
                model.name.clone(),
                info.fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field.visibility))
                    .collect(),
            );
            model_type_private_fields.extend(
                info.fields
                    .iter()
                    .filter(|(_, field)| field.is_type_private)
                    .map(|(name, _)| (model.name.clone(), name.clone())),
            );
        }
        self.type_info.declarations.model_field_types = model_field_types;
        self.type_info.declarations.model_field_visibilities = model_field_visibilities;
        self.type_info.declarations.model_type_private_fields = model_type_private_fields;
    }

    /// Validate local static dependencies before declaration checking.
    ///
    /// `static` initializers may only reference earlier local statics, and dependency cycles are rejected.
    fn validate_static_dependencies(&mut self) {
        if self.static_decls.is_empty() {
            return;
        }

        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        let static_spans: HashMap<String, Span> = self
            .static_decls
            .iter()
            .map(|(static_decl, span)| (static_decl.name.clone(), *span))
            .collect();

        for (static_decl, _) in self.static_decls.clone() {
            let mut deps = HashSet::new();
            let mut visiting_functions = HashSet::new();
            self.collect_static_dependencies_from_expr(&static_decl.value.node, &mut deps, &mut visiting_functions);
            self.collect_static_initializer_static_writes_from_expr(
                &static_decl.value,
                &static_decl.name,
                &mut HashSet::new(),
            );

            if let Some(current_idx) = self.static_decl_positions.get(&static_decl.name).copied() {
                for dep in &deps {
                    if let Some(dep_idx) = self.static_decl_positions.get(dep).copied()
                        && dep_idx >= current_idx
                    {
                        self.errors.push(errors::static_initializer_requires_earlier_static(
                            dep,
                            &static_decl.name,
                            static_decl.value.span,
                        ));
                    }
                }
            }

            graph.insert(static_decl.name.clone(), deps.into_iter().collect());
        }

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum VisitState {
            Visiting,
            Done,
        }

        fn visit(
            name: &str,
            graph: &HashMap<String, Vec<String>>,
            spans: &HashMap<String, Span>,
            state: &mut HashMap<String, VisitState>,
            stack: &mut Vec<String>,
            collected_errors: &mut Vec<CompileError>,
        ) {
            match state.get(name).copied() {
                Some(VisitState::Done) => return,
                Some(VisitState::Visiting) => {
                    if let Some(start_idx) = stack.iter().position(|item| item == name) {
                        let mut cycle = stack[start_idx..].to_vec();
                        cycle.push(name.to_string());
                        collected_errors.push(errors::static_dependency_cycle(
                            &cycle.join(" -> "),
                            spans.get(name).copied().unwrap_or_default(),
                        ));
                    }
                    return;
                }
                None => {}
            }

            state.insert(name.to_string(), VisitState::Visiting);
            stack.push(name.to_string());
            if let Some(deps) = graph.get(name) {
                for dep in deps {
                    visit(dep, graph, spans, state, stack, collected_errors);
                }
            }
            stack.pop();
            state.insert(name.to_string(), VisitState::Done);
        }

        let mut state: HashMap<String, VisitState> = HashMap::new();
        let mut stack = Vec::new();
        for name in graph.keys() {
            visit(name, &graph, &static_spans, &mut state, &mut stack, &mut self.errors);
        }
    }

    /// Recursively collect the names of local static dependencies from `expr` into `deps`.
    fn collect_static_dependencies_from_expr(
        &self,
        expr: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name) => {
                if self.static_decl_positions.contains_key(name) {
                    deps.insert(name.clone());
                }
            }
            Expr::Literal(_) | Expr::SelfExpr => {}
            Expr::Binary(left, _, right) | Expr::Index(left, right) => {
                self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_dependencies_from_expr(&inner.node, deps, visiting_functions);
            }
            Expr::Yield(None) => {}
            Expr::Call(func, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&func.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
                self.collect_local_function_static_dependencies(&func.node, deps, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_dependencies_from_expr(&step.node, deps, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_dependencies_from_expr(&object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_dependencies_from_expr(&partial.target.node, deps, visiting_functions);
                for arg in &partial.args {
                    self.collect_static_dependencies_from_expr(&arg.value.node, deps, visiting_functions);
                }
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_dependencies_from_expr(&scrutinee.node, deps, visiting_functions);
                self.collect_match_static_dependencies(arms, deps, visiting_functions);
            }
            Expr::If(if_expr) => {
                self.collect_if_static_dependencies(if_expr, deps, visiting_functions);
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Expr::Generator(_) | Expr::ListComp(_) | Expr::DictComp(_) => {
                self.collect_comprehension_static_dependencies(expr, deps, visiting_functions);
            }
            Expr::Closure(_, _) => {}
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_dependencies_from_expr(&item.node, deps, visiting_functions);
                }
            }
            Expr::List(items) => {
                for item in items {
                    match item {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                        }
                    }
                }
            }
            Expr::Dict(items) => {
                self.collect_dict_static_dependencies(items, deps, visiting_functions);
            }
            Expr::Constructor(_, args) => {
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                    }
                }
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_dependencies_from_expr(&start.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&end.node, deps, visiting_functions);
            }
            Expr::Surface(surface) => {
                self.collect_surface_static_dependencies(&surface.payload, deps, visiting_functions)
            }
            Expr::VocabBlock(block) => {
                for arg in &block.header_args {
                    self.collect_static_dependencies_from_expr(&arg.node, deps, visiting_functions);
                }
                for stmt in &block.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            // Only expression holes belong to ordinary Incan; the surrounding fragment is DSL-owned (RFC 081).
            Expr::Embedded(fragment) => {
                for hole in fragment.holes() {
                    self.collect_static_dependencies_from_expr(&hole.node, deps, visiting_functions);
                }
            }
        }
    }

    /// Collect match-arm dependencies in guard-before-body order after the scrutinee has been visited.
    fn collect_match_static_dependencies(
        &self,
        arms: &[Spanned<MatchArm>],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                self.collect_static_dependencies_from_expr(&guard.node, deps, visiting_functions);
            }
            match &arm.node.body {
                MatchBody::Expr(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions)
                }
                MatchBody::Block(stmts) => {
                    for stmt in stmts {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
        }
    }

    /// Follow a directly called local helper once on the active stack, leaving closure creation unevaluated.
    fn collect_local_function_static_dependencies(
        &self,
        callee: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        if let Expr::Ident(name) = callee
            && let Some(declaration) = self.local_function_decls.get(name)
            && visiting_functions.insert(name.clone())
        {
            for stmt in &declaration.body {
                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
            }
            visiting_functions.remove(name);
        }
    }

    /// Collect an if expression's condition and both branch bodies in the original traversal order.
    fn collect_if_static_dependencies(
        &self,
        expr: &IfExpr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_dependencies_from_expr(&expr.condition.node, deps, visiting_functions);
        for stmt in &expr.then_body {
            self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
        }
        if let Some(body) = &expr.else_body {
            for stmt in body {
                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
            }
        }
    }

    /// Collect result, source and filter dependencies for the comprehension forms selected by the expression visitor.
    fn collect_comprehension_static_dependencies(
        &self,
        expr: &Expr,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Generator(generator) => {
                self.collect_static_dependencies_from_expr(&generator.expr.node, deps, visiting_functions);
                for clause in &generator.clauses {
                    let expr = match clause {
                        ComprehensionClause::For { iter, .. } => iter,
                        ComprehensionClause::If(condition) => condition,
                    };
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
            }
            Expr::ListComp(comp) => {
                self.collect_static_dependencies_from_expr(&comp.expr.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            Expr::DictComp(comp) => {
                self.collect_static_dependencies_from_expr(&comp.key.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.value.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&comp.iter.node, deps, visiting_functions);
                if let Some(filter) = &comp.filter {
                    self.collect_static_dependencies_from_expr(&filter.node, deps, visiting_functions);
                }
            }
            _ => {}
        }
    }

    /// Collect dictionary keys, values and spread dependencies in their written order.
    fn collect_dict_static_dependencies(
        &self,
        items: &[DictEntry],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            match item {
                DictEntry::Pair(key, value) => {
                    self.collect_static_dependencies_from_expr(&key.node, deps, visiting_functions);
                    self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                }
                DictEntry::Spread(value) => {
                    self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions)
                }
            }
        }
    }

    /// Collect ordinary Incan dependencies within surface payloads without visiting DSL-owned syntax.
    fn collect_surface_static_dependencies(
        &self,
        payload: &SurfaceExprPayload,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match payload {
            SurfaceExprPayload::PrefixUnary(expr) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    self.collect_static_dependencies_from_expr(&arm.awaitable.node, deps, visiting_functions);
                    match &arm.body {
                        RaceForBody::Expr(expr) => {
                            self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                        }
                        RaceForBody::Block(stmts) => {
                            for stmt in stmts {
                                self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                            }
                        }
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { .. } => {}
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.collect_static_dependencies_from_expr(&left.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&right.node, deps, visiting_functions);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                self.collect_static_dependencies_from_call_args(args, deps, visiting_functions);
            }
        }
    }

    /// Recursively collect the names of local static dependencies from `args` into `deps`.
    fn collect_static_dependencies_from_call_args(
        &self,
        args: &[CallArg],
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
                }
            }
        }
    }

    /// Validate RFC 052 "no static assignment in static initializers" by walking expression-driven call graphs.
    ///
    /// This intentionally follows local helper function calls reachable from the initializer expression so hidden
    /// `static` writes in helper bodies are rejected before runtime emission.
    fn collect_static_initializer_static_writes_from_expr(
        &mut self,
        expr: &Spanned<Expr>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &expr.node {
            Expr::Literal(_) | Expr::Ident(_) | Expr::SelfExpr | Expr::Yield(None) => {}
            Expr::Binary(left, _, right) => {
                self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
            }
            Expr::Unary(_, inner) | Expr::Try(inner) | Expr::Paren(inner) | Expr::Yield(Some(inner)) => {
                self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions);
            }
            Expr::Call(func, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(func, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
                self.collect_static_initializer_local_function_writes(func, current_static, visiting_functions);
            }
            Expr::MethodCall(object, _, _type_args, args) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Partial(partial) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &partial.target,
                    current_static,
                    visiting_functions,
                );
                for arg in &partial.args {
                    self.collect_static_initializer_static_writes_from_expr(
                        &arg.value,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Expr::Index(object, index) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(index, current_static, visiting_functions);
            }
            Expr::Slice(target, slice) => {
                self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                if let Some(start) = &slice.start {
                    self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                }
                if let Some(end) = &slice.end {
                    self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
                }
                if let Some(step) = &slice.step {
                    self.collect_static_initializer_static_writes_from_expr(step, current_static, visiting_functions);
                }
            }
            Expr::Field(object, _) => {
                self.collect_static_initializer_static_writes_from_expr(object, current_static, visiting_functions);
            }
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.collect_static_initializer_static_writes_from_expr(item, current_static, visiting_functions);
                }
            }
            Expr::List(items) => {
                self.collect_static_initializer_list_writes(items, current_static, visiting_functions);
            }
            Expr::Dict(items) => {
                self.collect_static_initializer_dict_writes(items, current_static, visiting_functions);
            }
            Expr::FString(parts) => {
                self.collect_static_initializer_fstring_writes(parts, current_static, visiting_functions);
            }
            Expr::Range { start, end, .. } => {
                self.collect_static_initializer_static_writes_from_expr(start, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(end, current_static, visiting_functions);
            }
            Expr::Constructor(_, args) => {
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions);
            }
            Expr::Match(scrutinee, arms) => {
                self.collect_static_initializer_static_writes_from_expr(scrutinee, current_static, visiting_functions);
                self.collect_static_initializer_match_writes(arms, current_static, visiting_functions);
            }
            Expr::If(if_expr) => {
                self.collect_static_initializer_if_writes(if_expr, current_static, visiting_functions);
            }
            Expr::Loop(loop_expr) => {
                for stmt in &loop_expr.body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
            }
            Expr::Generator(generator) => {
                self.collect_static_initializer_generator_writes(generator, current_static, visiting_functions);
            }
            Expr::ListComp(list_comp) => {
                self.collect_static_initializer_list_comp_writes(list_comp, current_static, visiting_functions);
            }
            Expr::DictComp(dict_comp) => {
                self.collect_static_initializer_dict_comp_writes(dict_comp, current_static, visiting_functions);
            }
            Expr::Closure(_, _) => {}
            Expr::Surface(surface) => {
                self.collect_static_initializer_surface_writes(&surface.payload, current_static, visiting_functions)
            }
            Expr::VocabBlock(block) => {
                for arg in &block.header_args {
                    self.collect_static_initializer_static_writes_from_expr(arg, current_static, visiting_functions);
                }
                for stmt in &block.body {
                    self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
                }
            }
            // A static write can only be written in ordinary Incan, so only a fragment's expression holes can
            // reach one. `holes` is the single authority on which expressions those are (RFC 081, #1022).
            Expr::Embedded(fragment) => {
                for hole in fragment.holes() {
                    self.collect_static_initializer_static_writes_from_expr(hole, current_static, visiting_functions);
                }
            }
        }
    }

    /// Follow a local direct helper once on the active call stack; closure creation alone does not execute its body.
    fn collect_static_initializer_local_function_writes(
        &mut self,
        func: &Spanned<Expr>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        if let Expr::Ident(function_name) = &func.node
            && let Some(function_decl) = self.local_function_decls.get(function_name).cloned()
            && visiting_functions.insert(function_name.clone())
        {
            self.collect_static_initializer_body_writes(&function_decl.body, current_static, visiting_functions);
            visiting_functions.remove(function_name);
        }
    }

    /// Check statements in source order without changing the initializer call-stack cycle guard.
    fn collect_static_initializer_body_writes(
        &mut self,
        body: &[Spanned<Statement>],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for stmt in body {
            self.collect_static_initializer_static_writes_from_stmt(stmt, current_static, visiting_functions);
        }
    }

    /// Check dictionary keys, values and spreads in their written order.
    fn collect_static_initializer_dict_writes(
        &mut self,
        items: &[DictEntry],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            match item {
                DictEntry::Pair(key, value) => {
                    self.collect_static_initializer_static_writes_from_expr(key, current_static, visiting_functions);
                    self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
                }
                DictEntry::Spread(value) => {
                    self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
                }
            }
        }
    }

    /// Check list elements and spread values in their written order.
    fn collect_static_initializer_list_writes(
        &mut self,
        items: &[ListEntry],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for item in items {
            let (ListEntry::Element(value) | ListEntry::Spread(value)) = item;
            self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
        }
    }

    /// Check interpolated expressions while leaving literal format text outside the static-write analysis.
    fn collect_static_initializer_fstring_writes(
        &mut self,
        parts: &[FStringPart],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for part in parts {
            if let FStringPart::Expr { expr, .. } = part {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
        }
    }

    /// Check a list comprehension's result, source and optional filter in the existing analysis order.
    fn collect_static_initializer_list_comp_writes(
        &mut self,
        expr: &ListComp,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.expr, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.iter, current_static, visiting_functions);
        if let Some(filter) = &expr.filter {
            self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
        }
    }

    /// Check a dictionary comprehension's key, value, source and optional filter in the existing analysis order.
    fn collect_static_initializer_dict_comp_writes(
        &mut self,
        expr: &DictComp,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.key, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.value, current_static, visiting_functions);
        self.collect_static_initializer_static_writes_from_expr(&expr.iter, current_static, visiting_functions);
        if let Some(filter) = &expr.filter {
            self.collect_static_initializer_static_writes_from_expr(filter, current_static, visiting_functions);
        }
    }

    /// Check each match guard before its expression or statement body.
    fn collect_static_initializer_match_writes(
        &mut self,
        arms: &[Spanned<MatchArm>],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                self.collect_static_initializer_static_writes_from_expr(guard, current_static, visiting_functions);
            }
            match &arm.node.body {
                MatchBody::Expr(inner) => {
                    self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions)
                }
                MatchBody::Block(body) => {
                    self.collect_static_initializer_body_writes(body, current_static, visiting_functions)
                }
            }
        }
    }

    /// Check the condition and both branches of an initializer-reachable if expression.
    fn collect_static_initializer_if_writes(
        &mut self,
        expr: &IfExpr,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&expr.condition, current_static, visiting_functions);
        self.collect_static_initializer_body_writes(&expr.then_body, current_static, visiting_functions);
        if let Some(body) = &expr.else_body {
            self.collect_static_initializer_body_writes(body, current_static, visiting_functions);
        }
    }

    /// Check the generator result followed by its source and filter clauses, preserving existing traversal order.
    fn collect_static_initializer_generator_writes(
        &mut self,
        generator: &GeneratorExpr,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_expr(&generator.expr, current_static, visiting_functions);
        for clause in &generator.clauses {
            let expr = match clause {
                ComprehensionClause::For { iter, .. } => iter,
                ComprehensionClause::If(condition) => condition,
            };
            self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
        }
    }

    /// Check ordinary Incan slots within a surface payload, including race-arm bodies and scoped call arguments.
    fn collect_static_initializer_surface_writes(
        &mut self,
        payload: &SurfaceExprPayload,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match payload {
            SurfaceExprPayload::PrefixUnary(inner) => {
                self.collect_static_initializer_static_writes_from_expr(inner, current_static, visiting_functions)
            }
            SurfaceExprPayload::RaceFor(race) => {
                for arm in &race.arms {
                    self.collect_static_initializer_static_writes_from_expr(
                        &arm.awaitable,
                        current_static,
                        visiting_functions,
                    );
                    match &arm.body {
                        RaceForBody::Expr(expr) => self.collect_static_initializer_static_writes_from_expr(
                            expr,
                            current_static,
                            visiting_functions,
                        ),
                        RaceForBody::Block(body) => {
                            self.collect_static_initializer_body_writes(body, current_static, visiting_functions)
                        }
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { .. } => {}
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.collect_static_initializer_static_writes_from_expr(left, current_static, visiting_functions);
                self.collect_static_initializer_static_writes_from_expr(right, current_static, visiting_functions);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                self.collect_static_initializer_static_writes_from_call_args(args, current_static, visiting_functions)
            }
        }
    }

    /// Recurse through call arguments while checking initializer-reachable static writes.
    fn collect_static_initializer_static_writes_from_call_args(
        &mut self,
        args: &[CallArg],
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for arg in args {
            match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => {
                    self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
                }
            }
        }
    }

    /// Recurse through statements reachable from an initializer expression and reject any write to static storage.
    fn collect_static_initializer_static_writes_from_stmt(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::Assignment(_)
            | Statement::CompoundAssignment(_)
            | Statement::TupleUnpack(_)
            | Statement::ChainedAssignment(_) => {
                self.collect_static_initializer_binding_writes(stmt, current_static, visiting_functions);
            }
            Statement::FieldAssignment(_) | Statement::IndexAssignment(_) | Statement::TupleAssign(_) => {
                self.collect_static_initializer_place_writes(stmt, current_static, visiting_functions);
            }
            Statement::Assert(assert_stmt) => {
                self.collect_static_initializer_assert_writes(assert_stmt, current_static, visiting_functions);
            }
            Statement::If(if_stmt) => {
                self.collect_static_initializer_if_statement_writes(if_stmt, current_static, visiting_functions);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) | Statement::Break(Some(expr)) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Statement::Loop(loop_stmt) => {
                self.collect_static_initializer_body_writes(&loop_stmt.body, current_static, visiting_functions);
            }
            Statement::While(while_stmt) => {
                self.collect_static_initializer_static_writes_from_condition(
                    &while_stmt.condition,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_body_writes(&while_stmt.body, current_static, visiting_functions);
            }
            Statement::For(for_stmt) => {
                self.collect_static_initializer_static_writes_from_expr(
                    &for_stmt.iter,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_body_writes(&for_stmt.body, current_static, visiting_functions);
            }
            Statement::Unsafe(unsafe_stmt) => {
                self.collect_static_initializer_body_writes(&unsafe_stmt.body, current_static, visiting_functions);
            }
            Statement::VocabExpressionItem(item) => {
                self.collect_static_initializer_static_writes_from_expr(&item.expr, current_static, visiting_functions);
                for modifier in &item.modifiers {
                    self.collect_static_initializer_static_writes_from_expr(
                        &modifier.value,
                        current_static,
                        visiting_functions,
                    );
                }
            }
            Statement::Return(None)
            | Statement::Pass
            | Statement::Break(None)
            | Statement::Continue
            | Statement::Surface(_)
            | Statement::VocabBlock(_) => {}
        }
    }

    /// Check direct binding assignments and named assignment lists selected by the statement coordinator.
    fn collect_static_initializer_binding_writes(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::Assignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::CompoundAssignment(assign) => {
                if self.static_decl_positions.contains_key(&assign.name) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        &assign.name,
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleUnpack(assign) => {
                self.collect_static_initializer_named_targets_writes(
                    &assign.names,
                    &assign.value,
                    stmt.span,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::ChainedAssignment(assign) => {
                self.collect_static_initializer_named_targets_writes(
                    &assign.targets,
                    &assign.value,
                    stmt.span,
                    current_static,
                    visiting_functions,
                );
            }
            _ => {}
        }
    }

    /// Check each named target before the common assignment value, retaining source diagnostic order.
    fn collect_static_initializer_named_targets_writes(
        &mut self,
        targets: &[String],
        value: &Spanned<Expr>,
        span: Span,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        for target in targets {
            if self.static_decl_positions.contains_key(target) {
                self.errors.push(errors::static_initializer_static_write_not_allowed(
                    current_static,
                    target,
                    span,
                ));
            }
        }
        self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
    }

    /// Check receiver and index expressions for the assignment-place forms selected by the statement coordinator.
    fn collect_static_initializer_place_writes(
        &mut self,
        stmt: &Spanned<Statement>,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &stmt.node {
            Statement::FieldAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        assign.target_span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::IndexAssignment(assign) => {
                if let Some(target_static) = self.static_assignment_target_name(&assign.object) {
                    self.errors.push(errors::static_initializer_static_write_not_allowed(
                        current_static,
                        target_static.as_str(),
                        stmt.span,
                    ));
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.object,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.index,
                    current_static,
                    visiting_functions,
                );
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            Statement::TupleAssign(assign) => {
                for target in &assign.targets {
                    if let Some(target_static) = self.static_assignment_target_name(target) {
                        self.errors.push(errors::static_initializer_static_write_not_allowed(
                            current_static,
                            target_static.as_str(),
                            target.span,
                        ));
                    }
                    self.collect_static_initializer_static_writes_from_expr(target, current_static, visiting_functions);
                }
                self.collect_static_initializer_static_writes_from_expr(
                    &assign.value,
                    current_static,
                    visiting_functions,
                );
            }
            _ => {}
        }
    }

    /// Check the assertion value before its optional message for every assertion form.
    fn collect_static_initializer_assert_writes(
        &mut self,
        assert_stmt: &AssertStmt,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match &assert_stmt.kind {
            AssertKind::Condition(condition) => {
                self.collect_static_initializer_static_writes_from_expr(condition, current_static, visiting_functions)
            }
            AssertKind::IsPattern { value, .. } => {
                self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions)
            }
            AssertKind::Raises { call, .. } => {
                self.collect_static_initializer_static_writes_from_expr(call, current_static, visiting_functions)
            }
        }
        if let Some(message) = &assert_stmt.message {
            self.collect_static_initializer_static_writes_from_expr(message, current_static, visiting_functions);
        }
    }

    /// Check an if statement's condition and branches in written order, including each elif condition.
    fn collect_static_initializer_if_statement_writes(
        &mut self,
        if_stmt: &IfStmt,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        self.collect_static_initializer_static_writes_from_condition(
            &if_stmt.condition,
            current_static,
            visiting_functions,
        );
        for inner in &if_stmt.then_body {
            self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
        }
        for (condition, body) in &if_stmt.elif_branches {
            self.collect_static_initializer_static_writes_from_expr(condition, current_static, visiting_functions);
            for inner in body {
                self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
            }
        }
        if let Some(else_body) = &if_stmt.else_body {
            for inner in else_body {
                self.collect_static_initializer_static_writes_from_stmt(inner, current_static, visiting_functions);
            }
        }
    }

    /// Collect static writes performed inside initializer conditions.
    fn collect_static_initializer_static_writes_from_condition(
        &mut self,
        condition: &Condition,
        current_static: &str,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => {
                self.collect_static_initializer_static_writes_from_expr(expr, current_static, visiting_functions);
            }
            Condition::Let { value, .. } => {
                self.collect_static_initializer_static_writes_from_expr(value, current_static, visiting_functions);
            }
        }
    }

    /// Resolve the static root name for assignment targets such as `S`, `S.field`, or `S[idx]`.
    fn static_assignment_target_name(&self, expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => self.static_decl_positions.contains_key(name).then(|| name.clone()),
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => {
                self.static_assignment_target_name(object)
            }
            _ => None,
        }
    }

    /// Recursively collect the names of local static dependencies from `stmt` into `deps`.
    fn collect_static_dependencies_from_statement(
        &self,
        stmt: &Statement,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Assignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::FieldAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::IndexAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.object.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.index.node, deps, visiting_functions);
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::Return(Some(expr)) | Statement::Expr(expr) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Break(Some(expr)) => {
                self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions);
            }
            Statement::Return(None) | Statement::Pass | Statement::Break(None) | Statement::Continue => {}
            Statement::If(if_stmt) => {
                self.collect_static_dependencies_from_condition(&if_stmt.condition, deps, visiting_functions);
                for stmt in &if_stmt.then_body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
                for (condition, body) in &if_stmt.elif_branches {
                    self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    for stmt in body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
                if let Some(else_body) = &if_stmt.else_body {
                    for stmt in else_body {
                        self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                    }
                }
            }
            Statement::Loop(loop_stmt) => {
                for stmt in &loop_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::While(while_stmt) => {
                self.collect_static_dependencies_from_condition(&while_stmt.condition, deps, visiting_functions);
                for stmt in &while_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::For(for_stmt) => {
                self.collect_static_dependencies_from_expr(&for_stmt.iter.node, deps, visiting_functions);
                for stmt in &for_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::Unsafe(unsafe_stmt) => {
                for stmt in &unsafe_stmt.body {
                    self.collect_static_dependencies_from_statement(&stmt.node, deps, visiting_functions);
                }
            }
            Statement::CompoundAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleUnpack(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::TupleAssign(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
                for target in &assign.targets {
                    self.collect_static_dependencies_from_expr(&target.node, deps, visiting_functions);
                }
            }
            Statement::ChainedAssignment(assign) => {
                self.collect_static_dependencies_from_expr(&assign.value.node, deps, visiting_functions);
            }
            Statement::VocabExpressionItem(item) => {
                self.collect_static_dependencies_from_expr(&item.expr.node, deps, visiting_functions);
                for modifier in &item.modifiers {
                    self.collect_static_dependencies_from_expr(&modifier.value.node, deps, visiting_functions);
                }
            }
            Statement::Assert(assert_stmt) => {
                match &assert_stmt.kind {
                    AssertKind::Condition(condition) => {
                        self.collect_static_dependencies_from_expr(&condition.node, deps, visiting_functions);
                    }
                    AssertKind::IsPattern { value, .. } => {
                        self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
                    }
                    AssertKind::Raises { call, .. } => {
                        self.collect_static_dependencies_from_expr(&call.node, deps, visiting_functions);
                    }
                }
                if let Some(message) = &assert_stmt.message {
                    self.collect_static_dependencies_from_expr(&message.node, deps, visiting_functions);
                }
            }
            Statement::Surface(_) | Statement::VocabBlock(_) => {}
        }
    }

    /// Collect static dependencies referenced by a condition expression.
    fn collect_static_dependencies_from_condition(
        &self,
        condition: &Condition,
        deps: &mut HashSet<String>,
        visiting_functions: &mut HashSet<String>,
    ) {
        match condition {
            Condition::Expr(expr) => self.collect_static_dependencies_from_expr(&expr.node, deps, visiting_functions),
            Condition::Let { value, .. } => {
                self.collect_static_dependencies_from_expr(&value.node, deps, visiting_functions);
            }
        }
    }

    /// Mutable variant of [`lookup_local_variable_info`](Self::lookup_local_variable_info).
    ///
    /// Used by const-eval to update inferred types after evaluation.
    pub fn lookup_local_variable_info_mut(&mut self, name: &str) -> Option<&mut VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get_mut(id)?;
        match &mut sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Resolve a required field type for the active trait default method, if any.
    ///
    /// Retain the declaring trait's member identity for the adopter's checked field binding. Emit a dedicated
    /// diagnostic if a trait body accesses an undeclared required field.
    pub fn trait_required_field_type(&mut self, field: &str, span: Span) -> Option<ResolvedType> {
        let requires = self.current_trait_requires.as_ref()?;
        if let Some(ty) = requires.get(field).cloned() {
            if let Some(owner) = self
                .current_trait_name
                .as_deref()
                .and_then(|name| self.symbols.lookup(name))
                .and_then(|symbol| self.symbols.identity_of(symbol))
            {
                let identity = Self::synthetic_member_identity(owner, field, SemanticSourceTargetKind::Field);
                self.type_info.record_resolved_identity(span, identity);
            }
            return Some(ty);
        }
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_ref()
            && seen.contains(field)
        {
            return None;
        }
        let trait_name = self.current_trait_name.as_deref().unwrap_or("<trait>");
        self.errors
            .push(errors::trait_requires_missing_field(trait_name, field, span));
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_mut() {
            seen.insert(field.to_string());
        }
        None
    }

    fn validate_stdlib_type_usage(&mut self, ty: &Spanned<Type>) {
        self.validate_stdlib_type_usage_inner(&ty.node, ty.span);
    }

    /// Validate stdlib-owned type names recursively inside a type annotation.
    fn validate_stdlib_type_usage_inner(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_stdlib_type_name(name, span),
            Type::Qualified(segments) => {
                if let Some(first) = segments.first() {
                    self.validate_stdlib_type_name(first, span);
                }
            }
            Type::Dotted(_) => {}
            Type::Generic(name, args) => {
                self.validate_stdlib_type_name(name, span);
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::DottedGeneric(_, args) => {
                for arg in args {
                    self.validate_stdlib_type_usage_inner(&arg.node, arg.span);
                }
            }
            Type::ConstrainedPrimitive(name, _) => self.validate_stdlib_type_name(name, span),
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_stdlib_type_usage_inner(&param.node, param.span);
                }
                self.validate_stdlib_type_usage_inner(&ret.node, ret.span);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.validate_stdlib_type_usage_inner(&inner.node, inner.span);
            }
            Type::Tuple(elems) => {
                for elem in elems {
                    self.validate_stdlib_type_usage_inner(&elem.node, elem.span);
                }
            }
            Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {}
        }
    }

    /// Report a stdlib surface type name (`Response`, `FieldInfo`, ...) that no binding in scope provides.
    ///
    /// A name the program declares at module scope is its own type and never the stdlib's, whether or not the
    /// declaration has been collected yet when this annotation is resolved: a model's own method signatures are
    /// collected before the model itself (#1795).
    fn validate_stdlib_type_name(&mut self, name: &str, span: Span) {
        let Some(id) = surface_types::from_str(name) else {
            return;
        };
        if surface_types::stdlib_module_path(id).is_some()
            && !self.module_declared_type_names.contains(name)
            && self.symbols.lookup(name).is_none()
        {
            self.errors.push(errors::unknown_symbol(name, span));
        }
    }

    /// Collect the type names one program declares at module scope: models, classes, enums, newtypes, traits and
    /// type aliases.
    fn collect_module_declared_type_names(program: &Program) -> HashSet<String> {
        program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Model(model) => Some(model.name.clone()),
                Declaration::Class(class) => Some(class.name.clone()),
                Declaration::Enum(enum_decl) => Some(enum_decl.name.clone()),
                Declaration::Newtype(newtype) => Some(newtype.name.clone()),
                Declaration::Trait(trait_decl) => Some(trait_decl.name.clone()),
                Declaration::TypeAlias(alias) => Some(alias.name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Record every compiler-proven declaration identity referenced by one source type annotation.
    ///
    /// Generic constructors and their arguments are independent reference sites, so the walk retains each AST
    /// node's own span. Qualified paths resolve through the checked import metadata rather than being reconstructed
    /// from their written spelling. `Self` records the enclosing concrete nominal declaration in methods and the
    /// enclosing trait declaration in an abstract trait signature.
    fn record_type_reference_identities(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) => {
                self.record_named_type_reference_identity(name, ty.span);
            }
            Type::Generic(name, args) => {
                self.record_named_type_reference_identity(name, ty.span);
                for arg in args {
                    self.record_type_reference_identities(arg);
                }
            }
            Type::DottedGeneric(segments, args) => {
                self.record_dotted_type_reference_identity(segments, ty.span);
                for arg in args {
                    self.record_type_reference_identities(arg);
                }
            }
            Type::Function(params, ret) => {
                for param in params {
                    self.record_type_reference_identities(param);
                }
                self.record_type_reference_identities(ret);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.record_type_reference_identities(inner)
            }
            Type::Tuple(items) => {
                for item in items {
                    self.record_type_reference_identities(item);
                }
            }
            Type::SelfType => {
                if let Some(owner) = self
                    .current_method_owner
                    .clone()
                    .or_else(|| self.current_trait_name.clone())
                {
                    self.record_named_type_reference_identity(&owner, ty.span);
                }
            }
            Type::Qualified(segments) => self.record_qualified_rust_type_reference_identity(segments, ty.span),
            Type::Dotted(segments) => self.record_dotted_type_reference_identity(segments, ty.span),
            Type::IntLiteral(_) | Type::Unit | Type::Infer => {}
        }
    }

    /// Record one Rust item reached through an already-resolved `rust::` module binding.
    ///
    /// The import collector owns the root's canonical crate path. Appending the syntactically checked suffix yields
    /// the same target identity as a direct `from rust::... import Item` binding; no generated Rust spelling or local
    /// alias is consulted.
    fn record_qualified_rust_type_reference_identity(&mut self, segments: &[String], span: Span) {
        let Some((root, suffix)) = segments.split_first() else {
            return;
        };
        let Some(info) = self.lookup_symbol(root).and_then(|symbol| match &symbol.kind {
            SymbolKind::RustItem(info) if info.binding != RustImportBindingKind::FromImport => Some(info),
            _ => None,
        }) else {
            return;
        };
        let mut canonical_path = info.path.split("::").map(str::to_string).collect::<Vec<_>>();
        canonical_path.extend(suffix.iter().cloned());
        let Some(declaration_name) = canonical_path.last().cloned() else {
            return;
        };
        self.type_info.record_resolved_identity(
            span,
            CanonicalSymbolId {
                namespace: SymbolNamespace::OrdinaryLexical,
                origin: SymbolOrigin::RustCrate(canonical_path),
                declaration_name,
                kind: SemanticSourceTargetKind::RustItem,
                scope_discriminant: None,
                declaration_span: HirSourceSpan::new(0, 0),
            },
        );
    }

    /// Record one type reached through an already-resolved Incan module binding.
    ///
    /// The spelling was resolved by [`Self::resolve_qualified_type_annotations`] before this walk; this only attaches
    /// that proven identity to the reference site's own span.
    fn record_dotted_type_reference_identity(&mut self, segments: &[String], span: Span) {
        let spelling = segments.join(".");
        if let Some(identity) = self
            .type_info
            .qualified_type_reference(&spelling)
            .map(|reference| reference.identity.clone())
        {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Resolve every module-qualified spelling (`mod.Type`, `mod.Box[T]`) inside one annotation before the
    /// annotation itself is resolved.
    ///
    /// The shared type resolver only sees the symbol table, while a qualified spelling resolves through the module
    /// member registry, the stdlib cache, or a public library's checked manifest. So the checker proves each spelling
    /// here first and records the result keyed by the spelling; the resolver then reads that fact, and so does
    /// lowering when it places the nominal type. Nothing is reported here: the validation walk owns the diagnostic
    /// for a spelling that did not resolve, so it fires once in the checking pass like the other annotation
    /// diagnostics, while the facts are needed during declaration collection as well.
    fn resolve_qualified_type_annotations(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Dotted(segments) => self.resolve_qualified_type_annotation(segments),
            Type::DottedGeneric(segments, args) => {
                self.resolve_qualified_type_annotation(segments);
                for arg in args {
                    self.resolve_qualified_type_annotations(arg);
                }
            }
            Type::Generic(_, args) | Type::Tuple(args) => {
                for arg in args {
                    self.resolve_qualified_type_annotations(arg);
                }
            }
            Type::Function(params, ret) => {
                for param in params {
                    self.resolve_qualified_type_annotations(param);
                }
                self.resolve_qualified_type_annotations(ret);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.resolve_qualified_type_annotations(inner)
            }
            Type::Simple(_)
            | Type::Qualified(_)
            | Type::ConstrainedPrimitive(..)
            | Type::IntLiteral(_)
            | Type::Unit
            | Type::SelfType
            | Type::Infer => {}
        }
    }

    /// Resolve one module-qualified type spelling and retain the proof under that spelling.
    ///
    /// A spelling is a module-scope fact -- its root is a module binding and its tail names a member of that module --
    /// so every occurrence in one module resolves the same way. It is still resolved afresh each time rather than
    /// memoized: dependency interfaces are collected through this same path before the consumer's facts are reset,
    /// and two dependencies can spell `errors.T` against different `errors` bindings, so a remembered answer could
    /// outlive the binding it was proven against. A failed resolution removes any earlier entry for the same reason.
    fn resolve_qualified_type_annotation(&mut self, segments: &[String]) {
        let spelling = segments.join(".");
        match self.qualified_type_declaration(segments) {
            Some(reference) => {
                self.type_info
                    .declarations
                    .qualified_type_references
                    .insert(spelling, reference);
            }
            None => {
                self.type_info.declarations.qualified_type_references.remove(&spelling);
            }
        }
    }

    /// Resolve the declaration a module-qualified type spelling names, if the checker can prove one.
    ///
    /// The root must be an Incan module binding; the remaining segments walk nested modules and end at a member that
    /// declares a type or trait. A Loaf module answers through its checked manifest and a source module through the
    /// module member registry, each the way the matching direct import would. The resolved type is the one that direct
    /// import would have bound -- the declaration's own name, the target of a non-generic source type alias, or the
    /// Loaf-qualified spelling -- so values checked against either spelling of the same declaration are compatible.
    /// `None` means no type was proven; it never means "assume the written path".
    fn qualified_type_declaration(&mut self, segments: &[String]) -> Option<QualifiedTypeReferenceInfo> {
        let (root, remainder) = segments.split_first()?;
        let (name, nested_module) = remainder.split_last()?;
        let mut module_path = self.lookup_symbol(root).and_then(|symbol| match &symbol.kind {
            SymbolKind::Module(info) if !info.is_python => Some(info.path.clone()),
            _ => None,
        })?;
        module_path.extend(nested_module.iter().cloned());

        // A Rust item a Loaf module re-exports is that Rust item, spelled qualified or not.
        if let Some((library, rest)) = self.loaf_module(&module_path)
            && let Ok(Some(member)) = self.resolve_pub_library_module_symbol_member(&library, &rest, name)
            && let SymbolKind::RustItem(info) = &member.kind
            && let Some(identity) = member.canonical.clone()
        {
            return Some(QualifiedTypeReferenceInfo {
                identity,
                module_path,
                resolved: ResolvedType::RustPath(info.path.clone()),
            });
        }

        // Compiler-known surface types have no source declaration for the stdlib cache to return. Their registry
        // ownership is nevertheless the same proof a direct `from std.web import Html` uses, so retain that stable
        // builtin identity and canonical resolved spelling for a qualified annotation as well (#1824).
        if let Some(surface_type) = surface_types::from_str(name)
            && surface_types::stdlib_module_path(surface_type).is_some_and(|owner| owner == module_path.join("."))
        {
            let canonical = surface_types::as_str(surface_type).to_string();
            return Some(QualifiedTypeReferenceInfo {
                identity: CanonicalSymbolId {
                    namespace: SymbolNamespace::OrdinaryLexical,
                    origin: SymbolOrigin::Builtin,
                    declaration_name: canonical.clone(),
                    kind: SemanticSourceTargetKind::Builtin,
                    scope_discriminant: None,
                    declaration_span: HirSourceSpan::new(0, 0),
                },
                module_path,
                resolved: ResolvedType::Named(canonical),
            });
        }

        // ---- Loaf module: the checked manifest owns both the identity and the qualified type name ----
        if let Some((library, rest)) = self.loaf_module(&module_path) {
            let resolved = self
                .resolve_pub_library_module_symbol_member(&library, &rest, name)
                .ok()
                .flatten()?;
            if !matches!(resolved.kind, SymbolKind::Type(_) | SymbolKind::Trait(_)) {
                return None;
            }
            let identity = resolved.canonical?;
            let root_len = self.loaf_binding_root(&library).len();
            let mut source_type_path = resolved
                .source_module_path
                .iter()
                .skip(root_len)
                .cloned()
                .collect::<Vec<_>>();
            source_type_path.push(resolved.source_name);
            let canonical_name = canonical_public_library_type_name(&library, &source_type_path.join("::"));
            return Some(QualifiedTypeReferenceInfo {
                identity,
                module_path,
                resolved: ResolvedType::Named(canonical_name),
            });
        }

        // ---- Source module: the member registry proves the kind and the identity ----
        let import = ImportPath::simple(module_path.clone());
        let member_kind = self.dependency_member_symbol_for_path(&import, name);
        let type_like = member_kind
            .as_ref()
            .is_some_and(|kind| matches!(kind, SymbolKind::Type(_) | SymbolKind::Trait(_)));
        if !type_like {
            return None;
        }
        let identity = self.dependency_member_identity(&import, name)?;
        // A direct import of a type alias registers its target and lets alias expansion replace the name; the
        // qualified spelling registers nothing local, so the target is the resolved type here. A generic alias
        // needs its arguments substituted, which only the local alias registry does, so it keeps the nominal name.
        let resolved = match member_kind {
            Some(SymbolKind::Type(TypeInfo::TypeAlias)) => self
                .dependency_member_type_alias_for_path(&import, name)
                .filter(|alias| alias.type_params.is_empty())
                .map_or_else(|| ResolvedType::Named(name.clone()), |alias| alias.target),
            _ => ResolvedType::Named(name.clone()),
        };
        Some(QualifiedTypeReferenceInfo {
            identity,
            module_path,
            resolved,
        })
    }

    /// Record one type-like lexical binding without guessing an identity from its source spelling.
    fn record_named_type_reference_identity(&mut self, name: &str, span: Span) {
        if let Some(identity) = self.named_type_reference_identity(name) {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Return the identity already assigned to a checked type binding; lexical names select bindings only here.
    fn named_type_reference_identity(&self, name: &str) -> Option<CanonicalSymbolId> {
        self.symbols.lookup(name).and_then(|symbol_id| {
            let symbol = self.symbols.get(symbol_id)?;
            let type_like = matches!(symbol.kind, SymbolKind::Type(_) | SymbolKind::Trait(_))
                || matches!(
                    &symbol.kind,
                    SymbolKind::RustItem(info) if info.binding != RustImportBindingKind::CrateRoot
                );
            if !type_like {
                return None;
            }
            self.symbols.identity_of(symbol_id).cloned()
        })
    }

    /// Register the resolved target for a source-level type alias.
    pub fn register_type_alias_target(&mut self, alias: &TypeAliasDecl) {
        let type_params = alias
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>();
        let target = self.resolve_type_checked(&alias.target);
        let mut nominal_identities = std::collections::BTreeMap::new();
        self.retain_alias_nominal_identities(&target, &mut nominal_identities);
        self.type_aliases.insert(
            alias.name.clone(),
            TypeAliasTarget {
                type_params,
                target,
                nominal_identities,
            },
        );
    }

    /// Snapshot alias leaves while their declaration's lexical bindings are active. A consumer must not look up an
    /// expanded alias's unimported member in its own scope or infer its owner from a project-wide spelling search.
    fn retain_alias_nominal_identities(
        &self,
        ty: &ResolvedType,
        identities: &mut std::collections::BTreeMap<String, CanonicalSymbolId>,
    ) {
        match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                if let Some(identity) = self.named_type_reference_identity(name) {
                    identities.insert(name.clone(), identity);
                }
                if let ResolvedType::Generic(_, arguments) = ty {
                    for argument in arguments {
                        self.retain_alias_nominal_identities(argument, identities);
                    }
                }
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    self.retain_alias_nominal_identities(item, identities);
                }
            }
            ResolvedType::Function(parameters, result) => {
                for parameter in parameters {
                    self.retain_alias_nominal_identities(&parameter.ty, identities);
                }
                self.retain_alias_nominal_identities(result, identities);
            }
            ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner) => {
                self.retain_alias_nominal_identities(inner, identities);
            }
            ResolvedType::FrozenDict(key, value) => {
                self.retain_alias_nominal_identities(key, identities);
                self.retain_alias_nominal_identities(value, identities);
            }
            _ => {}
        }
    }

    /// Expand transparent source type aliases inside a resolved type. A nominal Rust alias keeps its public spelling.
    fn expand_type_aliases(&self, ty: ResolvedType) -> ResolvedType {
        let mut expanding = HashSet::new();
        self.expand_type_aliases_inner(ty, &mut expanding)
    }

    /// Recursively expand alias leaves within one resolved type while preserving non-aliased structure.
    fn expand_type_aliases_inner(&self, ty: ResolvedType, expanding: &mut HashSet<String>) -> ResolvedType {
        match ty {
            ResolvedType::Named(name) => self
                .expand_named_type_alias(name.clone(), Vec::new(), expanding)
                .unwrap_or(ResolvedType::Named(name)),
            ResolvedType::Generic(name, args) => {
                let expanded_args = args
                    .into_iter()
                    .map(|arg| self.expand_type_aliases_inner(arg, expanding))
                    .collect::<Vec<_>>();
                self.expand_named_type_alias(name.clone(), expanded_args.clone(), expanding)
                    .unwrap_or_else(|| {
                        if name == UNION_TYPE_NAME {
                            union_ty(expanded_args)
                        } else {
                            ResolvedType::Generic(name, expanded_args)
                        }
                    })
            }
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .into_iter()
                    .map(|param| CallableParam {
                        name: param.name,
                        ty: self.expand_type_aliases_inner(param.ty, expanding),
                        kind: param.kind,
                        has_default: param.has_default,
                        is_partial_preset: param.is_partial_preset,
                        is_mut: param.is_mut,
                    })
                    .collect(),
                Box::new(self.expand_type_aliases_inner(*ret, expanding)),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|item| self.expand_type_aliases_inner(item, expanding))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(self.expand_type_aliases_inner(*key, expanding)),
                Box::new(self.expand_type_aliases_inner(*value, expanding)),
            ),
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(self.expand_type_aliases_inner(*inner, expanding))),
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(self.expand_type_aliases_inner(*inner, expanding)))
            }
            other => other,
        }
    }

    /// Expand one named alias reference, applying generic arguments and stopping on alias cycles.
    fn expand_named_type_alias(
        &self,
        name: String,
        args: Vec<ResolvedType>,
        expanding: &mut HashSet<String>,
    ) -> Option<ResolvedType> {
        let alias = self.type_aliases.get(&name)?;
        if alias.type_params.len() != args.len() {
            return None;
        }
        if !expanding.insert(name.clone()) {
            return None;
        }
        let target = if alias.type_params.is_empty() {
            alias.target.clone()
        } else {
            let subst = type_param_subst_map(&alias.type_params, &args);
            substitute_resolved_type(&alias.target, &subst)
        };
        let expanded = self.expand_type_aliases_inner(target, expanding);
        expanding.remove(&name);
        Some(expanded)
    }

    /// Render one Rust generic argument while retaining compiled nominal ownership.
    ///
    /// Public-library type identity is intentionally kept beside `ResolvedType` for ordinary diagnostics and
    /// compatibility. Rust applications are opaque displays, however, so any imported nominal nested inside them must
    /// be encoded before the structured argument is rendered to text.
    fn render_provider_aware_rust_arg(&self, ty: &ResolvedType) -> String {
        let mut qualified = ty.clone();
        self.canonicalize_public_library_nominals(&mut qualified);
        render_resolved_type_as_rust_arg(&qualified)
    }

    /// Freeze provider-owned native generic arguments while source annotations keep their local lexical spelling.
    fn qualify_native_source_argument(&self, ty: &mut ResolvedType) {
        self.canonicalize_public_library_nominals(ty);
    }

    /// Replace locally aliased public-library nominals with their source-unspellable canonical provider path.
    ///
    /// Emitted Rust displays and compiled manifests must retain provider identity independently of the aliases that
    /// happen to be in scope for source typechecking.
    pub fn canonicalize_public_library_nominals(&self, ty: &mut ResolvedType) {
        match ty {
            ResolvedType::Named(name) => self.qualify_public_library_nominal_name(name),
            ResolvedType::Generic(name, args) => {
                self.qualify_public_library_nominal_name(name);
                for arg in args {
                    self.canonicalize_public_library_nominals(arg);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.canonicalize_public_library_nominals(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.canonicalize_public_library_nominals(key);
                self.canonicalize_public_library_nominals(value);
            }
            ResolvedType::Function(params, ret) => {
                for param in params {
                    self.canonicalize_public_library_nominals(&mut param.ty);
                }
                self.canonicalize_public_library_nominals(ret);
            }
            ResolvedType::Tuple(items) => {
                for item in items {
                    self.canonicalize_public_library_nominals(item);
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
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => {}
        }
    }

    /// Qualify one imported public type name when its local alias still carries provider identity.
    fn qualify_public_library_nominal_name(&self, name: &mut String) {
        if split_canonical_public_library_type_name(name).is_some()
            || !self.public_library_type_identities.contains_key(name)
        {
            return;
        }
        let Some([root, dependency_key, public_name]) = self.import_binding_path(name) else {
            return;
        };
        if root == PUBLIC_LIBRARY_NAMESPACE {
            *name = canonical_public_library_type_name(dependency_key, public_name);
        }
    }

    /// Resolve a type annotation and emit diagnostics for reserved or invalid type spellings.
    fn resolve_type_checked(&mut self, ty: &Spanned<Type>) -> ResolvedType {
        self.resolve_qualified_type_annotations(ty);
        if self.validate_source_type_names {
            self.validate_source_type_annotation_names(&ty.node, ty.span);
            self.record_type_reference_identities(ty);
        }
        self.validate_stdlib_type_usage(ty);
        self.refuse_marked_copied_scalars(ty);
        if let Type::Simple(name) = &ty.node
            && Self::reserved_numeric_type_name(name)
        {
            self.errors.push(CompileError::type_error(
                format!(
                    "`{name}` is reserved for numeric types; use `decimal[p, s]`/`numeric[p, s]` for decimals or an exact-width integer type"
                ),
                ty.span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some(decimal_ty) = self.resolve_decimal_type_checked(ty) {
            return decimal_ty;
        }
        if self.report_bare_builtin_collection_annotations(ty) {
            return ResolvedType::Unknown;
        }
        if let Type::Simple(name) = &ty.node
            && let Some(sym) = self.lookup_symbol(name.as_str())
            && let SymbolKind::RustItem(info) = &sym.kind
            && info.binding == RustImportBindingKind::CrateRoot
        {
            self.errors
                .push(errors::rust_crate_root_used_as_type(name.as_str(), &info.path, ty.span));
            return ResolvedType::Unknown;
        }
        let resolved = resolve_type_with_rust_arg_renderer(
            &ty.node,
            &self.symbols,
            &|arg| self.render_provider_aware_rust_arg(arg),
            &|arg| self.qualify_native_source_argument(arg),
            &|segments| self.qualified_type_annotation_resolved_type(segments),
        );
        let resolved = self.normalize_union_member_identity(resolved);
        let resolved = self.expand_type_aliases(resolved);
        // Only the checking pass sees every declaration's derives; collection may resolve a name declared further on.
        if self.validate_source_type_names {
            self.refuse_unhashable_collection_keys(ty, &resolved);
        }
        resolved
    }

    /// Return the nominal type a module-qualified spelling was proven to name, for the shared type resolver.
    ///
    /// The proof was recorded by [`Self::resolve_qualified_type_annotations`]; an unproven spelling stays `None` so
    /// the resolver yields `Unknown` and the validation walk reports it.
    fn qualified_type_annotation_resolved_type(&self, segments: &[String]) -> Option<ResolvedType> {
        self.type_info
            .qualified_type_reference(&segments.join("."))
            .map(|reference| self.qualified_reference_member_spelling(reference.resolved.clone(), &reference.identity))
    }

    /// Reject unbound names in source-authored annotations after declaration collection is complete.
    fn validate_source_type_annotation_names(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_source_type_annotation_name(name, span),
            Type::Generic(name, args) => {
                self.validate_source_type_annotation_name(name, span);
                for arg in args {
                    self.validate_source_type_annotation_names(&arg.node, arg.span);
                }
            }
            Type::Dotted(segments) => self.validate_source_dotted_type_annotation(segments, span),
            Type::DottedGeneric(segments, args) => {
                self.validate_source_dotted_type_annotation(segments, span);
                for arg in args {
                    self.validate_source_type_annotation_names(&arg.node, arg.span);
                }
            }
            Type::ConstrainedPrimitive(name, _) => self.validate_source_type_annotation_name(name, span),
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_source_type_annotation_names(&param.node, param.span);
                }
                self.validate_source_type_annotation_names(&ret.node, ret.span);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.validate_source_type_annotation_names(&inner.node, inner.span);
            }
            Type::Tuple(items) => {
                for item in items {
                    self.validate_source_type_annotation_names(&item.node, item.span);
                }
            }
            Type::Qualified(segments) => self.validate_source_qualified_type_annotation(segments, span),
            Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {}
        }
    }

    /// Require an imported Rust root and public metadata for a source-authored qualified type. Compiler-selected
    /// dependencies may have restricted metadata, which must not grant an explicit type annotation.
    fn validate_source_qualified_type_annotation(&mut self, segments: &[String], span: Span) {
        let resolved = resolve_type(&Type::Qualified(segments.to_vec()), &self.symbols);
        if let ResolvedType::RustPath(path) = &resolved {
            self.admit_source_rust_type(&segments.join("::"), path, span);
            return;
        }
        if resolved != ResolvedType::Unknown {
            return;
        }
        if let Some(root) = segments.first() {
            self.emit_unknown_source_type_annotation_name(root, span);
        }
    }

    /// Reject a module-qualified type annotation (`mod.Type`) the checker could not resolve to a declaration.
    ///
    /// An unbound root is an unknown symbol like any other. A bound root that is not a module, or a module that
    /// declares no such type, is refused with its own message: the spelling would otherwise resolve to `Unknown`
    /// and reach lowering as a dotted name, which the Rust emitter cannot spell (#1437). A spelling that resolved
    /// has its proof recorded under [`DeclarationArtifacts::qualified_type_references`] and needs nothing here.
    ///
    /// The C interop namespace is the one dotted root that is not a module: `c.i32` or `c.Owned[T]` names a
    /// vocabulary carrier that the checked-binding facet interprets, and `from std.interop import c` binds `c` as
    /// that namespace's marker type. Those spellings are accepted here exactly as they were before qualified module
    /// types resolved, so a safe facade over a binding keeps checking.
    fn validate_source_dotted_type_annotation(&mut self, segments: &[String], span: Span) {
        let Some((root, remainder)) = segments.split_first() else {
            return;
        };
        if self.symbols.lookup(root).is_none() {
            self.emit_unknown_source_type_annotation_name(root, span);
            return;
        }
        let spelling = segments.join(".");
        if self.type_info.qualified_type_reference(&spelling).is_some() {
            return;
        }
        if self
            .import_binding_path(root)
            .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        {
            return;
        }
        let key = (spelling.clone(), span);
        if !self.unknown_source_type_names_emitted.insert(key) {
            return;
        }
        let module_path = self.lookup_symbol(root).and_then(|symbol| match &symbol.kind {
            SymbolKind::Module(info) if !info.is_python => Some(info.path.clone()),
            _ => None,
        });
        let error = match (module_path, remainder.split_last()) {
            (Some(mut module_path), Some((member, nested))) => {
                module_path.extend(nested.iter().cloned());
                let module = Self::module_import_spelling(&module_path);
                // A compiler surface type the module provides (`std.web`'s `Html`) is bound by a direct import but
                // has no declaration a qualified reference could name (#1824); say so rather than that the module
                // declares nothing by that name.
                let module_provides_surface_type = surface_types::from_str(member)
                    .and_then(surface_types::stdlib_module_path)
                    .is_some_and(|provider| provider == module_path.join("."));
                if module_provides_surface_type {
                    errors::qualified_type_names_surface_type(&spelling, &module, member, span)
                } else {
                    errors::qualified_type_not_declared(&spelling, &module, member, span)
                }
            }
            _ => errors::qualified_type_root_not_a_module(&spelling, root, span),
        };
        self.errors.push(error);
    }

    /// Render a checked module path the way an import statement spells it, for a diagnostic.
    ///
    /// The path is the binding's resolved one rather than the alias the author wrote, so the message names the
    /// module that was actually searched (`beta`, not `m`) and its hint is a spelling that imports from it. A
    /// public library carries its `pub::` marker and dots below the library name, as the import reference documents.
    fn module_import_spelling(module_path: &[String]) -> String {
        match module_path {
            [root, library, rest @ ..] if root == PUBLIC_LIBRARY_NAMESPACE => {
                let mut spelling = format!("{root}::{library}");
                if !rest.is_empty() {
                    spelling.push('.');
                    spelling.push_str(&rest.join("."));
                }
                spelling
            }
            _ => module_path.join("."),
        }
    }

    /// Emit one diagnostic when a simple source annotation name has no declaration in the active scope.
    fn validate_source_type_annotation_name(&mut self, name: &str, span: Span) {
        if name == "_"
            || name == "Type"
            || Self::reserved_numeric_type_name(name)
            || runtime_errors::from_str(name).is_some()
        {
            return;
        }
        if !matches!(
            resolve_type(&Type::Simple(name.to_string()), &self.symbols),
            ResolvedType::TypeVar(_)
        ) {
            return;
        }
        self.emit_unknown_source_type_annotation_name(name, span);
    }

    /// Emit at most one unknown-symbol diagnostic for one source annotation occurrence.
    ///
    /// Inside a callable the diagnostic suggests declaring the name as a type parameter of that callable; elsewhere
    /// it keeps the import-or-define remedy.
    fn emit_unknown_source_type_annotation_name(&mut self, name: &str, span: Span) {
        let key = (name.to_string(), span);
        if !self.unknown_source_type_names_emitted.insert(key) {
            return;
        }
        let error = match &self.annotation_owner {
            Some(owner) => {
                errors::unknown_type_parameter_candidate(name, &owner.name, &owner.declared_type_params, span)
            }
            None => errors::unknown_symbol(name, span),
        };
        self.errors.push(error);
    }

    /// Make `owner` the callable whose annotations are being checked, returning the previous owner to restore.
    fn enter_annotation_owner(&mut self, name: &str, type_params: &[TypeParam]) -> Option<AnnotationOwner> {
        self.annotation_owner.replace(AnnotationOwner {
            name: name.to_string(),
            declared_type_params: type_params.iter().map(|param| param.name.clone()).collect(),
        })
    }

    /// Return whether a simple type name is reserved for a parameterized numeric family.
    fn reserved_numeric_type_name(name: &str) -> bool {
        numerics::decimal_constructor_from_str(name).is_some()
    }

    /// Resolve and validate a parameterized decimal type annotation.
    fn resolve_decimal_type_checked(&mut self, ty: &Spanned<Type>) -> Option<ResolvedType> {
        let Type::Generic(name, args) = &ty.node else {
            return None;
        };
        numerics::decimal_constructor_from_str(name.as_str())?;
        if args.len() != 2 {
            self.errors.push(CompileError::type_error(
                format!("{name}[...] expects exactly 2 integer parameters: precision and scale"),
                ty.span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let Some(precision) = decimal_type_int_arg(&args[0]) else {
            self.errors.push(CompileError::type_error(
                "Decimal precision must be an integer literal".to_string(),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let Some(scale) = decimal_type_int_arg(&args[1]) else {
            self.errors.push(CompileError::type_error(
                "Decimal scale must be an integer literal".to_string(),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        if !(1..=38).contains(&precision) {
            self.errors.push(CompileError::type_error(
                format!("Decimal precision must be between 1 and 38, found {precision}"),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if scale < 0 || scale > precision {
            self.errors.push(CompileError::type_error(
                format!("Decimal scale must be between 0 and precision {precision}, found {scale}"),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        // Every registered decimal constructor is one type identity. In particular, RFC 009 defines
        // `decimal128[p, s]` as an alias rather than a distinct nominal type.
        let canonical = numerics::decimal_constructor_info_for(
            incan_lang::lang::types::numerics::DecimalTypeConstructorId::Decimal,
        )
        .canonical;
        Some(ResolvedType::Generic(
            canonical.to_string(),
            vec![
                ResolvedType::TypeVar(precision.to_string()),
                ResolvedType::TypeVar(scale.to_string()),
            ],
        ))
    }

    /// Validate alias and partial relationships that need declaration-level context before symbol collection flattens
    /// aliases and projected callable presets.
    fn validate_alias_declarations(&mut self, program: &Program) {
        let mut public_decls = HashMap::new();
        let mut alias_targets = HashMap::new();
        let mut partial_targets = HashMap::new();
        let mut newtype_names = HashSet::new();
        let mut newtype_underlyings = HashMap::new();

        for decl in &program.declarations {
            if let Some(name) = declaration_name(decl) {
                public_decls.insert(name.to_string(), is_public_decl(decl));
            }
            if let Declaration::Newtype(nt) = &decl.node {
                newtype_names.insert(nt.name.clone());
            }
        }

        for decl in &program.declarations {
            match &decl.node {
                Declaration::Alias(_) | Declaration::Partial(_) => {
                    self.collect_checked_projection_targets(
                        decl,
                        &public_decls,
                        &mut alias_targets,
                        &mut partial_targets,
                    );
                }
                Declaration::Model(model) => {
                    self.validate_member_name_collisions(
                        &model.name,
                        &model.fields,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.properties,
                        &model.methods,
                    );
                    self.validate_method_alias_declarations(&model.name, &model.method_aliases, &model.methods);
                    self.validate_method_partial_declarations(
                        &model.name,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.methods,
                    );
                }
                Declaration::Class(class) => {
                    self.validate_member_name_collisions(
                        &class.name,
                        &class.fields,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.properties,
                        &class.methods,
                    );
                    self.validate_method_alias_declarations(&class.name, &class.method_aliases, &class.methods);
                    self.validate_method_partial_declarations(
                        &class.name,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.methods,
                    );
                }
                Declaration::Trait(tr) => {
                    self.validate_member_name_collisions(
                        &tr.name,
                        &[],
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.properties,
                        &tr.methods,
                    );
                    self.validate_method_alias_declarations(&tr.name, &tr.method_aliases, &tr.methods);
                    self.validate_method_partial_declarations(
                        &tr.name,
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.methods,
                    );
                }
                Declaration::Newtype(nt) => {
                    self.validate_member_name_collisions(
                        &nt.name,
                        &[],
                        &nt.method_aliases,
                        &nt.method_partials,
                        &[],
                        &nt.methods,
                    );
                    self.validate_method_alias_declarations(&nt.name, &nt.method_aliases, &nt.methods);
                    self.validate_method_partial_declarations(
                        &nt.name,
                        &nt.method_aliases,
                        &nt.method_partials,
                        &nt.methods,
                    );
                    if let Type::Simple(target) = &nt.underlying.node {
                        newtype_underlyings.insert(nt.name.clone(), (target.clone(), nt.underlying.span));
                    }
                }
                Declaration::Enum(en) => {
                    self.validate_enum_member_name_collisions(&en.name, &en.variants, &en.variant_aliases, &en.methods);
                }
                _ => {}
            }
        }

        self.validate_top_level_alias_cycles(&alias_targets);
        self.validate_top_level_partial_cycles(&alias_targets, &partial_targets);
        self.validate_newtype_underlying_cycles(&newtype_underlyings, &newtype_names);
    }

    /// Collect local alias or partial targets and reject public projections of private declarations before cycle
    /// checks.
    fn collect_checked_projection_targets(
        &mut self,
        decl: &Spanned<Declaration>,
        public_decls: &HashMap<String, bool>,
        alias_targets: &mut HashMap<String, (String, Span)>,
        partial_targets: &mut HashMap<String, (String, Span)>,
    ) {
        match &decl.node {
            Declaration::Alias(alias) => {
                if let [target] = alias.target.segments.as_slice() {
                    alias_targets.insert(alias.name.clone(), (target.clone(), decl.span));
                    if matches!(alias.visibility, Visibility::Public)
                        && public_decls.get(target).is_some_and(|is_public| !*is_public)
                    {
                        self.errors.push(CompileError::type_error(
                            format!(
                                "Public alias '{}' targets private symbol '{}'",
                                alias.name,
                                alias.target.segments.join(".")
                            ),
                            decl.span,
                        ));
                    }
                }
            }
            Declaration::Partial(partial) => {
                if let [target] = partial.target.segments.as_slice() {
                    partial_targets.insert(partial.name.clone(), (target.clone(), decl.span));
                    if matches!(partial.visibility, Visibility::Public)
                        && public_decls.get(target).is_some_and(|is_public| !*is_public)
                    {
                        self.errors.push(CompileError::type_error(
                            format!(
                                "Public partial '{}' targets private symbol '{}'",
                                partial.name,
                                partial.target.segments.join(".")
                            ),
                            decl.span,
                        ));
                    }
                }
                if matches!(partial.visibility, Visibility::Public) {
                    self.validate_public_partial_presets_do_not_reference_private_symbols(
                        &partial.name,
                        &partial.args,
                        public_decls,
                    );
                }
            }
            _ => {}
        }
    }

    /// Reject direct and indirect cycles between top-level aliases.
    fn validate_top_level_alias_cycles(&mut self, alias_targets: &HashMap<String, (String, Span)>) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors
                            .push(CompileError::type_error(format!("Alias cycle detected: {key}"), *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Reject direct and indirect cycles in top-level partial declarations, including cycles that route through
    /// aliases.
    fn validate_top_level_partial_cycles(
        &mut self,
        alias_targets: &HashMap<String, (String, Span)>,
        partial_targets: &HashMap<String, (String, Span)>,
    ) {
        let mut edges: HashMap<String, (String, Span)> = alias_targets.clone();
        edges.extend(partial_targets.clone());

        let mut reported = HashSet::new();
        for (name, (_, span)) in partial_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = edges.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if cycle.iter().any(|segment| partial_targets.contains_key(segment)) && reported.insert(key.clone())
                    {
                        self.errors.push(CompileError::type_error(
                            format!("Partial cycle detected: {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Public partial provenance must remain checkable for consumers without private module state.
    fn validate_public_partial_presets_do_not_reference_private_symbols(
        &mut self,
        partial_name: &str,
        args: &[PartialArg],
        public_decls: &HashMap<String, bool>,
    ) {
        for arg in args {
            self.validate_public_partial_preset_expr(partial_name, &arg.name, &arg.value, public_decls);
        }
    }

    /// Recursively reject private roots used by public partial preset values.
    fn validate_public_partial_preset_expr(
        &mut self,
        partial_name: &str,
        preset_name: &str,
        expr: &Spanned<Expr>,
        public_decls: &HashMap<String, bool>,
    ) {
        if let Some(root) = preset_private_root(expr, public_decls) {
            self.errors.push(CompileError::type_error(
                format!("Public partial '{partial_name}' preset '{preset_name}' references private symbol '{root}'"),
                expr.span,
            ));
        }

        match &expr.node {
            Expr::Paren(inner) => {
                self.validate_public_partial_preset_expr(partial_name, preset_name, inner, public_decls)
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, key, public_decls);
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                        DictEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Call(_, _, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            Expr::Constructor(_, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Reject direct and indirect cycles in newtype underlying declarations before coercion planning.
    fn validate_newtype_underlying_cycles(
        &mut self,
        newtype_underlyings: &HashMap<String, (String, Span)>,
        newtype_names: &HashSet<String>,
    ) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in newtype_underlyings {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = newtype_underlyings.get(&current) {
                if !newtype_names.contains(target) {
                    break;
                }
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(errors::newtype_coercion_cycle(&key, *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method alias names and targets for one method-bearing declaration.
    fn validate_method_alias_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .map(|method| method.node.name.as_str())
            .collect();
        let mut alias_targets = HashMap::new();

        for alias in aliases {
            if !self.member_binding_is_active(alias.span) {
                continue;
            }
            if !method_names.contains(alias.node.target.as_str()) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method alias '{}.{}' targets unknown method '{}'",
                        owner, alias.node.name, alias.node.target
                    ),
                    alias.span,
                ));
            }
            alias_targets.insert(alias.node.name.clone(), (alias.node.target.clone(), alias.span));
        }

        self.validate_method_alias_cycles(owner, &alias_targets);
    }

    /// Register every authored member spelling through one source-ordered first-wins collision mechanism.
    ///
    /// Model/class fields and properties occupy the instance surface. Receiverless methods occupy the type surface,
    /// so their spelling may match an instance member without becoming ambiguous at either use site. Method aliases
    /// and partials inherit the surface of the method they project.
    fn validate_member_name_collisions(
        &mut self,
        owner: &str,
        fields: &[Spanned<FieldDecl>],
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        properties: &[Spanned<PropertyDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut method_surfaces: HashMap<String, HashSet<MemberBindingSurface>> = HashMap::new();
        for method in methods {
            method_surfaces
                .entry(method.node.name.clone())
                .or_default()
                .insert(MemberBindingSurface::for_method(&method.node));
        }
        let mut alias_surfaces: HashMap<String, HashSet<MemberBindingSurface>> = HashMap::new();
        for alias in aliases {
            if let Some(surfaces) = method_surfaces.get(&alias.node.target) {
                alias_surfaces.insert(alias.node.name.clone(), surfaces.clone());
            }
        }

        let mut declarations =
            Vec::with_capacity(fields.len() + aliases.len() + partials.len() + properties.len() + methods.len());
        declarations.extend(fields.iter().map(|field| {
            (
                field.node.name.clone(),
                MemberBindingKind::Field,
                MemberBindingSurface::Instance,
                field.span,
            )
        }));
        declarations.extend(methods.iter().map(|method| {
            (
                method.node.name.clone(),
                MemberBindingKind::Method,
                MemberBindingSurface::for_method(&method.node),
                method.span,
            )
        }));
        declarations.extend(properties.iter().map(|property| {
            (
                property.node.name.clone(),
                MemberBindingKind::Property,
                MemberBindingSurface::Instance,
                property.span,
            )
        }));
        for alias in aliases {
            let surfaces = alias_surfaces
                .get(&alias.node.name)
                .cloned()
                .unwrap_or_else(|| HashSet::from([MemberBindingSurface::Instance]));
            declarations.extend(surfaces.into_iter().map(|surface| {
                (
                    alias.node.name.clone(),
                    MemberBindingKind::MethodAlias,
                    surface,
                    alias.span,
                )
            }));
        }
        for partial in partials {
            let surfaces = method_surfaces
                .get(&partial.node.target)
                .or_else(|| alias_surfaces.get(&partial.node.target))
                .cloned()
                .unwrap_or_else(|| HashSet::from([MemberBindingSurface::Instance]));
            declarations.extend(surfaces.into_iter().map(|surface| {
                (
                    partial.node.name.clone(),
                    MemberBindingKind::MethodPartial,
                    surface,
                    partial.span,
                )
            }));
        }
        self.register_source_ordered_member_bindings(owner, declarations);
    }

    /// Register enum variants, their aliases, and enum methods through the shared member-namespace mechanism.
    fn validate_enum_member_name_collisions(
        &mut self,
        owner: &str,
        variants: &[Spanned<VariantDecl>],
        aliases: &[Spanned<EnumVariantAliasDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let mut declarations = Vec::with_capacity(variants.len() + aliases.len() + methods.len());
        declarations.extend(variants.iter().map(|variant| {
            (
                variant.node.name.clone(),
                MemberBindingKind::Variant,
                MemberBindingSurface::Type,
                variant.span,
            )
        }));
        declarations.extend(aliases.iter().map(|alias| {
            (
                alias.node.name.clone(),
                MemberBindingKind::VariantAlias,
                MemberBindingSurface::Type,
                alias.span,
            )
        }));
        declarations.extend(methods.iter().map(|method| {
            (
                method.node.name.clone(),
                MemberBindingKind::Method,
                MemberBindingSurface::for_method(&method.node),
                method.span,
            )
        }));
        self.register_source_ordered_member_bindings(owner, declarations);
    }

    /// Apply the one source-ordered, first-wins collision decision shared by every member-bearing declaration.
    fn register_source_ordered_member_bindings(
        &mut self,
        owner: &str,
        mut declarations: Vec<(String, MemberBindingKind, MemberBindingSurface, Span)>,
    ) {
        declarations.sort_by_key(|(_, _, _, span)| *span);

        let mut seen = HashMap::new();
        for (name, kind, surface, span) in declarations {
            match register_binding(&mut seen, (name.clone(), surface), (kind, span)) {
                BindingRegistration::Registered => {
                    self.record_member_declaration_identity(&name, kind, span);
                }
                BindingRegistration::Collision {
                    existing: (MemberBindingKind::Method, _),
                } if kind == MemberBindingKind::Method => {
                    // Same-name methods form one overload binding.
                    self.record_member_declaration_identity(&name, kind, span);
                }
                BindingRegistration::Collision {
                    existing: (previous_kind, _),
                } if matches!(
                    (previous_kind, kind),
                    (MemberBindingKind::Field, MemberBindingKind::Method)
                        | (MemberBindingKind::Method, MemberBindingKind::Field)
                ) =>
                {
                    // A stored selection and method invocation are distinct source forms: `value.name` versus
                    // `value.name()`. Both declarations therefore remain active even when their spellings match. If
                    // the field appears second, retain it as the registry sentinel so a later duplicate field or
                    // field-like declaration still collides with storage rather than being mistaken for another
                    // callable coexistence case.
                    self.record_member_declaration_identity(&name, kind, span);
                    if kind == MemberBindingKind::Field {
                        seen.insert((name, surface), (kind, span));
                    }
                }
                BindingRegistration::Collision {
                    existing: (previous_kind, previous_span),
                } => {
                    self.rejected_member_bindings.insert(span);
                    self.record_member_name_collision(owner, &name, previous_kind, previous_span, kind, span);
                }
            }
        }
    }

    /// Return whether one member declaration survived the shared first-wins registry.
    pub(super) fn member_binding_is_active(&self, span: Span) -> bool {
        !self.rejected_member_bindings.contains(&span)
    }

    /// Export one accepted source-owned member declaration from the shared member registry.
    fn record_member_declaration_identity(&mut self, name: &str, kind: MemberBindingKind, span: Span) {
        let Some(kind) = kind.declaration_kind() else {
            // Aliases and partials introduce bindings to an existing method declaration; they do not mint one.
            return;
        };
        let identity = self.symbols.member_declaration_identity(name, kind, span);
        self.type_info
            .declarations
            .member_declaration_identities
            .insert(span, identity);
    }

    /// Render one collision already decided by the shared member registration.
    fn record_member_name_collision(
        &mut self,
        owner: &str,
        name: &str,
        previous_kind: MemberBindingKind,
        previous_span: Span,
        kind: MemberBindingKind,
        span: Span,
    ) {
        if matches!(
            previous_kind,
            MemberBindingKind::Variant | MemberBindingKind::VariantAlias
        ) && matches!(kind, MemberBindingKind::Variant | MemberBindingKind::VariantAlias)
        {
            self.errors.push(errors::duplicate_binding(name, previous_span, span));
            return;
        }

        let message = match kind {
            MemberBindingKind::MethodAlias => {
                format!("Duplicate method alias '{}' on '{}'", name, owner)
            }
            MemberBindingKind::MethodPartial => {
                format!("Duplicate method partial '{}.{}'", owner, name)
            }
            _ => format!(
                "Duplicate member '{}.{}' declared as both {} and {}",
                owner,
                name,
                previous_kind.label(),
                kind.label()
            ),
        };
        self.errors
            .push(CompileError::type_error(message, span).with_note(format!(
                "First declaration span: {}..{}",
                previous_span.start, previous_span.end
            )));
    }

    /// Reject direct and indirect cycles between method aliases on one owner type.
    fn validate_method_alias_cycles(&mut self, owner: &str, alias_targets: &HashMap<String, (String, Span)>) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(CompileError::type_error(
                            format!("Method alias cycle detected on '{owner}': {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method partial names and targets before collection turns them into method metadata.
    fn validate_method_partial_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .map(|method| method.node.name.as_str())
            .collect();
        let alias_names: HashSet<&str> = aliases
            .iter()
            .filter(|alias| self.member_binding_is_active(alias.span))
            .map(|alias| alias.node.name.as_str())
            .collect();

        for partial in partials {
            if !self.member_binding_is_active(partial.span) {
                continue;
            }
            if !method_names.contains(partial.node.target.as_str())
                && !alias_names.contains(partial.node.target.as_str())
            {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method partial '{}.{}' targets unknown method '{}'",
                        owner, partial.node.name, partial.node.target
                    ),
                    partial.span,
                ));
            }
        }
    }

    /// Collect concrete declarations first, then resolve aliases and partials in dependency order.
    ///
    /// Aliases may target partials and partials may target aliases, so two fixed phases are insufficient. Retrying
    /// unresolved projections to a fixed point preserves forward references without guessing through source names.
    /// Anything still unresolved after a no-progress pass is collected once so the normal unknown-target diagnostics
    /// remain authoritative (and the declaration validator can additionally report cycles).
    fn collect_declarations_for_check(&mut self, declarations: &[Spanned<Declaration>]) {
        for decl in declarations {
            if !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }

        let mut pending = declarations
            .iter()
            .filter(|decl| matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)))
            .collect::<Vec<_>>();
        while !pending.is_empty() {
            let mut deferred = Vec::new();
            let mut made_progress = false;
            for decl in pending {
                let target = match &decl.node {
                    Declaration::Alias(alias) => &alias.target.segments,
                    Declaration::Partial(partial) => &partial.target.segments,
                    _ => unreachable!("pending projection declarations contain only aliases and partials"),
                };
                if self.alias_target_symbol_kind(target).is_some() {
                    self.collect_declaration(decl);
                    made_progress = true;
                } else {
                    deferred.push(decl);
                }
            }
            if !made_progress {
                for decl in deferred {
                    self.collect_declaration(decl);
                }
                break;
            }
            pending = deferred;
        }
    }

    /// Return whether the program last checked declares `name` itself and marks that declaration `pub`.
    pub fn declares_public(&self, name: &str) -> bool {
        self.current_module_public_declarations.contains(name)
    }

    /// Check a program and return errors if any.
    ///
    /// Runs the two-pass type-checking algorithm:
    /// 1. **Collect**: register all type/function declarations in the symbol table.
    /// 2. **Check**: validate bodies, expressions, and cross-references.
    ///
    /// ## Parameters
    ///
    /// - `program`: the parsed AST to validate.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    ///
    /// ## Notes
    ///
    /// For multi-module projects, call [`import_module`](Self::import_module) first to populate dependency symbols,
    /// or use [`check_with_imports`](Self::check_with_imports) to import dependencies first.
    pub fn check_program(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        let preserve_dependency_semantics = std::mem::take(&mut self.dependency_semantics_pending);
        let result = self.check_program_internal(program, preserve_dependency_semantics);
        self.clear_nominal_identity_cache();
        result
    }

    /// Return the actual entries performed by the most recent program activation. The observation does not retain
    /// checker facts and never decides cache reuse, checking scope or publication readiness.
    pub fn check_work(&self) -> CheckWork {
        self.check_work
    }

    /// Check one program while retaining semantic-only type metadata collected from its dependency imports.
    fn check_program_with_dependency_semantics(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        self.dependency_semantics_pending = false;
        let result = self.check_program_internal(program, true);
        self.clear_nominal_identity_cache();
        result
    }

    /// Check one program in declaration, body and publication order, preserving dependency-only semantic caches
    /// only when the caller has collected them for this program. Accumulated fatal diagnostics prevent nominal
    /// snapshot publication.
    fn check_program_internal(
        &mut self,
        program: &Program,
        preserve_dependency_semantics: bool,
    ) -> Result<(), Vec<CompileError>> {
        self.reset_program_check(program, preserve_dependency_semantics);
        self.collect_program_check_declarations(program);
        self.record_program_header_facts();
        self.record_source_declaration_identities(program);
        self.visibility_point = VisibilityPoint::Collected;
        self.prepare_program_body_facts();
        self.check_program_declaration_bodies(program);
        self.record_program_lowering_facts(program);
        let result = self.finish_program_check(program);
        self.visibility_point = VisibilityPoint::Finished;
        result
    }

    /// Clear per-program observations before collecting declarations. Dependency semantic inventories survive only
    /// for an explicitly prepared imported program; symbol registration and alias validation keep their prior order.
    fn reset_program_check(&mut self, program: &Program, preserve_dependency_semantics: bool) {
        #[cfg(test)]
        self.body_local_observations.clear();
        #[cfg(test)]
        self.body_enclosing_observations.clear();
        self.check_work = CheckWork::default();
        self.body_checks.clear();
        self.checked_bodies.clear();
        self.checked_declarations.clear();
        self.diagnostic_owner_ranges.clear();
        self.body_open_diagnostics.clear();
        self.visibility_point = VisibilityPoint::Collecting;
        self.checking_nominal_scope.get_mut().take();
        self.validate_source_type_names = false;
        self.unknown_source_type_names_emitted.clear();
        self.source_type_declaration_names = Self::collect_source_type_declaration_names(&program.declarations);
        self.const_decls.clear();
        self.static_decls.clear();
        self.local_function_decls.clear();
        self.receiver_plan_inputs = Default::default();
        self.current_module_function_symbols.clear();
        self.rejected_member_bindings.clear();
        self.warned_public_c_abi_raw_call_owners.clear();
        self.static_decl_positions.clear();
        self.const_eval_state.clear();
        self.const_eval_cache.clear();
        self.type_info = TypeCheckInfo::default();
        self.warnings.clear();
        self.errors.clear();
        self.pending_uncopyable_dict_lookups.clear();
        self.mutable_bindings.clear();
        self.static_alias_bindings.clear();
        self.read_only_binding_spans.clear();
        self.closure_depth = 0;
        self.scoped_c_string_view_receivers.clear();
        self.unbound_scoped_c_string_views.clear();
        self.scoped_c_string_view_bindings.clear();
        self.testing_marker_import_bindings.clear();
        self.surface_function_import_bindings.clear();
        self.surface_type_import_bindings.clear();
        self.module_declared_type_names = Self::collect_module_declared_type_names(program);
        self.testing_fixture_names.clear();
        self.testing_marker_semantics = None;
        self.local_rust_derive_paths.clear();
        self.local_derive_facts.clear();
        self.hash_key_inference.local_requirements.clear();
        self.hash_key_inference.function_bounds.clear();
        self.source_import_targets.clear();
        self.surface_context = SurfaceContext::from_program(program);
        self.supertrait_closure.clear();
        self.enter_nominal_declaring_module(program);
        if !preserve_dependency_semantics {
            self.clear_nominal_declaring_modules();
            self.hash_key_inference.type_params.clear();
            self.transitive_pub_types.clear();
            self.public_library_type_identities.clear();
            self.transitive_pub_traits.clear();
            self.cached_pub_libraries.clear();
            self.foreign_pub_type_remappings.clear();
            self.pub_library_declaration_scopes.clear();
        }
    }

    /// Validate alias and reserved declaration spellings, then retain public names before signature collection.
    fn prepare_program_declaration_names(&mut self, program: &Program) {
        self.validate_alias_declarations(program);
        self.report_reserved_compiler_names(program);
        self.current_module_public_declarations = program
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(declaration_name)
            .map(str::to_string)
            .collect();
    }

    /// Collect signatures and settle inherited trait requirements before checking any body or call. Dependency
    /// supertraits resolve first, and callable capture planning uses the complete current declaration inventory.
    fn collect_program_check_declarations(&mut self, program: &Program) {
        self.prepare_program_declaration_names(program);
        // `check_with_imports` / `import_module` can queue supertrait bounds while collecting dependency ASTs.
        // Resolve those queued bounds into trait symbols before we collect and resolve the current program.
        if !self.pending_trait_supertraits.is_empty() {
            self.resolve_pending_trait_supertraits();
        }

        // First pass: collect concrete declarations, then aliases and partials after their possible targets are
        // available. Signatures that named a type declared further down the module now resolve to it (#1780).
        self.collect_declarations_for_check(&program.declarations);
        self.resolve_forward_type_references(&program.declarations);
        self.record_local_nominal_type_param_bounds(&program.declarations);
        // Every declaration is collected, so the signatures' hashed type parameters can be inferred before any body
        // or call is checked (#1758).
        self.infer_hash_key_type_params(&program.declarations, true);

        self.resolve_pending_trait_supertraits();
        self.finalize_supertrait_graph();
        self.merge_supertrait_requires_into_traits();
        self.validate_static_dependencies();
        self.collect_testing_fixture_names(program);
        // Every signature is collected, so the function-typed parameters and returns that hold capturing closures
        // can be decided before any call or body is checked (#1561).
        self.plan_closure_holding_callables(program);
    }

    /// Publish declaration-owned identity, import and alias bindings once collection has settled them. Body checking
    /// may add lexical identities, but it does not own or republish the module's header binding inventory.
    fn record_program_header_facts(&mut self) {
        self.export_module_declaration_bindings();
        self.export_checked_import_bindings();
        self.export_type_alias_targets();
    }

    /// Retain private declaration-time native and nominal facts for body checking, reusing only matching mutation
    /// revisions. Finalization publishes the current inventory only after accumulated fatal diagnostics are absent.
    fn prepare_program_body_facts(&mut self) {
        // Direct-route call and pattern facts need the already collected native owners and their public paths while
        // checking arguments and results. Keep this snapshot private; ordinary nominal tables retain their final
        // successful-check timing.
        self.type_info.declarations.body_ir_native_type_identities = self.checked_native_nominal_type_identities();
        self.type_info.rust.item_public_paths = self.checked_rust_item_public_paths();
        // Internal carrier identities must survive per-module fact resets before Result payloads are checked. Source
        // alias entries still publish only after successful checking.
        let nominal_scope = self.checked_nominal_type_identities();
        self.type_info.declarations.named_type_identities = nominal_scope
            .iter()
            .filter(|(name, identity)| {
                incan_semantics_core::body_ir::nominal_carrier_key(identity).as_deref() == Some(name.as_str())
            })
            .map(|(name, identity)| (name.clone(), identity.clone()))
            .collect();
    }

    /// Check constants before other declarations, then settle argument ownership and deferred dictionary refusals.
    /// Source-name validation is active across both passes and remains active until native declarations are checked.
    fn check_program_declaration_bodies(&mut self, program: &Program) {
        // Second pass: check consts first so their resolved types are available to later checks.
        self.validate_source_type_names = true;
        for decl in &program.declarations {
            if matches!(decl.node, Declaration::Const(_)) {
                self.check_environment_declaration(decl);
            }
        }
        for decl in &program.declarations {
            if !matches!(decl.node, Declaration::Const(_)) {
                self.check_environment_declaration(decl);
            }
        }
        #[cfg(test)]
        if self.legacy_body_checking {
            self.legacy_resolve_mut_arguments();
            self.refuse_copying_dict_lookups_of_uncopyable_values();
        } else {
            self.settle_body_checks(program);
        }
        #[cfg(not(test))]
        self.settle_body_checks(program);
    }

    /// Retain layout and trait facts after body checking without publishing a nominal snapshot yet. Native module
    /// validation precedes inferred-bound and nested-identity export; module header bindings have already been
    /// published.
    fn record_program_lowering_facts(&mut self, program: &Program) {
        self.type_info
            .c_abi
            .resolve_checked_facades(self.current_module_path.as_deref());
        self.record_trait_metadata_for_lowering();
        self.record_standard_registry_types();
        self.record_trait_adoption_arguments_for_lowering(program);
        self.record_body_ir_bare_adoption_arguments(program);
        self.record_model_field_visibilities_for_lowering(program);
        self.record_class_layouts_for_lowering(program);
        self.record_body_ir_class_inheritance(program);
        self.record_method_decorator_receiver_slots(program);

        // ---- RFC 023: validate rust.module() and @rust.extern rules ----
        self.validate_rust_module_and_extern(program);
        self.validate_source_type_names = false;
        // The module is checked; its inferred hashed type parameters now join its exported signatures (#1758).
        self.export_inferred_hash_key_bounds();

        self.export_nested_declaration_identities();
        self.record_binding_collision_diagnostics();
    }

    /// Resolve final destination and artifact demands, retain warnings, and publish only after fatal diagnostics are
    /// absent. Publication's own demands are validated again before the checked result is accepted.
    fn finish_program_check(&mut self, program: &Program) -> Result<(), Vec<CompileError>> {
        if self.loaves.keys().next().is_some() {
            for (name, identity) in &self.public_library_type_identities {
                if identity.selected_provider.is_none()
                    && matches!(
                        identity.canonical.as_ref().map(|identity| &identity.origin),
                        Some(incan_semantics_core::SymbolOrigin::Package { .. })
                    )
                {
                    self.errors.push(CompileError::type_error(
                        format!("compiled nominal `{name}` has no unique admitted declaring artifact"),
                        Span::default(),
                    ));
                }
            }
        }
        self.settle_open_value_destinations();
        self.errors.append(&mut self.body_open_diagnostics);
        if let Err(error) = self.loaves.validate_demands() {
            self.errors
                .push(CompileError::type_error(error.to_string(), Span::default()));
        }
        // Split fatal errors from non-fatal diagnostics.
        let all = std::mem::take(&mut self.errors);
        let (fatal, non_fatal): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|e| !matches!(e.kind, ErrorKind::Warning | ErrorKind::Lint));
        self.warnings.extend(non_fatal);

        if fatal.is_empty() {
            self.publish_checked_nominal_scope();
            self.publish_rust_storage_types(program);
            self.type_info.body_ir_newtypes = native_newtype_facts::project(program, &self.type_info);
            self.publish_body_ir_empty_lists();
            self.checking_nominal_scope.get_mut().take();
            if let Err(error) = self.loaves.validate_demands() {
                return Err(vec![CompileError::type_error(error.to_string(), Span::default())]);
            }
            Ok(())
        } else {
            self.checking_nominal_scope.get_mut().take();
            Err(fatal)
        }
    }

    /// Publish the checked module-scope nominal and native identity tables Body IR reads, once the module is checked.
    fn publish_checked_nominal_scope(&mut self) {
        self.type_info.declarations.named_type_identities = self.take_checked_nominal_type_identities();
        self.type_info.declarations.body_ir_native_type_identities = self.checked_native_nominal_type_identities();
        self.type_info.body_ir_runtime_types = self.checked_body_ir_runtime_types();
        self.type_info.rust.item_public_paths = self.checked_rust_item_public_paths();
        self.type_info.rust.owner_parameters = self.checked_rust_owner_parameters();
    }

    /// Retain checked native default templates at the checker boundary. Only successfully demanded type metadata
    /// contributes facts; Body IR never starts a second metadata query to choose an owner's argument count.
    fn checked_rust_owner_parameters(&self) -> HashMap<String, Vec<(String, Option<ResolvedType>)>> {
        let mut owners = HashMap::new();
        for path in self.observed_rust_metadata_queries() {
            let Some(metadata) = self.lookup_rust_item_metadata_for_path(&path) else {
                continue;
            };
            let RustItemKind::Type(info) = &metadata.kind else {
                continue;
            };
            let parameters = info
                .type_params
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let default = info.type_param_defaults.get(index).and_then(Option::as_deref);
                    (
                        name.clone(),
                        default.map(|default| self.native_rust_signature_type(default, &path)),
                    )
                })
                .collect::<Vec<_>>();
            let public = self
                .type_info
                .rust
                .item_public_paths
                .get(&path)
                .cloned()
                .unwrap_or(path);
            owners.insert(public, parameters);
        }
        owners
    }

    /// Select the metadata producer's public owner when it belongs to the selected extern namespace. A private
    /// definition or a definition reached through another crate still needs the admitted public import projection.
    /// Source imports cannot choose a different carrier merely by naming another reexport of the same definition.
    fn rust_public_definition_path<'a>(&self, path: &str, metadata: &'a RustItemMetadata) -> Option<&'a str> {
        let definition = metadata.definition_path.as_deref().unwrap_or(&metadata.canonical_path);
        if definition.split("::").next() != path.split("::").next() {
            return None;
        }
        let owner = self.rust_item_metadata_for_path(definition)?;
        (owner.visibility == incan_lang::interop::RustVisibility::Public).then_some(definition)
    }

    /// Map imported Rust spellings to the producer-selected public carrier per native definition and bound extern.
    /// The producer's DefId-backed path is independent of the importing module. Only inaccessible or cross-extern
    /// definitions use the smallest admitted public projection; private paths never become executable identities.
    fn checked_rust_item_public_paths(&self) -> HashMap<String, String> {
        // ---- Public representatives of native definitions ----
        let mut type_paths: HashMap<(String, String), (String, bool)> = HashMap::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            let metadata = info
                .metadata
                .clone()
                .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
            let Some(metadata) = metadata else {
                continue;
            };
            if !matches!(metadata.kind, RustItemKind::Type(_)) {
                continue;
            }
            let definition = metadata.definition_path.as_ref().unwrap_or(&metadata.canonical_path);
            let imported = Self::normalize_rust_namespace_path(&info.path);
            let domain = imported.split("::").next().unwrap_or("").to_owned();
            match type_paths.entry((domain, definition.clone())) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let producer = self.rust_public_definition_path(imported, &metadata);
                    entry.insert((producer.unwrap_or(imported).to_owned(), producer.is_some()));
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let (selected, producer_selected) = entry.get_mut();
                    if !*producer_selected && imported < selected.as_str() {
                        *selected = imported.to_owned();
                    }
                }
            }
        }

        // ---- Source spellings and defining paths ----
        let mut paths = HashMap::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            let public = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
            let metadata = info
                .metadata
                .clone()
                .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
            let domain = public.split("::").next().unwrap_or("").to_owned();
            let canonical = metadata
                .as_ref()
                .filter(|metadata| matches!(metadata.kind, RustItemKind::Type(_)))
                .map(|metadata| metadata.definition_path.as_ref().unwrap_or(&metadata.canonical_path))
                .and_then(|definition| type_paths.get(&(domain, definition.clone())))
                .map(|(public, _)| public.clone())
                .unwrap_or_else(|| public.clone());
            if let Some(definition) = metadata.and_then(|metadata| metadata.definition_path.clone()) {
                paths.entry(definition).or_insert_with(|| canonical.clone());
            }
            if self.lookup_symbol(&symbol.name).is_some_and(
                |selected| matches!(&selected.kind, SymbolKind::RustItem(selected) if selected.path == info.path),
            ) {
                paths.insert(symbol.name.clone(), canonical.clone());
            }
            paths.insert(public, canonical);
        }
        paths
    }

    /// The `std.*` path of the module declaring `identity`, when a standard package declares it: another Loaf's
    /// declaration (its origin names the package) or one of the program's own standard modules.
    pub(in crate::typechecker) fn standard_declaring_module(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Option<Vec<String>> {
        let std_path = |path: &[String]| {
            std::iter::once(stdlib::STDLIB_ROOT.to_string())
                .chain(path.iter().cloned())
                .collect()
        };
        match &identity.origin {
            SymbolOrigin::Package { library, module_path }
                if incan_lang::lang::standard_packages::standard_package_namespace_policy(library).is_some() =>
            {
                Some(std_path(module_path))
            }
            SymbolOrigin::Module(path) if path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) => {
                Some(path.clone())
            }
            SymbolOrigin::Module(path) if self.own_standard_package.is_some() => Some(std_path(path)),
            _ => None,
        }
    }

    /// The identity of the import binding that spells the surface type `surface` (`Html`) in this module: the
    /// declaration the owning standard package publishes.
    pub(in crate::typechecker) fn surface_type_identity(&self, surface: SurfaceTypeId) -> Option<CanonicalSymbolId> {
        self.surface_type_import_bindings
            .values()
            .find(|(bound, symbol)| *bound == surface && self.symbols.is_active_lookup_binding(*symbol))
            .and_then(|(_, symbol)| self.symbols.identity_of(*symbol).cloned())
    }

    /// Retain active native builtin owners once for opaque nominal storage and checked runtime operations, without
    /// changing legacy lookup. Active rusttype aliases retain their checked source owner for native enum contracts.
    /// Web placeholders use their checked surface binding and original Rust re-export;
    /// the kernel whitelist validates identities and source shadows yield nothing.
    fn checked_native_nominal_type_identities(&mut self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self
            .symbols
            .active_module_symbols()
            .filter_map(|(id, symbol)| {
                let identity = self.symbols.identity_of(id)?;
                if identity.kind == SemanticSourceTargetKind::Rusttype {
                    return Some((symbol.name.clone(), identity.clone()));
                }
                if identity.origin != incan_semantics_core::SymbolOrigin::Builtin {
                    return None;
                }
                incan_semantics_core::body_ir::native_carrier_nominal(identity)?;
                Some((symbol.name.clone(), identity.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for (name, (_, symbol)) in &self.surface_type_import_bindings {
            if self.symbols.lookup(name) == Some(*symbol)
                && let Some(identity) = self.symbols.identity_of(*symbol)
            {
                identities.insert(name.clone(), identity.clone());
            }
        }
        // Generated reflection calls retain their published runtime carrier without introducing a source binding.
        for (name, identity) in &self.type_info.declarations.body_ir_native_type_identities {
            if matches!(identity.origin, SymbolOrigin::Package { .. })
                && incan_semantics_core::body_ir::native_nominal_item(identity).is_some()
            {
                identities.insert(name.clone(), identity.clone());
            }
        }
        identities
    }

    /// Preserve accepted root builtin bindings solely for direct-route storage; existing nominal and legacy facts stay
    /// unchanged.
    fn checked_body_ir_runtime_types(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        self.symbols
            .active_module_symbols()
            .filter_map(|(id, symbol)| {
                let identity = self.symbols.identity_of(id)?;
                incan_semantics_core::RuntimeBuiltinType::from_identity(identity)?;
                Some((symbol.name.clone(), identity.clone()))
            })
            .collect()
    }

    /// Retain active module-scope nominal bindings from both source modules and admitted packages.
    ///
    /// Imported source types already carry their declaring identity in the symbol table. Body IR must retain that
    /// fact before publication; recovering it from a spelling in lowering would lose aliases and module ownership.
    fn collect_checked_nominal_scope(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self.checked_nominal_bindings();
        // Immutable declaration scopes contribute through their admission index, not a scan of every prior snapshot.
        let carriers = identities
            .values()
            .filter_map(|identity| {
                self.retained_nominal_carriers
                    .get(identity)
                    .cloned()
                    .or_else(|| incan_semantics_core::body_ir::nominal_carrier_key(identity))
                    .map(|key| (key, identity.clone()))
            })
            .collect::<Vec<_>>();
        for (key, identity) in carriers.into_iter().chain(
            self.retained_nominal_carriers
                .iter()
                .map(|(identity, key)| (key.clone(), identity.clone())),
        ) {
            identities.entry(key).or_insert(identity);
        }
        identities
    }

    /// Collect current lexical and public nominal owners, preserving public precedence and ambiguous alias removal.
    /// The symbol owner already indexes active bindings; historical rejected and dependency rows are never scanned.
    fn checked_nominal_bindings(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self
            .public_library_type_identities
            .iter()
            .filter_map(|(name, identity)| Some((name.clone(), identity.canonical.clone()?)))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (id, symbol) in self.symbols.active_module_symbols() {
            let Some(identity) = self.symbols.identity_of(id) else {
                continue;
            };
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::Newtype
            ) {
                identities
                    .entry(symbol.name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        let mut ambiguous = HashSet::new();
        for (name, alias) in &self.type_aliases {
            let active_alias = self.symbols.lookup(name).and_then(|id| self.symbols.get(id));
            if !active_alias.is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Type(TypeInfo::TypeAlias))) {
                continue;
            }
            for (leaf, identity) in &alias.nominal_identities {
                if identities.get(leaf).is_some_and(|bound| bound != identity) {
                    ambiguous.insert(leaf.clone());
                } else {
                    identities.insert(leaf.clone(), identity.clone());
                }
            }
        }
        for name in ambiguous {
            identities.remove(&name);
        }
        identities
    }

    /// Reuse the current scope only while all owning revisions match, including in-place alias target mutations.
    fn checked_body_nominal_scope(&self) -> Option<Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>> {
        let scope = self.checking_nominal_scope.borrow();
        let scope = scope.as_ref()?;
        (scope.inputs == self.nominal_scope_inputs()).then(|| Arc::clone(&scope.identities))
    }

    /// Read constant-size mutation witnesses instead of checking each public owner and alias binding at every use.
    fn nominal_scope_inputs(&self) -> NominalScopeInputs {
        NominalScopeInputs {
            symbols: self.symbols.nominal_revision(),
            aliases: self.type_aliases.revision(),
            public: self.public_library_type_identities.revision(),
            declarations: self.declaration_nominal_scopes.len(),
            carriers: self.retained_nominal_carriers.len(),
        }
    }

    /// Turn collisions from the shared symbol-registration mechanism into source diagnostics.
    ///
    /// The mechanism decides collision and ambiguity once; this layer owns user-facing wording. Two imports are
    /// ambiguous when their local spellings collide and their canonical identities differ or either target is
    /// unproven. A repeated binding to the same proven declaration is still a duplicate definition, not a new
    /// semantic declaration.
    fn record_binding_collision_diagnostics(&mut self) {
        for collision in self.symbols.take_binding_collisions() {
            let immutable_builtin = collision.existing_identity.as_ref().is_some_and(|identity| {
                matches!(identity.origin, incan_semantics_core::SymbolOrigin::Builtin)
                    && identity.declaration_name == builtins::as_str(BuiltinFnId::Print)
            });
            let ambiguous_import = collision.existing_is_import
                && collision.incoming_is_import
                && (collision.existing_identity.is_none()
                    || collision.incoming_identity.is_none()
                    || collision.existing_identity != collision.incoming_identity);
            let mut error = if immutable_builtin {
                errors::immutable_builtin_redefinition(&collision.name, collision.incoming_span)
            } else if ambiguous_import {
                errors::ambiguous_import_binding(&collision.name, collision.existing_span, collision.incoming_span)
            } else {
                errors::duplicate_binding(&collision.name, collision.existing_span, collision.incoming_span)
            };
            if let Some(identity) = &collision.existing_identity {
                error = error.with_note(format!("First canonical identity: {}", identity.render_compact()));
            }
            if let Some(identity) = &collision.incoming_identity {
                error = error.with_note(format!("Second canonical identity: {}", identity.render_compact()));
            }
            self.errors.push(error);
        }
    }

    /// Import symbols from another module's AST into the symbol table.
    ///
    /// Call this before [`check_program`](Self::check_program) so the main module can reference types and functions
    /// defined in dependencies.
    ///
    /// ## Parameters
    ///
    /// - `module_ast`: parsed AST of the dependency module.
    /// - `module_name`: dependency cache key whose registered path owns the collected declarations.
    pub fn import_module(&mut self, module_ast: &Program, module_name: &str) {
        let previous_surface_context = self.surface_context.clone();
        let previous_module_function_symbols = std::mem::take(&mut self.current_module_function_symbols);
        let previous_module_path = self.current_module_path.clone();
        let previous_type_aliases = self.type_aliases.clone();
        let dependency_module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
        self.set_current_module_path(Some(dependency_module_path));
        let previous_nominal_declarations = self.enter_nominal_declaring_module(module_ast);
        self.surface_context = SurfaceContext::from_program(module_ast);
        self.symbols.begin_dependency_interface_bindings();
        self.seed_dependency_interface_bindings(module_ast, true);

        // Imports are lexical rather than exported, but public declaration signatures may depend on their exact type
        // metadata. Collect them temporarily, then restore only the names they introduced after the public surface has
        // been cached.
        self.begin_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Import(_)) {
                self.collect_declaration(decl);
            }
        }
        let (imported_symbol_bindings, imported_type_alias_bindings) =
            self.finish_dependency_import_binding_transaction();
        // Collect only public declarations from the imported module.
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && !matches!(decl.node, Declaration::Alias(_) | Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if is_public_decl(decl) && matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.resolve_forward_type_references(&module_ast.declarations);
        self.record_local_nominal_type_param_bounds(&module_ast.declarations);
        self.resolve_pending_trait_supertraits();
        self.infer_hash_key_type_params(&module_ast.declarations, false);
        self.register_dependency_derivable_metadata(module_name, module_ast);
        self.cache_dependency_direct_member_symbols(module_name, module_ast, true);
        self.cache_dependency_registry_definitions(module_name, module_ast, true);
        self.cache_dependency_member_symbols(module_name, module_ast, true);
        let owned_names = module_ast
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(declaration_name)
            .collect::<HashSet<_>>();
        let owned_type_alias_names = module_ast
            .declarations
            .iter()
            .filter(|decl| is_public_decl(decl))
            .filter_map(|decl| match &decl.node {
                Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        self.restore_dependency_import_bindings(
            imported_symbol_bindings,
            imported_type_alias_bindings,
            &owned_names,
            &owned_type_alias_names,
        );
        self.dependency_semantics_pending = true;
        self.surface_context = previous_surface_context;
        self.current_module_function_symbols = previous_module_function_symbols;
        self.symbols.finish_dependency_interface_bindings();
        self.type_aliases = previous_type_aliases;
        self.restore_nominal_declaring_module(previous_nominal_declarations);
        self.set_current_module_path(previous_module_path);
    }

    /// Import all symbols from another module's AST into the symbol table.
    ///
    /// This is used for internal compiler passes that need type information across modules without enforcing `pub`
    /// visibility (e.g. codegen-only validation for dependencies).
    pub fn import_module_all(&mut self, module_ast: &Program, module_name: &str) {
        let previous_surface_context = self.surface_context.clone();
        let previous_module_function_symbols = std::mem::take(&mut self.current_module_function_symbols);
        let previous_module_path = self.current_module_path.clone();
        let previous_type_aliases = self.type_aliases.clone();
        let dependency_module_path = self
            .dependency_module_path_segments
            .get(module_name)
            .cloned()
            .unwrap_or_else(|| vec![module_name.to_string()]);
        self.set_current_module_path(Some(dependency_module_path));
        let previous_nominal_declarations = self.enter_nominal_declaring_module(module_ast);
        self.surface_context = SurfaceContext::from_program(module_ast);
        self.symbols.begin_dependency_interface_bindings();
        self.seed_dependency_interface_bindings(module_ast, false);

        // Dependency imports must be available while its declarations are collected so public signatures retain exact
        // provider-owned type metadata. They still belong only to the dependency's lexical scope, so remember which
        // root bindings they introduce and restore those names before the consumer program is collected.
        self.begin_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Import(_)) {
                self.collect_declaration(decl);
            }
        }
        let (imported_symbol_bindings, imported_type_alias_bindings) =
            self.finish_dependency_import_binding_transaction();
        for decl in &module_ast.declarations {
            if !matches!(
                decl.node,
                Declaration::Import(_) | Declaration::Alias(_) | Declaration::Partial(_)
            ) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Alias(_)) {
                self.collect_declaration(decl);
            }
        }
        for decl in &module_ast.declarations {
            if matches!(decl.node, Declaration::Partial(_)) {
                self.collect_declaration(decl);
            }
        }
        self.resolve_forward_type_references(&module_ast.declarations);
        self.record_local_nominal_type_param_bounds(&module_ast.declarations);
        self.resolve_pending_trait_supertraits();
        self.infer_hash_key_type_params(&module_ast.declarations, false);
        self.register_dependency_derivable_metadata(module_name, module_ast);
        self.cache_dependency_direct_member_symbols(module_name, module_ast, false);
        self.cache_dependency_registry_definitions(module_name, module_ast, false);
        self.cache_dependency_member_symbols(module_name, module_ast, false);
        let owned_names = module_ast
            .declarations
            .iter()
            .filter_map(declaration_name)
            .collect::<HashSet<_>>();
        let owned_type_alias_names = module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::TypeAlias(alias) => Some(alias.name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        self.restore_dependency_import_bindings(
            imported_symbol_bindings,
            imported_type_alias_bindings,
            &owned_names,
            &owned_type_alias_names,
        );
        self.dependency_semantics_pending = true;
        self.surface_context = previous_surface_context;
        self.current_module_function_symbols = previous_module_function_symbols;
        self.symbols.finish_dependency_interface_bindings();
        self.type_aliases = previous_type_aliases;
        self.restore_nominal_declaring_module(previous_nominal_declarations);
        self.set_current_module_path(previous_module_path);
    }

    /// Start a narrow transaction for source-visible bindings mutated while dependency imports are collected.
    fn begin_dependency_import_binding_transaction(&mut self) {
        self.symbols.begin_current_scope_binding_transaction();
        debug_assert!(self.dependency_import_type_alias_transaction.is_none());
        self.dependency_import_type_alias_transaction = Some(HashMap::new());
    }

    /// Finish the dependency import transaction and return the previous values of only the bindings it changed.
    fn finish_dependency_import_binding_transaction(
        &mut self,
    ) -> (
        HashMap<String, Option<SymbolId>>,
        HashMap<String, Option<TypeAliasTarget>>,
    ) {
        (
            self.symbols.finish_current_scope_binding_transaction(),
            self.dependency_import_type_alias_transaction.take().unwrap_or_default(),
        )
    }

    /// Record one type-alias binding before a dependency import changes it.
    fn record_dependency_import_type_alias_before_change(&mut self, name: &str) {
        let previous = self.type_aliases.get(name).cloned();
        if let Some(transaction) = &mut self.dependency_import_type_alias_transaction {
            transaction.entry(name.to_string()).or_insert(previous);
        }
    }

    /// Remove dependency-lexical imports from the consumer scope while retaining dependency-owned declarations.
    fn restore_dependency_import_bindings(
        &mut self,
        imported_symbol_bindings: HashMap<String, Option<SymbolId>>,
        imported_type_alias_bindings: HashMap<String, Option<TypeAliasTarget>>,
        owned_names: &HashSet<&str>,
        owned_type_alias_names: &HashSet<&str>,
    ) {
        for (name, previous_binding) in imported_symbol_bindings {
            if owned_names.contains(name.as_str()) {
                continue;
            }
            self.symbols.restore_current_scope_binding(&name, previous_binding);
        }
        for (name, previous_alias) in imported_type_alias_bindings {
            if owned_type_alias_names.contains(name.as_str()) {
                continue;
            }
            if let Some(previous_alias) = previous_alias {
                self.type_aliases.insert(name, previous_alias);
            } else {
                self.type_aliases.remove(&name);
            }
        }
    }

    /// Rebuild exact dependency-member caches after all dependency modules have been collected.
    ///
    /// Public facade modules may re-export items from sibling modules. Importing a facade before the underlying
    /// sibling has been fully collected can leave the facade cache with a placeholder type shell. A final cache pass
    /// after dependency collection lets re-exports resolve through the now-complete target module metadata.
    fn refresh_dependency_member_symbols(&mut self, dependencies: &[(&str, &Program)], public_only: bool) {
        for (name, dep_ast) in dependencies {
            self.cache_dependency_member_symbols(name, dep_ast, public_only);
        }
        self.cache_dependency_adoption_traits();
    }

    /// Cache exact member symbols exported by one dependency module for source `from ... import ...` resolution.
    fn cache_dependency_member_symbols(&mut self, module_name: &str, module_ast: &Program, public_only: bool) {
        self.retain_body_ir_dependency_pub_reexports(module_name, module_ast, public_only);
        let mut member_symbols = HashMap::new();
        let mut member_type_aliases = HashMap::new();
        let mut member_projections = HashMap::new();
        if let Some(direct_symbols) = self.dependency_direct_member_symbols.get(module_name) {
            member_symbols.extend(direct_symbols.clone());
        } else {
            self.cache_dependency_direct_member_symbols(module_name, module_ast, public_only);
            if let Some(direct_symbols) = self.dependency_direct_member_symbols.get(module_name) {
                member_symbols.extend(direct_symbols.clone());
            }
        }
        if let Some(direct_projections) = self.dependency_direct_member_partial_projections.get(module_name) {
            member_projections.extend(direct_projections.clone());
        }
        if let Some(direct_aliases) = self.dependency_direct_member_type_aliases.get(module_name) {
            member_type_aliases.extend(direct_aliases.clone());
        }
        let mut reexports = HashMap::new();
        // A facade's own relative imports resolve against the facade's module, not the consumer's. Resolving them
        // from the consumer would bind `pkg.facade`'s `from helpers import render` to the root `helpers`.
        let facade_module_path = self
            .canonical_owner_segments(module_name)
            .unwrap_or_else(|| vec![module_name.to_string()]);
        for (exported_name, module, item_name) in Self::dependency_source_reexport_targets(module_ast) {
            reexports.insert(exported_name.clone(), (module.clone(), item_name.clone()));
            if let Some(kind) = self.dependency_reexported_member_symbol(&facade_module_path, &module, &item_name) {
                member_symbols.insert(exported_name.clone(), kind);
            }
            if let Some(target) =
                self.dependency_member_type_alias_for_path_from(&facade_module_path, &module, &item_name)
            {
                member_type_aliases.insert(exported_name.clone(), target);
            }
            if let Some(mut projection) =
                self.dependency_member_partial_projection_for_path_from(&facade_module_path, &module, &item_name)
            {
                projection.name.clone_from(&exported_name);
                member_projections.insert(exported_name, projection);
            }
        }
        self.dependency_member_reexports
            .insert(module_name.to_string(), reexports);
        self.dependency_member_symbols
            .insert(module_name.to_string(), member_symbols);
        self.dependency_member_type_aliases
            .insert(module_name.to_string(), member_type_aliases);
        self.dependency_member_partial_projections
            .insert(module_name.to_string(), member_projections);
    }

    /// Resolve a re-exported member from another source module or from the Loaf that owns the re-exported module.
    ///
    /// A facade that republishes `std.*` must bind the same declaration a direct `from std.x import f` binds: the
    /// owning Loaf's checked declaration, whose identity is what lets lowering recover the compiled signature,
    /// defaults included.
    ///
    /// A facade that writes `from std.serde.json import Serialize` re-exports that trait exactly as it re-exports a
    /// function, so a consumer importing the spelling from the facade must bind the stdlib trait rather than a module
    /// placeholder: the placeholder let adoption and method calls through unproven, and lowering then had only the
    /// spelling to key the trait's protocol on (#1431).
    fn dependency_reexported_member_symbol(
        &mut self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<SymbolKind> {
        if let Some(kind) = self.dependency_member_symbol_for_path_from(base_module_path, module, item_name) {
            return Some(kind);
        }
        if module.parent_levels != 0 {
            return None;
        }
        let module_path = canonicalize_source_module_segments(&module.segments);
        let (library, rest) = self.loaf_module(&module_path)?;
        let (kind, _) = self.lookup_pub_library_module_symbol_member(&library, &rest, item_name)?;
        matches!(
            kind,
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) | SymbolKind::Trait(_)
        )
        .then_some(kind)
    }

    /// The Loaf owning a bound module path and the public module path inside its manifest: `pub.<key>.<path>` for a
    /// dependency, `std.<root>.<path>` for a standard package other than the program's own.
    pub(in crate::typechecker) fn loaf_module(&self, module_path: &[String]) -> Option<(String, Vec<String>)> {
        if self.owns_standard_module(module_path) {
            return None;
        }
        let entry = self.loaves.owner_of(module_path)?;
        let root = entry.binding_root();
        Some((entry.key.clone(), module_path[root.len()..].to_vec()))
    }

    /// Return whether `module` is one of the program's own standard modules: the program is a standard package and
    /// the `std.*` root belongs to it, so the module is its own source rather than another Loaf.
    pub(crate) fn owns_standard_module(&self, module: &[String]) -> bool {
        match (self.own_standard_package, module) {
            (Some(policy), [root, name, ..]) => {
                root == incan_lang::lang::stdlib::STDLIB_ROOT && policy.namespace_roots.contains(&name.as_str())
            }
            _ => false,
        }
    }

    /// Cache dependency declarations in their declaring nominal scope before consumer imports expose them. Fields and
    /// callable signatures retain the same declaration owners as their carrier metadata.
    fn cache_dependency_direct_member_symbols(&mut self, module_name: &str, module_ast: &Program, public_only: bool) {
        let mut direct_symbols = HashMap::new();
        let mut direct_type_aliases = HashMap::new();
        let mut direct_identities = HashMap::new();
        let mut direct_projections = HashMap::new();
        for name in Self::dependency_direct_member_names(module_ast, public_only) {
            if let Some(symbol_id) = self.symbols.lookup(name.as_str())
                && let Some(symbol) = self.symbols.get(symbol_id)
            {
                direct_symbols.insert(name.clone(), symbol.kind.clone());
                if let Some(identity) = self.symbols.identity_of(symbol_id) {
                    direct_identities.insert(name.clone(), identity.clone());
                }
            }
            if let Some(projection) = self.type_info.partial_projection(&name) {
                direct_projections.insert(name, projection.clone());
            }
        }
        let scope = self.checked_nominal_type_identities();
        let remapping = scope
            .iter()
            .filter_map(|(name, identity)| {
                incan_semantics_core::body_ir::nominal_carrier_key(identity).map(|key| (name.clone(), key))
            })
            .collect();
        let mut retained_scope = false;
        for (name, kind) in &mut direct_symbols {
            if let Some(identity) = direct_identities.get(name) {
                retained_scope |= self.retain_declaration_nominal_scope(identity, kind, &scope);
                self.remap_symbol_kind_with_import_aliases(kind, &remapping);
                if let SymbolKind::Type(info) = kind {
                    self.cache_retained_nominal_layout(Some(identity), info);
                }
            }
        }
        if retained_scope {
            self.retain_nominal_scope_carriers(&scope);
        }
        for declaration in &module_ast.declarations {
            let Declaration::TypeAlias(alias) = &declaration.node else {
                continue;
            };
            if public_only && alias.visibility != Visibility::Public {
                continue;
            }
            if let Some(target) = self.type_aliases.get(&alias.name) {
                let mut target = target.clone();
                let bindings = remapping
                    .iter()
                    .filter(|(name, _)| !target.type_params.contains(*name))
                    .map(|(name, qualified)| (name.clone(), qualified.clone()))
                    .collect();
                Self::remap_resolved_type_with_import_aliases(&mut target.target, &bindings);
                direct_type_aliases.insert(alias.name.clone(), target);
            }
        }
        for (name, kind) in &direct_symbols {
            match kind {
                SymbolKind::Type(info) if !matches!(info, TypeInfo::TypeAlias | TypeInfo::Builtin) => {
                    self.transitive_pub_types
                        .entry(name.clone())
                        .or_default()
                        .push(info.clone());
                }
                SymbolKind::Trait(info) => {
                    self.transitive_pub_traits
                        .entry(name.clone())
                        .or_default()
                        .push(info.clone());
                }
                _ => {}
            }
        }
        self.dependency_direct_member_symbols
            .insert(module_name.to_string(), direct_symbols);
        self.dependency_direct_member_type_aliases
            .insert(module_name.to_string(), direct_type_aliases);
        self.dependency_direct_member_identities
            .insert(module_name.to_string(), direct_identities);
        self.dependency_direct_member_partial_projections
            .insert(module_name.to_string(), direct_projections);
    }

    /// Share a module's immutable lexical scope across its declarations, fields and methods, returning whether this
    /// admission retained the supplied scope. A facade cannot replace an original or inherited member's scope.
    fn retain_declaration_nominal_scope(
        &mut self,
        identity: &CanonicalSymbolId,
        kind: &SymbolKind,
        scope: &Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>,
    ) -> bool {
        let newly_retained = !self.declaration_nominal_scopes.contains_key(identity);
        let scope = self
            .declaration_nominal_scopes
            .entry(identity.clone())
            .or_insert_with(|| Arc::clone(scope));
        let scope = Arc::clone(scope);
        if let SymbolKind::Function(function) = kind {
            self.declaration_callables
                .entry(identity.clone())
                .or_insert_with(|| DeclarationCallableTypes {
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                });
        }
        let fields = match kind {
            SymbolKind::Type(TypeInfo::Model(info)) => Some(&info.fields),
            SymbolKind::Type(TypeInfo::Class(info)) => Some(&info.fields),
            _ => None,
        };
        if let Some(fields) = fields {
            for member in fields.values().filter_map(|field| field.identity.as_ref()) {
                if member.origin == identity.origin {
                    self.declaration_nominal_scopes
                        .entry(member.clone())
                        .or_insert_with(|| Arc::clone(&scope));
                }
            }
        }
        let methods = match kind {
            SymbolKind::Type(TypeInfo::Model(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Class(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Newtype(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Enum(info)) => &info.methods,
            SymbolKind::Trait(info) => &info.methods,
            _ => return newly_retained,
        };
        for method in methods.values() {
            let Some(member) = &method.identity else { continue };
            if member.origin == identity.origin {
                self.declaration_nominal_scopes
                    .entry(member.clone())
                    .or_insert_with(|| Arc::clone(&scope));
                self.declaration_callables
                    .entry(member.clone())
                    .or_insert_with(|| DeclarationCallableTypes {
                        params: method.params.clone(),
                        return_type: method.return_type.clone(),
                    });
            }
        }
        newly_retained
    }

    /// Index a newly retained immutable scope once, retaining one spelling per canonical nominal. The scope registry
    /// never removes entries, so no discarded generation or source-scope snapshot is retained by this index.
    fn retain_nominal_scope_carriers(&mut self, scope: &std::collections::BTreeMap<String, CanonicalSymbolId>) {
        for identity in scope.values() {
            if self.retained_nominal_carriers.contains_key(identity) {
                continue;
            }
            if let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(identity) {
                self.retained_nominal_carriers.insert(identity.clone(), key);
            }
        }
    }

    /// Cache exact provider traits referenced by dependency-owned type adoptions.
    ///
    /// A dependency may write `@derive(json)` and therefore store `json.Serialize` on its exported model. The
    /// dependency's local `json` binding must not become ambient in a consumer, but method and bound resolution still
    /// need the trait interface. Cache it under the provider module plus source trait name; lookup remains
    /// semantic-only through [`Self::lookup_trait_adoption_info`].
    fn cache_dependency_adoption_traits(&mut self) {
        let adoptions = self
            .dependency_direct_member_symbols
            .values()
            .flat_map(HashMap::values)
            .filter_map(|kind| match kind {
                SymbolKind::Type(TypeInfo::Model(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Class(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Newtype(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Enum(info)) => Some(info.trait_adoptions.as_slice()),
                _ => None,
            })
            .flatten()
            .filter_map(|adoption| {
                let module_path = adoption.module_path.as_ref()?;
                let source_name = adoption
                    .source_name
                    .as_deref()
                    .or_else(|| adoption.name.rsplit('.').next())?;
                Some((module_path.clone(), source_name.to_string()))
            })
            .collect::<Vec<_>>();

        for (module_path, source_name) in adoptions {
            let key = format!("{}.{}", module_path.join("."), source_name);
            if self.dependency_module_traits.contains_key(&key) {
                continue;
            }
            if let Some((library, rest)) = self.loaf_module(&module_path)
                && let Some((SymbolKind::Trait(info), _)) =
                    self.lookup_pub_library_module_symbol_member(&library, &rest, &source_name)
            {
                self.dependency_module_traits.insert(key, info);
            }
        }
    }

    /// Cache canonical `Registry.define(...)` contracts from one dependency's owned static declarations.
    ///
    /// This accepts only actual `Registry.define(subjects=[...])` initializers. An imported `Registry[K, T]` type
    /// therefore never acquires registry authority by itself, while a consumer can use the defining contract without
    /// creating a second registry definition in its own `TypeCheckInfo`.
    fn cache_dependency_registry_definitions(&mut self, module_name: &str, module_ast: &Program, public_only: bool) {
        let mut definitions = HashMap::new();
        for declaration in &module_ast.declarations {
            let Declaration::Static(static_decl) = &declaration.node else {
                continue;
            };
            if public_only && static_decl.visibility != Visibility::Public {
                continue;
            }
            let static_type = self.resolve_type_checked(&static_decl.ty);
            let Some((key_type, descriptor_type)) = Self::registry_type_arguments(&static_type) else {
                continue;
            };
            let Ok(subjects) = self.registry_definition_subjects(static_decl) else {
                continue;
            };
            definitions.insert(
                static_decl.name.clone(),
                RegistryDefinitionInfo {
                    key_type: key_type.clone(),
                    descriptor_type: descriptor_type.clone(),
                    subjects,
                    is_public: static_decl.visibility == Visibility::Public,
                },
            );
        }
        self.dependency_registry_definitions.insert(
            module_name.to_string(),
            DependencyRegistryDefinitions {
                owner_module_path: Self::dependency_registry_owner_module_path(module_ast, module_name),
                definitions,
            },
        );
    }

    /// Recover the source-module identity for a dependency registry without inferring it from its lookup key.
    ///
    /// The parser records the module file path for normal compilation. Its path below `src/` is the source authority
    /// for metadata identity. Inline unit tests do not carry a file path, so their explicitly supplied dependency
    /// name remains the narrow test-only fallback.
    fn dependency_registry_owner_module_path(module_ast: &Program, module_name: &str) -> Vec<String> {
        let Some(source_path) = module_ast.source_path.as_deref() else {
            return vec![module_name.to_string()];
        };
        let path = std::path::Path::new(source_path);
        let mut after_src = false;
        let mut segments = Vec::new();
        for component in path.components() {
            let std::path::Component::Normal(component) = component else {
                continue;
            };
            let Some(component) = component.to_str() else {
                return vec![module_name.to_string()];
            };
            if component == "src" {
                after_src = true;
                segments.clear();
            } else if after_src {
                segments.push(component.to_string());
            }
        }
        let Some(last) = segments.last_mut() else {
            return vec![module_name.to_string()];
        };
        let Some(stem) = std::path::Path::new(last).file_stem().and_then(|stem| stem.to_str()) else {
            return vec![module_name.to_string()];
        };
        *last = stem.to_string();
        if matches!(last.as_str(), "mod" | "__init__") {
            segments.pop();
        }
        if segments.is_empty() {
            vec![module_name.to_string()]
        } else {
            canonicalize_source_module_segments(&segments)
        }
    }

    /// Return direct declaration names owned by a dependency module, excluding import re-exports.
    fn dependency_direct_member_names(module_ast: &Program, public_only: bool) -> HashSet<String> {
        if public_only {
            let mut names = exported_symbols(module_ast)
                .into_iter()
                .filter_map(|symbol| match symbol {
                    ExportedSymbol::Const(name)
                    | ExportedSymbol::Static(name)
                    | ExportedSymbol::Type(name)
                    | ExportedSymbol::Trait(name)
                    | ExportedSymbol::Function(name)
                    | ExportedSymbol::Capability(name) => Some(name),
                    ExportedSymbol::Variant { variant_name, .. } => Some(variant_name),
                    ExportedSymbol::Reexported(_) => None,
                })
                .collect::<HashSet<_>>();
            names.extend(module_ast.declarations.iter().filter_map(|decl| {
                let Declaration::Alias(alias) = &decl.node else {
                    return None;
                };
                matches!(alias.visibility, Visibility::Public).then(|| alias.name.clone())
            }));
            return names;
        }

        module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Const(decl) => Some(decl.name.clone()),
                Declaration::Static(decl) => Some(decl.name.clone()),
                Declaration::Model(decl) => Some(decl.name.clone()),
                Declaration::Class(decl) => Some(decl.name.clone()),
                Declaration::Trait(decl) => Some(decl.name.clone()),
                Declaration::Alias(decl) => Some(decl.name.clone()),
                Declaration::Partial(decl) => Some(decl.name.clone()),
                Declaration::TypeAlias(decl) => Some(decl.name.clone()),
                Declaration::Newtype(decl) => Some(decl.name.clone()),
                Declaration::Enum(decl) => Some(decl.name.clone()),
                Declaration::Function(decl) => Some(decl.name.clone()),
                Declaration::Capability(decl) => Some(decl.name.clone()),
                Declaration::Import(_)
                | Declaration::Docstring(_)
                | Declaration::TestModule(_)
                | Declaration::VocabBlock(_) => None,
            })
            .collect()
    }

    /// Return source-module import targets that a dependency module makes visible as member bindings.
    fn dependency_source_reexport_targets(module_ast: &Program) -> Vec<(String, ImportPath, String)> {
        module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Import(import) => Some(import),
                _ => None,
            })
            .filter_map(|import| match &import.kind {
                ImportKind::From { module, items } => Some((module, items.as_slice())),
                _ => None,
            })
            .flat_map(|(module, items)| {
                items.iter().map(move |item| {
                    (
                        item.alias.as_ref().unwrap_or(&item.name).clone(),
                        module.clone(),
                        item.name.clone(),
                    )
                })
            })
            .collect()
    }

    /// Select one dependency cache entry using source-resolution order and an unambiguous nested-entry fallback.
    ///
    /// During SDK provider construction, a granted public `std.*` path resolves to the provider's physical source
    /// path before ordinary consumer candidates. This also applies while following facade re-exports: a facade such
    /// as physical `fs` may itself import `std.fs.file`, and that binding must retain the one `fs.file` declaration
    /// identity emitted by the same provider.
    fn dependency_source_import_candidates(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
    ) -> Vec<Vec<String>> {
        let mut candidates = Vec::new();
        if module.parent_levels == 0 && self.owns_standard_module(&module.segments) {
            candidates.push(canonicalize_source_module_segments(&module.segments[1..]));
        }
        for candidate in logical_source_import_candidates(base_module_path, module) {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        candidates
    }

    /// Select one dependency cache entry using source-resolution order and an unambiguous nested-entry fallback.
    fn dependency_module_entry_for_path<'a, T>(
        &self,
        module: &ImportPath,
        entries: &'a HashMap<String, T>,
    ) -> Option<&'a T> {
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        self.dependency_module_entry_for_path_from(current_module_path, module, entries)
    }

    /// Select one dependency cache entry, resolving the import path against an explicit base module.
    ///
    /// A module's relative imports resolve against *its own* path. That is the consumer's path for an ordinary import,
    /// but a facade's own path when caching what that facade re-exports: `pkg.facade` writing `from helpers import
    /// render` means `pkg.helpers`, and resolving it from the consumer would silently bind the root `helpers` instead.
    fn dependency_module_entry_for_path_from<'a, T>(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        entries: &'a HashMap<String, T>,
    ) -> Option<&'a T> {
        for candidate in self.dependency_source_import_candidates(base_module_path, module) {
            if let Some(entry) = entries.get(&candidate.join("_")) {
                return Some(entry);
            }
        }
        if module.is_absolute || module.parent_levels > 0 {
            return None;
        }

        // A nested CLI entrypoint is represented as logical `main`, so its sibling prefix is unavailable here even
        // though source discovery already admitted the exact dependency. Bind a bare suffix only when one cached
        // canonical module owns it; ambiguous same-leaf modules deliberately remain unresolved.
        let suffix = canonicalize_source_module_segments(&module.segments).join("_");
        if suffix.is_empty() {
            return None;
        }
        let suffix = format!("_{suffix}");
        let mut matches = entries
            .iter()
            .filter_map(|(key, entry)| key.ends_with(&suffix).then_some(entry));
        let entry = matches.next()?;
        matches.next().is_none().then_some(entry)
    }

    /// Return the exact symbol kind for one dependency module member path.
    pub fn dependency_member_symbol_for_path(&self, module: &ImportPath, item_name: &str) -> Option<SymbolKind> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_symbols)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency member's symbol kind, resolving the import path against an explicit base module.
    pub fn dependency_member_symbol_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<SymbolKind> {
        self.dependency_module_entry_for_path_from(base_module_path, module, &self.dependency_member_symbols)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency type alias target using ordinary consumer import resolution.
    pub fn dependency_member_type_alias_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<TypeAliasTarget> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_type_aliases)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency type alias target relative to a dependency module that is being collected or refreshed.
    fn dependency_member_type_alias_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<TypeAliasTarget> {
        self.dependency_module_entry_for_path_from(base_module_path, module, &self.dependency_member_type_aliases)?
            .get(item_name)
            .cloned()
    }

    /// Maximum re-export hops followed while resolving one imported member to its declaring module.
    ///
    /// A malformed or cyclic facade chain must terminate rather than recurse forever; exceeding this yields no
    /// identity, which is the same fail-closed answer as any other unproven owner.
    const MAX_REEXPORT_DEPTH: usize = 16;

    /// Register a dependency's real module path segments alongside the flattened name it is cached under.
    ///
    /// Caller-supplied metadata, not a derived cache: it deliberately survives `check_with_imports`, which clears the
    /// dependency caches it rebuilds. Registering before checking is therefore the expected order, and re-registering
    /// one name overwrites it.
    ///
    /// Optional: an unregistered dependency still resolves, and its identity falls back to the matching candidate's
    /// path. Registering is what makes the origin name the module that answered rather than the spelling that asked,
    /// which matters wherever the flattened name is ambiguous. Mirrors
    /// `IrCodegen::add_module_with_path_segments`, which carries the same pair for emission.
    pub fn register_dependency_module_path_segments(&mut self, module_name: &str, path_segments: Vec<String>) {
        if path_segments.is_empty() {
            return;
        }
        self.dependency_module_path_segments
            .insert(module_name.to_string(), path_segments);
    }

    /// Return the module path segments that canonically identify a cached dependency, if they are known.
    ///
    /// A declaration identity must never be built from a *projection* spelling. The cache key is the flattened,
    /// underscore-joined name that also names the emitted Rust module, and it is not injective: the path
    /// `pkg.helpers` and a module literally named `pkg_helpers` are one key. Reading segments back out of it would
    /// invent an origin for a module that may not exist, and would give one module two identities depending on which
    /// spelling reached it.
    ///
    /// The segments therefore come from an explicit registration, and otherwise only from the one case the flattened
    /// name determines by itself: a name containing no `_` can only be a single segment. Anything else is genuinely
    /// ambiguous and yields no identity rather than a guess.
    fn canonical_owner_segments(&self, cache_key: &str) -> Option<Vec<String>> {
        if let Some(segments) = self.dependency_module_path_segments.get(cache_key) {
            return Some(segments.clone());
        }
        (!cache_key.is_empty() && !cache_key.contains('_')).then(|| vec![cache_key.to_string()])
    }

    /// Return the canonical identity of an imported member, following re-exports to the module that declares it.
    ///
    /// Import resolution is not literal: `logical_source_path_candidates` tries the sibling-relative path before the
    /// bare one, so the path written at an import is not necessarily the module it bound. An identity built from the
    /// written path can therefore name a different declaration that merely shares a leaf name. This follows the same
    /// candidate order resolution used, and where the winning candidate only re-exports the member it follows that
    /// link instead of naming the facade — a re-export is a binding to an existing declaration, not a second one.
    ///
    /// Returns `None` when the member is reachable only through an ambiguous fallback, when a re-export chain does not
    /// terminate in a declaration, or when the direct dependency cache lacks the compiler-minted canonical identity.
    /// A symbol kind, name, module key, and span are insufficient proof: absent canonical data must stay absent rather
    /// than being reconstructed into a plausible but invented declaration.
    pub fn dependency_member_identity(&self, module: &ImportPath, item_name: &str) -> Option<CanonicalSymbolId> {
        self.dependency_member_resolution(module, item_name)
            .map(|member| member.identity)
    }

    /// Resolve an imported member to its declaring identity and to the name its declaring module binds it under.
    ///
    /// The identity half is [`Self::dependency_member_identity`]. The name half is what a facade chain loses: a
    /// re-export may publish a declaration under a new name (`pub from provider import calculate as
    /// facade_calculate`), and a consumer of `facade_calculate` binds exactly the projection a direct import of
    /// `calculate` binds. The written item name therefore says nothing about how the declaring module spells the
    /// declaration, and lowering must not read a re-export rename as an `alias` declaration, which is a second name
    /// the declaring module itself binds to one identity. The walk knows the answer at its last hop: the item name it
    /// stopped at is the declaring module's own spelling, `calculate` for a re-export rename and `scale_alias` for
    /// `pub scale_alias = alias scale` (#1710).
    pub fn dependency_member_resolution(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<ResolvedDependencyMember> {
        // Resolve through the consumer's own module graph first. `dependency_source_import_candidates` already
        // orders a granted SDK provider ahead of ordinary candidates for a `std.*` path, so provider precedence is
        // preserved without keying on the spelling here.
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        if let Some(member) = self.dependency_member_resolution_from(current_module_path, module, item_name, 0) {
            return Some(member);
        }

        // Fall back to the provider registry keyed by the spelled path. A compiled provider is reachable by the path
        // its manifest publishes even when the consumer's source graph contains no module of that name, which is the
        // case this originally existed to serve.
        //
        // It must stay a fallback. Running it first let the spelling pre-empt resolution: `from helpers import
        // render` inside `pkg.app` selects the sibling `pkg.helpers`, but a root `helpers` declaring the same member
        // answered instead -- a module the import did not select.
        self.loaf_import_identity(module, item_name)
            .map(|identity| ResolvedDependencyMember::declared_as(identity, item_name))
    }

    /// Resolve one imported member to its declaring identity, using `from_module_path` as the resolution base.
    ///
    /// The base is the *consumer's* module for a direct import and the *facade's* module when following a re-export,
    /// because a facade's own relative import paths resolve against where the facade lives, not where the consumer
    /// does. Every arm that stops the walk answers by a direct lookup of `item_name` in the module it stopped at, so
    /// that name is the declaring module's spelling the result carries.
    fn dependency_member_resolution_from(
        &self,
        from_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
        depth: usize,
    ) -> Option<ResolvedDependencyMember> {
        if depth >= Self::MAX_REEXPORT_DEPTH {
            return None;
        }
        for owner in self.dependency_source_import_candidates(from_module_path, module) {
            let key = owner.join("_");
            let resolves_here = self
                .dependency_member_symbols
                .get(&key)
                .is_some_and(|members| members.contains_key(item_name));
            if !resolves_here {
                continue;
            }

            // Resolution stops at this candidate. It owns the declaration only if it declares the member directly.
            if self
                .dependency_direct_member_symbols
                .get(&key)
                .and_then(|direct| direct.get(item_name))
                .is_some()
            {
                return self
                    .dependency_direct_member_identities
                    .get(&key)
                    .and_then(|identities| identities.get(item_name))
                    .cloned()
                    .map(|identity| ResolvedDependencyMember::declared_as(identity, item_name));
            }

            // Otherwise the member arrived here as a re-export. Follow it to the declaration it names, resolving
            // against the facade's own module — the same base `cache_dependency_member_symbols` used when it bound
            // that link, so the identity names exactly the declaration the binding selected. A link into `std.*`
            // reaches the same provider registry and source metadata the binding itself came from, in the same order.
            let (target_module, target_name) = self.dependency_member_reexports.get(&key)?.get(item_name)?;
            return self
                .dependency_member_resolution_from(&owner, target_module, target_name, depth + 1)
                .or_else(|| {
                    self.loaf_import_identity(target_module, target_name)
                        .map(|identity| ResolvedDependencyMember::declared_as(identity, target_name))
                });
        }
        None
    }

    /// Return the identity the Loaf owning `module` publishes for `item_name`: the identity half of
    /// [`Self::dependency_reexported_member_symbol`], followed hop for hop, so the identity a facade records is the
    /// identity of the symbol it bound.
    fn loaf_import_identity(&self, module: &ImportPath, item_name: &str) -> Option<CanonicalSymbolId> {
        if module.parent_levels != 0 || module.is_absolute {
            return None;
        }
        self.loaf_member_identity(&canonicalize_source_module_segments(&module.segments), item_name)
    }

    /// Return the canonical identity the Loaf owning `module_path` publishes for `item_name`.
    pub(in crate::typechecker) fn loaf_member_identity(
        &self,
        module_path: &[String],
        item_name: &str,
    ) -> Option<CanonicalSymbolId> {
        let (library, rest) = self.loaf_module(module_path)?;
        let manifest = self.loaves.manifest(&library)?;
        let public_path = std::iter::once(manifest.name.clone())
            .chain(rest)
            .chain(std::iter::once(item_name.to_string()))
            .collect::<Vec<_>>();
        manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_path(&public_path)
    }

    /// Return one imported registry's checked defining contract and canonical owner path.
    ///
    /// Source import resolution uses the same candidate order as member-symbol resolution. The returned owner path is
    /// the resolved dependency path rather than the consumer's local alias, preserving one registry identity through
    /// typechecking, lowering, and package metadata.
    pub fn dependency_registry_definition_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<(RegistryDefinitionInfo, Vec<String>)> {
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        for candidate in self.dependency_source_import_candidates(current_module_path, module) {
            let key = canonicalize_source_module_segments(&candidate).join("_");
            if let Some(registries) = self.dependency_registry_definitions.get(&key)
                && let Some(definition) = registries.definitions.get(item_name)
            {
                return Some((definition.clone(), registries.owner_module_path.clone()));
            }
        }
        if module.is_absolute || module.parent_levels > 0 {
            return None;
        }

        let suffix = canonicalize_source_module_segments(&module.segments).join("_");
        if suffix.is_empty() {
            return None;
        }
        let suffix = format!("_{suffix}");
        let mut matches = self
            .dependency_registry_definitions
            .iter()
            .filter_map(|(module_key, definitions)| {
                module_key
                    .ends_with(&suffix)
                    .then(|| {
                        definitions
                            .definitions
                            .get(item_name)
                            .map(|definition| (module_key, definitions, definition))
                    })
                    .flatten()
            });
        let (_module_key, registries, definition) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some((definition.clone(), registries.owner_module_path.clone()))
    }

    /// Return the exact partial projection metadata for one dependency module member path.
    pub fn dependency_member_partial_projection_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_partial_projections)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency member's partial projection, resolving against an explicit base module.
    pub fn dependency_member_partial_projection_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_module_entry_for_path_from(
            base_module_path,
            module,
            &self.dependency_member_partial_projections,
        )?
        .get(item_name)
        .cloned()
    }

    /// Register RFC 024 metadata exported by a dependency source module.
    ///
    /// `__derives__` is compiler metadata rather than a public const, so consumers need this side channel even when the
    /// declaration itself is not imported as a symbol.
    fn register_dependency_derivable_metadata(&mut self, module_name: &str, module_ast: &Program) {
        let mut module_paths = vec![module_name.to_string()];
        if let Some(segments) = self.canonical_owner_segments(module_name) {
            let dotted = segments.join(".");
            if !module_paths.contains(&dotted) {
                module_paths.push(dotted);
            }
            let public = std::iter::once("std".to_owned()).chain(segments).collect::<Vec<_>>();
            if self.owns_standard_module(&public) {
                module_paths.push(public.join("."));
            }
        }
        let traits = Self::derivable_traits_from_program(module_ast);
        if !traits.is_empty() {
            for module_path in &module_paths {
                self.dependency_derivable_modules
                    .insert(module_path.clone(), traits.clone());
            }
        }

        for decl in &module_ast.declarations {
            let Declaration::Trait(tr) = &decl.node else {
                continue;
            };
            if let Some(info) = self.lookup_trait_info(&tr.name).cloned() {
                for module_path in &module_paths {
                    self.dependency_module_traits
                        .insert(format!("{module_path}.{}", tr.name), info.clone());
                }
            }
            let mut paths = Self::rust_derive_paths_from_trait(tr);
            if paths.is_empty()
                && let Some(module) = &module_ast.rust_module_path
                && let Some(path) = self.checked_native_derive_macro(&format!("{}::{}", module.node, tr.name))
            {
                paths.push(path);
            }
            if paths.is_empty() {
                continue;
            }
            for module_path in &module_paths {
                let qualified = format!("{module_path}.{}", tr.name);
                self.dependency_trait_rust_derive_paths.insert(qualified, paths.clone());
            }
        }
    }

    /// Extract RFC 024 `__derives__` trait names from an imported dependency AST.
    pub fn derivable_traits_from_program(program: &Program) -> Vec<String> {
        for decl in &program.declarations {
            let Declaration::Const(konst) = &decl.node else {
                continue;
            };
            if konst.name != "__derives__" {
                continue;
            }
            let Expr::List(entries) = &konst.value.node else {
                return Vec::new();
            };
            return entries
                .iter()
                .filter_map(|entry| match entry {
                    ListEntry::Element(expr) => match &expr.node {
                        Expr::Ident(name) => Some(name.clone()),
                        _ => None,
                    },
                    ListEntry::Spread(_) => None,
                })
                .collect();
        }
        Vec::new()
    }

    /// Extract `@rust.derive(...)` path strings from one imported trait declaration.
    fn rust_derive_paths_from_trait(tr: &TraitDecl) -> Vec<String> {
        let mut paths = Vec::new();
        for decorator in &tr.decorators {
            let path = decorator.node.path.segments.join(".");
            if core_decorators::from_str(&path) != Some(DecoratorId::RustDerive) {
                continue;
            }
            for arg in &decorator.node.args {
                let DecoratorArg::Positional(expr) = arg else {
                    continue;
                };
                let Expr::Literal(Literal::String(path)) = &expr.node else {
                    continue;
                };
                if !paths.iter().any(|existing| existing == path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    /// Predeclare exact per-module dependency interface members before collecting imported declarations.
    ///
    /// Cross-module public signatures may reference types and traits from modules collected later, including cyclic
    /// interfaces such as `session -> dataset -> session`. Shells live in the module-keyed import-resolution cache,
    /// never in the consumer's lexical symbol table, so sibling modules may export the same spelling safely.
    fn predeclare_dependency_interfaces(&mut self, dependencies: &[(&str, &Program)], public_only: bool) {
        for (module_name, dep_ast) in dependencies {
            let members = self
                .dependency_member_symbols
                .entry((*module_name).to_string())
                .or_default();
            for decl in &dep_ast.declarations {
                if public_only && !is_public_decl(decl) {
                    continue;
                }
                if let Some(symbol) = Self::dependency_interface_shell(decl) {
                    members.entry(symbol.name).or_insert(symbol.kind);
                }
            }
        }
    }

    /// Seed one dependency's temporary interface lookup view from its own declaration shells.
    fn seed_dependency_interface_bindings(&mut self, module_ast: &Program, public_only: bool) {
        for decl in &module_ast.declarations {
            if public_only && !is_public_decl(decl) {
                continue;
            }
            if let Some(symbol) = Self::dependency_interface_shell(decl) {
                self.symbols.define_import_binding(symbol, None);
            }
        }
    }

    /// Build a minimal placeholder for one type-like dependency declaration.
    ///
    /// The subsequent `import_module*` pass replaces the temporary lookup shell with full collected metadata. The
    /// module-keyed copy remains available to imports from modules that have not yet been collected.
    fn dependency_interface_shell(decl: &Spanned<Declaration>) -> Option<Symbol> {
        match &decl.node {
            Declaration::Model(model) => Some(Symbol {
                name: model.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                    type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: HashMap::new(),
                    field_order: Vec::new(),
                    properties: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                    method_aliases: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Class(class) => Some(Symbol {
                name: class.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                    type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    extends: class.extends.clone(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: HashMap::new(),
                    field_defaults: Box::new(HashMap::new()),
                    field_default_metadata: Box::new(HashMap::new()),
                    field_provider_libraries: Box::new(HashMap::new()),
                    field_order: Vec::new(),
                    properties: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                    method_aliases: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Trait(tr) => Some(Symbol {
                name: tr.name.clone(),
                kind: SymbolKind::Trait(TraitInfo {
                    type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    methods: HashMap::new(),
                    method_aliases: HashMap::new(),
                    properties: HashMap::new(),
                    requires: Vec::new(),
                    supertraits: Vec::new(),
                }),
                span: decl.span,
                scope: 0,
            }),
            Declaration::TypeAlias(alias) => Some(Symbol {
                name: alias.name.clone(),
                kind: SymbolKind::Type(TypeInfo::TypeAlias),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Newtype(nt) => Some(Symbol {
                name: nt.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                    type_params: nt.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    is_rusttype: nt.is_rusttype,
                    has_interop: !nt.interop_edges.is_empty(),
                    underlying: ResolvedType::Unknown,
                    constraints: Vec::new(),
                    implicit_coercion_enabled: true,
                    method_rebindings: HashMap::new(),
                    traits: nt.traits.iter().map(|trait_ref| trait_ref.node.name.clone()).collect(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    method_aliases: HashMap::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            Declaration::Enum(en) => Some(Symbol {
                name: en.name.clone(),
                kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                    type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                    traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                    trait_adoptions: Vec::new(),
                    variants: en.variants.iter().map(|v| v.node.name.clone()).collect(),
                    variant_identities: HashMap::new(),
                    variant_fields: HashMap::new(),
                    variant_aliases: en
                        .variant_aliases
                        .iter()
                        .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
                        .collect(),
                    value_enum: None,
                    derives: Vec::new(),
                    methods: HashMap::new(),
                    method_overloads: HashMap::new(),
                })),
                span: decl.span,
                scope: 0,
            }),
            _ => None,
        }
    }

    /// Check a program that may have dependencies on other modules.
    ///
    /// Convenience wrapper that calls [`import_module`](Self::import_module) for each dependency, then
    /// [`check_program`](Self::check_program).
    ///
    /// ## Parameters
    ///
    /// - `program`: parsed AST of the main module.
    /// - `dependencies`: list of `(module_name, ast)` pairs to import first.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if type checking succeeds.
    /// - `Err(Vec<CompileError>)` containing all accumulated errors.
    #[tracing::instrument(skip_all, fields(decl_count = program.declarations.len(), dep_count = dependencies.len()))]
    pub fn check_with_imports(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        self.dependency_semantics_pending = false;
        self.hash_key_inference.type_params.clear();
        self.transitive_pub_types.clear();
        self.public_library_type_identities.clear();
        self.transitive_pub_traits.clear();
        self.cached_pub_libraries.clear();
        self.foreign_pub_type_remappings.clear();
        self.pub_library_declaration_scopes.clear();
        self.dependency_exports.clear();
        self.dependency_member_symbols.clear();
        self.dependency_member_type_aliases.clear();
        self.dependency_registry_definitions.clear();
        self.dependency_member_partial_projections.clear();
        self.dependency_direct_member_symbols.clear();
        self.dependency_direct_member_type_aliases.clear();
        self.dependency_direct_member_identities.clear();
        self.dependency_member_reexports.clear();
        self.body_ir_dependency_pub_reexports.clear();
        self.dependency_direct_member_partial_projections.clear();
        self.dependency_derivable_modules.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.record_nominal_declaring_modules(program, dependencies);
        for (name, dep_ast) in dependencies {
            self.dependency_exports
                .insert(name.to_string(), exported_symbols(dep_ast));
        }
        self.predeclare_dependency_interfaces(dependencies, true);
        // First: import all dependencies
        for (name, dep_ast) in dependencies {
            self.import_module(dep_ast, name);
        }
        self.refresh_dependency_member_symbols(dependencies, true);

        // Then check the main program
        self.check_program_with_dependency_semantics(program)
    }

    /// Check a program with dependencies, but allow importing private items.
    ///
    /// This is intended for internal compiler stages (like IR codegen) where we need type information across modules
    /// without enforcing visibility restrictions.
    pub fn check_with_imports_allow_private(
        &mut self,
        program: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<(), Vec<CompileError>> {
        self.dependency_semantics_pending = false;
        self.hash_key_inference.type_params.clear();
        self.transitive_pub_types.clear();
        self.public_library_type_identities.clear();
        self.transitive_pub_traits.clear();
        self.cached_pub_libraries.clear();
        self.foreign_pub_type_remappings.clear();
        self.pub_library_declaration_scopes.clear();
        // Skip populating dependency exports so visibility checks are bypassed.
        self.dependency_exports.clear();
        self.dependency_member_symbols.clear();
        self.dependency_member_type_aliases.clear();
        self.dependency_registry_definitions.clear();
        self.dependency_member_partial_projections.clear();
        self.dependency_direct_member_symbols.clear();
        self.dependency_direct_member_type_aliases.clear();
        self.dependency_direct_member_identities.clear();
        self.dependency_member_reexports.clear();
        self.body_ir_dependency_pub_reexports.clear();
        self.dependency_direct_member_partial_projections.clear();
        self.dependency_derivable_modules.clear();
        self.dependency_module_traits.clear();
        self.dependency_trait_rust_derive_paths.clear();
        self.record_nominal_declaring_modules(program, dependencies);
        self.predeclare_dependency_interfaces(dependencies, false);
        for (name, dep_ast) in dependencies {
            self.import_module_all(dep_ast, name);
        }
        self.refresh_dependency_member_symbols(dependencies, false);
        self.check_program_with_dependency_semantics(program)
    }

    // ========================================================================
    // Type compatibility (shared helper)
    // ========================================================================

    /// Record an RFC 017 validated-newtype coercion when `actual` may flow into `expected` at an approved site.
    ///
    /// This deliberately stays outside [`Self::types_compatible`]: ordinary compatibility queries have no source span
    /// and are used in contexts where implicit coercion must not be inserted.
    pub fn record_validated_newtype_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> bool {
        let Some(coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Record an RFC 017 model/class field coercion that should aggregate validation errors.
    pub fn record_validated_newtype_field_coercion_if_possible(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        field_name: &str,
        span: Span,
    ) -> bool {
        let Some(mut coercion) = self.validated_newtype_coercion(actual, expected, span) else {
            return false;
        };
        coercion.mode = ValidatedNewtypeCoercionMode::AggregateField {
            field_name: field_name.to_string(),
        };
        self.type_info.record_validated_newtype_coercion(span, coercion);
        true
    }

    /// Return the coercion metadata needed to convert `actual` into expected newtype `expected`.
    fn validated_newtype_coercion(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<ValidatedNewtypeCoercionInfo> {
        if actual == expected {
            return None;
        }
        let steps = self.validated_newtype_coercion_steps(actual, expected, &mut HashSet::new(), span)?;
        Some(ValidatedNewtypeCoercionInfo {
            steps,
            target_type: expected.clone(),
            mode: ValidatedNewtypeCoercionMode::FailFast,
        })
    }

    /// Build the ordered underlying-to-target chain for one validated-newtype coercion.
    fn validated_newtype_coercion_steps(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        visiting: &mut HashSet<String>,
        span: Span,
    ) -> Option<Vec<ValidatedNewtypeCoercionStep>> {
        let target_name = match expected {
            ResolvedType::Named(name) => name,
            // Generic newtypes need type-parameter substitution for their underlying type. Keep that out of the
            // initial implicit path instead of silently guessing.
            ResolvedType::Generic(_, _) => return None,
            _ => return None,
        };
        if !visiting.insert(target_name.clone()) {
            self.errors.push(errors::newtype_coercion_cycle(target_name, span));
            return None;
        }

        let Some(TypeInfo::Newtype(newtype)) = self.lookup_semantic_type_info(target_name).cloned() else {
            visiting.remove(target_name);
            return None;
        };
        if newtype.is_rusttype {
            visiting.remove(target_name);
            return None;
        }

        let mut steps = if Self::validated_newtype_source_matches_underlying(actual, &newtype.underlying) {
            Vec::new()
        } else {
            self.validated_newtype_coercion_steps(actual, &newtype.underlying, visiting, span)?
        };
        if !newtype.implicit_coercion_enabled {
            self.errors
                .push(errors::implicit_newtype_coercion_disabled(target_name, span));
            visiting.remove(target_name);
            return Some(Vec::new());
        }
        let ctor = self.validated_newtype_ctor_name(target_name, &newtype);
        let ctor_identity = ctor
            .as_deref()
            .and_then(|name| newtype.methods.get(name))
            .and_then(|method| method.identity.clone());
        steps.push(ValidatedNewtypeCoercionStep {
            newtype_name: target_name.clone(),
            owner_identity: self.checked_nominal_type_identities().get(target_name).cloned(),
            ctor,
            ctor_identity,
            constraints: if self.validated_newtype_ctor_name(target_name, &newtype).is_some() {
                Vec::new()
            } else {
                newtype.constraints.clone()
            },
        });
        visiting.remove(target_name);
        Some(steps)
    }

    /// Return whether a source value can be passed to a newtype's underlying constructor without primitive parsing.
    fn validated_newtype_source_matches_underlying(actual: &ResolvedType, underlying: &ResolvedType) -> bool {
        matches!(actual, ResolvedType::Unknown | ResolvedType::TypeVar(_)) || actual == underlying
    }

    /// Retain a selected hook in the consumer's owner binding, including when an import renamed that owner.
    /// The caller must first validate the hook through `validated_newtype_ctor_name`; only its proven Ok owner is
    /// rebound, preserving the original parameters, error type, and canonical method identity.
    fn newtype_constructor_binding(method: &MethodInfo, owner: ResolvedType) -> type_info::FunctionBindingInfo {
        let mut return_type = method.return_type.clone();
        if let ResolvedType::Generic(_, arguments) = &mut return_type
            && let Some(success) = arguments.first_mut()
        {
            *success = owner;
        }
        type_info::FunctionBindingInfo {
            is_async: method.is_async,
            params: method.params.clone(),
            return_type,
            identity: method.identity.clone(),
        }
    }

    /// Return the checked-construction hook only when its collected signature returns this newtype's exact owner.
    /// Source aliases and retained carriers compare through their admitted declaration identities.
    pub fn validated_newtype_ctor_name(&self, newtype_name: &str, newtype: &NewtypeInfo) -> Option<String> {
        let method = newtype.methods.get(conventions::NEWTYPE_FROM_UNDERLYING_METHOD)?;
        if method.receiver.is_some() || method.params.len() != 1 {
            return None;
        }
        if !Self::validated_newtype_source_matches_underlying(&method.params[0].ty, &newtype.underlying) {
            return None;
        }
        let ResolvedType::Generic(result_name, result_args) = &method.return_type else {
            return None;
        };
        if collection_type_id(result_name.as_str()) != Some(CollectionTypeId::Result) {
            return None;
        }
        let ok_ty = result_args.first()?;
        if !(matches!(ok_ty, ResolvedType::Named(name) if name == newtype_name)
            || matches!(ok_ty, ResolvedType::SelfType)
            || self.retained_nominal_types_compatible(ok_ty, &ResolvedType::Named(newtype_name.to_owned()))
                == Some(true))
        {
            return None;
        }
        let err_ty = result_args.get(1)?;
        if !matches!(err_ty, ResolvedType::Named(name) if name == "ValidationError") {
            return None;
        }
        Some(conventions::NEWTYPE_FROM_UNDERLYING_METHOD.to_string())
    }

    /// Return the type parameter `ty` names when it is a type parameter of an enclosing generic declaration, seen from
    /// inside that declaration's body, where it is one fixed type whatever a caller later picks for it (#1561).
    ///
    /// Inside the body such a parameter is a named placeholder in scope; a callee's own type parameters, still to be
    /// inferred at a call, are type variables and are not rigid.
    fn rigid_type_param_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        let ResolvedType::Named(_) = ty else {
            return None;
        };
        self.generic_placeholder_name(ty)
            .and_then(|_| self.active_type_param_name(ty))
    }

    /// Whether a value of type `actual` is a value of the enclosing declaration's type parameter `type_param` (#1561).
    ///
    /// A value of a concrete type is not one: `return 0` from `def f[T](x: T) -> T` is refused, as the generated Rust
    /// refuses it, because a caller may pick any type for `T`. Only a value of that same parameter is, together with a
    /// borrowed one (`&T`, which the destination copies), a value whose type is still to be inferred (an unresolved
    /// callee type parameter) and a union whose members all are. An unknown value, left to the error that made it
    /// unknown, and a never-returning one are accepted.
    fn value_is_rigid_type_param(&self, actual: &ResolvedType, type_param: &str) -> bool {
        match actual {
            ResolvedType::Named(name) => name == type_param,
            ResolvedType::Unknown | ResolvedType::Never | ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer => {
                true
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.value_is_rigid_type_param(inner, type_param),
            ResolvedType::Generic(name, members) if name == UNION_TYPE_NAME => members
                .iter()
                .all(|member| self.value_is_rigid_type_param(member, type_param)),
            _ => false,
        }
    }

    /// Whether a value of the enclosing declaration's type parameter `type_param` (of type `actual`, the parameter
    /// itself) can be used where `expected` is required (#1561).
    ///
    /// Inside its declaration a type parameter is one type the declaration does not choose, so a value of it is not a
    /// value of a concrete type: `return x` from `def f[T](x: T) -> int` is refused, as the generated Rust refuses it.
    /// A bound makes it a value of a trait the bound implies (`T with Shape` where a `Shape` is expected). A
    /// `CallableN` bound does not make it a function value, since an adopting model meets the bound too; a bound on
    /// a trait the checker cannot see (a Rust `Fn` marker) may, and is left to the Rust compiler. A union takes the
    /// value when one member does, an `Option` when its payload does, and a borrow when the borrowed type does. A
    /// destination still to be inferred (a callee's type parameter, a placeholder of no enclosing declaration),
    /// `Self`, a Rust type and a name the checker does not know are left to the checks that own them.
    fn rigid_type_param_value_fits(&self, type_param: &str, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match expected {
            ResolvedType::Named(name) if name == type_param => true,
            ResolvedType::Unknown
            | ResolvedType::Never
            | ResolvedType::TypeVar(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_) => true,
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.types_compatible(actual, inner),
            ResolvedType::Generic(name, members) if name == UNION_TYPE_NAME => {
                members.iter().any(|member| self.types_compatible(actual, member))
            }
            _ if expected.is_option() => expected
                .option_inner_type()
                .is_some_and(|inner| self.types_compatible(actual, inner)),
            ResolvedType::Function(..) => !self.active_type_param_bounds_are_visible(type_param),
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                if self.is_generic_placeholder_type(expected) {
                    return true;
                }
                if self.lookup_semantic_trait_info(name).is_some() {
                    return self.active_type_param_implies_trait_type(type_param, expected);
                }
                self.lookup_semantic_type_info(name).is_none() && collection_type_id(name.as_str()).is_none()
            }
            _ => false,
        }
    }

    /// Whether the bounds of the enclosing declaration's type parameter `type_param` imply the trait `trait_ty` names
    /// (`Shape`, or `Iterable[int]` with its type arguments), so a value of the parameter is a value of that trait.
    fn active_type_param_implies_trait_type(&self, type_param: &str, trait_ty: &ResolvedType) -> bool {
        let (name, type_args) = match trait_ty {
            ResolvedType::Named(name) => (name, Vec::new()),
            ResolvedType::Generic(name, args) => (name, args.clone()),
            _ => return false,
        };
        let required = TypeBoundInfo {
            name: name.clone(),
            source_name: None,
            type_args,
            module_path: None,
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.active_type_param_satisfies_bound_info(type_param, &required, &HashMap::new())
    }

    /// Return whether `actual` can be used where `expected` is required, with a recursion cap for pathological unions.
    pub fn types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        const MAX_TYPE_COMPATIBILITY_DEPTH: usize = 512;
        if self.native_queries.type_compatibility_depth.get() >= MAX_TYPE_COMPATIBILITY_DEPTH {
            return false;
        }
        self.native_queries
            .type_compatibility_depth
            .set(self.native_queries.type_compatibility_depth.get() + 1);
        let _guard = TypeCompatibilityDepthGuard {
            depth: &self.native_queries.type_compatibility_depth,
        };
        self.types_compatible_unchecked(actual, expected)
    }

    /// Check if two types are compatible for assignment or comparison.
    ///
    /// Returns `true` if `actual` can be used where `expected` is required. Handles `Unknown` (error recovery), type
    /// variables (generics), and recursive checks for generics, functions, and tuples.
    fn types_compatible_unchecked(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        let expanded_actual = self.expand_type_aliases(actual.clone());
        let expanded_expected = self.expand_type_aliases(expected.clone());
        if &expanded_actual != actual || &expanded_expected != expected {
            return self.types_compatible(&expanded_actual, &expanded_expected);
        }

        if let Some(matches) = self.public_library_type_identities_compatible(actual, expected) {
            return matches;
        }

        if actual == expected {
            return true;
        }

        if let Some(matches) = self.retained_nominal_types_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = self.module_qualified_nominals_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = helpers::decimal_types_compatible(actual, expected) {
            return matches;
        }

        if let Some(matches) = self.rust_type_identities_compatible(actual, expected) {
            return matches;
        }
        self.types_compatible_inference_and_unions(actual, expected)
    }

    /// Resolve inference, numeric and union rules before nominal trait or container compatibility.
    fn types_compatible_inference_and_unions(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Never, _) => true,
            (ResolvedType::Unknown, _) | (_, ResolvedType::Unknown) => true,
            (actual, expected) if let Some(type_param) = self.rigid_type_param_name(expected) => {
                self.value_is_rigid_type_param(actual, type_param)
            }
            (ResolvedType::TypeVar(_), _) | (_, ResolvedType::TypeVar(_)) => true,
            (actual, expected) if let Some(type_param) = self.rigid_type_param_name(actual) => {
                self.rigid_type_param_value_fits(type_param, actual, expected)
            }
            (actual, _) if self.is_generic_placeholder_type(actual) => true,
            (_, expected) if self.is_generic_placeholder_type(expected) => true,
            (ResolvedType::CallSiteInfer, _) | (_, ResolvedType::CallSiteInfer) => true,
            (ResolvedType::TypeToken(actual_inner), ResolvedType::TypeToken(expected_inner)) => {
                self.nested_types_compatible(actual_inner, expected_inner)
            }
            (ResolvedType::SelfType, ResolvedType::Generic(trait_name, _))
                if self.current_trait_name.as_deref() == Some(trait_name.as_str()) =>
            {
                true
            }
            (actual, expected) if self.numeric_values_compatible(actual, expected) => true,
            (
                ResolvedType::Generic(actual_name, actual_members),
                ResolvedType::Generic(expected_name, expected_members),
            ) if actual_name == UNION_TYPE_NAME && expected_name == UNION_TYPE_NAME => {
                actual_members.iter().all(|actual_member| {
                    expected_members
                        .iter()
                        .any(|expected_member| self.nested_types_compatible(actual_member, expected_member))
                })
            }
            (ResolvedType::Generic(name, members), expected) if name == UNION_TYPE_NAME => members
                .iter()
                .all(|member| self.nested_types_compatible(member, expected)),
            (actual, ResolvedType::Generic(name, members))
                if name == UNION_TYPE_NAME
                    && actual.is_option()
                    && actual
                        .option_inner_type()
                        .is_some_and(|inner| matches!(inner, ResolvedType::Unknown))
                    && members.iter().any(|member| matches!(member, ResolvedType::Unit)) =>
            {
                true
            }
            (actual, ResolvedType::Generic(name, members)) if name == UNION_TYPE_NAME => {
                members.iter().any(|member| self.types_compatible(actual, member))
            }
            (actual, expected) if expected.is_option() && !actual.is_option() => {
                let Some(inner) = expected.option_inner_type() else {
                    return false;
                };
                self.types_compatible(actual, inner)
            }
            (ResolvedType::Generic(name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator)
                    && expected_args.len() == 1
                    && (trait_name == builtin_traits::as_str(TraitId::Iterable)
                        || trait_name == builtin_traits::as_str(TraitId::Iterator)
                        || trait_name == builtin_traits::as_str(TraitId::IntoIterator)) =>
            {
                actual_args.len() == 1 && self.nested_types_compatible(&actual_args[0], &expected_args[0])
            }

            _ => self.types_compatible_trait_upcasts(actual, expected),
        }
    }

    /// Check nullary trait references and instantiated supertrait upcasts before iterable trait adapters.
    fn types_compatible_trait_upcasts(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Named(type_name), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    self.trait_is_supertrait_of(type_name, trait_name)
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Named(trait_name))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(super_info) = self.lookup_semantic_trait_info(trait_name) else {
                        return false;
                    };
                    if !super_info.type_params.is_empty() {
                        return false;
                    }
                    self.trait_is_supertrait_of(type_name, trait_name)
                        && self
                            .instantiated_supertrait_args(type_name, actual_args, trait_name)
                            .is_some_and(|inst| inst.is_empty())
                } else {
                    self.type_implements_trait(type_name, trait_name)
                }
            }

            // ---- Context: RFC 042 — `Subtrait[T…]` assignable to `Supertrait[T…]` (trait upcast) ----
            (
                ResolvedType::Generic(subtrait_name, actual_args),
                ResolvedType::Generic(supertrait_name, expected_args),
            ) if self.lookup_semantic_trait_info(subtrait_name).is_some()
                && self.lookup_semantic_trait_info(supertrait_name).is_some() =>
            {
                let Some(instantiated) = self.instantiated_supertrait_args(subtrait_name, actual_args, supertrait_name)
                else {
                    return false;
                };
                if instantiated.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated.iter())
                    .all(|(e, a)| self.nested_types_compatible(a, e))
            }

            _ => self.types_compatible_iterable_traits(actual, expected),
        }
    }

    /// Check builtin iterable carriers and generic concrete implementations without weakening arity checks.
    fn types_compatible_iterable_traits(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Generic(actual_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable)
                    && expected_args.len() == 1
                    && actual_args.len() == 1 =>
            {
                let actual_is_iterable = actual_name == builtin_traits::as_str(TraitId::Iterator)
                    || matches!(
                        collection_type_id(actual_name.as_str()),
                        Some(
                            CollectionTypeId::List
                                | CollectionTypeId::Set
                                | CollectionTypeId::FrozenList
                                | CollectionTypeId::FrozenSet
                        )
                    );
                actual_is_iterable && self.nested_types_compatible(&actual_args[0], &expected_args[0])
            }
            (ResolvedType::FrozenList(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.nested_types_compatible(actual, &expected_args[0])
            }
            (ResolvedType::FrozenSet(actual), ResolvedType::Generic(trait_name, expected_args))
                if trait_name == builtin_traits::as_str(TraitId::Iterable) && expected_args.len() == 1 =>
            {
                self.nested_types_compatible(actual, &expected_args[0])
            }

            // RFC 042: `Concrete[T]` assignable to generic trait annotation `Trait[T]` (and similar).
            (ResolvedType::Generic(type_name, actual_args), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some()
                    && self.lookup_semantic_trait_info(type_name).is_none()
                    && self.lookup_semantic_type_info(type_name).is_some() =>
            {
                let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, actual_args, trait_name)
                else {
                    return false;
                };
                let concrete_arity_ok = match self.lookup_semantic_type_info(type_name) {
                    Some(TypeInfo::Model(m)) => actual_args.len() == m.type_params.len(),
                    Some(TypeInfo::Class(c)) => actual_args.len() == c.type_params.len(),
                    Some(TypeInfo::Newtype(n)) => actual_args.len() == n.type_params.len(),
                    Some(TypeInfo::Enum(e)) => actual_args.len() == e.type_params.len(),
                    _ => true,
                };
                if !concrete_arity_ok {
                    return false;
                }
                if instantiated_args.len() != expected_args.len() {
                    return false;
                }
                expected_args
                    .iter()
                    .zip(instantiated_args.iter())
                    .all(|(e, a)| self.nested_types_compatible(a, e))
            }

            _ => self.types_compatible_named_generics(actual, expected),
        }
    }

    /// Check bare nominal trait references and the existing unchecked generic-name compatibility surface.
    fn types_compatible_named_generics(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Named(type_name), ResolvedType::Generic(trait_name, expected_args))
                if self.lookup_semantic_trait_info(trait_name).is_some() =>
            {
                if self.lookup_semantic_trait_info(type_name).is_some() {
                    let Some(instantiated_args) = self.instantiated_supertrait_args(type_name, &[], trait_name) else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(e, a)| self.nested_types_compatible(a, e))
                } else {
                    let Some(instantiated_args) = self.instantiated_trait_args_for_type(type_name, &[], trait_name)
                    else {
                        return false;
                    };
                    if instantiated_args.len() != expected_args.len() {
                        return false;
                    }
                    expected_args
                        .iter()
                        .zip(instantiated_args.iter())
                        .all(|(expected, actual)| self.nested_types_compatible(actual, expected))
                }
            }
            // Allow bare surface generic types (e.g. `Json`) to match `Json[T]` when used without args.
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name
                    && self.symbols.lookup(name).is_some()
                    && surface_types::from_str(name.as_str())
                        .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Generic) =>
            {
                true
            }
            (ResolvedType::Named(name), ResolvedType::Generic(generic_name, _))
                if name == generic_name && self.nominal_has_type_parameters(name) =>
            {
                true
            }
            (ResolvedType::Generic(generic_name, _), ResolvedType::Named(name))
                if name == generic_name && self.nominal_has_type_parameters(name) =>
            {
                true
            }
            _ => self.types_compatible_borrowed_and_frozen(actual, expected),
        }
    }

    /// Return whether a semantic nominal declaration has parameters, including unchecked bare annotations.
    fn nominal_has_type_parameters(&self, name: &str) -> bool {
        match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(model)) => !model.type_params.is_empty(),
            Some(TypeInfo::Class(class)) => !class.type_params.is_empty(),
            Some(TypeInfo::Newtype(newtype)) => !newtype.type_params.is_empty(),
            Some(TypeInfo::Enum(info)) => !info.type_params.is_empty(),
            _ => false,
        }
    }

    /// Check reference materialization and frozen carrier compatibility before general structural matching.
    fn types_compatible_borrowed_and_frozen(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            // Internal references: `&mut T` may satisfy `&T`, but not the reverse.
            (ResolvedType::Ref(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::Ref(b))
            | (ResolvedType::RefMut(a), ResolvedType::RefMut(b)) => self.nested_types_compatible(a, b),
            // A borrowed source model or class becomes an owned value where one is required; Body IR clones it there.
            (ResolvedType::Ref(inner), expected)
                if !matches!(expected, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
                    && self.borrowed_source_value_materializes(actual) =>
            {
                self.types_compatible(inner, expected)
            }
            // Frozen const-eval values are compatible with their frozen wrappers. Ordinary runtime strings do not
            // implicitly become frozen values here; const-eval is responsible for freezing literal values first.
            (ResolvedType::FrozenStr, ResolvedType::FrozenStr) => true,
            (ResolvedType::FrozenStr, ResolvedType::Named(name))
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr) =>
            {
                true
            }
            // `FrozenStr` is a read-only string wrapper; allow it where `str` is expected.
            (ResolvedType::FrozenStr, ResolvedType::Str) => true,
            (ResolvedType::Named(name), ResolvedType::Str)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr) =>
            {
                true
            }
            (ResolvedType::FrozenBytes, ResolvedType::FrozenBytes) => true,
            (ResolvedType::FrozenBytes, ResolvedType::Named(name))
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes) =>
            {
                true
            }
            (ResolvedType::FrozenBytes, ResolvedType::Bytes) => true,
            // `bytes` and `list[u8]` share the same owned representation. This lets APIs use collection helpers
            // such as `list.repeat` when constructing byte buffers without falling back to manual Vec assembly.
            (ResolvedType::Generic(name, args), ResolvedType::Bytes)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List)
                    && args.len() == 1
                    && matches!(args[0], ResolvedType::Numeric(id) if incan_lang::lang::types::numerics::as_str(id) == "u8") =>
            {
                true
            }
            // Allow `FrozenBytes` where `bytes` is expected.
            (ResolvedType::Named(name), ResolvedType::Bytes)
                if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes) =>
            {
                true
            }
            (ResolvedType::FrozenList(a), ResolvedType::FrozenList(b)) => self.nested_types_compatible(a, b),
            (ResolvedType::FrozenList(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.nested_types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenList(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenList) && args.len() == 1 =>
            {
                self.nested_types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenSet(a), ResolvedType::FrozenSet(b)) => self.nested_types_compatible(a, b),
            (ResolvedType::FrozenSet(a), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.nested_types_compatible(a, &args[0])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenSet(b))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenSet) && args.len() == 1 =>
            {
                self.nested_types_compatible(&args[0], b)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::FrozenDict(k2, v2)) => {
                self.nested_types_compatible(k1, k2) && self.nested_types_compatible(v1, v2)
            }
            (ResolvedType::FrozenDict(k1, v1), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.nested_types_compatible(k1, &args[0]) && self.nested_types_compatible(v1, &args[1])
            }
            (ResolvedType::Generic(name, args), ResolvedType::FrozenDict(k2, v2))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::FrozenDict) && args.len() >= 2 =>
            {
                self.nested_types_compatible(&args[0], k2) && self.nested_types_compatible(&args[1], v2)
            }
            _ => self.types_compatible_structural_values(actual, expected),
        }
    }

    /// Compare tuples, generic instances and callable signatures, then apply the existing permissive Rust boundary.
    fn types_compatible_structural_values(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            // Treat `Tuple` as both:
            // - a concrete tuple type: `Tuple[T1, T2, ...]`
            // - a supertype for any tuple when used without args: `Tuple`
            //
            // A checked annotation never produces the bare form any more (`resolve_type_checked` refuses it, #1717);
            // the arm remains for the shared resolver's unchecked callers, which still spell "any tuple" this way.
            (ResolvedType::Tuple(_), ResolvedType::Named(name))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                true
            }
            (ResolvedType::Tuple(elems), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Generic(name, args), ResolvedType::Tuple(elems))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                elems.len() == args.len()
                    && elems
                        .iter()
                        .zip(args.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Generic(n1, a1), ResolvedType::Generic(n2, a2)) => {
                n1 == n2
                    && a1.len() == a2.len()
                    && a1
                        .iter()
                        .zip(a2.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            (ResolvedType::Function(p1, r1), ResolvedType::Function(p2, r2)) => {
                // The `mut` marker decides how the argument is passed, so it must agree exactly (#1790).
                p1.len() == p2.len()
                    && p1.iter().zip(p2.iter()).all(|(t1, t2)| {
                        t1.kind == t2.kind
                            && t1.is_mut == t2.is_mut
                            && self.callable_param_types_compatible(&t1.ty, &t2.ty)
                    })
                    && self.nested_types_compatible(r1, r2)
            }
            (ResolvedType::Tuple(e1), ResolvedType::Tuple(e2)) => {
                e1.len() == e2.len()
                    && e1
                        .iter()
                        .zip(e2.iter())
                        .all(|(t1, t2)| self.nested_types_compatible(t1, t2))
            }
            // Rust names one item several ways (`std::boxed::Box`, `alloc::boxed::Box`, a dependency's re-export of
            // `alloc`). Rust metadata records the ancestral spelling; Incan source imports the `std` one. Compare in
            // the ancestral namespace so every re-export of the same item is just another alias of it.
            (ResolvedType::RustPath(a), ResolvedType::RustPath(b)) => {
                incan_lang::interop::ancestral_rust_display(a) == incan_lang::interop::ancestral_rust_display(b)
            }
            // Without full Rust type knowledge, treat any `RustPath` as compatible with non-Rust surfaces so mixed
            // Incan/Rust-typed expressions stay checkable (RFC 005/041 permissive model).
            (ResolvedType::RustPath(_), _) | (_, ResolvedType::RustPath(_)) => true,
            _ => false,
        }
    }

    /// Check compatibility of a type written inside another type: a type argument, a tuple element, a callable
    /// parameter or return, a referenced type, or a member of a union value.
    ///
    /// A numeric type there matches only its own canonical type (`int` and `i64` are one type). RFC 009's lossless
    /// widening converts one value, and generated code converts it where the value reaches its destination; the values
    /// a collection, tuple, `Option` value or callable holds or produces have no such conversion, so `list[i8]` is not
    /// a `list[int]` and `(int) -> i8` is not an `(int) -> int`.
    fn nested_types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        let depth = self.native_queries.nested_type_compatibility_depth.get();
        self.native_queries.nested_type_compatibility_depth.set(depth + 1);
        let compatible = self.types_compatible(actual, expected);
        self.native_queries.nested_type_compatibility_depth.set(depth);
        compatible
    }

    /// Return whether a numeric value type is compatible with a numeric destination type: through lossless widening
    /// for a value (see the assignment table in the numeric semantics reference), and only by canonical identity inside
    /// another type (see [`Self::nested_types_compatible`]).
    fn numeric_values_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        if self.native_queries.nested_type_compatibility_depth.get() == 0 {
            return numeric_lossless_compatible(actual, expected);
        }
        let actual_id = numeric_type_id_for_compat(actual);
        actual_id.is_some() && actual_id == numeric_type_id_for_compat(expected)
    }

    /// Function parameters must preserve borrow shape exactly.
    ///
    /// Ordinary value compatibility is intentionally permissive at Rust boundaries, but callback signatures are called
    /// by Rust with a fixed argument ABI. A function that takes `T` or `&T` is not a valid substitute for one that Rust
    /// will call as `FnMut(&mut T)`.
    fn callable_param_types_compatible(&self, actual: &ResolvedType, expected: &ResolvedType) -> bool {
        match (actual, expected) {
            (ResolvedType::Ref(actual_inner), ResolvedType::Ref(expected_inner))
            | (ResolvedType::RefMut(actual_inner), ResolvedType::RefMut(expected_inner)) => {
                self.nested_types_compatible(actual_inner, expected_inner)
            }
            (ResolvedType::Ref(_) | ResolvedType::RefMut(_), _)
            | (_, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => false,
            _ => self.nested_types_compatible(actual, expected),
        }
    }
}

impl Default for TypeChecker {
    /// Initialize the same builtin scope and empty field groups as [`Self::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Return whether a declaration participates in public module import collection.
pub(crate) fn is_public_decl(decl: &Spanned<Declaration>) -> bool {
    match &decl.node {
        Declaration::Capability(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Const(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Static(s) => matches!(s.visibility, Visibility::Public),
        Declaration::Model(m) => matches!(m.visibility, Visibility::Public),
        Declaration::Class(c) => matches!(c.visibility, Visibility::Public),
        Declaration::Enum(e) => matches!(e.visibility, Visibility::Public),
        Declaration::Alias(a) => matches!(a.visibility, Visibility::Public),
        Declaration::TypeAlias(a) => matches!(a.visibility, Visibility::Public),
        Declaration::Newtype(n) => matches!(n.visibility, Visibility::Public),
        Declaration::Trait(t) => matches!(t.visibility, Visibility::Public),
        Declaration::Function(f) => matches!(f.visibility, Visibility::Public),
        Declaration::Partial(partial) => matches!(partial.visibility, Visibility::Public),
        Declaration::Import(_)
        | Declaration::Docstring(_)
        | Declaration::TestModule(_)
        | Declaration::VocabBlock(_) => false,
    }
}

/// Return the root symbol name declared by a declaration, when it has one.
pub(crate) fn declaration_name(decl: &Spanned<Declaration>) -> Option<&str> {
    match &decl.node {
        Declaration::Capability(c) => Some(c.name.as_str()),
        Declaration::Const(c) => Some(c.name.as_str()),
        Declaration::Static(s) => Some(s.name.as_str()),
        Declaration::Model(m) => Some(m.name.as_str()),
        Declaration::Class(c) => Some(c.name.as_str()),
        Declaration::Enum(e) => Some(e.name.as_str()),
        Declaration::Alias(a) => Some(a.name.as_str()),
        Declaration::TypeAlias(a) => Some(a.name.as_str()),
        Declaration::Newtype(n) => Some(n.name.as_str()),
        Declaration::Trait(t) => Some(t.name.as_str()),
        Declaration::Function(f) => Some(f.name.as_str()),
        Declaration::Partial(partial) => Some(partial.name.as_str()),
        Declaration::Import(_)
        | Declaration::Docstring(_)
        | Declaration::TestModule(_)
        | Declaration::VocabBlock(_) => None,
    }
}

/// Return the private top-level root named by a declaration-safe preset expression, if any.
fn preset_private_root(expr: &Spanned<Expr>, public_decls: &HashMap<String, bool>) -> Option<String> {
    match &expr.node {
        Expr::Ident(name) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        Expr::Field(base, _) => preset_private_root(base, public_decls),
        Expr::Paren(inner) => preset_private_root(inner, public_decls),
        Expr::Call(callee, _, _) => preset_private_root(callee, public_decls),
        Expr::Constructor(name, _) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        _ => None,
    }
}

/// Extract a decimal precision or scale argument from a type-position integer literal.
fn decimal_type_int_arg(arg: &Spanned<Type>) -> Option<i64> {
    match &arg.node {
        Type::IntLiteral(value) => Some(value.value),
        _ => None,
    }
}

/// Return whether two resolved numeric types are compatible through lossless widening.
fn numeric_lossless_compatible(actual: &ResolvedType, expected: &ResolvedType) -> bool {
    let Some(actual_id) = numeric_type_id_for_compat(actual) else {
        return false;
    };
    let Some(expected_id) = numeric_type_id_for_compat(expected) else {
        return false;
    };
    numeric_type_losslessly_widens_to(actual_id, expected_id)
}

/// Whether an adoption a compiled package's type recorded names the package trait declared in `module_path` as `name`.
///
/// The caller has already proved that the bound and adopting type come from the same package. Compare their declaring
/// module and declaration name after normalizing the library entrypoint spelling used on each side of the manifest.
fn adoption_names_declaration(adoption: &TypeBoundInfo, module_path: &[String], name: &str) -> bool {
    let trait_name = adoption
        .source_name
        .as_deref()
        .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
    adoption.module_path.as_deref().is_some_and(|adoption_module| {
        canonicalize_library_declaration_module_path(adoption_module)
            == canonicalize_library_declaration_module_path(module_path)
    }) && trait_name == name
}

/// Whether a trait adoption that records its trait's module names the trait a bound resolved to.
///
/// `bound` is the bound's `(module path, trait name)` in the checking module. The adoption's trait name is its source
/// name when it has one (`Serialize` for an adoption the declaring module spelled `json.Serialize`), so the comparison
/// does not depend on which names the checking module has imported. An adoption without a recorded module, or a bound
/// that does not resolve, answers `false` and is left to the name comparisons.
fn adoption_names_bound_identity(adoption: &TypeBoundInfo, bound: Option<&(Vec<String>, String)>) -> bool {
    let (Some(module_path), Some((bound_module, bound_trait))) = (adoption.module_path.as_ref(), bound) else {
        return false;
    };
    let trait_name = adoption
        .source_name
        .as_deref()
        .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
    module_path == bound_module && trait_name == bound_trait
}

/// Map an ordinary or exact numeric type to its canonical numeric id for compatibility checks.
pub fn numeric_type_id_for_compat(ty: &ResolvedType) -> Option<NumericTypeId> {
    match ty {
        ResolvedType::Int => Some(NumericTypeId::I64),
        ResolvedType::Float => Some(NumericTypeId::F64),
        ResolvedType::Numeric(id) => Some(*id),
        _ => None,
    }
}

/// Convenience function to type-check an AST
#[tracing::instrument(skip_all, fields(decl_count = program.declarations.len()))]
pub fn check(program: &Program) -> Result<(), Vec<CompileError>> {
    TypeChecker::new().check_program(program)
}

#[cfg(test)]
mod nominal_scope_tests {
    use super::{PublicLibraryTypeIdentity, TypeChecker};
    use crate::ast::{Program, Span};
    use crate::symbols::{ResolvedType, ScopeKind, Symbol, SymbolKind, VariableInfo};
    use incan_semantics_core::{CanonicalSymbolId, body_ir::nominal_carrier_key};
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    /// Parse a source module without changing its declarations or depending on a published standard library.
    fn program(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
        Ok(crate::parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
    }

    /// Reproduce the uncached carrier collection from immutable declaration scopes, independently of the new index.
    pub(super) fn fresh_scope(checker: &TypeChecker) -> BTreeMap<String, CanonicalSymbolId> {
        let mut identities = checker.checked_nominal_bindings();
        let mut visited = HashSet::new();
        let carriers = identities
            .values()
            .chain(
                checker
                    .declaration_nominal_scopes
                    .values()
                    .filter(|scope| visited.insert(Arc::as_ptr(scope)))
                    .flat_map(|scope| scope.values()),
            )
            .filter_map(|identity| nominal_carrier_key(identity).map(|key| (key, identity.clone())))
            .collect::<Vec<_>>();
        for (key, identity) in carriers {
            identities.entry(key).or_insert(identity);
        }
        identities
    }

    /// Compare mutation-owned lookup indexes with arena lookup, and prove unchanged requests share one computation.
    fn assert_current_inventory(checker: &TypeChecker) {
        let expected = checker
            .symbols
            .all_symbols()
            .iter()
            .enumerate()
            .filter(|(id, symbol)| symbol.scope == 0 && checker.symbols.is_active_lookup_binding(*id))
            .map(|(id, _)| id)
            .collect::<std::collections::BTreeSet<_>>();
        let actual = checker
            .symbols
            .active_module_symbols()
            .map(|(id, _)| id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(actual, expected);
        let scope = checker.checked_nominal_type_identities();
        assert_eq!(scope.as_ref(), &fresh_scope(checker));
        assert!(Arc::ptr_eq(&scope, &checker.checked_nominal_type_identities()));
    }

    /// Changing an alias target invalidates facts even when its source lookup binding and inventory length stay fixed.
    fn assert_alias_target_revision(checker: &mut TypeChecker) -> Result<(), Box<dyn std::error::Error>> {
        let previous = checker.type_aliases.get("Alias").cloned().ok_or("missing alias")?;
        let binding = checker.symbols.lookup("Alias");
        let before = checker.checked_nominal_type_identities();
        let alias = checker.type_aliases.get_mut("Alias").ok_or("missing alias")?;
        let identity = alias
            .nominal_identities
            .values_mut()
            .next()
            .ok_or("missing alias owner")?;
        identity.declaration_name = "Changed".to_string();
        assert_eq!(checker.symbols.lookup("Alias"), binding);
        assert!(checker.checked_body_nominal_scope().is_none());
        let after = checker.checked_nominal_type_identities();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_current_inventory(checker);
        checker.type_aliases.insert("Alias".to_string(), previous);
        assert_current_inventory(checker);
        Ok(())
    }

    /// Check admission, repeated imports, lexical shadow/restore and same-size canonical replacements against fresh
    /// facts.
    #[test]
    fn nominal_scope_inventory_matches_fresh_inputs() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Both constructors initialize the same semantic context ----
        for checker in [TypeChecker::new(), TypeChecker::default()] {
            assert_eq!(checker.current_yield_context, super::YieldContext::Disallowed);
            assert!(checker.errors.is_empty());
            assert!(checker.warnings.is_empty());
            assert!(checker.loop_stack.is_empty());
            assert_eq!(checker.symbols.current_scope_index(), 0);
            assert!(checker.symbols.lookup("int").is_some());
            assert!(checker.symbols.lookup("len").is_some());
        }

        // ---- Imported declarations and repeated admission ----
        let first = program("pub model First:\n    value: int\n")?;
        let second = program("pub model Second:\n    value: int\n")?;
        let mut checker = TypeChecker::default();
        for (name, dependency) in [("first", &first), ("second", &second), ("first", &first)] {
            checker.import_module_all(dependency, name);
            assert_current_inventory(&checker);
        }
        assert_eq!(checker.retained_nominal_carriers.len(), 2);
        let module = program("model Local:\n    value: int\ntype Alias = Local\n")?;
        checker.set_current_module_path(Some(vec!["consumer".to_string()]));
        checker.reset_program_check(&module, true);
        checker.collect_program_check_declarations(&module);
        checker.prepare_program_body_facts();
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert_eq!(
            checker.checked_body_nominal_scope().as_deref(),
            Some(&fresh_scope(&checker))
        );
        assert_alias_target_revision(&mut checker)?;

        // ---- Lexical alias shadowing and restoration ----
        checker.symbols.enter_scope(ScopeKind::Block);
        checker.symbols.define(Symbol {
            name: "Alias".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Int,
                is_mutable: false,
                is_used: false,
            }),
            span: Span::new(100, 110),
            scope: 0,
        });
        assert!(checker.checked_body_nominal_scope().is_none());
        assert_current_inventory(&checker);
        checker.symbols.exit_scope();
        assert_current_inventory(&checker);
        assert_eq!(
            checker.checked_body_nominal_scope().as_deref(),
            Some(&fresh_scope(&checker))
        );

        // ---- Equal-size public owner replacement and removal ----
        let local = checker
            .symbols
            .lookup("Local")
            .and_then(|id| checker.symbols.identity_of(id))
            .cloned()
            .ok_or("missing Local identity")?;
        checker.public_library_type_identities.insert(
            "foreign".to_string(),
            PublicLibraryTypeIdentity::new("demo", &["Local".to_string()]).with_canonical(Some(local.clone())),
        );
        checker.prepare_program_body_facts();
        let mut replacement = local;
        replacement.declaration_name = "Different".to_string();
        checker
            .public_library_type_identities
            .get_mut("foreign")
            .ok_or("missing public binding")?
            .canonical = Some(replacement);
        assert!(checker.checked_body_nominal_scope().is_none());
        assert_current_inventory(&checker);

        checker.prepare_program_body_facts();
        checker.public_library_type_identities.remove("foreign");
        checker.public_library_type_identities.insert(
            "unproven".to_string(),
            PublicLibraryTypeIdentity::new("demo", &["Unproven".to_string()]),
        );
        assert!(checker.checked_body_nominal_scope().is_none());
        assert_current_inventory(&checker);
        Ok(())
    }
}
