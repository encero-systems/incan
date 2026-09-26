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
//! - which caller-visible parameters a callable's body changes, directly, through the variable of a `for` loop that
//!   iterates one in place, or by passing them on to another callee that changes them; a method reached by trait
//!   dispatch and a callee known only by its callable type are taken to change every marked parameter;
//! - where a caller-visible parameter would be held by another name or value (a new binding, a literal, comprehension,
//!   field or element store, construction or `partial` preset, a `match`, `if`, `break` or `yield` value, a `match` arm
//!   binding, a closure that returns it, changes it or passes it on to a parameter that may change it), which is
//!   refused;
//! - which call arguments are refused (`INCAN-T0117`): an immutable binding or a field of one, an element of a
//!   collection, or a static, passed to a caller-visible parameter the callee changes, where the change would fail to
//!   build or be lost;
//! - which call arguments are handed over as a copy: an immutable binding or field passed to a caller-visible parameter
//!   the callee never changes, which is what such a call always meant.
//!
//! A caller-visible parameter cannot be rebound to a new value in its body: the caller would not see the rebinding.
//! Temporaries (literals, call results) are always accepted; nothing but the callee holds them.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    CallArg, ComprehensionClause, DictEntry, Expr, ListEntry, MatchBody, Param, ParamKind, Pattern, PatternArg, Span,
    Spanned, Statement,
};
use crate::diagnostics::errors::{self, MutArgumentPlace, MutParameterChange, MutParameterCopy, MutParameterLabel};
use crate::symbols::{CallableParam, ResolvedType, SymbolKind, TypeInfo};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::surface::constructors;
use incan_lang::lang::surface::{dict_methods, list_methods, set_methods};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::TypeChecker;
use super::helpers::collection_type_id;

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
    /// The callee's declaration identity, when the call resolved to one; `None` for a callable known only by its type.
    callee: Option<CanonicalSymbolId>,
    /// The callee's name as the refusal names it.
    callee_name: String,
    param: String,
    /// The parameter's 1-based position, which names it when its callable type gives it no name.
    position: usize,
    place: ArgumentPlace,
    span: Span,
    /// The declaration span of the local the call went through (`f(items)` after `f = extend`), when it did.
    through_local: Option<(usize, usize)>,
}

/// A caller-visible parameter passed on to another callee's caller-visible parameter.
#[derive(Debug, Clone)]
struct MutParamForward {
    /// The callable whose parameter is passed on, and that parameter.
    owner: CanonicalSymbolId,
    param: String,
    /// The callee it is passed to, and the callee's parameter.
    callee: CanonicalSymbolId,
    callee_param: String,
    /// The declaration span of the local the call went through, when it did.
    through_local: Option<(usize, usize)>,
}

/// The body currently being checked and its caller-visible `mut` parameters, by name, with their declaration spans.
#[derive(Debug, Clone)]
struct MutParamBody {
    identity: CanonicalSymbolId,
    params: HashMap<String, Span>,
    /// How many closures inside this body are being checked; a closure that changes a parameter holds it.
    closure_depth: usize,
    /// Variables of the `for` loops being checked that iterate over a caller-visible parameter or its elements, as
    /// (name, binding span, parameter): a change through one is a change to the parameter.
    loop_elements: Vec<(String, Span, String)>,
}

/// Checker state for caller-visible `mut` parameters across one module's check.
#[derive(Debug, Clone, Default)]
pub(crate) struct MutParamFacts {
    /// Declared parameters of every source callable with a caller-visible `mut` parameter, by declaration identity.
    declared: HashMap<CanonicalSymbolId, Vec<DeclaredParamSlot>>,
    /// Callables whose bodies this check has read, so their changes are known exactly.
    bodies_checked: HashSet<CanonicalSymbolId>,
    /// Caller-visible parameters each callable's body changes directly.
    changed: HashMap<CanonicalSymbolId, HashSet<String>>,
    /// Caller-visible parameters passed on to another callee's caller-visible parameter.
    forwards: Vec<MutParamForward>,
    /// Local bindings that hold a declared callable by reference, `f = extend`, keyed by the binding's declaration
    /// span, so a call through them is checked too.
    function_values: HashMap<(usize, usize), CanonicalSymbolId>,
    /// Binding spans of the variables of every `for` loop being checked, which are not mutable bindings.
    loop_variables: Vec<Span>,
    /// Local bindings reassigned anywhere in the module, keyed by declaration span. A call through such a local may
    /// run any callable it was assigned, so it counts as changing every marked parameter.
    reassigned_locals: HashSet<(usize, usize)>,
    /// Arguments waiting for the module's change facts.
    pending: Vec<PendingMutArgument>,
    /// The body being checked, when it declares caller-visible parameters.
    current: Option<MutParamBody>,
}

impl TypeChecker {
    // ========================================================================
    // Declarations
    // ========================================================================

    /// Record one source callable's declared `mut` parameters: for lowering, and for calls when any is caller-visible.
    ///
    /// `declared` and `resolved` are the declaration's parameters without the receiver, in declaration order, and
    /// `identity` is the declaration identity calls to it resolve to. Every ordinary `mut` parameter's marker
    /// ([`Self::def_param_shows_changes_to_caller`]) is published for lowering at collection, including those of
    /// imported source modules this check collects but does not check, so a trait default expanded into an adopter in
    /// another module is lowered with its own module's markers.
    pub(in crate::typechecker) fn record_caller_visible_mut_params(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        declared: &[Spanned<Param>],
        resolved: &[CallableParam],
    ) {
        let slots = declared
            .iter()
            .zip(resolved)
            .map(|(param, resolved)| DeclaredParamSlot {
                name: param.node.name.clone(),
                kind: param.node.kind,
                shows_changes_to_caller: self.def_param_shows_changes_to_caller(&param.node, &resolved.ty),
            })
            .collect::<Vec<_>>();
        for (param, resolved) in declared.iter().zip(resolved) {
            self.record_mut_param_marker(&param.node, param.span, &resolved.ty);
        }
        if let Some(identity) = identity
            && slots.iter().any(|slot| slot.shows_changes_to_caller)
        {
            self.mut_params.declared.insert(identity.clone(), slots);
        }
    }

    // ========================================================================
    // Bodies
    // ========================================================================

    /// Enter the body of a function or method declared as `name` at `span`, returning the enclosing body's state.
    ///
    /// `kind` selects the identity namespace the declaration was collected under. The body's caller-visible `mut`
    /// parameters become the places whose changes and rebinding this module tracks until
    /// [`Self::exit_mut_param_body`] restores the returned state.
    pub(in crate::typechecker) fn enter_mut_param_body(
        &mut self,
        name: &str,
        kind: SemanticSourceTargetKind,
        span: Span,
        params: &[Spanned<Param>],
        resolved: &[ResolvedType],
    ) -> MutParamBodyState {
        let identity = match kind {
            SemanticSourceTargetKind::Method => self.symbols.member_declaration_identity(name, kind, span),
            _ => self.symbols.module_declaration_identity(name, kind, span),
        };
        let caller_visible = params
            .iter()
            .zip(resolved)
            .filter(|(param, resolved)| self.def_param_shows_changes_to_caller(&param.node, resolved))
            .map(|(param, _)| (param.node.name.clone(), param.span))
            .collect::<HashMap<_, _>>();
        self.mut_params.bodies_checked.insert(identity.clone());
        let next = MutParamBody {
            identity,
            params: caller_visible,
            closure_depth: 0,
            loop_elements: Vec::new(),
        };
        MutParamBodyState(self.mut_params.current.replace(next))
    }

    /// Leave a body entered with [`Self::enter_mut_param_body`].
    pub(in crate::typechecker) fn exit_mut_param_body(&mut self, previous: MutParamBodyState) {
        self.mut_params.current = previous.0;
    }

    /// Make a parameter declared `mut` a mutable binding of the body it belongs to.
    ///
    /// `mut` on a parameter always lets the body change it in place; a scalar parameter may also be rebound, a
    /// caller-visible one may not (see [`Self::refuse_caller_visible_mut_param_rebinding`]). The name joins the
    /// checker's mutable bindings as a `mut` local does, so a Rust or C boundary that needs a mutable argument accepts
    /// the parameter too.
    pub(in crate::typechecker) fn bind_mut_param_as_mutable(&mut self, param: &Param) {
        if param.is_mut {
            self.mutable_bindings.insert(param.name.clone());
        }
    }

    /// Return the caller-visible parameter of the current body that `name` resolves to, if it resolves to one.
    ///
    /// A local that shadows the parameter is not the parameter: the name must resolve to the parameter's own binding.
    fn current_caller_visible_param(&self, name: &str) -> Option<String> {
        let body = self.mut_params.current.as_ref()?;
        let declared_span = body.params.get(name)?;
        self.lookup_symbol(name)
            .is_some_and(|symbol| symbol.span == *declared_span)
            .then(|| name.to_string())
    }

    /// Return the caller-visible parameter a change through the binding `name` reaches: the parameter itself, or a
    /// variable of a `for` loop over the parameter or its elements.
    fn caller_visible_param_reached_by(&self, name: &str) -> Option<String> {
        if let Some(param) = self.current_caller_visible_param(name) {
            return Some(param);
        }
        let body = self.mut_params.current.as_ref()?;
        let binding_span = self.lookup_symbol(name)?.span;
        body.loop_elements
            .iter()
            .rev()
            .find(|(element, span, _)| element == name && *span == binding_span)
            .map(|(_, _, param)| param.clone())
    }

    /// Return the caller-visible parameter whose elements a `for` loop's variable is a view into, resolved before the
    /// loop's own bindings are defined (so `for items in items:` reads the parameter).
    ///
    /// Only a loop over a list parameter itself, over a field of it, or over the variable of an enclosing such loop
    /// iterates the list in place, and only when its elements are not `int`, `float` or `bool`. Any other iterable (a
    /// call such as `list(items)` or `enumerate(items)`, a method such as `items.clone()` or `table.values()`, an
    /// element such as `items[0]`) yields copies, and a change through its variable does not reach the parameter.
    pub(in crate::typechecker) fn loop_view_param(&self, iter: &Spanned<Expr>) -> Option<String> {
        let mut node = iter;
        while let Expr::Paren(inner) | Expr::Field(inner, _) = &node.node {
            node = inner;
        }
        let Expr::Ident(root) = &node.node else {
            return None;
        };
        let yields_views = self
            .type_info
            .expr_type(iter.span)
            .is_some_and(list_of_changeable_elements);
        if !yields_views {
            return None;
        }
        self.caller_visible_param_reached_by(root)
    }

    /// Enter the body of `for <pattern> in ...`, whose bindings are already defined; returns the state to restore with
    /// [`Self::exit_for_loop_body`].
    ///
    /// Every variable the pattern binds is a loop variable. When `param` names the caller-visible parameter the loop
    /// iterates in place ([`Self::loop_view_param`]), each variable is a view into its elements, so a change through
    /// it is a change to the parameter.
    pub(in crate::typechecker) fn enter_for_loop_body(
        &mut self,
        pattern: &Pattern,
        param: Option<String>,
    ) -> (usize, usize) {
        let mut names = Vec::new();
        collect_pattern_bindings(pattern, &mut names);
        let bindings = names
            .into_iter()
            .filter_map(|name| {
                let span = self.lookup_symbol(&name)?.span;
                Some((name, span))
            })
            .collect::<Vec<_>>();
        let previous_variables = self.mut_params.loop_variables.len();
        self.mut_params
            .loop_variables
            .extend(bindings.iter().map(|(_, span)| *span));
        let Some(body) = &mut self.mut_params.current else {
            return (0, previous_variables);
        };
        let previous_elements = body.loop_elements.len();
        if let Some(param) = param {
            body.loop_elements
                .extend(bindings.into_iter().map(|(name, span)| (name, span, param.clone())));
        }
        (previous_elements, previous_variables)
    }

    /// Leave a loop body entered with [`Self::enter_for_loop_body`].
    pub(in crate::typechecker) fn exit_for_loop_body(
        &mut self,
        (previous_elements, previous_variables): (usize, usize),
    ) {
        self.mut_params.loop_variables.truncate(previous_variables);
        if let Some(body) = &mut self.mut_params.current {
            body.loop_elements.truncate(previous_elements);
        }
    }

    /// Record that the current body changes the caller-visible parameter a place is rooted at, if it is rooted at one.
    ///
    /// A change made inside a closure is a closure holding the parameter, which is refused.
    fn note_place_change(&mut self, place: &Spanned<Expr>) {
        let Some(root) = place_root_name(place) else {
            return;
        };
        let Some(param) = self.caller_visible_param_reached_by(root) else {
            return;
        };
        let in_closure = self
            .mut_params
            .current
            .as_ref()
            .is_some_and(|body| body.closure_depth > 0);
        if in_closure {
            self.refuse_held_mut_param(&param, place.span);
        }
        if let Some(body) = &self.mut_params.current {
            self.mut_params
                .changed
                .entry(body.identity.clone())
                .or_default()
                .insert(param);
        }
    }

    // ========================================================================
    // Holding a caller-visible parameter
    // ========================================================================

    /// Refuse `value` when it is a caller-visible `mut` parameter itself, in a position that holds it in a new binding
    /// or another value: an assignment's value, a field or element stored, or a `break` value.
    pub(in crate::typechecker) fn refuse_mut_param_held_by(&mut self, value: &Spanned<Expr>) {
        match &value.node {
            Expr::Paren(inner) => self.refuse_mut_param_held_by(inner),
            Expr::Ident(name) => {
                if let Some(param) = self.current_caller_visible_param(name) {
                    self.refuse_held_mut_param(&param, value.span);
                }
            }
            _ => {}
        }
    }

    /// Refuse each caller-visible `mut` parameter an expression holds directly in a new value: an element of a tuple,
    /// list, set or dict literal or of a comprehension, an argument of a construction (a model, class or newtype, an
    /// enum variant, `Some`, `Ok`, `Err`) or a `partial` preset, the value of a `match` arm, an `if` branch or a
    /// `yield`, the body of a closure, and the scrutinee of a `match` whose arm binds the whole value to a name.
    ///
    /// A value is only refused where it is the parameter itself; a name the arm's pattern, the branch, a closure
    /// parameter or a comprehension's `for` clause rebinds in its own scope is not the parameter.
    pub(in crate::typechecker) fn refuse_mut_params_held_in(&mut self, expr: &Spanned<Expr>) {
        if self.mut_params.current.is_none() {
            return;
        }
        match &expr.node {
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.refuse_mut_param_held_by(item);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    if let ListEntry::Element(item) = entry {
                        self.refuse_mut_param_held_by(item);
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    if let DictEntry::Pair(key, value) = entry {
                        self.refuse_mut_param_held_by(key);
                        self.refuse_mut_param_held_by(value);
                    }
                }
            }
            Expr::ListComp(comp) => {
                if !value_is_bound_by_clauses(&comp.expr, &comp.clauses) {
                    self.refuse_mut_param_held_by(&comp.expr);
                }
            }
            Expr::DictComp(comp) => {
                for value in [&comp.key, &comp.value] {
                    if !value_is_bound_by_clauses(value, &comp.clauses) {
                        self.refuse_mut_param_held_by(value);
                    }
                }
            }
            Expr::Generator(generator) => {
                if !value_is_bound_by_clauses(&generator.expr, &generator.clauses) {
                    self.refuse_mut_param_held_by(&generator.expr);
                }
            }
            Expr::Yield(Some(value)) => self.refuse_mut_param_held_by(value),
            Expr::Closure(params, body) => {
                if !bare_name(body).is_some_and(|name| params.iter().any(|param| param.node.name == name)) {
                    self.refuse_mut_param_held_by(body);
                }
            }
            Expr::Partial(partial) => {
                for arg in &partial.args {
                    self.refuse_mut_param_held_by(&arg.value);
                }
            }
            Expr::Constructor(_, args) => self.refuse_mut_params_passed_in(args),
            Expr::Call(callee, _, args) if self.callee_constructs_a_value(callee) => {
                self.refuse_mut_params_passed_in(args);
            }
            Expr::MethodCall(base, variant, _, args) if self.names_an_enum_variant(base, variant) => {
                self.refuse_mut_params_passed_in(args);
            }
            Expr::Match(scrutinee, arms) => {
                if arms.iter().any(|arm| pattern_binds_whole_value(&arm.node.pattern.node)) {
                    self.refuse_mut_param_held_by(scrutinee);
                }
                for arm in arms {
                    let value = match &arm.node.body {
                        MatchBody::Expr(value) => Some(value),
                        MatchBody::Block(body) => trailing_value(body),
                    };
                    if let Some(value) = value
                        && !value_is_rebound_by_pattern(value, &arm.node.pattern.node)
                    {
                        self.refuse_mut_param_held_by(value);
                    }
                }
            }
            Expr::If(if_expr) => {
                for body in std::iter::once(&if_expr.then_body).chain(if_expr.else_body.as_ref()) {
                    if let Some(value) = trailing_value(body)
                        && !value_is_rebound_in(value, body)
                    {
                        self.refuse_mut_param_held_by(value);
                    }
                }
            }
            _ => {}
        }
    }

    /// Refuse each argument of a construction that is a caller-visible `mut` parameter itself.
    fn refuse_mut_params_passed_in(&mut self, args: &[CallArg]) {
        for arg in args {
            if let CallArg::Positional(value) | CallArg::Named(_, value) = arg {
                self.refuse_mut_param_held_by(value);
            }
        }
    }

    /// Return whether a call's callee constructs a value that holds its arguments: a model, class or newtype, an enum
    /// variant, or a built-in constructor such as `Some` or `Ok`.
    fn callee_constructs_a_value(&self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => {
                constructors::from_str(name).is_some()
                    || self.lookup_symbol(name).is_some_and(|symbol| {
                        matches!(
                            symbol.kind,
                            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
                                | SymbolKind::Variant(_)
                        )
                    })
            }
            Expr::Field(base, variant) => self.names_an_enum_variant(base, variant),
            _ => false,
        }
    }

    /// Return whether `base.variant` names a variant of an enum, which `Wrap.Held(...)` constructs.
    fn names_an_enum_variant(&self, base: &Spanned<Expr>, variant: &str) -> bool {
        match &base.node {
            Expr::Ident(name) => self.lookup_symbol(name).is_some_and(|symbol| match &symbol.kind {
                SymbolKind::Type(TypeInfo::Enum(info)) => info.variants.iter().any(|known| known == variant),
                _ => false,
            }),
            _ => false,
        }
    }

    /// Enter a closure checked inside the current body; a caller-visible parameter it changes is refused as held.
    pub(in crate::typechecker) fn enter_mut_param_closure(&mut self) {
        if let Some(body) = &mut self.mut_params.current {
            body.closure_depth += 1;
        }
    }

    /// Leave a closure entered with [`Self::enter_mut_param_closure`].
    pub(in crate::typechecker) fn exit_mut_param_closure(&mut self) {
        if let Some(body) = &mut self.mut_params.current {
            body.closure_depth = body.closure_depth.saturating_sub(1);
        }
    }

    /// Report the caller-visible parameter `param` held at `span`, once per span.
    fn refuse_held_mut_param(&mut self, param: &str, span: Span) {
        let copy = self.mut_param_copy_route(param);
        let error = errors::caller_visible_mut_parameter_held(param, &copy, span);
        let already_reported = self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message);
        if !already_reported {
            self.errors.push(error);
        }
    }

    /// Return how an independent copy of the caller-visible parameter `param` is written: `list(items)` or
    /// `str(items)` for a list or a string, and a new value built from it otherwise.
    fn mut_param_copy_route(&self, param: &str) -> MutParameterCopy {
        let Some(SymbolKind::Variable(info)) = self.lookup_symbol(param).map(|symbol| &symbol.kind) else {
            return MutParameterCopy::NewValue;
        };
        let constructor = match &info.ty {
            ResolvedType::Str => Some("str"),
            ResolvedType::Generic(name, _) if collection_type_id(name) == Some(CollectionTypeId::List) => Some("list"),
            _ => None,
        };
        match constructor {
            Some(constructor) => MutParameterCopy::Expression(format!("{constructor}({param})")),
            None => MutParameterCopy::NewValue,
        }
    }

    /// Record a field or element assignment through `object` (`items[0] = 1`, `box.count = 2`).
    pub(in crate::typechecker) fn note_mut_param_write(&mut self, object: &Spanned<Expr>) {
        self.note_place_change(object);
    }

    /// Record a method call that may change its receiver, when the receiver is rooted at a caller-visible parameter.
    ///
    /// The classification errs toward "changes": the reading builtin collection methods and a source method whose
    /// every candidate takes plain `self` read; every other method, including one the checker cannot resolve, may
    /// change the receiver, so an argument for the parameter is never silently copied when it would matter.
    pub(in crate::typechecker) fn note_mut_param_method_call(
        &mut self,
        receiver: &Spanned<Expr>,
        receiver_ty: &ResolvedType,
        method: &str,
    ) {
        if place_root_name(receiver).is_none_or(|root| self.caller_visible_param_reached_by(root).is_none()) {
            return;
        }
        if self.method_may_change_receiver(receiver_ty, method) {
            self.note_place_change(receiver);
        }
    }

    /// Record an argument passed to a Rust or C parameter that takes it exclusively, which may change it.
    pub(in crate::typechecker) fn note_mut_param_exclusive_use(&mut self, argument: &Spanned<Expr>) {
        self.note_place_change(argument);
    }

    /// Return whether calling `method` on a value of `receiver_ty` may change that value.
    fn method_may_change_receiver(&self, receiver_ty: &ResolvedType, method: &str) -> bool {
        match receiver_ty {
            ResolvedType::Generic(name, _) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) => !matches!(
                    list_methods::from_str(method),
                    Some(
                        list_methods::ListMethodId::Clone
                            | list_methods::ListMethodId::Contains
                            | list_methods::ListMethodId::Count
                            | list_methods::ListMethodId::Index
                    )
                ),
                Some(CollectionTypeId::Dict) => !matches!(
                    dict_methods::from_str(method),
                    Some(
                        dict_methods::DictMethodId::Keys
                            | dict_methods::DictMethodId::Values
                            | dict_methods::DictMethodId::Get
                            | dict_methods::DictMethodId::ContainsKey
                    )
                ),
                Some(CollectionTypeId::Set) => {
                    set_methods::from_str(method) != Some(set_methods::SetMethodId::Contains)
                }
                Some(_) => false,
                None => self.nominal_method_may_change_receiver(name, method),
            },
            ResolvedType::Named(name) => self.nominal_method_may_change_receiver(name, method),
            ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::Tuple(_)
            | ResolvedType::Unit => false,
            _ => true,
        }
    }

    /// Return whether a method of the source type `type_name` may change its receiver: any candidate takes `mut self`,
    /// or no declaration by that name is known.
    fn nominal_method_may_change_receiver(&self, type_name: &str, method: &str) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return true;
        };
        let (methods, overloads) = match info {
            TypeInfo::Class(class) => (&class.methods, &class.method_overloads),
            TypeInfo::Model(model) => (&model.methods, &model.method_overloads),
            TypeInfo::Newtype(newtype) => (&newtype.methods, &newtype.method_overloads),
            TypeInfo::Enum(enum_info) => (&enum_info.methods, &enum_info.method_overloads),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return true,
        };
        let candidates = methods
            .get(method)
            .into_iter()
            .chain(overloads.get(method).into_iter().flatten())
            .collect::<Vec<_>>();
        candidates.is_empty()
            || candidates
                .iter()
                .any(|candidate| candidate.receiver == Some(crate::ast::Receiver::Mutable))
    }

    /// Refuse rebinding a caller-visible `mut` parameter (`items = [0]`, `label += "!"`) in its own body.
    ///
    /// The caller sees the parameter's changes, not a new value bound to its name, so a rebinding could only be lost.
    /// Returns whether the name was such a parameter.
    pub(in crate::typechecker) fn refuse_caller_visible_mut_param_rebinding(&mut self, name: &str, span: Span) -> bool {
        if self.current_caller_visible_param(name).is_none() {
            return false;
        }
        self.errors
            .push(errors::caller_visible_mut_parameter_rebinding(name, span));
        true
    }

    /// Remember that the local binding declared at `target_span` holds the declared callable named by `value`.
    ///
    /// `f = extend` makes `f(items)` a call of `extend`; a call through `f` resolves to the local binding, whose
    /// declaration span is `target_span`.
    pub(in crate::typechecker) fn note_function_value_binding(&mut self, target_span: Span, value: &Spanned<Expr>) {
        if !matches!(value.node, Expr::Ident(_)) {
            return;
        }
        let Some(callee) = self.type_info.resolved_identity(value.span).cloned() else {
            return;
        };
        if self.mut_params.declared.contains_key(&callee) {
            self.mut_params
                .function_values
                .insert((target_span.start, target_span.end), callee);
        }
    }

    /// Record that the local binding `name` resolves to is reassigned, so calls through it are not taken to run the
    /// callable it was first bound to.
    pub(in crate::typechecker) fn note_local_reassignment(&mut self, name: &str) {
        if let Some(symbol) = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            && matches!(symbol.kind, SymbolKind::Variable(_))
        {
            self.mut_params
                .reassigned_locals
                .insert((symbol.span.start, symbol.span.end));
        }
    }

    // ========================================================================
    // Calls
    // ========================================================================

    /// Record each argument a call passes to a caller-visible `mut` parameter of its resolved callee.
    ///
    /// `callee` is the callee expression of a function call, or the receiver and name of a method call; `call_span` is
    /// the whole call. The call's resolved declaration is recorded at the callee expression of a function call and at
    /// the whole expression of a method call. Positional arguments bind ordinary
    /// parameters in order and named arguments bind by name; an unpacked argument ends positional binding. For a
    /// source declaration this check collected, the caller-visible parameter names are published for lowering at
    /// `call_span`, so the call passes them the way the declaration takes them. A callee known only by its callable
    /// type (a compiled library's function, a function value, a closure) has its marked parameters checked the same
    /// way, its body taken to change them. Whether an argument is refused or copied is decided once the module's
    /// bodies are known.
    pub(in crate::typechecker) fn record_mut_arguments(
        &mut self,
        callee: MutArgumentCallee<'_>,
        call_span: Span,
        args: &[CallArg],
    ) {
        let callee_span = match callee {
            MutArgumentCallee::Function(callee) => callee.span,
            MutArgumentCallee::Method { .. } => call_span,
        };
        let mut identity = self
            .type_info
            .resolved_identity(callee_span)
            .cloned()
            .or_else(|| match callee {
                MutArgumentCallee::Method { receiver, method } => self.trait_self_method_identity(receiver, method),
                MutArgumentCallee::Function(_) => None,
            });
        let mut through_local = None;
        if let Some(local) = &identity
            && local.kind == SemanticSourceTargetKind::Local
        {
            let key = (local.declaration_span.start, local.declaration_span.end);
            through_local = Some(key);
            if let Some(callee) = self.mut_params.function_values.get(&key) {
                identity = Some(callee.clone());
            }
        }
        let declared = identity
            .as_ref()
            .and_then(|identity| self.mut_params.declared.get(identity).cloned());
        let slots = match declared {
            Some(slots) => {
                self.type_info.calls.caller_visible_mut_arguments.insert(
                    (call_span.start, call_span.end),
                    slots
                        .iter()
                        .filter(|slot| slot.shows_changes_to_caller)
                        .map(|slot| slot.name.clone())
                        .collect(),
                );
                slots
            }
            None => match self.callable_type_mut_slots(callee, call_span) {
                Some(slots) => {
                    // The callable's body is not one this check reads, so its changes are unknown.
                    identity = None;
                    slots
                }
                None => return,
            },
        };
        let callee_name = identity
            .as_ref()
            .map(|identity| identity.declaration_name.clone())
            .or_else(|| {
                self.type_info
                    .resolved_identity(callee_span)
                    .map(|identity| identity.declaration_name.clone())
            })
            .unwrap_or_else(|| "the called function".to_string());
        // A method reached by trait dispatch runs whichever implementation the receiver's type provides, and an
        // override may change a parameter the declaration this call resolved to only reads.
        if let MutArgumentCallee::Method { receiver, .. } = callee
            && self.receiver_dispatches_through_a_trait(receiver)
        {
            identity = None;
        }
        let mut next_positional = Some(0usize);
        for arg in args {
            let (index, value) = match arg {
                CallArg::Positional(value) => {
                    let index = next_positional
                        .filter(|index| slots.get(*index).is_some_and(|slot| slot.kind == ParamKind::Normal));
                    next_positional = index.map(|index| index + 1);
                    (index, value)
                }
                CallArg::Named(name, value) => (
                    slots
                        .iter()
                        .position(|slot| !slot.name.is_empty() && slot.name == name.node),
                    value,
                ),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {
                    next_positional = None;
                    continue;
                }
            };
            let Some((index, slot)) = index
                .and_then(|index| slots.get(index).map(|slot| (index, slot)))
                .filter(|(_, slot)| slot.shows_changes_to_caller)
            else {
                continue;
            };
            let place = self.classify_argument_place(value);
            if let ArgumentPlace::Mutable {
                forwarded_param: Some(param),
            } = &place
                && self
                    .mut_params
                    .current
                    .as_ref()
                    .is_some_and(|body| body.closure_depth > 0)
            {
                // A closure that passes the parameter on to a parameter that may change it holds the parameter.
                self.refuse_held_mut_param(param, value.span);
            }
            if let ArgumentPlace::Mutable {
                forwarded_param: Some(param),
            } = &place
                && let Some(body) = &self.mut_params.current
            {
                match &identity {
                    Some(callee) => self.mut_params.forwards.push(MutParamForward {
                        owner: body.identity.clone(),
                        param: param.clone(),
                        callee: callee.clone(),
                        callee_param: slot.name.clone(),
                        through_local,
                    }),
                    // Passing the parameter on to a callee whose body is unknown may change it.
                    None => {
                        let owner = body.identity.clone();
                        self.mut_params.changed.entry(owner).or_default().insert(param.clone());
                    }
                }
            }
            self.mut_params.pending.push(PendingMutArgument {
                callee: identity.clone(),
                callee_name: callee_name.clone(),
                param: slot.name.clone(),
                position: index + 1,
                place,
                span: value.span,
                through_local,
            });
        }
    }

    /// Return the declaration a trait default's `self.method(...)` call names: the method of the trait being checked or
    /// of one of its supertraits.
    ///
    /// Inside a default, `self` is any adopter, so the call does not resolve to one implementation; the trait's own
    /// declaration still says which parameters are marked.
    fn trait_self_method_identity(&self, receiver: &Spanned<Expr>, method: &str) -> Option<CanonicalSymbolId> {
        if !matches!(self.type_info.expr_type(receiver.span), Some(ResolvedType::SelfType)) {
            return None;
        }
        let mut pending = vec![self.current_trait_name.clone()?];
        let mut visited = HashSet::new();
        while let Some(trait_name) = pending.pop() {
            if !visited.insert(trait_name.clone()) {
                continue;
            }
            let Some(info) = self.lookup_trait_info(&trait_name) else {
                continue;
            };
            if let Some(identity) = info.methods.get(method).and_then(|method| method.identity.clone()) {
                return Some(identity);
            }
            pending.extend(info.supertraits.iter().map(|(name, _)| name.clone()));
        }
        None
    }

    /// Return whether a method call on `receiver` is dispatched to an implementation chosen by the receiver's type at
    /// run time: the receiver is a value of a type parameter, `Self`, or a trait type, as `g` in
    /// `def run[T with Grower](g: T)` and `self` in a trait's default method are.
    fn receiver_dispatches_through_a_trait(&self, receiver: &Spanned<Expr>) -> bool {
        let Some(receiver_ty) = self.type_info.expr_type(receiver.span) else {
            return matches!(receiver.node, Expr::SelfExpr);
        };
        self.type_dispatches_through_a_trait(receiver_ty)
    }

    /// Return whether a value of `ty` calls its methods through a trait rather than on one known implementation.
    ///
    /// A type parameter (resolved as a type variable, or by name to the checker's type-variable placeholder), `Self`
    /// and a trait dispatch; a model, class, enum, newtype or Rust type names its one implementation. A name the
    /// checker cannot resolve counts as dispatching, so an override is never assumed away.
    fn type_dispatches_through_a_trait(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::TypeVar(_) | ResolvedType::SelfType => true,
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.type_dispatches_through_a_trait(inner),
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                self.lookup_symbol(name).is_none_or(|symbol| match &symbol.kind {
                    SymbolKind::Type(TypeInfo::Builtin) => true,
                    SymbolKind::Type(_) | SymbolKind::RustItem(_) => false,
                    _ => true,
                })
            }
            _ => false,
        }
    }

    /// Return the parameter slots of a callee known only by its callable type, when any of them is marked `mut`.
    ///
    /// The parameters are the ones the checker resolved the call against: for a function call, the function a callee
    /// name binds (an imported library function) or the function type of a callee value (a function value, a
    /// parameter of function type, a closure); for a method call, the method of the receiver's nominal type (a
    /// compiled library's model or class); and for any call, the call's recorded callable parameters.
    fn callable_type_mut_slots(
        &self,
        callee: MutArgumentCallee<'_>,
        call_span: Span,
    ) -> Option<Vec<DeclaredParamSlot>> {
        let resolved = match callee {
            MutArgumentCallee::Function(callee) => {
                let bound = match &callee.node {
                    Expr::Ident(name) => self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
                        SymbolKind::Function(info) => Some(info.params.as_slice()),
                        SymbolKind::Variable(info) => function_type_params(&info.ty),
                        _ => None,
                    }),
                    _ => None,
                };
                bound.or_else(|| self.type_info.expr_type(callee.span).and_then(function_type_params))
            }
            MutArgumentCallee::Method { receiver, method } => self
                .type_info
                .expr_type(receiver.span)
                .and_then(nominal_type_name)
                .and_then(|type_name| self.lookup_symbol(type_name))
                .and_then(|symbol| match &symbol.kind {
                    SymbolKind::Type(TypeInfo::Class(info)) => info.methods.get(method),
                    SymbolKind::Type(TypeInfo::Model(info)) => info.methods.get(method),
                    SymbolKind::Type(TypeInfo::Newtype(info)) => info.methods.get(method),
                    SymbolKind::Type(TypeInfo::Enum(info)) => info.methods.get(method),
                    _ => None,
                })
                .map(|info| info.params.as_slice()),
        };
        [resolved, self.type_info.call_site_callable_params(call_span)]
            .into_iter()
            .flatten()
            .map(|params| {
                params
                    .iter()
                    .map(|param| DeclaredParamSlot {
                        name: param.name.clone().unwrap_or_default(),
                        kind: param.kind,
                        shows_changes_to_caller: callable_param_is_marked(param),
                    })
                    .collect::<Vec<_>>()
            })
            .find(|slots| slots.iter().any(|slot| slot.shows_changes_to_caller))
    }

    /// Classify an argument by the storage a change to it would reach.
    fn classify_argument_place(&self, expr: &Spanned<Expr>) -> ArgumentPlace {
        let mut node = expr;
        let mut through_element = false;
        loop {
            match &node.node {
                Expr::Field(base, _) | Expr::Paren(base) => node = base,
                Expr::Index(base, _) => {
                    through_element = true;
                    node = base;
                }
                _ => break,
            }
        }
        let root = match &node.node {
            Expr::Ident(name) => name.as_str(),
            Expr::SelfExpr => keywords::as_str(KeywordId::SelfKw),
            _ => return ArgumentPlace::Temporary,
        };
        let Some(symbol) = self.lookup_symbol(root) else {
            return ArgumentPlace::Temporary;
        };
        let is_mutable = match &symbol.kind {
            SymbolKind::Variable(info) => info.is_mutable,
            SymbolKind::Static(_) => {
                return ArgumentPlace::Detached {
                    place: MutArgumentPlace::Static,
                };
            }
            _ => return ArgumentPlace::Temporary,
        };
        if through_element {
            return ArgumentPlace::Detached {
                place: MutArgumentPlace::Element,
            };
        }
        if !is_mutable && self.mut_params.loop_variables.contains(&symbol.span) {
            return ArgumentPlace::Detached {
                place: MutArgumentPlace::LoopVariable(root.to_string()),
            };
        }
        if is_mutable {
            return ArgumentPlace::Mutable {
                forwarded_param: self.caller_visible_param_reached_by(root),
            };
        }
        ArgumentPlace::Immutable {
            binding: matches!(expr.node, Expr::Ident(_)).then(|| root.to_string()),
        }
    }

    /// Decide every recorded argument once the module's bodies have been checked.
    ///
    /// A callee changes a caller-visible parameter when its body writes through it, calls a method that may change it,
    /// or passes it on to a parameter another callee changes; a callee whose body this
    /// check did not read (an imported declaration, a trait method without a default, a method reached by trait
    /// dispatch, a callable known only by its type) is taken to change it. An immutable binding or field, an element or
    /// a static passed to a changed parameter is refused with `INCAN-T0117`; an immutable binding or field passed to an
    /// unchanged one is published for lowering as a copy.
    pub(in crate::typechecker) fn resolve_mut_arguments(&mut self) {
        let changed = self.mut_param_change_closure();
        let pending = std::mem::take(&mut self.mut_params.pending);
        for argument in pending {
            let known_callee = argument.callee.as_ref().filter(|callee| {
                self.mut_params.bodies_checked.contains(*callee)
                    && !self.call_goes_through_a_reassigned_local(argument.through_local)
            });
            let change = match known_callee {
                Some(_) => MutParameterChange::Changes,
                None => MutParameterChange::MayChange,
            };
            let changes = known_callee.is_none_or(|callee| changed.contains(&(callee.clone(), argument.param.clone())));
            let refused_place = match (&argument.place, changes) {
                (ArgumentPlace::Immutable { binding }, true) => Some(match binding {
                    Some(name) => MutArgumentPlace::Binding(name.clone()),
                    None => MutArgumentPlace::Field,
                }),
                (ArgumentPlace::Detached { place }, true) => Some(place.clone()),
                (ArgumentPlace::Immutable { .. }, false) => {
                    self.type_info
                        .calls
                        .mut_argument_copies
                        .insert((argument.span.start, argument.span.end));
                    None
                }
                _ => None,
            };
            let Some(place) = refused_place else {
                continue;
            };
            let parameter = if argument.param.is_empty() {
                MutParameterLabel::Position(argument.position)
            } else {
                MutParameterLabel::Named(&argument.param)
            };
            let error = errors::immutable_argument_to_mut_parameter(
                parameter,
                &argument.callee_name,
                change,
                place,
                argument.span,
            );
            let already_reported = self
                .errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message);
            if !already_reported {
                self.errors.push(error);
            }
        }
    }

    /// Return whether a call went through a local that is reassigned somewhere in the module, so the callable it runs
    /// is not known at the call.
    fn call_goes_through_a_reassigned_local(&self, through_local: Option<(usize, usize)>) -> bool {
        through_local.is_some_and(|local| self.mut_params.reassigned_locals.contains(&local))
    }

    /// Return every (callable, parameter) pair whose caller-visible parameter the callable changes, transitively.
    fn mut_param_change_closure(&self) -> HashSet<(CanonicalSymbolId, String)> {
        let mut changed = self
            .mut_params
            .changed
            .iter()
            .flat_map(|(identity, params)| params.iter().map(move |param| (identity.clone(), param.clone())))
            .collect::<HashSet<_>>();
        loop {
            let mut grew = false;
            for forward in &self.mut_params.forwards {
                let callee_changes = !self.mut_params.bodies_checked.contains(&forward.callee)
                    || self.call_goes_through_a_reassigned_local(forward.through_local)
                    || changed.contains(&(forward.callee.clone(), forward.callee_param.clone()));
                if callee_changes && changed.insert((forward.owner.clone(), forward.param.clone())) {
                    grew = true;
                }
            }
            if !grew {
                return changed;
            }
        }
    }
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
