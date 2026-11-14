//! Bodies, native attributes, checked input profiles, and body representation and generator queries.

use super::*;

/// Body IR v0 for a single function or method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body {
    /// Lowering-derived owner bindings for a concrete nominal frame; method binders retain their own scope.
    #[serde(skip)]
    pub nominal_instantiation: Vec<TraitTypeArgument>,
    /// Lowering-derived trait-default scope, kept separate from the method's own binders and never published.
    #[serde(skip)]
    pub trait_default_instantiation: Option<TraitDefaultInstantiation>,
    /// Ordered declaration type parameters, retained for checker-selected native instantiation.
    #[serde(default)]
    pub type_parameters: Vec<String>,
    /// Identity of the owning declaration, matching the [`crate::HirDeclaration::id`] this body was lowered from.
    pub decl_id: CompilerNodeId,
    /// Exact source-local identity used to dispatch a direct named Body-IR call.
    ///
    /// For a top-level function, [`Self::decl_id`] and this identity both use the declaration span and intentionally
    /// coincide. Method HIR does not yet have its own declaration node, so a method body retains the owner/name HIR
    /// compatibility identity in `decl_id` while this field keeps the collision-safe method declaration span. Both
    /// are scoped to this [`BodyIrModule`] and must never stand in for an imported callable identity.
    pub direct_call_id: CompilerNodeId,
    /// Checker-minted canonical identity of this callable declaration, when checking proved one.
    ///
    /// Consumers must fail closed when this is absent or disagrees with the physical declaration. In particular,
    /// [`Self::name`] is diagnostic source spelling and is never sufficient to select an entrypoint or child frame.
    pub canonical: Option<CanonicalSymbolId>,
    /// Source-level function/method name.
    pub name: String,
    /// Full source span of the declaration this body was lowered from.
    pub span: HirSourceSpan,
    /// Fully resolved source return type used to validate direct-execution results.
    pub return_type: IncanType,
    /// Signature representation facts retained from the checker. Older representations have no such proof;
    /// consumers must refuse callable signatures when this is absent rather than assume function-pointer coercion.
    #[serde(default)]
    pub callable_representation: Option<CallableRepresentation>,
    /// Source-owned admission for explicit inputs, independent of callable ABI classification. Missing legacy
    /// metadata does not authorize an explicit mode; consumers validate the owner and complete frame-slot inventory.
    #[serde(default)]
    pub checked_input_profile: Option<CheckedInputProfile>,
    /// Explicit local bindings this body declares, in source order. Each requires the checked profile independently
    /// of the parameter inventory, so a body whose profile is lost still exposes its explicit-input obligation.
    #[serde(default)]
    pub explicit_input_bindings: Vec<ExplicitInputBinding>,
    /// The result depends on one source place. Shared or exclusive access returns a view whose callers alias the
    /// substituted place; owned access retains the parameter loan inside an ordinary owned native result.
    #[serde(default)]
    pub returned_view: Option<ReturnedView>,
    /// Checked nominal bindings used by this body's retained type positions. Publication prunes unused bindings,
    /// rebases source-local identities, and records their public requirements; consumers never resolve the keys.
    ///
    /// Shared, not copied: a cloned body shares the map, and a writer copies it once through `Arc::make_mut`.
    #[serde(with = "shared_identities")]
    pub named_type_identities: SharedIdentities,
    /// Every local (parameter, user binding, or compiler-introduced temporary) declared in this body, in
    /// declaration order. Referenced elsewhere by [`LocalId`] index.
    pub locals: Vec<LocalDecl>,
    /// The callable contract for every parameter, in declaration order.
    ///
    /// Unlike `param_locals`, this keeps the direct-consumer binding contract: parameter identity, source span,
    /// resolved type, and whether omission requires a call-time computation, uses a construction-time partial
    /// preset, or must refuse because v0 cannot evaluate the source default. A target must consume this field rather
    /// than reconstructing defaults from AST/HIR/typechecker state.
    pub params: Vec<CallableParam>,
    /// Locals bound to this body's parameters, in parameter order.
    ///
    /// Kept as a compatibility projection for pre-#1172 consumers that only support required arguments. New
    /// consumers must use `params` so omission behavior stays visible and fail-closed.
    pub param_locals: Vec<LocalId>,
    /// Source scope tree for this body, used to map statements/locals back to lexical scopes for diagnostics.
    pub scopes: Vec<ScopeInfo>,
    /// Normalized statement sequence for the body, rooted at the function's top-level block.
    pub block: Block,
    /// Runtime/helper requirements this body imposes, deduplicated and kept in first-seen (source-order) sequence
    /// for deterministic snapshots — [`crate::types::AbiV0RuntimeRequirement`] does not derive `Ord`, so lowering
    /// relies on deterministic traversal order rather than sorting. Lets a later target profile decide
    /// hosted-only / alloc-requiring / panic-requiring / potentially freestanding-compatible without inferring
    /// facts from helper-call shapes.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    /// Panic-interaction facts recorded for this body, without committing to a stable public panic strategy.
    pub panic_facts: Vec<PanicFact>,
    /// Whether the source declaration was marked `async` (#1164).
    ///
    /// Deliberately a **stored** field, unlike the derived [`Self::is_generator`] next to it. Async-ness is a
    /// declaration-level fact: an `async def` with no `await` in its body is still async, and its caller still gets
    /// an awaitable. Deriving this by scanning for [`StatementKind::Await`] the way `is_generator` scans for
    /// [`StatementKind::Yield`] would therefore be wrong, not merely a different implementation. This is the first
    /// declaration-level fact `Body` carries; see [`Self::is_generator`]'s docs for why that one stayed derived.
    pub is_async: bool,
    /// Positional wire tombstone for the removed RFC 171 Rust-facade delegation slot.
    ///
    /// Postcard encodes struct fields by position. Keeping an always-`None` pair here lets the next compiler
    /// generation read interfaces produced from the same facade-free source by the preceding generation without
    /// retaining an extern-delegation type or behavior. No checker, lowering stage, or consumer may interpret it.
    #[doc(hidden)]
    pub retired_rust_facade_wire: Option<(String, String)>,
    /// Native attribute macros the source declaration applies, in written order (#2256).
    ///
    /// The checker selects each from a decorator that names an exported attribute macro in the providing Loaf's
    /// facts. rustc expands them on the declaration; Body IR retains only the selection and the written arguments,
    /// never a prediction of the expansion.
    ///
    /// Not part of a published body: rustc has expanded the macro when the providing unit compiled, and a consumer
    /// never re-applies it, so the executable representation of a Loaf keeps its layout.
    #[serde(skip)]
    pub native_attributes: Vec<NativeAttribute>,
}

/// The checked adoption used to specialize a default body; nominal and method instances subsequently close its types.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDefaultInstantiation {
    /// The adopter's retained receiver type, including its owner parameters before nominal instantiation.
    pub receiver_type: IncanType,
    /// Trait-scoped bindings, independent of the default method's own type parameters.
    pub arguments: Vec<TraitTypeArgument>,
}

/// One native attribute macro application: `#[path(arguments)]` on a source declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAttribute {
    /// Canonical Rust path of the exported attribute macro, such as `incan_web_macros::route`.
    pub path: String,
    /// The decorator's arguments in written order.
    pub arguments: Vec<NativeAttributeArgument>,
}

/// One argument of a native attribute application, positional when `name` is `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeAttributeArgument {
    /// The keyword the argument was written under.
    pub name: Option<String>,
    /// The argument's value.
    pub value: NativeAttributeValue,
}

/// A literal value an attribute macro receives as a Rust literal, or a list of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NativeAttributeValue {
    /// A string literal.
    Text(String),
    /// An integer literal.
    Int(i64),
    /// A floating-point literal.
    Float(f64),
    /// A boolean literal.
    Bool(bool),
    /// A bracketed list of values.
    List(Vec<NativeAttributeValue>),
}

/// The checked source body admitted by the current direct-input implementation profile. This is not inferred from
/// a borrowed native ABI: only the source ownership checker and its complete Body IR projection can publish it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedInputProfile {
    /// Source-call result types whose independence must be established for this closed instance. Each retains
    /// its original call anchor. Empty only when the source proof has no unresolved result dependency obligation.
    pub result_requirements: Vec<(HirSourceSpan, IncanType)>,
    /// Semantic profile version, distinct from the surrounding positional executable representation version.
    pub version: u32,
    /// Exact declaration that owns the checked source flow and the resulting native frame.
    pub owner: CanonicalSymbolId,
    /// Complete physical parameter-slot inventory, including ordinary slots next to explicit ones.
    pub parameters: Vec<CheckedInputParameter>,
    /// Canonical whole-owner initializer transfers required by the conditional cleanup profile. Version 1 carries
    /// no entries; a source annotation or an unrelated native Move cannot stand in for this retained authority.
    pub owned_initializers: Vec<CheckedOwnedInitializer>,
}

/// An explicit Owned initializer that transferred an existing whole owner. The source checker authenticates this
/// relationship before lowering; consumers must match one exact whole assignment and both canonical local slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedOwnedInitializer {
    /// Canonical parameter or local whose existing value was transferred, not an alias or a temporary.
    pub source: CanonicalSymbolId,
    /// Canonical Owned local receiving the transferred value.
    pub destination: CanonicalSymbolId,
    /// Full assignment-statement span, retaining expansion identity independently of the initializer expression.
    pub assignment: HirSourceSpan,
}

/// A body's result borrows one slot of `Body::params` (a method's receiver is slot 0) with shared or exclusive
/// access, either as the whole result or inside a checked aggregate. The body requires a checked profile, and so
/// does every caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnedView {
    /// Zero-based index in the owning `Body::params` inventory.
    pub slot: usize,
    /// `Shared` for a `Borrowed[T]` result, `Exclusive` for a `Reference[T]` result.
    pub access: crate::IncanInputAccess,
    /// Structural type-component path to the reference stored in the physical result. Empty means the whole result.
    #[serde(default)]
    pub component: Vec<usize>,
    /// The result payload carrying the view: `0` for the `Ok` or `Some` payload, `1` for the `Err` payload; `None` for
    /// a view that is the whole result.
    pub carried: Option<usize>,
}

/// An explicit local binding's checked input access, retained on its body independently of the checked profile.
///
/// A loan names existing storage rather than a frame slot, so the obligation is keyed by the binding's declaration
/// span. The checker identity is session-local; publication erases it as it erases other local identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplicitInputBinding {
    /// Span of the binding's name at its declaration.
    pub declaration: HirSourceSpan,
    /// Checker identity of the binding within its source compilation; absent once published.
    pub identity: Option<CanonicalSymbolId>,
    /// Checked access: `Shared` or `Exclusive` for a loan, `Owned` for an owned initializer.
    pub access: crate::IncanInputAccess,
}

/// One frame slot's checked input request. The slot is explicit so reordered or duplicated metadata refuses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedInputParameter {
    /// Zero-based index in the owning Body.params inventory.
    pub slot: usize,
    /// Checked input access, independent of mutability and native-owned collection storage.
    pub access: crate::IncanInputAccess,
}

impl Body {
    /// A resolved source callee's explicit inputs require independent caller authority, even when the caller has
    /// no explicit parameters, and a caller's own explicit local bindings require it even when the callee is
    /// ordinary. Consumers must check this relationship after canonical call resolution: a missing persisted caller
    /// profile cannot be detected from its ordinary parameter ABI alone.
    pub fn input_call_profile_gap(&self, callee: &Body) -> Option<&'static str> {
        if callee.returned_view.is_some()
            || callee
                .params
                .iter()
                .any(|parameter| !parameter.input_access.is_inferred())
        {
            if let Some(gap) = callee.input_profile_gap() {
                return Some(gap);
            }
        } else if self.explicit_input_bindings.is_empty() {
            return None;
        }
        if self.checked_input_profile.is_none() {
            Some("explicit source call lacks a checked caller body profile")
        } else {
            self.input_profile_gap()
        }
    }

    /// Store the exact owner bindings used by Incan nominal specialization, independently of method binders.
    pub fn set_nominal_instantiation(&mut self, arguments: Vec<TraitTypeArgument>) {
        self.nominal_instantiation = arguments;
    }

    /// Retain the checked adoption scope when Incan lowering specializes a trait default, without modifying its
    /// statements.
    pub fn set_trait_default_instantiation(&mut self, receiver_type: IncanType, arguments: Vec<TraitTypeArgument>) {
        self.trait_default_instantiation = Some(TraitDefaultInstantiation {
            receiver_type,
            arguments,
        });
    }

    /// Store this already-checked body in the recursive representation used by deferred nominal field defaults.
    ///
    /// This allocation preserves every retained fact unchanged and keeps Rust recursive-storage plumbing out of
    /// the Incan lowering. Consumers must still validate and specialize the body before storing it.
    pub fn into_box(self) -> Box<Self> {
        Box::new(self)
    }

    /// Return the locals in this body whose type is not [`crate::types::AbiV0Ownership::CopyOrTrivial`] and are
    /// therefore drop-relevant if a panic unwinds through this body.
    ///
    /// This is a **conservative over-approximation**: it returns every non-Copy local in the body rather than the
    /// precise set still live at a specific panic site, because computing the precise per-site set needs full
    /// control-flow dataflow that is out of scope for v0 (see the module-level docs). Callers that need a stable
    /// panic strategy must not treat this as a final drop plan; it only exposes that such locals exist.
    ///
    /// [`LocalOrigin::Receiver`] locals are excluded unconditionally, regardless of their type's Copy-ness: `self`/
    /// `mut self` is always a Rust-level reference at the emission boundary, and references have no destructor of
    /// their own to run on unwind — only the value a reference points at might, and that value is owned by the
    /// method's caller, not by this body.
    pub fn locals_requiring_unwind_drop(&self) -> Vec<LocalId> {
        self.locals
            .iter()
            .filter(|local| !matches!(local.origin, LocalOrigin::Receiver { .. }))
            .filter(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
            .map(|local| local.id)
            .collect()
    }

    /// Whether this body is a generator body: it contains at least one statement-position `yield value`
    /// ([`StatementKind::Yield`]) reachable from its top-level block.
    ///
    /// This is a **derived** fact, walked from the already-lowered statement tree, rather than a flag stored
    /// redundantly on `Body`. It does not fold in a return-type check: `Generator[T]` is declaration-level information
    /// a `Body` alone does not carry, so a caller that has the owning declaration in hand can combine the two. In
    /// practice a well-typed program's `yield` only ever appears inside a function whose declared return type is
    /// `Generator[T]` (the typechecker enforces this), so this alone is already a reliable generator signal for a
    /// `Body` in isolation.
    ///
    /// The walk recurses into every statement kind that carries a nested [`Block`] -- [`StatementKind::If`],
    /// [`StatementKind::Loop`], [`StatementKind::UnsafeRegion`], and each [`StatementKind::Race`] arm -- but does not
    /// recurse into a [`Rvalue::Closure`]'s [`ClosureBody`] or a [`Rvalue::Generator`]'s [`GeneratorBody`]. A yield
    /// nested in either value does not make the *enclosing* function body a generator, matching how the existing
    /// backend's own `body_contains_yield` walker never descends into nested closure/function literals.
    pub fn is_generator(&self) -> bool {
        block_contains_yield(&self.block)
    }

    /// Render a deterministic maintainer-facing snapshot of this body.
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();
        let async_marker = if self.is_async { " async" } else { "" };
        let _ = writeln!(
            &mut out,
            "body{async_marker} {} {} span={}..{} canonical={}",
            self.name,
            self.decl_id,
            self.span.start,
            self.span.end,
            self.canonical
                .as_ref()
                .map_or_else(|| "<unresolved>".to_string(), CanonicalSymbolId::render_compact)
        );
        for local in &self.locals {
            let _ = writeln!(&mut out, "  {}", local.render_snapshot());
        }
        if !self.params.is_empty() {
            let _ = writeln!(&mut out, "  params:");
            for param in &self.params {
                let _ = writeln!(&mut out, "    {}", param.render_snapshot());
            }
        }
        render_block(&mut out, &self.block, 1);
        if !self.runtime_requirements.is_empty() {
            let _ = writeln!(&mut out, "  runtime_requirements:");
            for req in &self.runtime_requirements {
                let _ = writeln!(&mut out, "    {}", render_runtime_requirement(req));
            }
        }
        if !self.panic_facts.is_empty() {
            let _ = writeln!(&mut out, "  panic_facts:");
            for fact in &self.panic_facts {
                let _ = writeln!(&mut out, "    {}", fact.render_snapshot());
            }
        }
        out
    }
}

impl Body {
    /// The description of the first construct Body IR could not represent in this body, or `None` when it
    /// represents all of it.
    ///
    /// A gap is an `unsupported(..)` statement or a parameter default Body IR could not source, found anywhere in the
    /// body: nested `if`/`loop`/`race` blocks, `match` arm guards and bodies, closure and generator bodies, and the
    /// statements of a deferred default computation, including closures' own parameters. This is the fact a consumer
    /// of Body IR needs before it commits to a body, and it does not depend on what any one consumer can execute.
    pub fn first_representation_gap(&self) -> Option<&str> {
        self.input_profile_gap()
            .or_else(|| params_default_representation_gap(&self.params))
            .or_else(|| statements_representation_gap(&self.block.stmts))
    }

    /// Missing or mismatched source authority cannot be replaced by parameter ABI inference. The initial profile
    /// admits synchronous source-function and method frames, including retained generic binders and representable
    /// source defaults, with a method's receiver keeping its ordinary borrowed slot; deferred and native boundaries
    /// remain staged.
    pub fn input_profile_gap(&self) -> Option<&'static str> {
        let explicit = self
            .params
            .iter()
            .any(|parameter| !parameter.input_access.is_inferred());
        let Some(profile) = &self.checked_input_profile else {
            if explicit {
                return Some("explicit input access lacks a checked body profile");
            }
            if self.returned_view.is_some() {
                return Some("result dependency lacks a checked body profile");
            }
            return (!self.explicit_input_bindings.is_empty())
                .then_some("explicit local binding lacks a checked body profile");
        };
        // Profile 1 is a straight-line body; profile 2 a body with statement-ifs, whose owned initializers record the
        // whole-owner transfers its branches may perform. Profiles 3 and 4 retain these respective shapes with
        // unresolved result obligations that closed native instantiation must discharge.
        if !matches!(profile.version, 1 | 2 | 3 | 4) || self.canonical.as_ref() != Some(&profile.owner) {
            return Some("explicit input profile has an unsupported version or different canonical owner");
        }
        if matches!(profile.version, 3 | 4) != !profile.result_requirements.is_empty() {
            return Some("generic result profile has missing or unexpected dependency requirements");
        }
        if let Some(view) = &self.returned_view {
            let source = self.params.get(view.slot);
            if view.access == crate::IncanInputAccess::Owned {
                if !view.component.is_empty()
                    || view.carried.is_some()
                    || source.is_none_or(|source| source.input_access.is_inferred())
                {
                    return Some("owned result dependency must retain one explicit parameter slot");
                }
            } else if !matches!(
                view.access,
                crate::IncanInputAccess::Shared | crate::IncanInputAccess::Exclusive
            ) || source.is_none_or(|source| source.input_access == crate::IncanInputAccess::Owned)
            {
                return Some("returned view must borrow an existing parameter slot with shared or exclusive access");
            }
            // A `Result` carries a view in either payload, an `Option` only in its one payload, a tuple in any element.
            if let Some(payload) = view.carried
                && !match (&self.return_type, self.return_type.collection()) {
                    (crate::IncanType::Tuple(elements), _) => payload < elements.len(),
                    (_, Some((CollectionTypeId::Tuple, elements))) => payload < elements.len(),
                    (_, Some((CollectionTypeId::Result, [_, _]))) => payload < 2,
                    (_, Some((CollectionTypeId::Option, [_]))) => payload == 0,
                    _ => false,
                }
            {
                return Some(
                    "carried returned view requires a payload of a Result or Option result or a tuple element",
                );
            }
            if let Some(payload) = view.carried
                && view.component.as_slice() != [payload]
            {
                return Some("carried returned view disagrees with its structural result component");
            }
        }
        if matches!(profile.version, 1 | 3) && !profile.owned_initializers.is_empty() {
            return Some("straight-line input profile cannot authorize conditional initializer cleanup");
        }
        if !matches!(
            profile.owner.kind,
            crate::SemanticSourceTargetKind::Function | crate::SemanticSourceTargetKind::Method
        ) || self.is_async
            || self.is_generator()
            || !self.native_attributes.is_empty()
        {
            return Some("explicit input profile does not admit this callable boundary");
        }
        if profile.parameters.len() != self.params.len() {
            return Some("explicit input profile does not cover every parameter");
        }
        for (slot, (retained, parameter)) in profile.parameters.iter().zip(&self.params).enumerate() {
            if retained.slot != slot || retained.access != parameter.input_access {
                return Some("explicit input profile disagrees with its parameter slot or access");
            }
            if parameter.kind != crate::IncanCallableParamKind::Normal
                || !matches!(
                    parameter.default,
                    CallableParamDefault::Required | CallableParamDefault::Source(_)
                )
                || (!parameter.input_access.is_inferred() && parameter.native_owned)
            {
                return Some("explicit input profile does not admit packed, deferred or native-owned inputs");
            }
        }
        None
    }
}

/// Refuse unimplemented explicit input access and any gap inside a parameter's deferred default computation.
pub(super) fn params_representation_gap(params: &[CallableParam]) -> Option<&str> {
    if params.iter().any(|param| !param.input_access.is_inferred()) {
        return Some("explicit input access in a nested callable lacks its own checked body profile");
    }
    params_default_representation_gap(params)
}

/// Deferred default computations retain their own representation obligations even in an admitted parent frame.
pub(super) fn params_default_representation_gap(params: &[CallableParam]) -> Option<&str> {
    params.iter().find_map(|param| match &param.default {
        CallableParamDefault::Unsupported { description, .. } => Some(description.as_str()),
        CallableParamDefault::Source(computation) => statements_representation_gap(&computation.stmts),
        CallableParamDefault::Required | CallableParamDefault::PartialPreset { .. } => None,
    })
}

/// The first gap in a statement sequence; see [`Body::first_representation_gap`].
pub(super) fn statements_representation_gap(stmts: &[Statement]) -> Option<&str> {
    stmts.iter().find_map(statement_representation_gap)
}

/// The first gap in one statement, descending into every nested block and rvalue that holds statements.
pub(super) fn statement_representation_gap(stmt: &Statement) -> Option<&str> {
    match &stmt.kind {
        StatementKind::Unsupported { description } => Some(description.as_str()),
        StatementKind::Assign { rvalue, .. } => rvalue_representation_gap(rvalue),
        StatementKind::If {
            then_block, else_block, ..
        } => statements_representation_gap(&then_block.stmts).or_else(|| {
            else_block
                .as_ref()
                .and_then(|block| statements_representation_gap(&block.stmts))
        }),
        StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => {
            statements_representation_gap(&body.stmts)
        }
        StatementKind::Race { arms, .. } => arms
            .iter()
            .find_map(|arm| statements_representation_gap(&arm.body.stmts)),
        // Listed rather than matched by a wildcard: a statement kind that gains a nested block must be walked here.
        StatementKind::Call { .. }
        | StatementKind::Drop { .. }
        | StatementKind::Break { .. }
        | StatementKind::Continue
        | StatementKind::Return { .. }
        | StatementKind::Await { .. }
        | StatementKind::Yield { .. }
        | StatementKind::Assert { .. }
        | StatementKind::Expr { .. }
        | StatementKind::TryPropagate { .. }
        | StatementKind::IterNext { .. } => None,
    }
}

/// The first gap inside an rvalue: closure and generator bodies and `match` arms carry their own statements.
pub(super) fn rvalue_representation_gap(rvalue: &Rvalue) -> Option<&str> {
    match rvalue {
        Rvalue::Closure { params, body, .. } => {
            params_representation_gap(params).or_else(|| statements_representation_gap(&body.stmts))
        }
        Rvalue::Generator { body, .. } => statements_representation_gap(&body.stmts),
        Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
            statements_representation_gap(&arm.guard_stmts).or_else(|| statements_representation_gap(&arm.body_stmts))
        }),
        // Listed rather than matched by a wildcard, for the same reason as the statement walk.
        Rvalue::Use(_)
        | Rvalue::UnaryOp(..)
        | Rvalue::BinaryOp(..)
        | Rvalue::IsInstance(_)
        | Rvalue::Aggregate(..)
        | Rvalue::Dict(_)
        | Rvalue::ValueEnumVariant(_)
        | Rvalue::FieldlessEnumVariant(_)
        | Rvalue::ResultVariant(_)
        | Rvalue::Format(_) => None,
    }
}

/// Whether any statement in `block`, or in a block nested under one of its `If`/`Loop` statements, is a
/// [`StatementKind::Yield`]. Backs [`Body::is_generator`]; see that method's docs for why this does not recurse
/// into nested closure bodies.
pub(super) fn block_contains_yield(block: &Block) -> bool {
    block.stmts.iter().any(statement_contains_yield)
}

/// Whether `stmt` is itself a [`StatementKind::Yield`], or contains one in a nested `If`/`Loop` block. See
/// [`block_contains_yield`].
pub(super) fn statement_contains_yield(stmt: &Statement) -> bool {
    match &stmt.kind {
        StatementKind::Yield { .. } => true,
        StatementKind::If {
            then_block, else_block, ..
        } => block_contains_yield(then_block) || else_block.as_ref().is_some_and(block_contains_yield),
        StatementKind::Loop { body } | StatementKind::UnsafeRegion { body, .. } => block_contains_yield(body),
        // A race arm body is a nested block like any other. A body cannot currently be both async and a generator,
        // so this is unreachable today; walking it anyway keeps the traversal total over the statement vocabulary
        // rather than silently under-reporting if that ever changes.
        StatementKind::Race { arms, .. } => arms.iter().any(|arm| block_contains_yield(&arm.body)),
        _ => false,
    }
}

/// Render one [`AbiV0RuntimeRequirement`] using a stable, deterministic spelling.
///
/// [`AbiV0RuntimeRequirement`] does not derive `Display`, so Body IR renders it locally rather than depending on
/// `{:?}` output remaining stable across unrelated changes to that enum's derive list.
pub(super) fn render_runtime_requirement(req: &AbiV0RuntimeRequirement) -> String {
    match req {
        AbiV0RuntimeRequirement::RuntimeHelper(name) => format!("runtime_helper({name})"),
        AbiV0RuntimeRequirement::HostedStd => "hosted_std".to_string(),
        AbiV0RuntimeRequirement::Allocator => "allocator".to_string(),
        AbiV0RuntimeRequirement::PanicStrategy => "panic_strategy".to_string(),
        AbiV0RuntimeRequirement::AsyncRuntime => "async_runtime".to_string(),
    }
}
