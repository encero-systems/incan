//! `mut` parameters whose changes reach the caller, and the arguments a call passes to them (#1773).
//!
//! A parameter declared `mut` is a mutable binding inside its function. When its type is not `int`, `float`, `bool`
//! (or an alias of one) or a Rust type, and it is not a `*args` or `**kwargs` parameter, the function's changes to it
//! are also visible to the caller: collections, models and class objects are shared with the caller, scalars are the
//! function's own copy. This module owns that decision and the facts that follow from it:
//!
//! - which declared parameters are caller-visible, recorded for lowering by parameter so the Rust shape of the
//!   declaration and of every call agrees ([`TypeCheckInfo`](super::TypeCheckInfo) declarations);
//! - which caller-visible parameters a callable's body changes, directly or by passing them on to another callee that
//!   changes them;
//! - which call arguments are refused (`INCAN-T0117`): an immutable binding or a field of one, an element of a
//!   collection, or a static, passed to a caller-visible parameter the callee changes, where the change would fail to
//!   build or be lost;
//! - which call arguments are handed over as a copy: an immutable binding or field passed to a caller-visible parameter
//!   the callee never changes, which is what such a call always meant.
//!
//! A caller-visible parameter cannot be rebound to a new value in its body: the caller would not see the rebinding.
//! Temporaries (literals, call results) are always accepted; nothing but the callee holds them.

use std::collections::{HashMap, HashSet};

use crate::ast::{CallArg, Expr, Param, ParamKind, Span, Spanned, Type};
use crate::diagnostics::errors::{self, MutArgumentPlace};
use crate::symbols::{CallableParam, ResolvedType, SymbolKind, TypeInfo};
use incan_lang::lang::keywords::{self, KeywordId};
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

/// One argument recorded at a call, resolved once every body of the module has been checked.
#[derive(Debug, Clone)]
struct PendingMutArgument {
    /// The callee's declaration identity, when the call resolved to one; `None` for a callable known only by its type.
    callee: Option<CanonicalSymbolId>,
    /// The callee's name as the refusal names it.
    callee_name: String,
    param: String,
    place: ArgumentPlace,
    span: Span,
}

/// The body currently being checked and its caller-visible `mut` parameters, by name, with their declaration spans.
#[derive(Debug, Clone)]
struct MutParamBody {
    identity: CanonicalSymbolId,
    params: HashMap<String, Span>,
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
    /// A caller-visible parameter passed on to another callee's caller-visible parameter: (owner, param, callee,
    /// param).
    forwards: Vec<(CanonicalSymbolId, String, CanonicalSymbolId, String)>,
    /// Local bindings that hold a declared callable by reference, `f = extend`, keyed by the binding's declaration
    /// span, so a call through them is checked too.
    function_values: HashMap<(usize, usize), CanonicalSymbolId>,
    /// Arguments waiting for the module's change facts.
    pending: Vec<PendingMutArgument>,
    /// The body being checked, when it declares caller-visible parameters.
    current: Option<MutParamBody>,
}

impl TypeChecker {
    // ========================================================================
    // Declarations
    // ========================================================================

    /// Return whether a declared parameter shows the callee's changes to the caller: whether it is marked.
    ///
    /// Only an ordinary `mut` parameter can, and not every one: a parameter whose type resolves to `int`, `float` or
    /// `bool`, also through an alias, receives its own copy of the argument, and a Rust-typed parameter receives the
    /// value itself. For those `mut` only makes the parameter reassignable inside the body. The decision is made here
    /// once, on the resolved type, and recorded for lowering. It is the marker rule of #1701's
    /// `def_param_shows_changes_to_caller` (`mut_marker.rs`), which replaces this function when that change lands.
    fn mut_param_shows_changes_to_caller(&self, param: &Param, resolved: &ResolvedType) -> bool {
        if !param.is_mut || param.kind != ParamKind::Normal {
            return false;
        }
        if matches!(
            self.expand_type_aliases(resolved.clone()),
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::RustPath(_)
        ) {
            return false;
        }
        let head = match &param.ty.node {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) | Type::Generic(name, _) => Some(name.as_str()),
            Type::Qualified(segments) => segments.first().map(String::as_str),
            _ => None,
        };
        !head.is_some_and(|name| {
            self.lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        })
    }

    /// Record one source callable's declared `mut` parameters: for lowering, and for calls when any is caller-visible.
    ///
    /// `declared` and `resolved` are the declaration's parameters without the receiver, in declaration order, and
    /// `identity` is the declaration identity calls to it resolve to. Every ordinary `mut` parameter's marker is
    /// published for lowering, keyed by the parameter, including those of imported source modules this check
    /// collects, so a trait default expanded into an adopter in another module is lowered with its own module's
    /// markers.
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
                shows_changes_to_caller: self.mut_param_shows_changes_to_caller(&param.node, &resolved.ty),
            })
            .collect::<Vec<_>>();
        for (param, slot) in declared.iter().zip(&slots) {
            if param.node.is_mut && param.node.kind == ParamKind::Normal {
                self.type_info.declarations.record_mut_param_caller_visibility(
                    param.span,
                    &param.node.name,
                    slot.shows_changes_to_caller,
                );
            }
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
            .filter(|(param, resolved)| self.mut_param_shows_changes_to_caller(&param.node, resolved))
            .map(|(param, _)| (param.node.name.clone(), param.span))
            .collect::<HashMap<_, _>>();
        self.mut_params.bodies_checked.insert(identity.clone());
        let next = MutParamBody {
            identity,
            params: caller_visible,
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

    /// Record that the current body changes the caller-visible parameter a place is rooted at, if it is rooted at one.
    fn note_place_change(&mut self, place: &Spanned<Expr>) {
        let Some(root) = place_root_name(place) else {
            return;
        };
        let Some(param) = self.current_caller_visible_param(root) else {
            return;
        };
        if let Some(body) = &self.mut_params.current {
            self.mut_params
                .changed
                .entry(body.identity.clone())
                .or_default()
                .insert(param);
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
        if place_root_name(receiver).is_none_or(|root| self.current_caller_visible_param(root).is_none()) {
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

    // ========================================================================
    // Calls
    // ========================================================================

    /// Record each argument a call passes to a caller-visible `mut` parameter of its resolved callee.
    ///
    /// `callee_span` is the span the call recorded its resolved declaration at: the callee expression of a function
    /// call, the whole expression of a method call; `call_span` is the whole call. Positional arguments bind ordinary
    /// parameters in order and named arguments bind by name; an unpacked argument ends positional binding. For a
    /// source declaration this check collected, the caller-visible parameter names are published for lowering at
    /// `call_span`, so the call passes them the way the declaration takes them. A callee known only by its callable
    /// type (a compiled library's function, a function value, a closure) has its marked parameters checked the same
    /// way, its body taken to change them. Whether an argument is refused or copied is decided once the module's
    /// bodies are known.
    pub(in crate::typechecker) fn record_mut_arguments(
        &mut self,
        callee_span: Span,
        call_span: Span,
        args: &[CallArg],
    ) {
        let mut identity = self.type_info.resolved_identity(callee_span).cloned();
        if let Some(local) = &identity
            && local.kind == SemanticSourceTargetKind::Local
            && let Some(callee) = self
                .mut_params
                .function_values
                .get(&(local.declaration_span.start, local.declaration_span.end))
        {
            identity = Some(callee.clone());
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
            None => match self.callable_type_mut_slots(callee_span, call_span) {
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
        let mut next_positional = Some(0usize);
        for arg in args {
            let (slot, value) = match arg {
                CallArg::Positional(value) => {
                    let slot = next_positional
                        .and_then(|index| slots.get(index))
                        .filter(|slot| slot.kind == ParamKind::Normal);
                    next_positional = slot.and(next_positional.map(|index| index + 1));
                    (slot, value)
                }
                CallArg::Named(name, value) => (slots.iter().find(|slot| slot.name == name.node), value),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {
                    next_positional = None;
                    continue;
                }
            };
            let Some(slot) = slot.filter(|slot| slot.shows_changes_to_caller) else {
                continue;
            };
            let place = self.classify_argument_place(value);
            if let ArgumentPlace::Mutable {
                forwarded_param: Some(param),
            } = &place
                && let Some(body) = &self.mut_params.current
            {
                match &identity {
                    Some(callee) => self.mut_params.forwards.push((
                        body.identity.clone(),
                        param.clone(),
                        callee.clone(),
                        slot.name.clone(),
                    )),
                    // Passing the parameter on to a callee whose body is unknown may change it.
                    None => {
                        let owner = body.identity.clone();
                        self.mut_params
                            .changed
                            .entry(owner)
                            .or_default()
                            .insert(param.clone());
                    }
                }
            }
            self.mut_params.pending.push(PendingMutArgument {
                callee: identity.clone(),
                callee_name: callee_name.clone(),
                param: slot.name.clone(),
                place,
                span: value.span,
            });
        }
    }

    /// Return the parameter slots of a callee known only by its callable type, when any of them is marked `mut`.
    ///
    /// The parameters are the ones the checker resolved the call against: the call's recorded callable parameters,
    /// or, for a function call, the callee expression's function type. A method call's own expression type is its
    /// result, never its signature, so only the recorded parameters describe it.
    fn callable_type_mut_slots(&self, callee_span: Span, call_span: Span) -> Option<Vec<DeclaredParamSlot>> {
        let params = match self.type_info.call_site_callable_params(call_span) {
            Some(params) => params.to_vec(),
            None if callee_span != call_span => match self.type_info.expr_type(callee_span)? {
                ResolvedType::Function(params, _) => params.clone(),
                _ => return None,
            },
            None => return None,
        };
        let slots = params
            .iter()
            .map(|param| DeclaredParamSlot {
                name: param.name.clone().unwrap_or_default(),
                kind: param.kind,
                shows_changes_to_caller: callable_param_is_marked(param),
            })
            .collect::<Vec<_>>();
        slots.iter().any(|slot| slot.shows_changes_to_caller).then_some(slots)
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
        if is_mutable {
            return ArgumentPlace::Mutable {
                forwarded_param: self.current_caller_visible_param(root),
            };
        }
        ArgumentPlace::Immutable {
            binding: matches!(expr.node, Expr::Ident(_)).then(|| root.to_string()),
        }
    }

    /// Decide every recorded argument once the module's bodies have been checked.
    ///
    /// A callee changes a caller-visible parameter when its body writes through it, calls a method that may change it,
    /// or passes it on to a parameter another callee changes; a callee whose body this check did not read (an imported
    /// declaration, a trait method without a default) is taken to change it. An immutable binding or field, an element
    /// or a static passed to a changed parameter is refused with `INCAN-T0117`; an immutable binding or field passed
    /// to an unchanged one is published for lowering as a copy.
    pub(in crate::typechecker) fn resolve_mut_arguments(&mut self) {
        let changed = self.mut_param_change_closure();
        let pending = std::mem::take(&mut self.mut_params.pending);
        for argument in pending {
            let changes = argument.callee.as_ref().is_none_or(|callee| {
                !self.mut_params.bodies_checked.contains(callee)
                    || changed.contains(&(callee.clone(), argument.param.clone()))
            });
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
            let error = errors::immutable_argument_to_mut_parameter(
                &argument.param,
                &argument.callee_name,
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
            for (owner, param, callee, callee_param) in &self.mut_params.forwards {
                let callee_changes = !self.mut_params.bodies_checked.contains(callee)
                    || changed.contains(&(callee.clone(), callee_param.clone()));
                if callee_changes && changed.insert((owner.clone(), param.clone())) {
                    grew = true;
                }
            }
            if !grew {
                return changed;
            }
        }
    }
}

/// Return whether a parameter of a callable known only by its type is marked `mut`: whether the callable's changes to
/// it reach the caller.
///
/// This is the one place the checker reads that marker, for a compiled library's function, a function value and a
/// closure alike. It is #1701's `CallableParam::is_mut`, carried across a library boundary by `ParamExport::is_mut`;
/// callable types in this tree carry no marker, so no parameter reached this way is marked.
fn callable_param_is_marked(_param: &CallableParam) -> bool {
    false
}

/// Opaque saved state of the enclosing body, restored by [`TypeChecker::exit_mut_param_body`].
#[derive(Debug)]
pub(crate) struct MutParamBodyState(Option<MutParamBody>);

/// Return the binding name a place expression is rooted at, following fields, elements and parentheses.
fn place_root_name(expr: &Spanned<Expr>) -> Option<&str> {
    match &expr.node {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Field(base, _) | Expr::Index(base, _) | Expr::Paren(base) => place_root_name(base),
        _ => None,
    }
}
