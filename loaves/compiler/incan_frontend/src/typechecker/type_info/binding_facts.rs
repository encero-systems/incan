//! Partial projections, source targets, coercions, and function, property and constructor bindings.

use super::*;

/// Source-level partial projection metadata preserved after collection.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PartialProjectionInfo {
    /// Source name of the partial declaration visible in the current module.
    pub name: String,
    /// Resolved source path to the projected target.
    pub target_path: Vec<String>,
    /// Semantic kind of the projected target when collection could classify it.
    pub target_kind: PartialProjectionTargetKind,
    /// Preset keyword expressions supplied by the partial declaration.
    pub presets: Vec<PartialProjectionPreset>,
}

/// One preset keyword/value pair from a partial declaration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PartialProjectionPreset {
    pub name: String,
    pub value: Spanned<Expr>,
}

/// Target kinds that matter to downstream partial projection consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PartialProjectionTargetKind {
    Function,
    ModelConstructor,
    ClassConstructor,
    NewtypeConstructor,
    Unknown,
}

/// One checked public declaration reached through explicit source facade links at an otherwise unresolved call.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrReexportCallInfo {
    /// The manifest's minted declaring identity, never the facade or consumer spelling.
    pub canonical: CanonicalSymbolId,
    /// Parameters of the selected published declaration; call-site substitutions remain in the ordinary checked call
    /// facts.
    pub parameters: Vec<CallableParam>,
}

/// One module-qualified type annotation (`mod.Type`) resolved to the declaration it names.
///
/// Each part serves a different consumer. The identity is the RFC 120 proof of which declaration was selected, for
/// reference-site facts. The module path is the checked path the spelling walked -- the module binding's resolved
/// path plus any nested segments -- which is where lowering places the nominal reference, since this module holds no
/// import binding for the bare type name; it is the same path a call through the binding is emitted against. The
/// resolved type is what a direct import of the same declaration would have bound, so a value checked against the
/// qualified spelling is compatible with one checked against the imported name.
#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedTypeReferenceInfo {
    /// Canonical identity of the declaration the qualified spelling names.
    pub identity: CanonicalSymbolId,
    /// Checked module path the spelling reached the declaration through (`["std", "toml"]`, `["pub", "widgets"]`).
    pub module_path: Vec<String>,
    /// The nominal type the annotation denotes, exactly as a direct import of the declaration resolves it.
    pub resolved: ResolvedType,
}

/// Compiler-proven source declaration target for codegraph call/reference records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTargetInfo {
    /// Import path segments **as written** at the reference site.
    ///
    /// Not necessarily the module that owns the declaration: resolution tries sibling-relative candidates before bare
    /// ones, so this can name a different module that merely declares the same leaf name. The proven owner lives in
    /// [`DeclarationArtifacts::resolved_import_identities`].
    pub module_path: Vec<String>,
    /// Source declaration name in the owning module.
    pub name: String,
    /// Source declaration kind, matching the codegraph declaration `kind` spelling.
    pub kind: String,
}

impl SourceTargetInfo {
    /// Return whether this recorded declaration target is the canonical function kind.
    pub fn is_function(&self) -> bool {
        SemanticSourceTargetKind::from_kind_str(&self.kind) == SemanticSourceTargetKind::Function
    }
}

/// Coercion category selected by the typechecker for a Rust-boundary call argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustArgCoercionKind {
    /// Builtin boundary matrix coercion (`i16 -> i64`, `str -> &str`, ...).
    Builtin(CoercionPolicy),
    /// Rusttype alias can flow to its backing Rust type without an explicit adapter call.
    RustTypeUnwrap,
    /// Rusttype alias uses a declared `interop:` adapter edge.
    RustTypeInterop,
    /// The Rust enum variant stores this payload as `Box<T>`; the argument is the semantic `T` and lowering boxes it.
    BoxPayload,
    /// Rust metadata requires a concrete reference, so preserve its borrow shape during emission.
    Borrow {
        /// Whether Rust requires an exclusive mutable borrow.
        mutable: bool,
    },
    /// Rust metadata requires a trait-object reference, so preserve the required borrow shape at emission.
    TraitObjectBorrow {
        /// Whether Rust requires an exclusive `&mut dyn Trait` borrow.
        mutable: bool,
    },
}

/// Lowering metadata for one Rust-boundary call argument.
#[derive(Debug, Clone, PartialEq)]
pub struct RustArgCoercionInfo {
    /// Normalized Rust parameter type display from metadata (e.g. `f32`, `&str`).
    pub rust_target_type: String,
    /// Resolved target type for lowering IR typing.
    pub target_type: ResolvedType,
    /// Coercion strategy to apply.
    pub kind: RustArgCoercionKind,
}

/// One typechecker-approved validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedNewtypeCoercionInfo {
    /// Ordered underlying-to-target conversion steps.
    pub steps: Vec<ValidatedNewtypeCoercionStep>,
    /// Final target type after all steps.
    pub target_type: ResolvedType,
    /// Runtime failure strategy selected for the coercion site.
    pub mode: ValidatedNewtypeCoercionMode,
}

/// Runtime failure behavior for one validated-newtype coercion site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedNewtypeCoercionMode {
    /// Ordinary sites panic on the first validation error.
    FailFast,
    /// Model/class constructor fields collect this field's validation error before the constructor fails.
    AggregateField { field_name: String },
}

/// One conversion step in a validated-newtype coercion chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNewtypeCoercionStep {
    /// Newtype being constructed by this step.
    pub newtype_name: String,
    /// Checker-selected declaration owning the tuple storage, including a signature-only imported newtype.
    pub owner_identity: Option<CanonicalSymbolId>,
    /// Canonical validation hook to call. `None` means direct newtype wrapping is sufficient.
    pub ctor: Option<String>,
    /// Exact source declaration selected for `ctor`, when the hook has compiler-owned identity metadata.
    ///
    /// Lowering uses this to select the RFC 120 physical projection without reconstructing declaration provenance
    /// from the conventional `from_underlying` spelling.
    pub ctor_identity: Option<CanonicalSymbolId>,
    /// Generated constrained-primitive predicates to enforce before direct wrapping.
    pub constraints: Vec<NewtypePrimitiveConstraint>,
}

/// Lowering metadata for one source function or method declaration.
///
/// Shared by [`DeclarationArtifacts::function_bindings`]/[`DeclarationArtifacts::function_bindings_by_span`] (top-
/// level `def`) and [`DeclarationArtifacts::method_bindings_by_span`] (class/model/trait methods, #1121) rather than
/// duplicating an equivalent struct per callable kind.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBindingInfo {
    /// Invoking this declaration constructs a lazy future whose output is `return_type`.
    pub is_async: bool,
    /// Typechecker-resolved source parameters, including default-presence markers.
    pub params: Vec<CallableParam>,
    /// Typechecker-resolved source return type.
    pub return_type: ResolvedType,
    /// RFC 120 canonical identity of this declaration, minted once when the binding is recorded.
    ///
    /// This is the declaration-side fact Body IR lowering consumes for `NamedCallableTarget::canonical` instead of
    /// re-deriving an identity from module path plus spelling. Span-keyed entries always carry it for local
    /// declarations (each overload keeps its own); name-keyed *imported* entries carry the declaring module's proven
    /// identity or `None` when import resolution could not prove one — absent is never permission to reconstruct.
    pub identity: Option<CanonicalSymbolId>,
}

/// Checked facts needed to retain a property as a canonical immutable getter on the direct route.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrPropertyBindingInfo {
    /// Zero-argument callable signature, excluding the implicit receiver, under the property declaration's identity.
    pub callable: FunctionBindingInfo,
    /// Declared owner frame, whose parameters close through a checked concrete receiver at a read.
    pub receiver_type: ResolvedType,
    /// Ordered binders declared by that owner, never inferred from the getter's result spelling.
    pub owner_parameters: Vec<String>,
    /// Exact implicit receiver binding created when the getter body is checked; absent for abstract requirements.
    pub receiver_identity: Option<CanonicalSymbolId>,
}

/// Typechecker-resolved binding of one `model`/`class` construction's arguments to the declared field layout, or a
/// newtype's single positional argument to tuple slot zero.
///
/// Slots index the type's declared field order. `argument_slots` is in **written source order**, so a consumer sees
/// both which field each argument fills and the order the argument expressions were written — the two facts that
/// differ whenever a caller writes fields out of declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorFieldBinding {
    /// Declared field slot filled by each written argument, in written source order.
    pub argument_slots: Vec<usize>,
    /// Declared field slots the call site omitted, ascending. Each takes its declared field default.
    pub defaulted_slots: Vec<usize>,
    /// Total declared field count, i.e. the construction's declaration-order arity.
    pub field_count: usize,
}

/// Lowering metadata for one RFC 036 decorated function binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedFunctionBindingInfo {
    /// Final type of the module-visible binding after applying all user-defined decorators.
    pub ty: ResolvedType,
    /// Original callable type before decorators are applied.
    pub original_ty: ResolvedType,
    /// Source-declared type parameters preserved for explicit and inferred call-site generic arguments.
    pub type_params: Vec<String>,
    /// Explicit source-declared bounds per type parameter.
    pub type_param_bounds: HashMap<String, Vec<String>>,
    /// Resolved source-declared bounds, preserving generic type arguments.
    pub type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>>,
    /// Whether the original declaration is async.
    pub is_async: bool,
}

/// Lowering metadata for one RFC 036 decorated method binding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecoratedMethodBindingInfo {
    /// Final unbound callable type after applying all user-defined decorators. The receiver is the first parameter.
    pub unbound_ty: ResolvedType,
    /// Original unbound callable type before decorators are applied. The receiver is the first parameter.
    pub original_unbound_ty: ResolvedType,
}
