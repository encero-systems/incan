//! `mut` parameters whose changes reach the caller, and the arguments a call passes to them (#1773).
//!
//! A parameter declared `mut` is a mutable binding inside its function. When it is marked (its type is not `int`,
//! `float`, `bool`, an alias of one or a Rust type, and it is not a `*args` or `**kwargs` parameter; see
//! `mut_marker.rs`), the function's changes to it are also visible to the caller: collections, models and class
//! objects are shared with the caller, scalars are the function's own copy. This module owns the facts that follow
//! from the marker:
//!
//! - each declared parameter's marker, recorded at collection for lowering by parameter span and name, so the Rust
//!   shape of the declaration and of every call agrees ([`TypeCheckInfo`](super::TypeCheckInfo) declarations);
//! - which caller-visible parameters a callable's body changes, directly, through a view into one (the variable of a
//!   `for` loop that iterates one in place, or a name a `match`, `if let` or `while let` pattern binds from one), or by
//!   passing them on to another callee that changes them; any use of a `Generator` parameter advances it and so changes
//!   it; a method reached by trait dispatch and a callee known only by its callable type are taken to change every
//!   marked parameter;
//! - where a caller-visible parameter would be held by another name or value (a new binding, a literal, comprehension,
//!   field or element store, construction or `partial` preset, a `match`, `if`, `break` or `yield` value, a `match` arm
//!   binding, a closure that returns it, changes it or passes it on to a parameter that may change it), which is
//!   refused;
//! - which call arguments are refused (`INCAN-T0117`): an immutable binding or a field of one, an element of a
//!   collection, or a static, passed to a caller-visible parameter the callee changes, where the change would fail to
//!   build or be lost;
//! - which call arguments a caller-visible parameter the callee never changes accepts: an immutable binding or field is
//!   accepted, and lowering passes it as the parameter's own marker (`CallableParam::is_mut`) says; one whose type
//!   cannot be copied, such as a `Generator`, is refused (`INCAN-T0117`) instead.
//!
//! A caller-visible parameter cannot be rebound to a new value in its body: the caller would not see the rebinding.
//! Temporaries (literals, call results) are always accepted; nothing but the callee holds them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ast::{
    CallArg, ComprehensionClause, DictEntry, Expr, ListEntry, MatchBody, Param, ParamKind, Pattern, PatternArg, Span,
    Spanned, Statement,
};
use crate::diagnostics::errors::{
    self, MutArgumentPlace, MutParameterChange, MutParameterCopy, MutParameterLabel, ReadOnlyScrutinee,
};
use crate::symbols::{CallableParam, ResolvedType, SymbolKind, TypeInfo};
use incan_lang::lang::builtins::{self, BuiltinFnId};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::surface::constructors;
use incan_lang::lang::surface::dict_methods::{self, DictMethodId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::TypeChecker;

mod argument_recording;
mod held_params;
#[cfg(test)]
mod legacy;
mod param_bodies_and_views;
#[cfg(test)]
mod record_projection;

pub(in crate::typechecker) use super::flow::{MutationEffects, MutationForward as MutParamForward};
use crate::typechecker::helpers::collection_type_id;
use crate::typechecker::receiver_change::ReceiverChange;

/// Exact callee/parameter projections selected by one caller's recorded pending arguments.
pub(in crate::typechecker) type MutationEffectReads = HashMap<(CanonicalSymbolId, String), Option<bool>>;

/// One declared parameter of a callable that has at least one caller-visible `mut` parameter.
#[derive(Debug, Clone)]
pub(crate) struct DeclaredParamSlot {
    /// The parameter's declared name, which a named argument binds to.
    name: String,
    /// Whether the parameter is ordinary or a rest parameter; positional arguments bind only ordinary ones in order.
    kind: ParamKind,
    /// Whether the callable's changes to this parameter are visible to the caller.
    shows_changes_to_caller: bool,
}

/// How one argument for a caller-visible `mut` parameter relates to the caller's own storage.
#[derive(Debug, Clone)]
enum ArgumentPlace {
    /// A literal, a call result or another value only the callee will hold.
    Temporary,
    /// A place the caller may change: a `mut` binding or parameter, `self` in a `mut self` method, or a field of one.
    /// `forwarded_param` names the enclosing callable's caller-visible parameter the place is rooted at, if any.
    Mutable { forwarded_param: Option<String> },
    /// An immutable binding or a field of one; `binding` is the name when the argument is the binding itself.
    Immutable { binding: Option<String> },
    /// An element of a collection or a static, which the call receives as a copy of the stored value.
    Detached { place: MutArgumentPlace },
}

/// The callee of a call whose arguments for `mut` parameters are recorded.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MutArgumentCallee<'a> {
    /// A function call's callee expression.
    Function(&'a Spanned<Expr>),
    /// A method call's receiver and method name.
    Method {
        receiver: &'a Spanned<Expr>,
        method: &'a str,
    },
}

/// One argument recorded at a call, resolved once every body of the module has been checked.
#[derive(Debug, Clone)]
struct PendingMutArgument {
    /// Legacy argument traversal order, independent of producer completion.
    order: usize,
    /// The callee's declaration identity, when the call resolved to one; `None` for a callable known only by its type.
    callee: Option<CanonicalSymbolId>,
    /// The callee's name as the refusal names it.
    callee_name: String,
    param: String,
    /// The parameter's 1-based position, which names it when its callable type gives it no name.
    position: usize,
    place: ArgumentPlace,
    /// The argument's type, spelled for the refusal, when a value of it cannot be copied (a `Generator`, a type
    /// holding one); decided at the call, where the caller's type parameters are in scope.
    uncopyable_type: Option<String>,
    /// Whether a public native field is still reached through a shared native owner and cannot grant exclusive access.
    shared_native_field: bool,
    span: Span,
    /// The declaration span of the local the call went through (`f(items)` after `f = extend`), when it did.
    through_local: Option<Span>,
}

/// What a binding is a view into, and how the view was bound.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct ViewSource {
    /// The caller-visible parameter the view reaches; `None` for a view into any other place, whose changes do not
    /// concern a caller's argument.
    param: Option<String>,
    /// Whether the place the view reaches permits a change through it.
    access: ViewAccess,
    /// The spans of the `match`, `if let` and `while let` scrutinees the view was bound through, outermost first. A
    /// change through the view reaches the parameter only when each of them is matched in place, so a change records
    /// every one of them for lowering ([`TypeCheckInfo::match_scrutinee_is_changed_in_place`]).
    ///
    /// [`TypeCheckInfo::match_scrutinee_is_changed_in_place`]: super::TypeCheckInfo::match_scrutinee_is_changed_in_place
    scrutinees: Vec<Span>,
}

/// Whether the place a view reaches permits a change through the view.
#[derive(Debug, Clone)]
enum ViewAccess {
    /// A place the body may change: a caller-visible parameter, a `mut` binding or parameter, `self` in a `mut self`
    /// method, or a field or list element of one.
    Writable,
    /// A place the body may not change; the payload names it for the refusal.
    ReadOnly(ReadOnlyPlace),
    /// A value of the view's own: bound from a temporary or a loop over one, so a change through it reaches nothing
    /// else.
    Owned,
}

/// A place a view reaches that does not permit a change through the view.
#[derive(Debug, Clone)]
enum ReadOnlyPlace {
    /// A local binding declared without `mut`.
    Binding(String),
    /// A parameter declared without `mut`.
    Parameter(String),
    /// A module static.
    Static(String),
    /// A local bound directly to a module static, which names the static's own storage: the local, then the static.
    StaticAlias(String, String),
    /// The items of a place a generator expression reads, which it holds as copies: the place's root name.
    GeneratorItems(String),
    /// A local a closure captures from its enclosing callable, which the closure holds as its own copy.
    Captured(String),
    /// `self` in a plain-`self` method.
    SelfReceiver,
    /// A dict value, which a pattern binds as a copy.
    DictValue,
}

impl ViewSource {
    /// A view of a place the body may change, with no scrutinee in between yet.
    fn writable(param: Option<String>) -> Self {
        Self {
            param,
            access: ViewAccess::Writable,
            scrutinees: Vec::new(),
        }
    }

    /// A view with the given access to a place that concerns no caller-visible parameter.
    fn with_access(access: ViewAccess) -> Self {
        Self {
            param: None,
            access,
            scrutinees: Vec::new(),
        }
    }
}

/// A binding in scope that is a view into a place.
#[derive(Debug, Clone)]
struct ParamView {
    name: String,
    /// The binding's declaration span, which a later name must resolve to.
    span: Span,
    source: ViewSource,
}

/// The body currently being checked and its caller-visible `mut` parameters, by name, with their declaration spans.
#[derive(Debug, Clone)]
struct MutParamBody {
    identity: CanonicalSymbolId,
    params: HashMap<String, Span>,
    /// How many closures inside this body are being checked; a closure that changes a parameter holds it.
    closure_depth: usize,
    /// Bindings in scope that are views into a place: the variables of the `for` loops that iterate a list in place,
    /// and the names a `match`, `if let` or `while let` pattern binds. A change through a view is a change to the
    /// place, and to the caller-visible parameter the place belongs to, if any.
    views: Vec<ParamView>,
}

/// Checker state for caller-visible `mut` parameters across one module's check.
#[derive(Debug, Clone, Default)]
pub(crate) struct MutationFacts {
    /// Declared parameters of every source callable with a caller-visible `mut` parameter, by declaration identity.
    declared: HashMap<CanonicalSymbolId, Vec<DeclaredParamSlot>>,
    /// Source-builder facts waiting to enter the active callable's canonical ownership graph.
    pub(in crate::typechecker) source: MutationSource,
    /// Local bindings that hold a declared callable by reference, `f = extend`, keyed by the binding's declaration
    /// span, so a call through them is checked too.
    function_values: HashMap<Span, CanonicalSymbolId>,
    /// Binding spans of the variables of every `for` loop being checked, which are not mutable bindings, each with
    /// whether its loop's pattern destructures the element.
    loop_variables: Vec<(Span, bool)>,
    /// Arguments waiting for the module's change facts.
    pending: Vec<PendingMutArgument>,
    /// The body being checked, when it declares caller-visible parameters.
    current: Option<MutParamBody>,
}

/// One body's mutation effects, forwarding edges, local reassignment evidence and pending argument decisions.
/// Declared parameter contracts stay with the collected environment and never become another body's output.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct MutationLocal {
    /// Source-builder facts recorded by this lexical owner.
    pub(in crate::typechecker) source: MutationSource,
    /// Local bindings that hold a declared callable by reference, `f = extend`, keyed by the binding's declaration
    /// span, so a call through them is checked too.
    function_values: HashMap<Span, CanonicalSymbolId>,
    /// Binding spans of the variables of every `for` loop being checked, which are not mutable bindings, each with
    /// whether its loop's pattern destructures the element.
    loop_variables: Vec<(Span, bool)>,
    /// Arguments waiting for the module's change facts.
    pending: Vec<PendingMutArgument>,
    /// The body being checked, when it declares caller-visible parameters.
    current: Option<MutParamBody>,
}

/// Transient facts recorded while the checker still owns lexical scopes and resolved source expressions.
///
/// These values are projected into [`super::flow::Graph`] before flow analysis. The retained copy exists only for
/// checker-record transport and body-check reuse; it does not close edges or decide diagnostics.
#[derive(Debug, Clone, Default)]
pub(in crate::typechecker) struct MutationSource {
    pub(in crate::typechecker) bodies_checked: HashSet<CanonicalSymbolId>,
    pub(in crate::typechecker) changed: HashMap<CanonicalSymbolId, HashSet<String>>,
    pub(in crate::typechecker) forwards: Vec<MutParamForward>,
    pub(in crate::typechecker) reassigned_locals: HashSet<Span>,
}

/// Deterministically ordered, completed mutation facts accepted by the checker record adapter.
/// Empty changed-parameter sets remain explicit owner entries. Canonical owners and source spans still belong to
/// this check; the record adapter must bind them to stable identities and anchors before persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::typechecker) struct RecordedMutation {
    /// Source bodies whose mutation effects are known, including bodies that change no parameter.
    pub(in crate::typechecker) bodies: BTreeSet<CanonicalSymbolId>,
    /// Exact direct changed-parameter sets; an owner with an empty set is not omitted.
    pub(in crate::typechecker) changed: BTreeMap<CanonicalSymbolId, BTreeSet<String>>,
    /// Exact reassigned binding spans, retaining any generated-source expansion identity.
    pub(in crate::typechecker) reassigned: BTreeSet<Span>,
}

/// Return the parameters of a function type.
fn function_type_params(ty: &ResolvedType) -> Option<&[CallableParam]> {
    match ty {
        ResolvedType::Function(params, _) => Some(params.as_slice()),
        _ => None,
    }
}

/// Return the name of the nominal type a receiver's type names, through a reference.
fn nominal_type_name(ty: &ResolvedType) -> Option<&str> {
    match ty {
        ResolvedType::Named(name) | ResolvedType::Generic(name, _) => Some(name.as_str()),
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => nominal_type_name(inner),
        _ => None,
    }
}

/// Return whether a parameter of a callable known only by its type is marked `mut`: whether the callable's changes to
/// it reach the caller.
///
/// This is the one place the checker reads that marker, for a compiled library's function, a function value and a
/// closure alike: the `mut` marker of the callable type, `(mut T) -> R`, which a library carries across its boundary
/// in its manifest.
fn callable_param_is_marked(param: &CallableParam) -> bool {
    param.is_mut
}

/// Opaque saved state of the enclosing body, restored by [`TypeChecker::exit_mut_param_body`].
#[derive(Debug)]
pub(crate) struct MutParamBodyState(Option<MutParamBody>);

/// Return whether a `match` arm pattern binds the whole matched value to a name (`xs => ...`).
fn pattern_binds_whole_value(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Binding(_) => true,
        Pattern::Group(inner) => pattern_binds_whole_value(&inner.node),
        Pattern::Or(alternatives) => alternatives
            .iter()
            .any(|alternative| pattern_binds_whole_value(&alternative.node)),
        _ => false,
    }
}

/// Return whether a value of `ty` is a list whose elements a loop can change in place: not `int`, `float` or `bool`.
fn list_of_changeable_elements(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => list_of_changeable_elements(inner),
        ResolvedType::Generic(name, args) => {
            collection_type_id(name) == Some(CollectionTypeId::List)
                && args.first().is_some_and(|element| {
                    !matches!(element, ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool)
                })
        }
        _ => false,
    }
}

/// Return whether `ty` is a dict whose values can be changed in place, which a loop over its `values()` reaches: any
/// value type other than `int`, `float` or `bool`.
fn dict_of_changeable_values(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => dict_of_changeable_values(inner),
        ResolvedType::Generic(name, args) => {
            collection_type_id(name) == Some(CollectionTypeId::Dict)
                && args
                    .get(1)
                    .is_some_and(|value| !matches!(value, ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool))
        }
        _ => false,
    }
}

/// Return whether reading a value of `ty` advances it: a `Generator` or an `Iterator`, whose items a read hands out.
fn advances_when_read(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => advances_when_read(inner),
        ResolvedType::Generic(name, _) => {
            collection_type_id(name) == Some(CollectionTypeId::Generator) || ty.iterator_item_type().is_some()
        }
        _ => false,
    }
}

/// Collect every name a pattern binds.
fn collect_pattern_bindings(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Binding(name) => names.push(name.clone()),
        Pattern::Constructor(_, args) => {
            for arg in args {
                match arg {
                    PatternArg::Positional(inner) | PatternArg::Named(_, inner) => {
                        collect_pattern_bindings(&inner.node, names);
                    }
                }
            }
        }
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                collect_pattern_bindings(&item.node, names);
            }
        }
        Pattern::Group(inner) => collect_pattern_bindings(&inner.node, names),
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// Return whether `value` names a binding one of a comprehension's `for` clauses introduces.
fn value_is_bound_by_clauses(value: &Spanned<Expr>, clauses: &[ComprehensionClause]) -> bool {
    let Some(name) = bare_name(value) else {
        return false;
    };
    clauses.iter().any(|clause| match clause {
        ComprehensionClause::For { pattern, .. } => pattern_binds_name(&pattern.node, name),
        ComprehensionClause::If(_) => false,
    })
}

/// Return the value a block produces: its last statement when that is an expression.
fn trailing_value(body: &[Spanned<Statement>]) -> Option<&Spanned<Expr>> {
    match &body.last()?.node {
        Statement::Expr(value) => Some(value),
        _ => None,
    }
}

/// Return whether `value` names a binding the arm's pattern introduces, which shadows any parameter of that name.
fn value_is_rebound_by_pattern(value: &Spanned<Expr>, pattern: &Pattern) -> bool {
    let Some(name) = bare_name(value) else {
        return false;
    };
    pattern_binds_name(pattern, name)
}

/// Return whether `value` names a binding the block itself introduces before producing it.
fn value_is_rebound_in(value: &Spanned<Expr>, body: &[Spanned<Statement>]) -> bool {
    let Some(name) = bare_name(value) else {
        return false;
    };
    body.iter().any(|statement| match &statement.node {
        Statement::Assignment(assign) => assign.name == name,
        Statement::TupleUnpack(unpack) => unpack.names.iter().any(|bound| bound == name),
        _ => false,
    })
}

/// Return the name a (possibly parenthesized) identifier expression spells.
fn bare_name(value: &Spanned<Expr>) -> Option<&str> {
    match &value.node {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Paren(inner) => bare_name(inner),
        _ => None,
    }
}

/// Return whether a pattern binds `name` anywhere inside it.
fn pattern_binds_name(pattern: &Pattern, name: &str) -> bool {
    match pattern {
        Pattern::Binding(bound) => bound == name,
        Pattern::Constructor(_, args) => args.iter().any(|arg| match arg {
            PatternArg::Positional(inner) | PatternArg::Named(_, inner) => pattern_binds_name(&inner.node, name),
        }),
        Pattern::Tuple(items) | Pattern::Or(items) => items.iter().any(|item| pattern_binds_name(&item.node, name)),
        Pattern::Group(inner) => pattern_binds_name(&inner.node, name),
        Pattern::Wildcard | Pattern::Literal(_) => false,
    }
}

/// Return the binding name a place expression is rooted at, following fields, elements and parentheses.
fn place_root_name(expr: &Spanned<Expr>) -> Option<&str> {
    match &expr.node {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Field(base, _) | Expr::Index(base, _) | Expr::Paren(base) => place_root_name(base),
        _ => None,
    }
}

impl TypeChecker {
    /// Attach caller-visible mutation facts to the active callable's canonical ownership graph.
    ///
    /// A callable already analyzed for explicit input flow receives the facts on that graph. Otherwise, a minimal
    /// graph carries the authenticated owner and facts without forcing unrelated bodies through explicit-flow
    /// admission. There is never a second graph for the same callable.
    pub(in crate::typechecker) fn record_mutation_flow(&mut self, span: Span) {
        let Some(owner) = self.current_body_identity().cloned() else {
            return;
        };
        if !self.mutation.activates_graph(&owner) {
            return;
        }
        let mutation = self.mutation.graph_body(&owner);
        let existing = self.type_info.protocols.explicit_input_flow.get_mut(&owner);
        match existing {
            Some(super::InputFlow::Checked { graph, .. } | super::InputFlow::Invalid { graph, .. }) => {
                graph.mutation = mutation;
            }
            Some(super::InputFlow::Unavailable(_)) | None => {
                self.type_info
                    .protocols
                    .mutation_flows
                    .insert(owner.clone(), super::flow::Graph::mutation_only(owner, span, mutation));
            }
        }
    }
}

impl MutationFacts {
    /// Return whether the active callable contributes caller-visible mutation facts or argument obligations.
    /// Ordinary bodies with none of these facts do not activate ownership-graph construction.
    pub(in crate::typechecker) fn activates_graph(&self, owner: &CanonicalSymbolId) -> bool {
        self.declared.contains_key(owner)
            || self.source.changed.contains_key(owner)
            || self.source.forwards.iter().any(|forward| &forward.owner == owner)
            || !self.source.reassigned_locals.is_empty()
    }

    /// Project the active callable's checked mutation facts into its canonical ownership graph.
    ///
    /// The source writer retains a copy only because checker-record reuse is a separate artifact lifecycle. All
    /// same-run effect closure and call diagnostics consume this graph projection.
    pub(in crate::typechecker) fn project_graph(&self, owner: &CanonicalSymbolId, graph: &mut super::flow::Graph) {
        graph.mutation = self.graph_body(owner);
    }

    /// Materialize one active callable's mutation projection for its canonical graph builder.
    fn graph_body(&self, owner: &CanonicalSymbolId) -> super::flow::MutationBody {
        super::flow::MutationBody {
            known: self.declared.contains_key(owner) && self.source.bodies_checked.contains(owner),
            changed: self.source.changed.get(owner).cloned().unwrap_or_default(),
            forwards: self
                .source
                .forwards
                .iter()
                .filter(|forward| &forward.owner == owner)
                .cloned()
                .collect(),
            reassigned_locals: self.source.reassigned_locals.clone(),
        }
    }

    /// Move body-owned mutation state out while leaving all declared parameter inputs readable.
    pub(in crate::typechecker) fn take_body_local(&mut self) -> MutationLocal {
        MutationLocal {
            source: std::mem::take(&mut self.source),
            function_values: std::mem::take(&mut self.function_values),
            loop_variables: std::mem::take(&mut self.loop_variables),
            pending: std::mem::take(&mut self.pending),
            current: std::mem::take(&mut self.current),
        }
    }

    /// Restore the enclosing callable's exact mutation writer, including its current parameter frame.
    pub(in crate::typechecker) fn install_body_local(&mut self, local: MutationLocal) {
        self.source = local.source;
        self.function_values = local.function_values;
        self.loop_variables = local.loop_variables;
        self.pending = local.pending;
        self.current = local.current;
    }

    /// Append completed local effects and pending arguments in declaration traversal order.
    pub(in crate::typechecker) fn append_body_local(&mut self, local: MutationLocal) {
        self.source.bodies_checked.extend(local.source.bodies_checked);
        self.source.changed.extend(local.source.changed);
        self.source.forwards.extend(local.source.forwards);
        self.source.reassigned_locals.extend(local.source.reassigned_locals);
        self.function_values.extend(local.function_values);
        self.loop_variables.extend(local.loop_variables);
        self.pending.extend(local.pending);
        debug_assert!(local.current.is_none());
    }
}

impl MutationLocal {
    /// Move the prepared callable frame into its statement writer without giving the declaration producer its
    /// body effects. The frame returns when that writer closes; the known-body contribution stays with the body.
    pub(in crate::typechecker) fn enter_statement_frame(&mut self, writer: &mut MutationFacts) {
        writer.current = self.current.take();
        if let Some(frame) = &writer.current
            && self.source.bodies_checked.remove(&frame.identity)
        {
            writer.source.bodies_checked.insert(frame.identity.clone());
        }
    }

    /// Return only the active frame to the declaration operation that will restore its enclosing callable.
    pub(in crate::typechecker) fn return_statement_frame(&mut self, enclosing: &mut Self) {
        enclosing.current = self.current.take();
    }

    /// Render this caller's pending argument decisions from final callee effects. No syntax or local inference is read.
    pub(in crate::typechecker) fn finalize_arguments(
        &self,
        effects: &MutationEffects,
        diagnostics: &mut Vec<(usize, crate::diagnostics::CompileError)>,
    ) {
        for argument in &self.pending {
            let known_callee = argument.callee.as_ref().filter(|callee| {
                effects.bodies_checked.contains(*callee)
                    && !argument
                        .through_local
                        .is_some_and(|local| effects.reassigned_locals.contains(&local))
            });
            let change = match known_callee {
                Some(_) => MutParameterChange::Changes,
                None => MutParameterChange::MayChange,
            };
            let changes =
                known_callee.is_none_or(|callee| effects.changed.contains(&(callee.clone(), argument.param.clone())));
            let immutable_place = if argument.shared_native_field {
                Some(MutArgumentPlace::Field)
            } else {
                match &argument.place {
                    ArgumentPlace::Immutable { binding } => Some(match binding {
                        Some(name) => MutArgumentPlace::Binding(name.clone()),
                        None => MutArgumentPlace::Field,
                    }),
                    ArgumentPlace::Detached { place } => Some(place.clone()),
                    ArgumentPlace::Temporary | ArgumentPlace::Mutable { .. } => None,
                }
            };
            let Some(place) = immutable_place else {
                continue;
            };
            let parameter = if argument.param.is_empty() {
                MutParameterLabel::Position(argument.position)
            } else {
                MutParameterLabel::Named(&argument.param)
            };
            let error = if argument.shared_native_field {
                errors::shared_native_field_to_mut_parameter(parameter, &argument.callee_name, argument.span)
            } else if changes {
                errors::immutable_argument_to_mut_parameter(
                    parameter,
                    &argument.callee_name,
                    change,
                    place,
                    argument.span,
                )
            } else if let Some(value_type) = &argument.uncopyable_type {
                errors::uncopyable_argument_to_mut_parameter(
                    parameter,
                    &argument.callee_name,
                    value_type,
                    place,
                    argument.span,
                )
            } else {
                // An element or a static already reaches the call as a copy of the stored value; a binding the caller
                // cannot change, or a loop variable, handed to a parameter the callee never changes is accepted too:
                // the call passes it as its parameter's own marker says.
                continue;
            };
            diagnostics.push((argument.order, error));
        }
    }
}

impl MutationLocal {
    /// Rebuild transient callable graph projections from the authenticated mutation record.
    ///
    /// Forwarding edges and pending arguments make a body unrecordable, so every restorable projection is complete:
    /// known bodies, direct changes and callable-local invalidations. Existing explicit-flow graphs remain the one
    /// graph for their callable; other owners receive a minimal canonical graph.
    pub(in crate::typechecker) fn restore_flow_graphs(
        &self,
        protocols: &mut crate::typechecker::type_info::ProtocolArtifacts,
    ) {
        let mut owners = self
            .source
            .bodies_checked
            .iter()
            .chain(self.source.changed.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        if !self.source.reassigned_locals.is_empty() {
            owners.extend(protocols.explicit_input_flow.keys().cloned());
        }
        for owner in owners {
            let mutation = super::flow::MutationBody {
                known: self.source.bodies_checked.contains(&owner),
                changed: self.source.changed.get(&owner).cloned().unwrap_or_default(),
                forwards: Vec::new(),
                reassigned_locals: self.source.reassigned_locals.clone(),
            };
            let existing = protocols.explicit_input_flow.get_mut(&owner);
            match existing {
                Some(super::InputFlow::Checked { graph, .. } | super::InputFlow::Invalid { graph, .. }) => {
                    graph.mutation = mutation;
                }
                Some(super::InputFlow::Unavailable(_)) | None => {
                    protocols.mutation_flows.insert(
                        owner.clone(),
                        super::flow::Graph::mutation_only(owner.clone(), owner.declaration_span, mutation),
                    );
                }
            }
        }
    }

    /// Remove finalized argument obligations while retaining all local effect, forwarding and reassignment evidence.
    pub(in crate::typechecker) fn clear_pending_arguments(&mut self) {
        self.pending.clear();
    }
}

impl MutationLocal {
    /// Retain only effect projections that can change this caller's refusal. Reassigned and unknown callees remain
    /// conservative and do not subscribe to a known body's more permissive effect projection.
    pub(in crate::typechecker) fn effect_reads(&self, effects: &MutationEffects) -> MutationEffectReads {
        self.pending
            .iter()
            .filter_map(|argument| {
                if matches!(argument.place, ArgumentPlace::Temporary | ArgumentPlace::Mutable { .. })
                    || argument
                        .through_local
                        .is_some_and(|local| effects.reassigned_locals.contains(&local))
                {
                    return None;
                }
                let callee = argument.callee.as_ref()?;
                let key = (callee.clone(), argument.param.clone());
                let value = effects
                    .bodies_checked
                    .contains(callee)
                    .then(|| effects.changed.contains(&key));
                Some((key, value))
            })
            .collect()
    }
}

impl MutationLocal {
    /// Return recorded argument count for a new invocation's traversal binding, without re-evaluating any argument.
    pub(in crate::typechecker) fn argument_count(&self) -> usize {
        self.pending.len()
    }
}

impl MutationLocal {
    /// Expose the known-body contribution only when every other mutation capability is empty. This temporary native
    /// profile never turns an unrepresented obligation into an empty effect or a successful source result.
    pub(in crate::typechecker) fn plain_bodies(&self) -> Option<&HashSet<CanonicalSymbolId>> {
        let Self {
            source,
            function_values,
            loop_variables,
            pending,
            current,
        } = self;
        (source.changed.is_empty()
            && source.forwards.is_empty()
            && function_values.is_empty()
            && loop_variables.is_empty()
            && source.reassigned_locals.is_empty()
            && pending.is_empty()
            && current.is_none())
        .then_some(&source.bodies_checked)
    }

    /// Restore an explicitly witnessed empty-effect body contribution without running mutation settlement.
    pub(in crate::typechecker) fn from_plain_bodies(bodies: Vec<CanonicalSymbolId>) -> Self {
        Self {
            source: MutationSource {
                bodies_checked: bodies.into_iter().collect(),
                ..MutationSource::default()
            },
            ..Self::default()
        }
    }
}

impl MutationLocal {
    /// Project completed facts only when every capability absent from the record shape is empty. Forwarding edges,
    /// callable bindings, loop views, pending decisions and an active frame are refused rather than silently lost.
    /// Ordered collections remove hash iteration order without flattening empty changed-owner entries.
    pub(in crate::typechecker) fn record_projection(&self) -> Option<RecordedMutation> {
        let Self {
            source,
            function_values,
            loop_variables,
            pending,
            current,
        } = self;
        if !source.forwards.is_empty()
            || !function_values.is_empty()
            || !loop_variables.is_empty()
            || !pending.is_empty()
            || current.is_some()
        {
            return None;
        }
        Some(RecordedMutation {
            bodies: source.bodies_checked.iter().cloned().collect(),
            changed: source
                .changed
                .iter()
                .map(|(owner, parameters)| (owner.clone(), parameters.iter().cloned().collect()))
                .collect(),
            reassigned: source.reassigned_locals.iter().copied().collect(),
        })
    }

    /// Restore all completed facts after the record adapter has rebound their identities and source anchors.
    /// Unsupported capabilities are empty because the projection admits none, not because restoration drops them.
    pub(in crate::typechecker) fn from_record_projection(record: RecordedMutation) -> Self {
        Self {
            source: MutationSource {
                bodies_checked: record.bodies.into_iter().collect(),
                changed: record
                    .changed
                    .into_iter()
                    .map(|(owner, parameters)| (owner, parameters.into_iter().collect()))
                    .collect(),
                reassigned_locals: record.reassigned.into_iter().collect(),
                ..MutationSource::default()
            },
            ..Self::default()
        }
    }
}
