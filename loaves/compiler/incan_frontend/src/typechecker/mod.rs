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

pub(crate) mod body_check;
mod body_ir_classes;
mod capturing_callables;
mod check_decl;
mod check_work;
pub(crate) use body_check::BodyIrBaselineInputs;
#[cfg(test)]
pub(crate) use body_check::CheckedBodyInputError;
pub use check_work::CheckWork;
pub(crate) mod canonical_nodes;
mod check_expr;
mod check_stmt;
mod checked_facts;
mod collect;
mod collection_annotations;
mod const_eval;
mod construction;
mod control_contexts;
mod declaration_validation;
mod declaration_views;
mod decorated_method_receivers;
mod dependency_members;
mod dependency_registry;
mod derive_contract;
mod derive_requirements;
mod diagnostic_values;
mod for_item_taking;
mod hash_key_inference;
mod helpers;
mod iteration_borrows;
mod json_member_requirements;
mod lifetime_contracts;
mod lowering_facts;
mod member_bindings;
mod module_imports;
mod mut_arguments;
mod mut_marker;
mod native_call_state;
mod native_newtype_facts;
mod native_storage;
pub(crate) use native_storage::native_frozen_constructor;
mod nominal_identities;
mod nominal_scope;
mod nominal_type_param_bounds;
mod open_native_owners;
mod ownership_contracts;
mod program_check;
mod rust_metadata;
mod rust_signatures;
mod static_dependencies;
mod static_initializer_writes;
mod symbol_lookup;
mod type_aliases;
mod type_compatibility;
mod type_references;
mod type_resolution;
pub(crate) use ownership_contracts::carried_view_binding;
pub(crate) use ownership_contracts::projection::{CheckedInputBody, CheckedInputInvocation, CheckedInputSource};
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
pub use native_call_state::{
    CAbiRawCallResult, CAbiSpanLocal, OpenRustGenericBinding, OpenRustGenericCall, OpenRustPassiveUse,
    PendingCAbiOutputSlot,
};
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
    /// Test-only cold baseline bypasses local table extraction while retaining the original semantic operations.
    #[cfg(test)]
    legacy_body_checking: bool,
    /// Verification-only declaration activations; a statement producer may not run inside any of them.
    #[cfg(test)]
    active_declaration_checks: Vec<Span>,
    /// Per-body complete compatibility snapshots for cold-baseline comparisons, absent from production.
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
    /// Registry structural values of dependency source modules' `pub const` declarations, keyed by canonical identity
    /// and computed in the declaring module's scope. A descriptor naming an imported const records this value, as one
    /// naming a local const records its own; a const that is not structural keeps its refusal.
    pub dependency_registry_consts:
        HashMap<CanonicalSymbolId, Result<incan_semantics_core::SemanticRegistryValue, String>>,
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
    /// Checked declaring identities of source-module derive entries, including imported trait aliases.
    pub dependency_derivable_trait_identities: HashMap<String, CanonicalSymbolId>,
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

/// The elements of a tuple annotation, written `(A, B)` or `tuple[A, B]`.
pub(in crate::typechecker) fn written_tuple_elements(ty: &Type) -> Option<&[Spanned<Type>]> {
    match ty {
        Type::Tuple(elements) => Some(elements),
        Type::Generic(name, elements)
            if incan_lang::lang::types::collections::from_str(name) == Some(CollectionTypeId::Tuple) =>
        {
            Some(elements)
        }
        _ => None,
    }
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
mod nominal_scope_tests;
