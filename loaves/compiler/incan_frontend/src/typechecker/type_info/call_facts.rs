//! Call, protocol, operator and method dispatch facts, and identifier kinds.

use super::*;

/// Call-site semantic decisions selected by the typechecker.
#[derive(Debug, Default, Clone)]
pub struct CallArtifacts {
    /// Body IR-only layout requirements for checked constructor field arguments, keyed by the written field span.
    /// Each pair retains the selected field and constructor owner; navigation keeps the field identity while
    /// executable dependency selection loads its checked layout instead of treating private storage as a callable.
    pub body_ir_constructor_field_requirements: HashMap<crate::ast::Span, (CanonicalSymbolId, CanonicalSymbolId)>,
    /// Validated fixed scalar calls through source re-exports ending at a checked public library declaration.
    /// Only Body IR consumes their target and slots; legacy retains its unresolved module placeholder.
    pub body_ir_reexport_calls: HashMap<crate::ast::Span, BodyIrReexportCallInfo>,
    /// Primitive string slice members selected from checked receiver and int bounds. Only Body IR consumes these
    /// identities; legacy retains its unknown method result and ordinary reference tables.
    pub body_ir_string_slice_members: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Body IR-only calls whose selected instance method declares an exact Self result, keyed by call span.
    /// The retained canonical method prevents a speculative or subsequently replaced overload from authorizing
    /// receiver-carrier propagation; legacy keeps its existing substituted expression result unchanged.
    pub body_ir_self_result_methods: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Validated async function and method calls, keyed by complete call span and selected canonical callable. Body IR
    /// uses their eventual output as a future argument; legacy expression snapshots retain the eventual output alone.
    pub body_ir_async_call_targets: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Body IR-only tuple constructor facts selected by an active std.web surface binding. Legacy keeps its existing
    /// construction table; the canonical owner is checked from the original Rust re-export at this call site.
    pub body_ir_native_constructor_bindings: HashMap<crate::ast::Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
    /// Complete default-only class bindings checked through a published module's selected type declaration.
    /// The canonical owner authenticates this private fallback; legacy's constructor slot table stays unchanged.
    pub body_ir_module_constructor_bindings: HashMap<crate::ast::Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
    /// Native surface payload types missing from legacy's expression snapshot. An already checked String symbol or
    /// literal authorizes Html's fixed String slot; legacy expression typing remains unchanged.
    pub body_ir_native_argument_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Checked variable identities accompanying native payload types when legacy did not visit the argument. This
    /// selects the existing parameter/local, never a same-spelled external or a new semantic binding.
    pub body_ir_native_argument_identities: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Complete argument types for Body IR native instances, normalized from checked call relationships only.
    ///
    /// Concrete constructor carriers and ordinary checked arguments share this ordered fact. Legacy never reads this
    /// additive table: its own contextual types and monomorph decisions remain untouched.
    pub body_ir_call_type_arguments: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Closed call-site parameters under the same checked instantiation, preserving declaration roles and defaults.
    /// Only Body IR reads this contract; legacy parameter snapshots retain their original inferred holes.
    pub body_ir_call_parameter_types: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Checked static factory constraints that conflict with their destination. Only Body IR reads this denial;
    /// later supplemental value propagation cannot authorize a closed result at the same call span.
    pub body_ir_factory_result_conflicts: HashSet<crate::ast::Span>,
    /// The exact nominal owner closed by a checked static factory, independent of wrappers in its return type.
    pub body_ir_factory_owner_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable constructor carriers recovered from original declared fields and checked values after contextual
    /// checking substituted a callee's open parameters into those fields. Only Body IR reads this additive fact.
    pub body_ir_constructor_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable call results under the same retained argument binding, consumed only by Body IR.
    pub body_ir_call_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Selected frozen codec members whose legacy registry omits an identity. This independent Body IR fact is
    /// recorded only at the existing checked builtin branch; legacy reference and method tables stay unchanged.
    pub body_ir_native_codec_members: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Concrete owner types inferred from checked nominal constructor fields before an open caller context masks them.
    ///
    /// Body IR retains these executable carriers while legacy checking preserves its contextual expression type.
    pub inferred_constructor_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Executable result types closed by a checked constructor argument at an inferred generic call.
    ///
    /// These accompany the retained call instantiation; legacy contextual expression types remain unchanged.
    pub inferred_call_result_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Method calls, by full call span, whose receiver's source type declares the method only with `mut self`.
    ///
    /// Lowering marks the binding at the root of such a call's receiver as borrowed mutably, so every later question
    /// about whether a body changes that binding, such as whether a `for` loop must reach its items in place, sees the
    /// call wherever it sits in the body (#1561). A method that trait dispatch selects carries its receiver in the
    /// dispatch fact instead.
    pub mutable_receiver_method_calls: HashSet<crate::ast::Span>,
    /// Compiler-owned builtin selected for a call, keyed by the full call span.
    ///
    /// This distinguishes an explicit `std.builtins.name(...)` or unshadowed ambient builtin from a source/import
    /// declaration with the same spelling without asking lowering to reconstruct name resolution.
    pub resolved_builtin_calls: HashMap<crate::ast::Span, BuiltinFnId>,
    /// Compiler-owned builtin method selected for a call, keyed by the full call span.
    ///
    /// Canonical identity remains declaration provenance. This operation identity is the closed semantic key used by
    /// Body IR, ownership projection, and native lowering without reclassifying a declaration-name string.
    pub resolved_builtin_method_calls: HashMap<crate::ast::Span, BuiltinMethodId>,
    /// RFC 038: unpack operands whose static shape has been proven by call binding.
    ///
    /// Lowering consumes these plans to rewrite fixed/static unpack operands into ordinary IR call arguments. This
    /// keeps backend emission from re-deriving the frontend's binding decision from raw IR shape.
    pub fixed_unpack_plans: HashMap<crate::ast::Span, FixedUnpackPlan>,
    /// Map a full call expression span to its checked type arguments in declaration order.
    ///
    /// Functions retain both inferred and explicit bindings; methods retain explicit bindings (RFC 054). Body IR
    /// consumes these decisions for native instantiation. Legacy emission consumes them only for written `[...]`,
    /// preserving Rust inference on other calls and ensuring `_` never reaches codegen as `IrType::Unknown`.
    ///
    /// ## Span stability
    ///
    /// Checker and Body IR retain the same source range and expansion context. Generated calls at one original
    /// location therefore keep independent bindings without changing diagnostic coordinates.
    pub call_site_monomorph_type_args: HashMap<crate::ast::Span, Vec<ResolvedType>>,
    /// Checked target facts for compiler-owned `isinstance(value, Target)` calls, keyed by the full call span.
    ///
    /// `Target` is source syntax for a type rather than a runtime argument. Retaining the resolved type, optional
    /// declaration identity, and target-local span here lets Body IR represent the test without reparsing a name or
    /// asking a runtime to materialize arbitrary type values.
    pub isinstance_targets: HashMap<crate::ast::Span, IsInstanceTargetInfo>,
    /// RFC 038: Rest-aware callable signatures keyed by full call expression span.
    ///
    /// Function-value calls can recover this from the callee expression type, but method calls need a snapshot because
    /// lowering does not retain the frontend method table.
    pub call_site_callable_params: HashMap<crate::ast::Span, Vec<CallableParam>>,
    /// Resolved winner-binding type for one `race for` arm, keyed by that arm's awaitable expression span (#1164).
    ///
    /// A race arm binds the shared `race for value:` name to the *awaited output* type, not to the awaitable's own
    /// type: `Awaitable[T]` binds `T`, and `JoinHandle[T]` binds `Result[T, TaskJoinError]`. Only the typechecker
    /// performs that unwrapping (`await_output_type`). The shared header span identifies the source binding, while
    /// the awaitable span distinguishes each arm's refined type, so that type is recorded here instead of re-derived.
    pub race_arm_binding_types: HashMap<crate::ast::Span, ResolvedType>,
    /// Resolved `model`/`class` construction field binding, keyed by full call expression span (#1158).
    ///
    /// Source-level construction is named-only, so the constructor check is the stage that decides which declared
    /// field each written argument fills — including resolving a field alias to its canonical name. Recording that
    /// decision here is what lets Body IR lowering represent construction faithfully: declared field order lives in
    /// the symbol table (`ModelInfo::field_order`/`ClassInfo::field_order`), which lowering deliberately cannot
    /// reach, and re-deriving the binding from AST spelling would duplicate alias resolution in a second place.
    pub constructor_field_bindings: HashMap<crate::ast::Span, ConstructorFieldBinding>,
    /// RFC 028: User-defined operator dispatch resolved by the typechecker.
    ///
    /// Lowering consumes this map so `a + b`, `-a`, and `a[b]` can become direct dunder method calls without
    /// re-running backend-side infix/index semantics. Primitive operators are intentionally absent from this map.
    pub resolved_operator_calls: HashMap<crate::ast::Span, ResolvedOperatorCall>,
    /// Trait-backed method dispatch selected by overload resolution.
    ///
    /// Lowering consumes this for calls whose selected method lives in a trait impl rather than an inherent Rust impl.
    /// This keeps codegen from re-deriving dispatch from method names or argument shapes.
    pub resolved_method_calls: HashMap<crate::ast::Span, ResolvedMethodCall>,
    /// Compiler-generated member identities observed at checked call sites.
    ///
    /// These helpers retain owner-discriminated semantic identities for tooling, but they are not source declarations
    /// and therefore must not receive an RFC 120 recoverable source-symbol projection during lowering. Each entry
    /// retains its canonical nominal owner as the context dependency used by executable admission.
    pub compiler_generated_member_identities: HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    /// Collection constructors selected from the canonical collection vocabulary.
    ///
    /// Lowering consumes this decision instead of interpreting a source spelling such as `set(...)` as an ordinary
    /// function call or independently guessing whether a same-named binding shadows the collection constructor.
    pub resolved_collection_constructors: HashMap<crate::ast::Span, CollectionTypeId>,
    /// Selected runtime-string helper methods, keyed by full call expression span.
    ///
    /// This carries the typechecker's canonical [`StringMethodId`] through Body IR so a consumer can select an
    /// admitted helper operation from a resolved identity rather than rediscovering a method target from source text.
    pub resolved_string_helper_calls: HashMap<crate::ast::Span, StringMethodId>,
    /// Function-typed parameters of this module's functions and methods that hold any callable of their type, since
    /// the body only calls them, keyed by the parameter's span (#1561).
    ///
    /// Lowering spells each such parameter `impl Fn(A) -> R`, which holds a closure that captures local values as well
    /// as a named function; any other function-typed parameter is a function pointer.
    pub closure_holding_params: HashSet<crate::ast::Span>,
    /// Functions and methods of this module whose one returned value is a closure that captures local values, keyed by
    /// the span of their declared function return type, which lowering spells `impl Fn(A) -> R` (#1561).
    pub closure_returning_callables: HashSet<crate::ast::Span>,
    /// New local bindings whose value is a closure that captures local values, a local partial or the closure a
    /// closure-returning function returns, keyed by the assignment statement span (#1561).
    ///
    /// The binding takes the value's own type, so lowering does not spell the binding's function-type annotation, and
    /// spells the parameter types of a closure bound there instead.
    pub capturing_callable_bindings: HashSet<crate::ast::Span>,
    /// Exact source string methods selected for display operands. Body IR emits these as ordinary shared-receiver
    /// calls; native Rust formatting never supplies a source method by guesswork.
    pub source_display_methods: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Types this module declares whose values display through `message()`, keyed by type name.
    ///
    /// A model, class, enum or newtype that adopts `Error` and has no `Display` of its own displays its `message()`,
    /// so it satisfies a `Display` bound. The value is the `message()` call the checker resolved on the type, which
    /// lowering makes the body of the Rust `Display` it gives the type.
    pub error_message_display_types: HashMap<String, ErrorMessageDisplay>,
}

/// The `message()` call one displayed `Error` adopter renders through (#1778).
///
/// Resolved by the checker exactly as a written `value.message()` call: through the adopter's own method, a trait
/// default it inherits, or the `Error` bound of a type parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorMessageDisplay {
    /// Canonical identity of the selected `message` declaration, when declaration provenance retained one.
    pub identity: Option<CanonicalSymbolId>,
    /// Trait dispatch the call needs, when the selected `message` is reached through a trait.
    pub dispatch: Option<ResolvedMethodDispatch>,
    /// Whether the operand is `self` inside a default method of a trait that extends `Error`.
    ///
    /// Such a display is checked once against the trait's `Self`, like a written `self.message()` there, which the
    /// checker resolves no further; lowering expands the default into each adopter.
    pub receiver_is_trait_self: bool,
    /// For a trait-`Self` display, the adopters in this module that have a `Display` of their own and so keep it
    /// where the default is expanded into them. Empty for every other display.
    pub self_displaying_adopters: BTreeSet<String>,
}

/// Typechecker-owned meaning of one `isinstance` target expression.
#[derive(Debug, Clone, PartialEq)]
pub struct IsInstanceTargetInfo {
    /// Alias-expanded semantic target type.
    pub ty: ResolvedType,
    /// Canonical declaration identity for a nominal target, when resolution proved one.
    ///
    /// Compiler-owned primitives and aliases expanded to primitives need no declaration identity. An absent identity
    /// on another target is a visible downstream refusal, never permission to dispatch from its source spelling.
    pub canonical: Option<CanonicalSymbolId>,
    /// Original source range of the target expression, including parentheses when they were written.
    pub span: Span,
}

/// Custom protocol decisions that lower into explicit runtime calls.
#[derive(Debug, Default, Clone)]
pub struct ProtocolArtifacts {
    /// Exact selected input contracts of successful calls, including calls from otherwise ordinary bodies.
    pub(in crate::typechecker) explicit_input_calls: BTreeMap<Span, super::ownership_contracts::InputCall>,
    /// The contract each native call's selected Rust signature gives it, keyed by that call's callee identity. Derived
    /// from the checked native calls whenever the body's input calls are built or restored, never recorded.
    pub(in crate::typechecker) native_input_contracts:
        BTreeMap<CanonicalSymbolId, crate::ownership_metadata::CheckedOwnershipContract>,
    /// Canonically bound local mode requests, captured before lexical shadowing. Native lowering may consume them
    /// only together with the owning body's complete flow proof.
    pub(in crate::typechecker) explicit_local_modes: BTreeMap<CanonicalSymbolId, super::ownership_contracts::LocalMode>,
    /// Explicit input flow is body-owned. The native executable gate remains closed until the complete projection
    /// and storage contract agree; record profiles without this payload must refuse rather than discard it.
    pub(in crate::typechecker) explicit_input_flow: BTreeMap<CanonicalSymbolId, super::ownership_contracts::InputFlow>,
    /// Body-owned decisions from the checker's Copy relation, independent of the flow graph. Exact checked types
    /// select their retained decision; missing evidence never falls back to an ABI or source-name heuristic.
    pub(in crate::typechecker) explicit_input_copy: BTreeMap<CanonicalSymbolId, HashMap<ResolvedType, bool>>,
    /// Receiver value identities retained only for Body IR before synthetic truthiness dispatch reuses their spans.
    /// The legacy reference at that span still selects `__bool__`; this separate record never changes that table.
    pub(crate) body_ir_truthiness_receivers: HashMap<crate::ast::Span, CanonicalSymbolId>,
    /// Independent checked hook calls for Body IR, separate from the legacy iteration record and written call facts.
    pub body_ir_iterations: HashMap<crate::ast::Span, BodyIrIterationHooksInfo>,
    /// RFC 068: Custom `for` iteration protocol choices keyed by iterable expression span.
    ///
    /// Lowering consumes this so a structural `__iter__` / `__next__` pair can become an explicit loop that calls the
    /// resolved hooks without relying on Rust's `IntoIterator`.
    pub iterations: HashMap<crate::ast::Span, ProtocolIterationInfo>,
    /// `for` loops that take the items out of the list binding they iterate, keyed by iterable expression span
    /// (#1844).
    ///
    /// The loop body hands on by value items that can be neither copied nor cloned, so lowering iterates the list by
    /// value and owns each item. The checker refuses every read of the list inside the loop or after it, until an
    /// assignment that no branch, `break` or `continue` can skip gives the binding a new list, a closure that captured
    /// the list before the loop, and a repeat of the loop that would reach the emptied list.
    pub item_taking_iterations: HashSet<crate::ast::Span>,
    /// Comprehension collection sources whose direct identifier read is consumed, keyed by iterable expression span
    /// (#1983).
    ///
    /// Both outcomes are recorded: `true` makes lowering move the collection and iterate its owned elements, while
    /// `false` keeps the collection and requires the borrowed-item iteration plan. Recording the negative outcome is
    /// essential because it prevents lowering from running a second last-use heuristic that could disagree with the
    /// checker about whether cloning an element is required.
    pub comprehension_source_consumption: HashMap<crate::ast::Span, bool>,
    /// Scrutinee spans of the `match`, `if let` and `while let` forms whose pattern binds a view into a caller-visible
    /// `mut` parameter that an arm changes (#1561).
    ///
    /// The change has to reach the parameter, so lowering matches such a scrutinee in place, through a mutable
    /// reference, and binds each pattern name to the part of the parameter it names rather than to a copy.
    pub in_place_match_scrutinees: HashSet<crate::ast::Span>,
    /// Scrutinee spans of the `match`, `if let` and `while let` forms whose value holds no loan: its checked type
    /// names no reference and no Rust type with lifetime parameters, at any depth.
    ///
    /// Such a value cannot borrow from the temporaries its evaluation made, so lowering ends them before the arms run
    /// and an arm may move the owner they borrowed.
    pub loan_free_scrutinees: HashSet<crate::ast::Span>,
}

/// A typechecker-resolved user-defined operator call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOperatorCall {
    /// The concrete dunder method name selected by frontend method/trait dispatch.
    pub method: String,
    /// The AST operator shape this call replaces.
    pub kind: ResolvedOperatorKind,
}

/// Metadata for one imported Rust trait binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustTraitImportInfo {
    /// Canonical import path used by Incan for this trait binding.
    pub trait_path: String,
    /// Resolved Rust definition path after re-export resolution, when available.
    pub definition_path: Option<String>,
    /// Method names this trait can place in Rust method-lookup scope.
    pub methods: HashSet<String>,
    /// Whether `methods` is the trait's declared surface, from inspected metadata or the compiler's fallback trait
    /// vocabulary.
    ///
    /// An import with neither has an unknown surface: the compiler cannot say which methods it provides, or even
    /// that it is a trait. Such an import stays a candidate for any method call that no inspected surface resolves,
    /// so the generated `use` survives when Rust method lookup may need it (#1450).
    pub methods_known: bool,
    /// Method signatures this trait metadata provided, keyed by method name.
    pub method_signatures: HashMap<String, RustFunctionSig>,
}

/// A typechecker-resolved Rust extension-trait import required by a method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustMethodTraitImportUse {
    /// Local import binding to retain in generated Rust.
    pub binding: String,
    /// Trait path selected for the call.
    pub trait_path: String,
    /// Method name observed at the call site.
    pub method: String,
    /// Trait method signature, when metadata supplied it.
    pub signature: Option<RustFunctionSig>,
}

/// A typechecker-resolved method call consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMethodCall {
    /// The concrete source-level method name selected by frontend method/trait dispatch.
    pub method: String,
    /// How the backend should emit this method call.
    pub dispatch: ResolvedMethodDispatch,
}

/// Semantic dispatch target for a resolved method call.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedMethodDispatch {
    /// Preserve the selected trait owner and receiver contract for downstream lowering.
    Trait {
        /// Source-level trait name selected by semantic resolution.
        trait_name: String,
        /// Canonical source module that owns the trait, when the import boundary supplies one.
        module_path: Option<Vec<String>>,
        /// Concrete trait type arguments selected by overload resolution.
        type_args: Vec<ResolvedType>,
        /// Compiler-resolved generic header attached to the exact selected implementation.
        implementation_type_params: Vec<ImplementationTypeParamInfo>,
        /// Whether the selected trait method declares `mut self`.
        receiver_is_mutable: bool,
    },
}

/// Exact checker-selected calls behind one structural or fallible iteration; only Body IR consumes this record.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrIterationHooksInfo {
    /// Call that materializes the iterator before the first poll. None transfers a checked owned source iterator
    /// directly into its polling frame; it is already its own iteration state.
    pub iter: Option<BodyIrProtocolCallInfo>,
    /// Call that advances the materialized iterator inside the loop.
    pub next: BodyIrProtocolCallInfo,
}

/// Retained signature and identity for one synthetic hook, never inferred by a backend from its spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrProtocolCallInfo {
    /// Exact source or active SDK declaration; absent remains a named refusal.
    pub canonical: Option<CanonicalSymbolId>,
    /// Exact checked trait instantiation for this Body IR-only hook snapshot; the written call's facts stay intact.
    pub dispatch: Option<ResolvedMethodDispatch>,
    /// Checked ordinary parameters, excluding the receiver, including their default-presence contract.
    pub params: Option<Vec<CallableParam>>,
    /// Checked method-scoped instantiation arguments, independent of the written iterable's generic call.
    pub type_args: Vec<ResolvedType>,
    /// Checked result after receiver and trait arguments have been applied.
    pub return_type: ResolvedType,
    /// Declared instance receiver mode; an absent or ambiguous receiver remains refused.
    pub receiver_is_mutable: Option<bool>,
}

/// Typechecker-resolved custom iteration protocol consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolIterationInfo {
    /// Method selected on the iterable expression.
    pub iter_method: String,
    /// Concrete iterator object type returned from `__iter__`.
    pub iterator_type: ResolvedType,
    /// Method selected on the iterator object.
    pub next_method: String,
    /// Element type unwrapped from `__next__() -> Option[T]`.
    pub item_type: ResolvedType,
    /// Exact semantic dispatch selected for `__iter__`, when the hook belongs to an adopted trait.
    pub iter_dispatch: Option<ResolvedMethodDispatch>,
    /// Exact semantic dispatch selected for `__next__`, when the hook belongs to an adopted trait.
    pub next_dispatch: Option<ResolvedMethodDispatch>,
    /// Error type propagated by `for item in iterable?`, when this is a fallible iteration route.
    pub fallible_error_type: Option<ResolvedType>,
}

/// Lowering metadata for one RFC 046 computed property read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedPropertyAccessInfo {
    pub owner_type: String,
    pub property: String,
}

/// Operator expression shape for a resolved user-defined operator call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedOperatorKind {
    Binary,
    Unary,
    Index,
    /// `obj[start:end:step]` on a type that defines `__getslice__` (`Sliceable[T]`).
    Slice,
    IndexAssign,
    Truthiness,
    Len,
    Contains,
    Call,
}

/// Typechecker-proven call-unpack shape consumed by IR lowering.
#[derive(Debug, Clone, PartialEq)]
pub enum FixedUnpackPlan {
    /// `*expr` has a statically known ordered shape with one type per contributed positional item.
    Positional(Vec<ResolvedType>),
    /// `**expr` has statically known string keys in source order.
    Keyword(Vec<String>),
}

/// How an identifier expression resolved in the symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentKind {
    /// A value binding (variable/field), or a callable value (function).
    Value,
    /// A module static binding.
    Static,
    /// A type name (models/classes/enums/newtypes).
    TypeName,
    /// An enum variant constructor identifier.
    Variant,
    /// A module-like namespace (e.g. imported module placeholders).
    Module,
    /// A Rust import placeholder (`import rust::...` / `from rust::... import ...`).
    RustImport,
    /// A Rust value import, such as a public Rust constant.
    RustValue,
    /// A trait name (may be used as a type-like namespace).
    Trait,
}
