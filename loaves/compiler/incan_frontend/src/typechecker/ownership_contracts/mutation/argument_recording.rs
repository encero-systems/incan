//! Recording of `mut` arguments, forwarding, trait dispatch and argument places at call sites.

use super::*;

impl TypeChecker {
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
            let key = local.declaration_span;
            through_local = Some(key);
            if let Some(callee) = self.mutation.function_values.get(&key) {
                identity = Some(callee.clone());
            }
        }
        let declared = identity
            .as_ref()
            .and_then(|identity| self.mutation.declared.get(identity).cloned());
        let slots = match declared {
            Some(slots) => slots,
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
            self.record_mut_argument_forwarding(&place, identity.as_ref(), &slot.name, through_local, value.span);
            let uncopyable_type = self
                .type_info
                .expr_type(value.span)
                .filter(|ty| self.value_type_cannot_be_copied(ty))
                .map(ToString::to_string);
            let shared_native_field = self.shared_native_field_argument(value);
            self.mutation.pending.push(PendingMutArgument {
                order: self.check_work.mutation_arguments,
                callee: identity.clone(),
                callee_name: callee_name.clone(),
                param: slot.name.clone(),
                position: index + 1,
                place,
                uncopyable_type,
                shared_native_field,
                span: value.span,
                through_local,
            });
            self.check_work.mutation_arguments += 1;
        }
    }

    /// Report whether a native field argument is projected through a shared native receiver.
    ///
    /// Its source-facing collection may be cloneable, but the retained native place is still a loan from the owner;
    /// a `mut` source parameter cannot manufacture exclusive access to that place.
    fn shared_native_field_argument(&self, expr: &Spanned<Expr>) -> bool {
        let mut node = expr;
        while let Expr::Paren(inner) = &node.node {
            node = inner;
        }
        let Expr::Field(base, _) = &node.node else {
            return false;
        };
        self.type_info.rust.field_values.contains_key(&node.span)
            && matches!(self.type_info.expr_type(base.span), Some(ResolvedType::Ref(_)))
    }

    /// Retain caller-visible forwarding effects before recording the argument's eventual copy or refusal policy.
    ///
    /// A closure forwarding a mutable parameter holds it. An unknown callee conservatively changes the parameter;
    /// a known declaration records an edge whose effects are resolved after all bodies have been checked.
    pub(super) fn record_mut_argument_forwarding(
        &mut self,
        place: &ArgumentPlace,
        identity: Option<&CanonicalSymbolId>,
        callee_param: &str,
        through_local: Option<Span>,
        span: Span,
    ) {
        let ArgumentPlace::Mutable {
            forwarded_param: Some(param),
        } = place
        else {
            return;
        };
        if self
            .mutation
            .current
            .as_ref()
            .is_some_and(|body| body.closure_depth > 0)
        {
            self.refuse_held_mut_param(param, span);
        }
        if let Some(body) = &self.mutation.current {
            match identity {
                Some(callee) => self.mutation.source.forwards.push(MutParamForward {
                    owner: body.identity.clone(),
                    param: param.clone(),
                    callee: callee.clone(),
                    callee_param: callee_param.to_owned(),
                    through_local,
                }),
                None => {
                    let owner = body.identity.clone();
                    self.mutation
                        .source
                        .changed
                        .entry(owner)
                        .or_default()
                        .insert(param.clone());
                }
            }
        }
    }

    /// Return the declaration a trait default's `self.method(...)` call names: the method of the trait being checked or
    /// of one of its supertraits.
    ///
    /// Inside a default, `self` is any adopter, so the call does not resolve to one implementation; the trait's own
    /// declaration still says which parameters are marked.
    pub(super) fn trait_self_method_identity(
        &self,
        receiver: &Spanned<Expr>,
        method: &str,
    ) -> Option<CanonicalSymbolId> {
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
    pub(super) fn receiver_dispatches_through_a_trait(&self, receiver: &Spanned<Expr>) -> bool {
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
    pub(super) fn type_dispatches_through_a_trait(&self, ty: &ResolvedType) -> bool {
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
    pub(super) fn callable_type_mut_slots(
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
    pub(super) fn classify_argument_place(&self, expr: &Spanned<Expr>) -> ArgumentPlace {
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
        // A closure holds its own copy of a local it captures, so a change the call makes would not reach the local
        // (#1561); a caller-visible parameter is refused as held by the closure instead.
        if let Some(symbol_id) = self.symbols.lookup(root)
            && self.local_is_captured_by_closure(symbol_id)
            && !self.reaches_caller_visible_param(root)
        {
            return ArgumentPlace::Detached {
                place: MutArgumentPlace::Captured(root.to_string()),
            };
        }
        if through_element {
            return ArgumentPlace::Detached {
                place: MutArgumentPlace::Element,
            };
        }
        if !is_mutable
            && let Some((_, destructured)) = self
                .mutation
                .loop_variables
                .iter()
                .find(|(span, _)| *span == symbol.span)
        {
            return ArgumentPlace::Detached {
                place: MutArgumentPlace::LoopVariable {
                    name: root.to_string(),
                    destructured: *destructured,
                },
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
}
