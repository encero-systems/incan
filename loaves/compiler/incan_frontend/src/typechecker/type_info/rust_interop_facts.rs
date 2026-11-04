//! Checked Rust interop artifacts: layouts, enums, conversions, calls, signatures and iteration hooks.

use super::*;

/// Rust interop decisions that must be preserved exactly across lowering.
#[derive(Debug, Default, Clone)]
pub struct RustInteropArtifacts {
    /// Structural native storage keyed by the original checked type. Diagnostic displays are opaque RustInteropPath
    /// keys; consumers never parse or complete them.
    pub storage_types: HashMap<IncanType, IncanType>,
    /// Declaration-order native binders and defaults used while the accepting checker freezes native storage.
    pub owner_parameters: HashMap<String, Vec<(String, Option<ResolvedType>)>>,
    /// Native by-value receiver calls whose nongeneric owner has an unconditional metadata-selected Copy
    /// implementation. The adapter verifies that capability against the exact linked native layout.
    pub copy_receivers: HashMap<crate::ast::Span, String>,
    /// Public native field layouts selected during checking. Hidden fields remain absent from metadata and the
    /// adapter refuses incomplete layouts before any construction or projection is emitted.
    pub checked_layouts: HashMap<String, RustCheckedLayout>,
    /// Complete native enum payload templates selected by constructor patterns, keyed by the public owner path.
    pub checked_enums: HashMap<String, RustCheckedEnum>,
    /// Public Rust constants selected from metadata, keyed by their complete value-expression span.
    pub checked_constants: HashMap<crate::ast::Span, String>,
    /// Resolved Rust callees with closed native arguments and signatures, keyed by the complete call span.
    /// Body IR consumes these records without resolving source spellings or inferring foreign types again.
    pub checked_calls: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Checked native argument adaptations, including dereference calls and owned payload storage constructors,
    /// keyed by argument expression span. Body IR emits these exact steps before the source call.
    pub argument_adjustments: HashMap<crate::ast::Span, Vec<RustCheckedCall>>,
    /// Exact infallible rusttype edges selected from authored adapters and inspected native signatures.
    /// Ordinary source boundaries select only a matching source/destination pair, never any edge on the owner.
    pub(crate) declared_conversions: Vec<RustDeclaredConversion>,
    /// Authored rusttype edges whose complete adapter signatures passed declaration checking. Fallible edges remain
    /// declaration authority even though automatic native conversion currently consumes only infallible edges.
    pub(crate) validated_rusttype_edges: HashSet<crate::ast::Span>,
    /// Exact owned native From calls applied to each source list element at an argument boundary. The list is
    /// evaluated once; the selected conversion is executed once per element in source order.
    pub element_adjustments: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Metadata-selected shared dereference steps before a native public field read, keyed by the field expression.
    /// Field identity and payload remain in their ordinary checked-field facts.
    pub field_adjustments: HashMap<crate::ast::Span, Vec<RustCheckedCall>>,
    /// Exact physical field paths selected for source extractor forwarding, including the native tuple slot.
    pub field_projection_paths: HashMap<crate::ast::Span, Vec<CanonicalSymbolId>>,
    /// Exact native values of public field expressions before source collection and scalar projection.
    pub field_values: HashMap<crate::ast::Span, RustCheckedFieldValue>,
    /// Safe closed native functions selected as values, keyed by the read span rather than an application span.
    /// Their signatures share call publication while their value role never supplies a call's result provenance.
    pub checked_function_values: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Closed native iteration calls selected from IntoIterator and Iterator facts at the iterable's span.
    pub iteration_hooks: HashMap<crate::ast::Span, RustIterationHooks>,
    /// Native result item selected for each local binding, even when its source type projects to str or a collection.
    pub value_item_paths: HashMap<CanonicalSymbolId, String>,
    /// Native carriers of Rust-derived local values before source reference and collection projection.
    pub value_native_types: HashMap<CanonicalSymbolId, ResolvedType>,
    /// Validated synchronous diverging Rust import calls, keyed by full call span. Body IR alone consumes the
    /// selected foreign path; legacy reference identities and Rust emission facts remain unchanged (#1698).
    pub(crate) body_ir_diverging_import_calls: HashMap<crate::ast::Span, Vec<String>>,
    /// Source-proven receiver and returned-borrow relationships keyed by the exact checked method-call span.
    /// Unknown metadata never authorizes a shared alias; lowering must not reconstruct this from method names.
    pub receiver_contracts: HashMap<crate::ast::Span, incan_lang::interop::RustReceiverContract>,
    /// `rusttype` Incan name → canonical Rust path string (`substrait::proto::type::Binary`), when the checker
    /// resolved the underlying type to [`ResolvedType::RustPath`]. Used by lowering so `m::T` spellings emit full
    /// paths without re-running import resolution.
    pub rusttype_canonical_paths: HashMap<String, String>,
    /// Every spelling of an imported Rust item (its import path and its definition path) → the public path that names
    /// it. Rust metadata spells a returned type by its definition (`std::sync::poison::mutex::Mutex`), an annotation
    /// by its import (`std::sync::Mutex`); direct-route storage keys both on the one public path (DD-0006).
    pub item_public_paths: HashMap<String, String>,
    /// Rust-boundary coercion decisions keyed by argument expression span.
    pub arg_coercions: HashMap<crate::ast::Span, RustArgCoercionInfo>,
    /// Rust trait imports keyed by the source binding name with the trait path and method names they can place in
    /// scope.
    ///
    /// Lowering carries this into IR import items so codegen can retain extension-trait imports when Rust method
    /// lookup needs the trait in scope even though emitted call tokens do not otherwise mention the trait name.
    pub trait_imports: HashMap<String, RustTraitImportInfo>,
    /// Body-less rusttype Rust-trait adoptions proven by metadata and therefore satisfied by the backing type alias,
    /// keyed by `(rusttype name, written trait name)`.
    ///
    /// Body IR keeps such a rusttype a transparent alias (`is_direct_native_rusttype_alias`): Rust coherence treats
    /// the alias as the foreign backing type, which already implements the trait. The typechecker records only
    /// non-generic trait paths that metadata proved.
    pub rusttype_forwarded_trait_adoptions: HashSet<(String, String)>,
    /// Rust-boundary coercion decisions for method return values, keyed by the call expression span.
    ///
    /// Checker working state: recorded when metadata shows a `rusttype` method's actual Rust return type requires
    /// coercion to the Incan-declared type (e.g. `&str` → `String` for a method declared `-> str`), and read by later
    /// trait-method selection in the same check.
    pub return_coercions: HashMap<crate::ast::Span, RustArgCoercionInfo>,
    /// Borrowed Rust call results that must remain in their native Rust representation at the next call boundary.
    ///
    /// Exact generic parameters provide this evidence directly. An unresolved imported Rust call also preserves its
    /// direct Rust-return arguments rather than inventing an Incan-owned conversion before rustc sees the boundary.
    /// The marker makes either decision independent of whether result or argument checking runs first.
    pub native_return_consumers: HashSet<crate::ast::Span>,
    /// Rust closure parameter displays keyed by closure-expression span.
    ///
    /// This is populated when contextual Rust metadata proves a closure is being used as a Rust callable boundary
    /// whose parameter shape cannot be faithfully represented by ordinary Incan surface types, such as `&[T]`.
    /// Lowering/emission consumes the displays directly so generated closures keep Rust inference stable.
    pub closure_param_type_displays: HashMap<crate::ast::Span, Vec<String>>,
    /// Native closure storage admitted at a Rust callback boundary, keyed by its originating closure expression.
    pub closure_storage: HashMap<crate::ast::Span, incan_semantics_core::body_ir::NativeClosureStorage>,
    /// Native callback parameter slots paired with their exact declaration-order generic argument positions.
    pub native_callable_parameters: HashMap<String, Vec<(usize, usize)>>,
    /// Exact Rust parameter displays for source functions passed directly to a Rust callable boundary.
    ///
    /// Source annotations intentionally use Incan collection vocabulary (`&mut list[f32]`), while Rust callback
    /// bounds can require a slice (`&mut [f32]`). Lowering uses this fact only for the proven callback function so it
    /// does not globally change how Incan lists are represented.
    pub function_param_type_displays: HashMap<String, Vec<String>>,
    /// Rust async call expressions proven during call validation, keyed by the full call-expression span.
    ///
    /// Rust metadata is now loaded lazily at the call boundary rather than during import collection. `await` consumes
    /// this artifact so direct async Rust calls still realize to their output type without reintroducing import-time
    /// extraction or relying on stale symbol metadata.
    pub async_call_realizations: HashSet<crate::ast::Span>,
    /// Metadata-selected IntoFuture conversions keyed by the await operand's span; Body IR emits each once before
    /// suspension.
    pub await_conversions: HashMap<crate::ast::Span, RustCheckedCall>,
    /// Transparent rusttype aliases with their native owner template and source type parameters.
    pub type_aliases: HashMap<String, (Vec<String>, ResolvedType)>,
}

/// A metadata-selected native struct layout, generic over its declaring type parameters.
#[derive(Debug, Clone)]
pub struct RustCheckedLayout {
    /// Source visibility of each native field slot, aligned with `fields`.
    pub field_public: Vec<bool>,
    /// An unconditional native Copy implementation retained with the owner, independent of which module uses it.
    pub is_copy: bool,
    /// Declared type binders; lifetime arguments carry no layout and are absent.
    pub type_parameters: Vec<String>,
    /// Ordered native field names and exact storage templates, including inaccessible private slots.
    pub fields: Vec<CallableParam>,
    /// Canonical field targets minted with the same owner as the retained layout.
    pub field_identities: Vec<CanonicalSymbolId>,
}

/// A public field's native carrier and item identity before source type projection.
#[derive(Debug, Clone)]
pub struct RustCheckedFieldValue {
    /// Instantiated native field type, including references and type arguments.
    pub native_type: ResolvedType,
    /// Public native item selected from metadata, when the field has a nominal carrier.
    pub item_path: Option<String>,
}

/// A complete native enum layout selected from metadata; declaration binders remain open until storage is closed.
#[derive(Debug, Clone)]
pub struct RustCheckedEnum {
    /// Unconditional native Copy capability, retained before any concrete enum carrier is emitted.
    pub is_copy: bool,
    /// Type binders in native declaration order, excluding lifetimes.
    pub type_parameters: Vec<String>,
    /// Ordered variant names, exact native payload templates and named-field slots, before match ergonomics adds
    /// references.
    pub variants: Vec<(String, Vec<ResolvedType>, Vec<String>)>,
}

/// One authored rusttype edge and the exact native adapter selected for its source and destination types.
#[derive(Debug, Clone)]
pub(crate) struct RustDeclaredConversion {
    /// Exact source type accepted by this edge.
    pub(crate) source: ResolvedType,
    /// Exact destination selected by this edge.
    pub(crate) destination: ResolvedType,
    /// The authored edge's span, used to admit only declarations whose complete native edges were checked.
    pub(crate) span: Span,
    /// Inspected adapter signature and canonical callable identity.
    pub(crate) call: RustCheckedCall,
}

/// A metadata-selected Rust call, retaining the signature and receiver passing mode the checker admitted.
#[derive(Debug, Clone, PartialEq)]
pub struct RustCheckedCall {
    /// Fully qualified structural signature, published by the accepting checker before Body IR construction.
    pub storage: Option<RustCheckedSignature>,
    /// Source declaration binders active when this native call was admitted, independently of nominal spellings.
    pub source_type_parameters: Vec<String>,
    /// Flat, checked Deref/DerefMut callees evaluated on the receiver before written ordinary arguments.
    pub receiver_adjustments: Vec<RustCheckedCall>,
    /// Public item path of the selected free function or associated function.
    pub path: String,
    /// Native type arguments in the Rust declaration's order, including impl arguments for a method.
    pub type_arguments: Vec<ResolvedType>,
    /// Instantiated parameters; a method's receiver is first, including its reference mode.
    pub parameters: Vec<CallableParam>,
    /// The checked instantiated result type.
    pub return_type: ResolvedType,
    /// Native result's nominal item before source scalar or collection projection, when metadata names one.
    pub result_item_path: Option<String>,
    /// Whether the source receiver is a value passed in the first parameter slot.
    pub has_receiver: bool,
}

/// Native call storage and source nominal owners selected in the checker's declaration environment.
#[derive(Debug, Clone, PartialEq)]
pub struct RustCheckedSignature {
    /// Native argument types, including the receiver's exact reference mode.
    pub parameters: Vec<IncanType>,
    /// Native result before source boundary conversions.
    pub return_type: IncanType,
    /// Native type arguments in declaration order.
    pub type_arguments: Vec<IncanType>,
    /// Exact source owners backing identity-encoded storage leaves.
    pub identities: BTreeMap<String, CanonicalSymbolId>,
}

/// Native iteration retains both calls independently of the written expression's call facts.
#[derive(Debug, Clone, PartialEq)]
pub struct RustIterationHooks {
    /// Consuming conversion that materializes iterator storage once.
    pub iter: RustCheckedCall,
    /// Exclusive poll whose Option payload defines the loop item's carrier.
    pub next: RustCheckedCall,
}
