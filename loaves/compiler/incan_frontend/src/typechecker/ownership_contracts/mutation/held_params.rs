//! Changes through `mut` parameters and refusals of parameters held, passed or rebound.

use super::*;

impl TypeChecker {
    /// Record that the current body changes the place `place` names, when it is rooted at a view or a caller-visible
    /// parameter.
    ///
    /// `certain` says whether the change is known rather than assumed: a method the checker cannot classify counts as
    /// a change to a parameter and to a writable place, but a place that does not permit changes refuses only a known
    /// one.
    pub(super) fn note_place_change(&mut self, place: &Spanned<Expr>, certain: bool) {
        let Some(root) = place_root_name(place) else {
            return;
        };
        let Some(source) = self.view_source_of(root) else {
            return;
        };
        self.note_param_change(source, root, place.span, certain);
    }

    /// Record a change at `span` through the binding `binding`, which reaches the place `source` describes.
    ///
    /// A change through a name a pattern bound from a writable place records each scrutinee it goes through as matched
    /// in place. A `certain` change through a name a pattern or a `for` loop bound from a read-only place is refused:
    /// the loop would otherwise borrow a place declared without `mut` mutably, and the pattern would change a copy. A
    /// change to a caller-visible parameter is recorded as the parameter's, and when made inside a closure it is a
    /// closure holding the parameter, which is refused.
    pub(super) fn note_param_change(&mut self, source: ViewSource, binding: &str, span: Span, certain: bool) {
        let ViewSource {
            param,
            access,
            scrutinees,
        } = source;
        match access {
            ViewAccess::Writable => {
                for scrutinee in scrutinees {
                    self.type_info.record_match_scrutinee_changed_in_place(scrutinee);
                }
            }
            ViewAccess::ReadOnly(place) if certain => {
                let place = match &place {
                    ReadOnlyPlace::Binding(name) => ReadOnlyScrutinee::Binding(name),
                    ReadOnlyPlace::Parameter(name) => ReadOnlyScrutinee::Parameter(name),
                    ReadOnlyPlace::Static(name) => ReadOnlyScrutinee::Static(name),
                    ReadOnlyPlace::StaticAlias(alias, static_name) => {
                        ReadOnlyScrutinee::StaticAlias { alias, static_name }
                    }
                    ReadOnlyPlace::GeneratorItems(place) => ReadOnlyScrutinee::GeneratorItems(place),
                    ReadOnlyPlace::Captured(name) => ReadOnlyScrutinee::Captured(name),
                    ReadOnlyPlace::SelfReceiver => ReadOnlyScrutinee::SelfReceiver,
                    ReadOnlyPlace::DictValue => ReadOnlyScrutinee::DictValue,
                };
                let error = errors::change_through_binding_of_read_only_place(binding, place, span);
                let already_reported = self
                    .errors
                    .iter()
                    .any(|existing| existing.span == error.span && existing.message == error.message);
                if !already_reported {
                    self.errors.push(error);
                }
                return;
            }
            ViewAccess::ReadOnly(_) | ViewAccess::Owned => return,
        }
        let Some(param) = param else {
            return;
        };
        let in_closure = self
            .mutation
            .current
            .as_ref()
            .is_some_and(|body| body.closure_depth > 0);
        if in_closure {
            self.refuse_held_mut_param(&param, span);
        }
        if let Some(body) = &self.mutation.current {
            self.mutation
                .source
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
        if self.mutation.current.is_none() {
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
    pub(super) fn refuse_mut_params_passed_in(&mut self, args: &[CallArg]) {
        for arg in args {
            if let CallArg::Positional(value) | CallArg::Named(_, value) = arg {
                self.refuse_mut_param_held_by(value);
            }
        }
    }

    /// Return whether a call's callee constructs a value that holds its arguments: a model, class or newtype, an enum
    /// variant, or a built-in constructor such as `Some` or `Ok`.
    pub(super) fn callee_constructs_a_value(&self, callee: &Spanned<Expr>) -> bool {
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
    pub(super) fn names_an_enum_variant(&self, base: &Spanned<Expr>, variant: &str) -> bool {
        match &base.node {
            Expr::Ident(name) => self.lookup_symbol(name).is_some_and(|symbol| match &symbol.kind {
                SymbolKind::Type(TypeInfo::Enum(info)) => info.variants.iter().any(|known| known == variant),
                _ => false,
            }),
            _ => false,
        }
    }

    /// Enter a closure checked inside the current body; a caller-visible parameter it changes is refused as held, and a
    /// scoped C text view it reads is refused as captured.
    pub(in crate::typechecker) fn enter_mut_param_closure(&mut self) {
        self.closure_depth += 1;
        if let Some(body) = &mut self.mutation.current {
            body.closure_depth += 1;
        }
    }

    /// Leave a closure entered with [`Self::enter_mut_param_closure`].
    pub(in crate::typechecker) fn exit_mut_param_closure(&mut self) {
        self.closure_depth = self.closure_depth.saturating_sub(1);
        if let Some(body) = &mut self.mutation.current {
            body.closure_depth = body.closure_depth.saturating_sub(1);
        }
    }

    /// Report the caller-visible parameter `param` held at `span`, once per span.
    pub(super) fn refuse_held_mut_param(&mut self, param: &str, span: Span) {
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
    pub(super) fn mut_param_copy_route(&self, param: &str) -> MutParameterCopy {
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
        self.note_place_change(object, true);
    }

    /// Record a method call that may change its receiver, when the receiver is rooted at a caller-visible parameter.
    ///
    /// The call counts unless every declaration it may run only reads its receiver ([`Self::method_receiver_change`]):
    /// a method the checker cannot classify, including one it cannot resolve, may change the receiver, so an argument
    /// for the parameter is never silently copied when it would matter. A method of a `Generator` advances it.
    pub(in crate::typechecker) fn note_mut_param_method_call(
        &mut self,
        receiver: &Spanned<Expr>,
        receiver_ty: &ResolvedType,
        method: &str,
    ) {
        if place_root_name(receiver).is_none_or(|root| self.view_source_of(root).is_none()) {
            return;
        }
        match self.method_receiver_change(receiver_ty, method, receiver.span) {
            ReceiverChange::Reads => {}
            ReceiverChange::Changes => self.note_place_change(receiver, true),
            ReceiverChange::Unknown => self.note_place_change(receiver, false),
        }
    }

    /// Record an argument passed to a Rust or C parameter that takes it exclusively, which may change it.
    pub(in crate::typechecker) fn note_mut_param_exclusive_use(&mut self, argument: &Spanned<Expr>) {
        self.note_place_change(argument, true);
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
        if self.mutation.declared.contains_key(&callee) {
            self.mutation.function_values.insert(target_span, callee);
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
            self.mutation.source.reassigned_locals.insert(symbol.span);
        }
    }
}
