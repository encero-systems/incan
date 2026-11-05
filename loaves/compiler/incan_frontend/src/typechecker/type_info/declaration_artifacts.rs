//! Declaration artifacts, Body IR decorator and adoption facts, and checked declaration infos.

use super::*;

/// Declaration-level binding rewrites and visibility facts consumed by lowering.
#[derive(Debug, Default, Clone)]
pub struct DeclarationArtifacts {
    /// Explicit callable contracts keyed by the checked declaring identity, including refused executable declarations.
    pub ownership_contracts: BTreeMap<CanonicalSymbolId, crate::ownership_metadata::CheckedOwnershipContract>,
    /// Checked source-signature identities published before body checks or native representation selection.
    pub source_identities: BTreeMap<CanonicalSymbolId, incan_semantics_core::stable_identity::StableDeclarationId>,
    /// Body IR-only canonical trait adoptions, retaining marker protocols and ordered checked arguments.
    /// Legacy conformance, derive selection, and method tables never consume this snapshot.
    pub body_ir_trait_adoptions: HashMap<String, Vec<BodyIrCheckedTraitAdoption>>,
    /// Rust trait method slots a class fills with its own methods. The native compiler checks each implementation.
    pub native_trait_implementations: Vec<incan_semantics_core::body_ir::TraitImplementation>,
    /// Body IR-only checked local inheritance and ordered ancestor field identities; legacy class layouts stay intact.
    pub body_ir_class_inheritance: HashMap<String, BodyIrClassInheritance>,
    /// Body IR-only source trait slots adopted by checked derive bundles, keyed by the adopting declaration.
    pub body_ir_derived_methods: HashMap<String, Vec<BodyIrDerivedMethod>>,
    /// Checked payload types keyed by their source annotation span; direct lowering never resolves raw annotations.
    pub enum_payload_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Explicit and automatic native derives selected by checking for each source enum; a std.serde.json protocol
    /// selects serde's derive path, as for models.
    pub enum_derives: HashMap<String, Vec<String>>,
    /// Explicit builtin, canonical serde, and implicit native derives selected for source models, in legacy emission
    /// order, except that the builtins a `@rust.derive(...)` names follow the `@derive(...)` selections whichever
    /// decorator comes first (the order of a Rust derive list has no observable effect). Derived Display remains a
    /// semantic selection, not a Rust macro.
    pub model_derives: HashMap<String, Vec<String>>,
    /// Body IR-only native derive paths after checked RFC 024 module and trait expansion. Descriptor is omitted
    /// because its checked structural opt-in has no native macro; legacy keeps consuming `model_derives` unchanged.
    pub body_ir_model_derives: HashMap<String, Vec<String>>,
    /// Declaration spans of `@rust.extern` functions whose Rust item is an exported attribute macro, with its
    /// canonical path. Such a declaration names a macro that rustc applies to an item; it has no callable body to
    /// delegate to.
    pub native_attribute_externs: HashMap<crate::ast::Span, String>,
    /// Declaration spans of functions that apply exported attribute macros through their decorators, with each
    /// application in written order. The providing Loaf's facts select the macro; the written arguments are kept.
    pub native_attribute_applications: HashMap<crate::ast::Span, Vec<incan_semantics_core::body_ir::NativeAttribute>>,
    /// Canonical explicit native derives selected for newtypes; automatic derives remain construction facts.
    pub body_ir_newtype_derives: HashMap<String, Vec<String>>,
    /// The same native derive selection for source classes, whose automatic derives match a model's.
    pub class_derives: HashMap<String, Vec<String>>,
    /// Serde derives a source class's trait adoptions imply, directly or through a supertrait, keyed by class name.
    ///
    /// A class retains the plain native derives besides these; legacy emission forwards the same selection.
    pub class_adoption_derives: HashMap<String, Vec<String>>,
    /// Source types whose `@rust.derive(...)` decorators are completely covered by their retained native selections.
    /// Builtins and inspected exported derive macros retain their canonical targets; expansion predictions are absent.
    pub rust_native_derive_types: HashSet<String>,
    /// Active source and package nominal bindings and their canonical carrier keys, retained by the checker before
    /// lexical context is discarded. Body IR copies this proof without rebuilding a name table.
    pub named_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Active compiler-owned native nominal bindings for Body IR only. Legacy continues to consume the ordinary
    /// nominal table; a real source binding under the same spelling cannot authorize this foreign ABI.
    pub body_ir_native_type_identities: std::collections::BTreeMap<String, CanonicalSymbolId>,
    /// Checked local model field types keyed by exact field declaration span.
    pub model_field_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Checked field visibility for local models, keyed first by model name and then canonical field name.
    ///
    /// Field modifiers and the containing model's visibility are resolved by the frontend. Lowering consumes this
    /// snapshot instead of reinterpreting the source visibility contract.
    pub model_field_visibilities: HashMap<String, HashMap<String, Visibility>>,
    /// Local model fields whose checked privacy boundary is the declaring type rather than only the source module.
    pub model_type_private_fields: HashSet<(String, String)>,
    /// Checked, parent-first field layouts for local classes.
    ///
    /// Local subclasses can inherit from classes reconstructed from compiled dependency manifests. Those parents do
    /// not have consumer-side AST declarations, so lowering must consume this checked layout instead of rediscovering
    /// inherited fields from syntax.
    pub class_layouts: HashMap<String, ClassLayoutInfo>,
    /// Compiler-checked construction semantics for local newtypes.
    ///
    /// Lowering consumes this snapshot instead of rediscovering newtypes from tuple-struct shape or raw decorators.
    pub newtype_construction: HashMap<String, NewtypeConstructionInfo>,
    /// Module-local function declarations keyed by source name after annotation resolution.
    ///
    /// Lowering consumes this instead of re-lowering raw AST annotations so aliases such as `type Expr = Union[...]`
    /// do not produce a different callable surface from typechecked call sites.
    pub function_bindings: HashMap<String, FunctionBindingInfo>,
    /// Canonical identity proven for each imported binding, keyed by the local name the import introduced.
    ///
    /// [`SourceTargetInfo::module_path`] records the import path *as written*, which is not necessarily the module
    /// resolution selected: sibling-relative candidates are tried before bare ones, so a written path can name a
    /// different module that merely declares the same leaf name. A declaration identity must never be built from the
    /// written path, so the proven identity is recorded here and is simply absent when resolution did not prove one.
    /// A re-export resolves to the identity of the module that *declares* the member, never to the facade.
    pub resolved_import_identities: HashMap<String, CanonicalSymbolId>,
    /// Module-qualified type annotations the checker resolved, keyed by their dotted source spelling.
    ///
    /// `mod.Type` in type position reaches a declaration through a module binding rather than through a local type
    /// name, so no import binding carries it and lowering has no local symbol to consult (#1437). The spelling is a
    /// module-scope fact: its root is a module binding and its tail names a member of that module, so one spelling
    /// resolves to one declaration wherever it appears in the module. The checker resolves it once through the
    /// module member registry and records the proven identity here; lowering reads it to place the nominal type at
    /// its declaring module and must never rebuild that placement from the written segments. An absent entry means
    /// the checker did not prove a type and reported that at the annotation.
    pub qualified_type_references: HashMap<String, QualifiedTypeReferenceInfo>,
    /// The target of each non-generic type alias this module declares or imports, keyed by its local name.
    ///
    /// Lowering reads an imported alias's target here where the emitter cannot place the alias by its name, because
    /// another module of the crate declares a type of that name too (#1796).
    pub type_alias_targets: HashMap<String, ResolvedType>,
    /// RFC 120 identities of this module's own top-level declarations, keyed by declaration span.
    ///
    /// Exported from the symbol table's minting after checking as a compatibility view for span-keyed declaration
    /// consumers. Binding-aware consumers use [`Self::hir_bindings_by_span`], which also represents imports and
    /// aliases carrying a target's identity.
    pub declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Canonical identities of source-owned declarations below module scope, keyed by declaration span.
    ///
    /// This is a complete checked declaration inventory, including bindings that have no reads. Stable identity
    /// projection uses it so adding the first use of an existing sibling cannot change another binding's ordinal.
    pub nested_declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// RFC 120 identities of accepted source-owned member declarations, keyed by their declaration span.
    ///
    /// Fields, methods, properties, and enum variants do not occupy the module's ordinary lexical declaration map,
    /// but declaration-aware consumers still need their compiler-owned identity before any use site exists. This
    /// map is populated from the checked member registry after collision resolution; an absent entry is unproven and
    /// must never be reconstructed from an owner/name pair.
    pub member_declaration_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Checked source bindings introduced by each top-level declaration, in source binding order.
    ///
    /// This is the declaration-level HIR handoff. It preserves the local spelling separately from the canonical
    /// identity of the declaration it names, and it can represent every binding of one multi-item import without
    /// asking HIR to reinterpret import syntax. An absent identity is an explicit unproven result.
    pub hir_bindings_by_span: BTreeMap<crate::ast::Span, Vec<CheckedSourceBinding>>,
    /// Checked provider-operation declarations, keyed by their provider function's canonical identity.
    ///
    /// This is the producer-side fact that package publication persists. Body-IR lowering consumes the resulting
    /// provider-plan projection rather than re-reading a decorator or inferring an operation from a module name.
    pub provider_operations: BTreeMap<CanonicalSymbolId, ProviderOperationDeclarationInfo>,
    /// Checked RFC 104 capability declarations, keyed by their own canonical identity.
    ///
    /// The declaration is validated at its own site, so its resolved `requires` edges are already known there.
    /// Retaining them makes the authority contract inspectable as static facts: a consumer asking which capability
    /// a package declares, or what that capability itself needs, reads checked identities instead of re-resolving a
    /// dotted reference or parsing source (RFC 104, #1027).
    pub capabilities: BTreeMap<CanonicalSymbolId, CapabilityDeclarationInfo>,
    /// Module-local function declarations keyed by declaration span, preserving same-name overloads.
    pub function_bindings_by_span: HashMap<crate::ast::Span, FunctionBindingInfo>,
    /// Checked method signatures keyed by revision-local declaration span (#1121).
    ///
    /// This distinguishes same-name methods on different owners and same-owner overloads, mirroring
    /// `function_bindings_by_span`. Method declaration preparation records the resolved parameters and return type
    /// for concrete, trait-default and abstract declarations that reach signature validation. Model, class, newtype
    /// and enum methods use the same boundary. Body IR consumes these checked facts rather than re-resolving raw
    /// annotations; the record adapter supplies persistent identity separately from this source-revision handle.
    pub method_bindings_by_span: HashMap<crate::ast::Span, FunctionBindingInfo>,
    /// Body IR-only getter signatures and receiver provenance, keyed by the exact property declaration span.
    /// Legacy property metadata and callable binding tables remain unchanged.
    pub body_ir_property_bindings: HashMap<crate::ast::Span, BodyIrPropertyBindingInfo>,
    /// Overload candidates keyed by the source binding name visible in the current module.
    ///
    /// This includes declarations, imports, and aliases so call resolution, export metadata, and lowering all see one
    /// overload surface instead of rebuilding overload sets from syntax-specific paths.
    pub function_overloads: HashMap<String, Vec<FunctionOverloadInfo>>,
    /// Module-visible partial projections keyed by their source binding name.
    ///
    /// Constructor partials can be materialized in const contexts without calling the generated wrapper. Keeping the
    /// target and preset expressions here lets const-eval and lowering consume one resolved projection surface.
    pub partial_projections: HashMap<String, PartialProjectionInfo>,
    /// Same-type method aliases keyed by nominal type name (`alias -> target_method`).
    ///
    /// This includes imported type metadata so lowering can rewrite calls through aliases such as
    /// `Path.__truediv__` or `OrdinalMap.nbytes` even when the alias was declared in stdlib or a dependency module
    /// rather than the current source file.
    pub type_method_rebindings: HashMap<String, HashMap<String, String>>,
    /// RFC 036: Decorated function bindings keyed by declaration span, preserving same-name overloads.
    pub decorated_function_bindings_by_span: HashMap<crate::ast::Span, DecoratedFunctionBindingInfo>,
    /// Checked decorator applications whose sole parameter and result are the same generic type variable.
    ///
    /// This relation is captured before substitution, so native representation can follow the input callable
    /// without inferring representation identity from structurally equal result types.
    pub generic_identity_decorator_applications: HashSet<crate::ast::Span>,
    /// Validated decorator substitutions and ordered factory arguments retained exclusively for Body IR.
    /// Legacy keeps its original expression and call-site tables; this snapshot closes the factory only after its
    /// returned decorator has been checked against the decorated declaration.
    pub body_ir_decorator_applications: HashMap<crate::ast::Span, BodyIrDecoratorApplication>,
    /// Module namespace or static receivers of checked qualified decorators, retained separately from the selected
    /// member at the same synthetic span. Legacy reference and expression-type tables remain unchanged.
    pub body_ir_decorator_static_receivers: HashMap<crate::ast::Span, BodyIrDecoratorStaticReceiver>,
    /// RFC 036: Method names whose declaration was rebound through a user-defined decorator chain.
    pub decorated_method_bindings: HashMap<(String, String), DecoratedMethodBindingInfo>,
    /// Local function declarations that take a decorated method's receiver, keyed by declaration span (#1790).
    ///
    /// Source spells a method decorator's receiver the way the method does, `(Box, int) -> str`, and the checker
    /// proved each of these declarations takes it. Lowering passes the receiver to them the way the method's
    /// generated wrapper passes it.
    pub method_decorator_receiver_slots: HashMap<crate::ast::Span, MethodDecoratorReceiverSlot>,
    /// Whether each ordinary `mut` parameter of a function or method is marked, keyed by parameter span and name
    /// (#1790, #1773).
    ///
    /// A marked parameter's changes reach the caller; an unmarked one (an `int`, `float` or `bool`, also through an
    /// alias, or a Rust handle) is the function's own value. Lowering takes each `mut` parameter's passing mode from
    /// this fact, not from the parameter's IR type. The parameters of imported source modules a check collects are
    /// recorded too, for their trait defaults expanded into this module; the name keeps them apart from this module's
    /// parameters at the same offsets.
    pub mut_param_markers: HashMap<(crate::ast::Span, String), bool>,
}

/// A checked decorator application before substitution, with the bindings needed for its native instance.
#[derive(Debug, Clone)]
pub struct BodyIrDecoratorApplication {
    /// Returned decorator signature before applying the validated bindings.
    pub callable_type: ResolvedType,
    /// Generic variable bindings established by ordinary callable argument validation. A checked generic identity
    /// method application additionally retains its receiver's passing reference here, exclusively for Body IR.
    pub type_bindings: HashMap<String, ResolvedType>,
    /// Factory type arguments in the resolved declaration's parameter order, before application substitution.
    pub factory_type_args: Vec<ResolvedType>,
}

/// The checked namespace or static value a qualified decorator uses, without conflating it with the selected member.
#[derive(Debug, Clone)]
pub struct BodyIrDecoratorStaticReceiver {
    /// Source binding used for this receiver; aliases keep their canonical storage identity below.
    pub binding_name: String,
    /// Canonical checked static declaration, including its provider module.
    pub identity: CanonicalSymbolId,
    /// Static carrier before the factory result overwrites the shared synthetic expression span.
    pub ty: ResolvedType,
}

/// Checked local inheritance facts for direct execution, without changing the legacy class or symbol tables.
#[derive(Debug, Clone)]
pub struct BodyIrClassInheritance {
    /// Canonical parent edge and nearest inherited source-method implementations.
    pub inheritance: incan_semantics_core::body_ir::ClassInheritance,
    /// Parent-first physical field identities, preserving each field's declaring class.
    pub field_identities: Vec<CanonicalSymbolId>,
}

/// Checked nominal conformance before Body IR projects declaration binders and final inferred clause arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrCheckedTraitAdoption {
    /// Actual trait declaration selected by the checked binding or provider member.
    pub trait_identity: CanonicalSymbolId,
    /// Checked arguments, retaining legacy's representation until Body IR applies its binder projection.
    pub arguments: Vec<ResolvedType>,
    /// Complete checked method identities; `None` cannot authorize an empty native marker.
    pub methods: Option<Vec<CanonicalSymbolId>>,
    /// Exact stored fields selected when checking this trait's required fields against the adopter.
    pub fields: Vec<incan_semantics_core::body_ir::TraitFieldImplementation>,
    /// Physical adoption or derive clause used to select the final Body IR-only inferred argument witness.
    pub span: Span,
}

/// A checked source trait slot introduced by a derive bundle; this does not supply its executable body.
#[derive(Debug, Clone)]
pub struct BodyIrDerivedMethod {
    /// Source slot identity retained from the resolved trait's method metadata.
    pub method: CanonicalSymbolId,
    /// Whether that resolved slot supplies a default body; consumers still require its retained source body.
    pub has_default: bool,
    /// Checked adoption arguments paired with the trait's declared parameters.
    pub type_arguments: Vec<(String, ResolvedType)>,
}

/// Where a local function declaration takes a decorated method's receiver, and how the method takes it (#1790).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodDecoratorReceiverSlot {
    /// Whether the decorated method takes `mut self`, so its receiver is spelled `mut Box` in the chain.
    pub mutable: bool,
    /// Which positions of the declaration's signature hold the receiver.
    pub role: MethodDecoratorReceiverRole,
}

/// The positions of a declaration's signature that hold a decorated method's receiver (#1790).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodDecoratorReceiverRole {
    /// A decorator, `def as_int(func: (Box, int) -> str) -> (Box, int) -> int`: the first parameter of the callable
    /// type it accepts first and of the callable type it returns.
    Decorator,
    /// A decorator factory, `def logged(label: str) -> (((Box, int) -> str) -> (Box, int) -> str)`: the same two
    /// positions inside the decorator shape it returns.
    Factory,
    /// A function a decorator returns in the method's place, `def parse(box: Box, value: int) -> int`: its first
    /// parameter.
    Replacement,
}

/// One active source binding exported for declaration-level HIR lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedSourceBinding {
    /// Spelling introduced in the current module.
    pub local_name: String,
    /// Canonical declaration identity proven for this binding, if any.
    pub canonical: Option<CanonicalSymbolId>,
}

/// One provider function's checked authority requirement.
///
/// The callable and capability are both resolved identities. The source decorator never contributes a stringly
/// authority name to a backend; it only tells the typechecker which two already-resolved declarations are related.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperationDeclarationInfo {
    /// Canonical identity of the provider function this declaration annotates.
    pub operation: CanonicalSymbolId,
    /// Canonical identity of the RFC 104 capability required to invoke that operation.
    pub required_capability: CanonicalSymbolId,
    /// Compiler-owned requirements the operation's runtime implementation imposes.
    ///
    /// The first vertical currently records none. A future declaration contract may add requirements only when it
    /// has a checked source form; lowering must not infer them from a provider name or generated implementation.
    pub runtime_requirements: Vec<incan_semantics_core::AbiV0RuntimeRequirement>,
}

/// One checked RFC 104 capability declaration.
///
/// Every field is a resolved fact rather than source spelling. `requires` in particular holds the canonical
/// identities the declaration's references resolved to, not the dotted paths the author wrote: two packages can
/// spell a requirement the same way and mean different capabilities, so a consumer that compared spellings would
/// conflate them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDeclarationInfo {
    /// Canonical identity of the capability being declared.
    pub capability: CanonicalSymbolId,
    /// Prose from the `description` clause.
    pub description: Option<String>,
    /// Typed scope dimensions from the `scope:` block, in declaration order, as `(name, rendered type)`.
    pub scope: Vec<(String, String)>,
    /// Canonical identities of the capabilities this one declares it requires, in declaration order.
    ///
    /// A requirement that failed to resolve is absent: the reference already reported its own error, and recording a
    /// placeholder would let a consumer read an unproven edge as a checked one.
    pub requires: Vec<CanonicalSymbolId>,
    /// Whether the declaration is public to package consumers.
    pub is_public: bool,
}

impl DeclarationArtifacts {
    /// Return whether the `mut` parameter declared at `span` as `name` is marked, when the checker recorded it.
    pub fn mut_param_marker(&self, span: Span, name: &str) -> Option<bool> {
        self.mut_param_markers.get(&(span, name.to_string())).copied()
    }
}
