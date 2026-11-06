//! Callees, checked bridge and foreign calls, callable targets, provider operations and method targets.

use super::*;

/// Call target for a [`StatementKind::Call`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Callee {
    /// A direct named function or a locally held callable value.
    ///
    /// Full call-target resolution (which physical declaration binds through imports/traits/overloads) mirrors the
    /// typechecker/backend resolution passes and is deferred past v0; Body IR records the source-level callee
    /// spelling plus argument ownership facts, which is enough to prove the model end-to-end.
    Function(CallableTarget),
    /// A method call `receiver.method(args)`. `args[0]` in the surrounding [`StatementKind::Call`] is the receiver.
    ///
    /// Also used for compiler-synthesized collection-growth calls a comprehension desugar introduces (`push`/
    /// `insert`) that have no source-level call site of their own -- see `lower_comprehension_terminal` in
    /// `loaves/compiler/incan_frontend/src/body_ir.rs` for the synthesized case.
    Method(MethodTarget),
    /// A compiler-owned runtime/helper operation, represented explicitly instead of as a generated-Rust helper-call
    /// idiom (#653 criterion 3).
    Helper(HelperOp),
    /// An admitted provider-service operation, carrying the checked plan its execution needs (#1213).
    ///
    /// Deliberately a sibling of [`Self::Function`] rather than a flavor of it. A named function target says which
    /// declaration was selected and nothing more; a provider operation additionally carries the provider activation,
    /// the RFC 104 capability its invocation must be authorized against, and the runtime requirements its execution
    /// imposes. Folding those onto [`NamedCallableTarget`] would give every ordinary call fields that are only ever
    /// absent, and would let a consumer treat a call with no plan as an unchecked provider operation.
    ///
    /// Boxed because the plan is by far the largest payload a callee can carry — two canonical identities, the
    /// provider record, and one fact per input — while being the rarest. Inlining it would grow every
    /// [`StatementKind::Call`] in every body by that much.
    ProviderOperation(Box<ProviderOperationPlan>),
    /// A pure compiler-generated operation on a checked type, preserving its binder until instantiation.
    TypeMetadata(TypeMetadataTarget),
    /// One source-selected C symbol whose exact descriptor passed target verification.
    CheckedForeign(Box<CheckedForeignCall>),
    /// A checked compiler-managed bridge operation, with its declaration authority and result storage retained.
    CheckedBridge(Box<CheckedBridgeCall>),
}

/// The checked bridge operation selected before lowering; none is inferred from a method spelling downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckedBridgeOperation {
    /// Validate input text and create owned NUL-terminated storage.
    MakeCString,
    /// Borrow a validated input buffer's pointer for one raw call.
    CStringPointer,
    /// Copy a foreign view within an explicit byte bound and validate UTF-8.
    CopyUtf8,
    /// Allocate a fresh output position whose semantic initialized state remains unavailable.
    NewOutputSlot,
    /// Allocate an output position initialized from the caller's exact scalar value.
    NewInOutSlot,
    /// Read one output position after the checker proved its declared initialization condition.
    TakeOutputSlot,
    /// Move an owned `bytes` or `list[f32]` buffer into a typed span without copying its elements.
    MakeSpan,
    /// Borrow a span's buffer as a constant element pointer for one raw call.
    SpanConstPointer,
    /// Borrow a mutable span's buffer as a mutable element pointer for one raw call.
    SpanMutPointer,
    /// Read a span's element count, or a mutable span's capacity, as a `usize`.
    SpanCount,
    /// Keep a mutable span's first `written` elements, refusing a count outside its capacity.
    FinishSpan,
}

/// A checked foreign bridge operation, independent of source class layouts or backend helper names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedBridgeCall {
    /// The actual standard namespace or verified binding declaration authorizing this operation.
    pub authority: CanonicalSymbolId,
    /// Exact operation selected by checking and slot-state analysis.
    pub operation: CheckedBridgeOperation,
    /// The already selected native result storage, including private pointer and buffer carriers.
    pub return_type: IncanType,
}

/// An unsafe C call selected by the checker and verified against the declared target header.
/// This is a foreign ABI boundary, independent of the source declaration's class-shaped projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedForeignCall {
    /// Canonical source binding declaration that owns the symbol.
    pub binding: CanonicalSymbolId,
    /// Exact source member selected within the verified binding.
    pub member: String,
    /// Physical linker symbol, without casing or namespace reconstruction.
    pub symbol: String,
    /// Explicit system library accepted by declaration checking.
    pub library: String,
    /// Verified header retained as declaration provenance.
    pub header: String,
    /// Exact native parameter carriers in declaration order.
    pub parameters: Vec<IncanType>,
    /// Exact native result carrier, without semantic widening.
    pub return_type: IncanType,
}

/// A generated type member selected by the checker, never a same-spelled source method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeMetadataTarget {
    /// Exact generated method identity selected at the source call.
    pub member: CanonicalSymbolId,
    /// Checked type or generic binder owning the generated member.
    pub owner: CanonicalSymbolId,
    /// Type to close using the active function's ordered type arguments.
    pub receiver_type: IncanType,
    /// Pure operation retained by the frontend.
    pub operation: TypeMetadataOperation,
}

/// Metadata operations with no source-value receiver evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeMetadataOperation {
    /// The source name exposed by the concrete model or class's generated class-name member.
    ClassName,
    /// The length of the generated reflection-visible field table; no table elements escape this operation.
    FieldCount,
    /// The immutable table of reflection-visible field metadata for the concrete nominal type.
    Fields,
}

impl Callee {
    /// Render a deterministic maintainer-facing spelling for this callee.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Function(target) => target.render_snapshot(),
            Self::Method(target) => target.render_snapshot(),
            Self::Helper(op) => format!("helper:{}", op.as_str()),
            Self::ProviderOperation(plan) => plan.render_snapshot(),
            Self::TypeMetadata(target) => format!(
                "type-metadata:{}<{}>",
                match target.operation {
                    TypeMetadataOperation::ClassName => "class_name",
                    TypeMetadataOperation::FieldCount => "field_count",
                    TypeMetadataOperation::Fields => "fields",
                },
                target.receiver_type
            ),
            Self::CheckedForeign(target) => format!("checked-c:{}::{}", target.binding.declaration_name, target.member),
            Self::CheckedBridge(target) => format!("checked-c-bridge:{:?}", target.operation),
        }
    }
}

/// The source-authoritative target carried by [`Callee::Function`].
///
/// The outer `Callee::Function` spelling is retained as the established call statement category: both alternatives
/// ultimately invoke a callable. The alternatives themselves are intentionally distinct. In particular, a stored
/// closure/partial is never represented as [`Self::Named`] with a fabricated source name; it is a [`Self::Local`]
/// operand, carrying the lexical-environment ownership decision the caller made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CallableTarget {
    /// A direct call to a named Incan function.
    ///
    /// The callee's source-level spelling is always present. Which physical declaration that spelling binds is
    /// carried separately and only where it is proven: [`NamedCallableTarget::canonical`] for the declaration
    /// identity, [`NamedCallableTarget::direct_call_id`] for a same-module span identity. Resolution through traits,
    /// and through overloads whose candidates a name cannot separate, remains deferred past v0, so both facts are
    /// absent for those and the spelling alone is never a dispatch key.
    Named(NamedCallableTarget),
    /// Invoke a callable value held in one local place.
    ///
    /// The [`PlaceOperand`] records the read's Duckborrower ownership fact and last-use marker exactly once, at the
    /// call target. Frontend lowering emits only an unprojected local place here: field/index call targets remain
    /// unsupported until Body IR has an equally explicit representation for their receiver/projection evaluation.
    /// Consumers must preserve this operand's ownership decision when invoking the value, because it can own a
    /// closure's lexical environment.
    Local(LocalCallableTarget),
}

/// One arm of a [`StatementKind::Race`].
///
/// The source spells a single binding declaration shared by every arm (`race for value:`). Each arm owns a distinct
/// type-refined `binding` local because arm result types may differ, while those locals retain the same canonical
/// source identity and exact header token span. The body is a full [`Block`] plus a `result` operand, mirroring how
/// [`ClosureBody`] carries a nested statement sequence with an explicit value instead of relying on a
/// trailing-statement convention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaceArm {
    /// This arm's awaitable, evaluated before selection along with every other arm's.
    pub awaitable: Operand,
    /// The local this arm binds the winning value to, visible only inside `body`.
    pub binding: LocalId,
    /// Statements that run only if this arm wins.
    pub body: Block,
    /// This arm's result value, written to the race's destination when it wins.
    pub result: Operand,
}

impl RaceArm {
    /// Render this arm's header and result. Its block is rendered separately by the statement renderer, so nested
    /// control flow inside an arm keeps the same line-oriented, indented shape it has anywhere else.
    pub(super) fn render_header(&self) -> String {
        format!("await {} => _{}:", self.awaitable.render_snapshot(), self.binding.0)
    }
}

/// A locally stored callable target and the resolved binding of its call arguments.
///
/// The binding makes a local partial's two source-level call conventions explicit: positional arguments skip
/// preset-default slots, while named arguments may override them. Callers that lower an ordinary local closure use
/// [`ArgumentBinding::positional`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalCallableTarget {
    /// The local read that owns the callable value and its lexical environment.
    pub operand: PlaceOperand,
    /// Resolved binding of the surrounding [`StatementKind::Call::args`] to this callable's declared parameters.
    pub binding: ArgumentBinding,
}

/// A direct call to a named function, with its resolved call-site identity.
///
/// Explicit call-site type arguments are part of that identity rather than decoration: `decode_rows[Order](path)`
/// and `decode_rows[Row](path)` are different calls, and a consumer that only saw the name could not tell them
/// apart. `type_args` holds the *typechecker-resolved* arguments, so lowering never re-derives them from the source
/// spelling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedCallableTarget {
    /// Source-level function name.
    pub name: String,
    /// Exact same-module declaration selected for this call, when Body IR can retain one.
    ///
    /// `None` is never permission for an executor to guess an imported, unresolved, or otherwise non-local target
    /// from [`Self::name`]. A compiler-owned callable uses [`Self::builtin`] instead.
    pub direct_call_id: Option<CompilerNodeId>,
    /// Compiler-recognized builtin target, if the typechecker proved this call did not resolve to a source binding.
    ///
    /// This is `None` for every same-module, imported, unresolved, or otherwise external target. Consumers must
    /// reject an absent `direct_call_id` and absent `builtin` rather than using [`Self::name`] as a dispatch key —
    /// or resolve [`Self::canonical`] through [`BodyIrModule::body_for_canonical_target`], which is the only route
    /// that also reaches a declaration this module does not own.
    pub builtin: Option<BuiltinFnId>,
    /// Resolved explicit call-site type arguments, in declared type-parameter order. Empty when the call site wrote
    /// none.
    pub type_args: Vec<IncanType>,
    /// Resolved binding of the surrounding [`StatementKind::Call::args`] to this function's declared parameters.
    pub binding: ArgumentBinding,
    /// RFC 120 canonical identity of the declaration this call selected, independent of the call site's spelling.
    ///
    /// [`Self::direct_call_id`] is a *span* identity and so exists only for a declaration physically present in this
    /// module. This answers the different question of *which declaration* was selected, in a form that survives an
    /// import, an alias, or a re-export, and is `None` whenever that answer is not proven. [`Self::name`] remains the
    /// call site's own spelling, so the pair records both what was written and what it means.
    pub canonical: Option<CanonicalSymbolId>,
    /// The type an associated function is called on: `Point` in `Point.default()`, `T` in `T.default()`.
    ///
    /// `None` for an ordinary function call. When [`Self::canonical`] is a trait's method, this is what selects the
    /// implementation, as `<T as Default>::default()` does in Rust; for a type's own static method it repeats the
    /// owner.
    pub receiver_type: Option<IncanType>,
}

/// Whether the provider owning an operation can be executed against in this compilation.
///
/// The three states are the ones the checked provider catalog already distinguishes, kept apart here for the same
/// reason it keeps them apart: they have different remedies. A disabled provider is present but not selected by the
/// project graph, while an unavailable one is selected but has no locally verified artifact. Collapsing them into a
/// single "not usable" flag would leave a consumer unable to say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderActivationState {
    /// Enabled and locally available, so an admitted operation may be planned for execution.
    Active,
    /// Known to the catalog but not enabled by the project or component selection.
    Disabled,
    /// Enabled, but no locally verified artifact backs it.
    Unavailable,
}

impl ProviderActivationState {
    /// Compact maintainer-facing spelling used in snapshots and refusal text.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The checked provider an operation belongs to, and whether this compilation can execute against it.
///
/// [`Self::provider_key`] is carried as provenance, never as a dispatch key. Nothing in Body IR or in a consumer may
/// branch on it: which operation was invoked is answered by [`ProviderOperationPlan::operation`], a canonical
/// identity, and whether it may run is answered by [`Self::state`]. Recording the key anyway is what lets a receipt
/// or a diagnostic say *which* provider without re-resolving the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderActivation {
    /// Stable key of the catalog record this operation was admitted from.
    pub provider_key: String,
    /// Canonical module path the operation is declared under, as the catalog claims it.
    pub module_path: Vec<String>,
    /// Whether this compilation can execute against the provider.
    pub state: ProviderActivationState,
}

impl ProviderActivation {
    /// Render a deterministic maintainer-facing spelling.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "{}@{}:{}",
            self.provider_key,
            self.module_path.join("."),
            self.state.as_str()
        )
    }
}

/// One provider-operation input, described alongside its runtime call operand rather than re-carried.
///
/// The evaluated *values* stay in the surrounding [`StatementKind::Call::args`], exactly as they do for every other
/// call shape, because an [`Operand`] carries an ownership decision and a last-use marker that must be recorded once
/// and only once. Duplicating the operand into the plan would record both twice. What the plan adds is the facts a
/// consumer cannot recover from the operand alone: which declared slot the value fills, where it was written (and so
/// when it was evaluated relative to its siblings), its checked type, and its own source span — which is what lets an
/// authority denial point at the argument that carried an out-of-scope value rather than at the whole call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderOperationInput {
    /// Declared parameter slot this input supplies. Indexes the same slot space as [`BoundArgument::slot`].
    pub slot: usize,
    /// Zero-based position among the call site's written arguments, which is also its evaluation order.
    pub written_position: usize,
    /// Checked type of the evaluated argument expression.
    pub ty: IncanType,
    /// Span of the argument expression itself.
    pub span: HirSourceSpan,
}

impl ProviderOperationInput {
    /// Render a deterministic maintainer-facing spelling.
    pub(super) fn render_snapshot(&self) -> String {
        format!("slot{}@{}:{}", self.slot, self.written_position, self.ty)
    }
}

/// A checked execution plan for one resolved provider-operation invocation (#1213).
///
/// This is a compiler-owned internal plan, not a public provider API and not a second source-meaning model. Every
/// fact on it is *consumed* from an upstream owner rather than decided here: the canonical operation identity comes
/// from the RFC 120 callable-target contract, the provider and its activation come from the checked provider catalog,
/// and [`Self::required_capability`] is an RFC 104 capability declaration's own canonical identity. The plan
/// deliberately holds no grant set, no mode, no decision, and no receipt — obtaining an authority decision is the
/// consumer's call through [`Self::authority_request`], and executing the operation belongs to whoever holds a
/// runtime.
///
/// Lowering emits a plan only for an *admitted* operation. An operation whose provider is not active, whose required
/// capability identity does not name a capability, or whose inputs cannot all be represented is refused at its
/// source span instead, so it has no plan to execute and no receipt to emit. Consumers must still validate a plan:
/// constructed or corrupt IR is never evidence that source admission occurred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderOperationPlan {
    /// RFC 120 canonical identity of the operation declaration this call selected.
    ///
    /// This is the only thing that answers "which operation is this". A consumer must not recover that answer from a
    /// call-site spelling, a provider name, or an emitted Rust name.
    pub operation: CanonicalSymbolId,
    /// The checked provider that owns the operation, and its activation in this compilation.
    pub provider: ProviderActivation,
    /// Canonical identity of the RFC 104 capability this invocation's authority must be decided against.
    ///
    /// A capability identity, not a grant spelling: the spelling is a rendering of the identity (see
    /// [`Self::authority_request`]), and only the identity survives an import, an alias, or a re-export.
    pub required_capability: CanonicalSymbolId,
    /// Runtime requirements executing this operation imposes, in the catalog's own order.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    /// Runtime call inputs, one per declared slot, in written source order.
    pub inputs: Vec<ProviderOperationInput>,
    /// Span of the invocation, which is where a refusal or a denial is reported.
    pub call_span: HirSourceSpan,
}

impl ProviderOperationPlan {
    /// Build the RFC 104 authority request this invocation must have decided before it may execute.
    ///
    /// This is the whole of the plan's relationship with authority: it phrases the question and hands it to whatever
    /// implements [`crate::authority::AuthorityDecisionSource`]. It does not consult a grant set, interpret a mode, or
    /// decide anything, so a governed host, a permissive local run, and a test double all answer the same request.
    ///
    /// `requested_scope` is empty. RFC 104 binds scope *values* at grant time and checks them against the operation's
    /// actual attributes at the moment it happens, so a request assembled during lowering has no scope values to
    /// carry; a consumer that has evaluated [`Self::inputs`] may add them to the returned request.
    pub fn authority_request(&self) -> crate::authority::AuthorityRequest {
        crate::authority::AuthorityRequest {
            capability: self.required_capability.clone(),
            operation: self.operation.clone(),
            request_span: self.call_span,
            requested_scope: Vec::new(),
            suggested_grant: render_symbol_path(&self.required_capability),
        }
    }

    /// Render a deterministic maintainer-facing spelling.
    pub(super) fn render_snapshot(&self) -> String {
        let inputs: Vec<String> = self
            .inputs
            .iter()
            .map(ProviderOperationInput::render_snapshot)
            .collect();
        let requirements: Vec<String> = self
            .runtime_requirements
            .iter()
            .map(render_runtime_requirement)
            .collect();
        format!(
            "provider_operation:{} provider={} capability={} inputs=[{}] requires=[{}]",
            render_symbol_path(&self.operation),
            self.provider.render_snapshot(),
            render_symbol_path(&self.required_capability),
            inputs.join(", "),
            requirements.join(", ")
        )
    }
}

/// Render a canonical identity as its dotted declaration path.
///
/// This is a *projection of* the identity for humans and for RFC 104's suggested-grant spelling, never a substitute
/// for it: two declarations can share a rendering while remaining distinct identities, so nothing may compare or
/// dispatch on the result. The origin is included because a capability's grant spelling is its declaration location —
/// `host.http.request` is the `request` capability declared in the `host.http` module, not a string an author chose.
pub(super) fn render_symbol_path(symbol: &CanonicalSymbolId) -> String {
    let prefix: Vec<String> = match &symbol.origin {
        crate::SymbolOrigin::Module(path) | crate::SymbolOrigin::RustCrate(path) => path.clone(),
        crate::SymbolOrigin::Package { library, module_path } => std::iter::once(library.clone())
            .chain(module_path.iter().cloned())
            .collect(),
        crate::SymbolOrigin::Builtin => Vec::new(),
    };
    prefix
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(symbol.declaration_name.as_str()))
        .collect::<Vec<_>>()
        .join(".")
}

/// A method call target and its resolved call-site identity.
///
/// Carries the same resolved type arguments and argument binding as [`NamedCallableTarget`]; the receiver itself
/// stays `args[0]` of the surrounding [`StatementKind::Call`] and is deliberately *not* part of the binding, whose
/// slots index the method's own declared parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodTarget {
    /// Source-level method name.
    pub name: String,
    /// Checked compiler-owned Option/Result receiver for intrinsic carrier methods; source methods retain no carrier.
    #[serde(default)]
    pub intrinsic_carrier: Option<String>,
    /// Compiler-generated field reflection selected through the checker's generated-member ownership table.
    /// Source methods and older publications carry None, so a spelling alone never grants reflection behavior.
    #[serde(default)]
    pub reflection: Option<ReflectionOperation>,
    /// Exact Loaf trait selected by checking, including its trait arguments. The receiver layout remains in args[0].
    /// Calls without checked trait dispatch into a Loaf retain None.
    #[serde(default)]
    pub loaf_trait: Option<LoafTraitCall>,
    /// Source trait arguments selected by checking, distinct from method binders and native delegation metadata.
    /// An absent fact cannot distinguish multiple adoptions of the same slot.
    #[serde(default)]
    pub trait_arguments: Option<Vec<IncanType>>,
    /// Canonical method declaration or compiler-owned member selected by typechecking.
    ///
    /// Compiler-synthesized calls without a source resolution site carry `None`; consumers must keep those on an
    /// explicitly internal path and may not use [`Self::name`] to grant source method behavior.
    pub canonical: Option<CanonicalSymbolId>,
    /// Resolved explicit call-site type arguments, in declared type-parameter order. Empty when the call site wrote
    /// none.
    pub type_args: Vec<IncanType>,
    /// Loaf trait item and native instantiation selected at this exact checked call site. Declaration records alone
    /// cannot distinguish two calls of one generic trait slot. A source implementation keeps dispatch priority;
    /// consumers validate the entire recorded native signature before using this selection. Boxed so ordinary method
    /// calls do not reserve the full native signature when no delegation is selected.
    #[serde(default)]
    pub native_trait_delegation: Option<Box<LoafCallable>>,
    /// Resolved binding of the surrounding call's arguments *after* the receiver to this method's declared
    /// parameters.
    pub binding: ArgumentBinding,
}

impl MethodTarget {
    /// Authenticate generic metadata against its checked binder without treating a method spelling as authority.
    /// Publication additionally proves that the binder belongs to its enclosing callable before rebasing either
    /// identity; native lowering checks the concrete receiver's retained reflection capability.
    pub fn has_valid_generic_metadata(&self) -> bool {
        let Some(ReflectionOperation::GenericMetadata(owner)) = &self.reflection else {
            return false;
        };
        let Some(member) = &self.canonical else {
            return false;
        };
        owner.kind == crate::SemanticSourceTargetKind::GenericBinder
            && owner.namespace == crate::SymbolNamespace::OrdinaryLexical
            && owner.scope_discriminant.is_some()
            && owner.declaration_span.start < owner.declaration_span.end
            && member.kind == crate::SemanticSourceTargetKind::Method
            && member.namespace == crate::SymbolNamespace::Member
            && member.declaration_name == "__fields__"
            && member.origin == owner.origin
            && member.scope_discriminant == owner.scope_discriminant
            && member.declaration_span == owner.declaration_span
            && self.type_args.is_empty()
            && self.intrinsic_carrier.is_none()
            && self.loaf_trait.is_none()
            && self.trait_arguments.is_none()
            && self.native_trait_delegation.is_none()
    }

    /// Validate the selected SDK signature's relation to this call without granting native execution. The driver
    /// still authenticates the item, receiver implementation and fully instantiated signature against SDK metadata.
    pub fn has_valid_native_trait_selection(&self) -> bool {
        let Some(selection) = &self.native_trait_delegation else {
            return true;
        };
        let self_type = IncanType::TypeVar("Self".to_owned());
        self.canonical.as_ref() == Some(&selection.canonical)
            && selection.type_parameters == ["Self"]
            && selection.native_type_arguments.first() == Some(&self_type)
            && matches!(selection.parameters.first(), Some(IncanType::Ref(inner) | IncanType::RefMut(inner)) if inner.as_ref() == &self_type)
            && selection.parameter_defaults.is_empty()
            && selection.rust_path().is_some()
    }

    /// Build a method target for a compiler-synthesized positional call with no resolved declared signature.
    ///
    /// Used for calls a desugar introduces (a comprehension's `push`/`insert`, an iteration protocol's `__iter__`)
    /// that have no source-level call site and whose declared parameters this stage never resolved, so the binding
    /// is [`ArgumentBinding::UnresolvedPositional`] rather than a fabricated identity slot map.
    pub fn synthesized(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            intrinsic_carrier: None,
            reflection: None,
            loaf_trait: None,
            trait_arguments: None,
            canonical: None,
            type_args: Vec::new(),
            native_trait_delegation: None,
            binding: ArgumentBinding::UnresolvedPositional,
        }
    }

    /// Render this method target without losing its resolved type arguments or non-trivial argument binding.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "method:{}{}{}",
            self.name,
            render_type_arguments(&self.type_args),
            self.binding.render_snapshot()
        )
    }
}

/// A checked trait slot a Loaf declares, separate from the method's own type arguments and the concrete receiver frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoafTraitCall {
    /// Declaring trait identity, authenticated against the exact method declaration.
    pub trait_identity: CanonicalSymbolId,
    /// Compiled Loaf item path for that declaration, never inferred by the lowering from a method spelling.
    pub native_item: String,
    /// Checked trait instantiation in declaration order; Self comes from the concrete nominal frame or checked scalar
    /// receiver.
    pub type_arguments: Vec<IncanType>,
    /// Checked explicit receiver before physical lowering, preserving lexical binders and decimal precision and scale.
    /// Nominal hooks and older publications retain None; consumers must not infer a scalar Self from its physical
    /// carrier.
    #[serde(default)]
    pub receiver_type: Option<IncanType>,
}

/// Checked reflection operation and its receiver-dependent result contract, retained before generic substitution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReflectionOperation {
    /// Return the declaring nominal's immutable metadata without consuming its receiver.
    Metadata,
    /// Lookup clones the concrete field into its checked typed overlay.
    TypedValue,
    /// Items clone concrete fields into checked typed tuples in declaration order.
    TypedItems,
    /// Lookup through a type parameter stringifies an eligible scalar field.
    StringValue,
    /// Items through a type parameter stringify eligible scalar fields in declaration order.
    StringItems,
    /// Return immutable metadata after substituting this checked generic binder with its reflectable declaration.
    GenericMetadata(CanonicalSymbolId),
}

impl PartialEq<str> for MethodTarget {
    /// Compare against a bare method name, ignoring resolved type arguments and argument binding.
    ///
    /// This keeps existing bounded consumers' direct method-name checks working without each having to reach into
    /// the target's fields, mirroring [`CallableTarget`]'s own compatibility implementation.
    fn eq(&self, other: &str) -> bool {
        self.name == other
    }
}

impl std::fmt::Display for MethodTarget {
    /// Render the bare method name for concise diagnostics labels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A nominal `model`/`class` construction target and the resolved binding of its field arguments.
///
/// Source-level construction is named-only, so the binding is what records which declared field each operand fills.
/// The surrounding [`Rvalue::Aggregate`] operand vector is in declared field order; the binding carries the written
/// source order alongside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstructorTarget {
    /// Source-level nominal type name.
    pub name: String,
    /// Canonical nominal declaration selected by typechecking.
    ///
    /// This is the semantic target. [`Self::direct_declaration_id`] is the physical same-module representation and
    /// must agree with it before execution; neither may be reconstructed from [`Self::name`].
    pub canonical: Option<CanonicalSymbolId>,
    /// Exact source-local nominal declaration selected for this construction, when the module retained one.
    ///
    /// An absent identity is not permission to look up [`Self::name`] in another compiler structure: imports,
    /// aliases, generic nominals, and any unretained nominal target must refuse at the constructor span.
    pub direct_declaration_id: Option<CompilerNodeId>,
    /// Canonical declared field names retained with the exact source-local declaration selected for this call.
    ///
    /// This is present only beside [`Self::direct_declaration_id`] and comes from the same checked Body-IR
    /// declaration snapshot. Consumers compare it with the module declaration before binding operands, so a
    /// malformed declaration cannot shift values merely by changing field order or names after lowering.
    pub canonical_field_layout: Option<Vec<String>>,
    /// Resolved binding of the surrounding aggregate's operands to the type's declared fields.
    pub binding: ArgumentBinding,
}

impl CallableTarget {
    /// Render this callable target without collapsing local values into named functions.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Named(target) => format!(
                "fn:{}{}{}{}",
                target.name,
                render_type_arguments(&target.type_args),
                target
                    .receiver_type
                    .as_ref()
                    .map_or_else(String::new, |ty| format!(" on {ty}")),
                target.binding.render_snapshot()
            ),
            Self::Local(target) => format!(
                "local:{}{}",
                Operand::Place(target.operand.clone()).render_snapshot(),
                target.binding.render_snapshot()
            ),
        }
    }
}

impl PartialEq<str> for CallableTarget {
    /// A local callable can never compare equal to a direct function name. This compatibility implementation keeps
    /// existing bounded consumers' direct-function checks conservative while they visibly reject `Local`.
    fn eq(&self, other: &str) -> bool {
        matches!(self, Self::Named(target) if target.name == other)
    }
}

impl std::fmt::Display for CallableTarget {
    /// Render a concise diagnostics label without exposing a local callable as a function name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(target) => f.write_str(&target.name),
            Self::Local(target) => write!(
                f,
                "<local:{}>",
                Operand::Place(target.operand.clone()).render_snapshot()
            ),
        }
    }
}
