//! `mut` parameter bodies, caller-visible parameters, and the views loops, comprehensions and patterns take.

use super::*;

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
            self.mutation.declared.insert(identity.clone(), slots);
        }
    }

    // ========================================================================
    // Bodies
    // ========================================================================

    /// Enter the body of a function or method declared as `name` at `span`, returning the enclosing body's state.
    ///
    /// `kind` selects the identity namespace the declaration was collected under. The body's ordinary caller-visible
    /// `mut` parameters become the places whose changes and rebinding this module tracks until
    /// [`Self::exit_mut_param_body`] restores the returned state. Explicit ownership inputs retain their separate
    /// source obligations and must not acquire this legacy parameter-rebinding prohibition.
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
            .enumerate()
            // Explicit modes own their access obligations. In particular, assignment through BorrowedMut writes
            // through, while assignment to Owned replaces a local slot; neither is legacy parameter rebinding.
            .filter(|(slot, _)| self.checked_input_contract(&identity, *slot).is_none())
            .filter(|(_, (param, resolved))| self.def_param_shows_changes_to_caller(&param.node, resolved))
            .map(|(_, (param, _))| (param.node.name.clone(), param.span))
            .collect::<HashMap<_, _>>();
        if !caller_visible.is_empty() {
            self.mutation.source.bodies_checked.insert(identity.clone());
        }
        let next = MutParamBody {
            identity,
            params: caller_visible,
            closure_depth: 0,
            views: Vec::new(),
        };
        MutParamBodyState(self.mutation.current.replace(next))
    }

    /// Return the declaration identity of the function or method whose body is being checked, if any.
    pub(in crate::typechecker) fn current_body_identity(&self) -> Option<&CanonicalSymbolId> {
        self.mutation.current.as_ref().map(|body| &body.identity)
    }

    /// Detach a validated declaration's mutation frame before its independent body producer is entered. Knowing a
    /// concrete body exists belongs to its statement activation. Abstract methods retain the legacy known-callable
    /// contribution with their declaration, without inventing a statement producer.
    pub(in crate::typechecker) fn suspend_mut_param_body(
        &mut self,
        previous: MutParamBodyState,
        has_statement_body: bool,
    ) -> MutParamBodyState {
        let prepared = MutParamBodyState(std::mem::replace(&mut self.mutation.current, previous.0));
        if has_statement_body
            && let Some(frame) = &prepared.0
            && !frame.params.is_empty()
        {
            self.mutation.source.bodies_checked.remove(&frame.identity);
        }
        prepared
    }

    /// Consume a completed declaration's mutation inputs and mark this body known exactly once, without repeating
    /// parameter classification or declaration validation.
    pub(in crate::typechecker) fn resume_mut_param_body(
        &mut self,
        prepared: MutParamBodyState,
        has_statement_body: bool,
    ) -> MutParamBodyState {
        if has_statement_body
            && let Some(frame) = &prepared.0
            && !frame.params.is_empty()
        {
            self.mutation.source.bodies_checked.insert(frame.identity.clone());
        }
        MutParamBodyState(std::mem::replace(&mut self.mutation.current, prepared.0))
    }

    /// Leave a body entered with [`Self::enter_mut_param_body`].
    pub(in crate::typechecker) fn exit_mut_param_body(&mut self, previous: MutParamBodyState) {
        self.mutation.current = previous.0;
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
    pub(super) fn current_caller_visible_param(&self, name: &str) -> Option<String> {
        let body = self.mutation.current.as_ref()?;
        let declared_span = body.params.get(name)?;
        self.lookup_symbol(name)
            .is_some_and(|symbol| symbol.span == *declared_span)
            .then(|| name.to_string())
    }

    /// Return the caller-visible parameter a change through the binding `name` reaches: the parameter itself, or a
    /// view into it (a variable of a `for` loop over the parameter or its elements, a name a pattern binds from it).
    pub(super) fn caller_visible_param_reached_by(&self, name: &str) -> Option<String> {
        self.view_source_of(name).and_then(|source| source.param)
    }

    /// Return whether a change through the binding `name` reaches a caller-visible parameter of the current body.
    pub(in crate::typechecker) fn reaches_caller_visible_param(&self, name: &str) -> bool {
        self.caller_visible_param_reached_by(name).is_some()
    }

    /// Return what a change through the place root `root` reaches.
    ///
    /// A caller-visible parameter or a view answers as [`Self::view_source_of`] does. Any other binding answers by its
    /// own declaration: a `mut` binding is writable; a binding or parameter declared without `mut`, a static and a
    /// plain `self` are read-only; the variable of a `for` loop that is no view holds an item of its own. `self` in a
    /// `mut self` method is writable. A root that is no binding (a call, a literal) is no place.
    pub(super) fn view_root_source(&self, root: &Expr) -> Option<ViewSource> {
        match root {
            Expr::Ident(name) => {
                if let Some(source) = self.view_source_of(name) {
                    return Some(source);
                }
                let symbol_id = self.symbols.lookup(name)?;
                let symbol = self.symbols.get(symbol_id)?;
                // A local bound to a static names the static's storage, which a loop or a pattern changes no more than
                // it changes the static itself (#1561).
                if let Some(static_name) = self.static_alias_bindings.get(&symbol_id) {
                    return Some(ViewSource::with_access(ViewAccess::ReadOnly(
                        ReadOnlyPlace::StaticAlias(name.clone(), static_name.clone()),
                    )));
                }
                // A local a closure captures is the closure's own copy, which a change never carries back (#1561).
                if self.local_is_captured_by_closure(symbol_id) {
                    return Some(ViewSource::with_access(ViewAccess::ReadOnly(ReadOnlyPlace::Captured(
                        name.clone(),
                    ))));
                }
                let access = match &symbol.kind {
                    SymbolKind::Variable(info) if info.is_mutable => ViewAccess::Writable,
                    SymbolKind::Variable(_)
                        if self
                            .mutation
                            .loop_variables
                            .iter()
                            .any(|(span, _)| *span == symbol.span) =>
                    {
                        ViewAccess::Owned
                    }
                    SymbolKind::Variable(_)
                        if self
                            .symbols
                            .identity_of(symbol_id)
                            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Parameter) =>
                    {
                        ViewAccess::ReadOnly(ReadOnlyPlace::Parameter(name.clone()))
                    }
                    SymbolKind::Variable(_) => ViewAccess::ReadOnly(ReadOnlyPlace::Binding(name.clone())),
                    SymbolKind::Static(_) => ViewAccess::ReadOnly(ReadOnlyPlace::Static(name.clone())),
                    _ => return None,
                };
                Some(ViewSource::with_access(access))
            }
            Expr::SelfExpr => {
                let symbol = self.lookup_symbol(keywords::as_str(KeywordId::SelfKw))?;
                let SymbolKind::Variable(info) = &symbol.kind else {
                    return None;
                };
                Some(ViewSource::with_access(if info.is_mutable {
                    ViewAccess::Writable
                } else {
                    ViewAccess::ReadOnly(ReadOnlyPlace::SelfReceiver)
                }))
            }
            _ => None,
        }
    }

    /// Return what a change through the binding `name` reaches: the caller-visible parameter itself, with no
    /// scrutinee in between, or the source of the view `name` is.
    pub(super) fn view_source_of(&self, name: &str) -> Option<ViewSource> {
        if let Some(param) = self.current_caller_visible_param(name) {
            return Some(ViewSource::writable(Some(param)));
        }
        let body = self.mutation.current.as_ref()?;
        let binding_span = self.lookup_symbol(name)?.span;
        body.views
            .iter()
            .rev()
            .find(|view| view.name == name && view.span == binding_span)
            .map(|view| view.source.clone())
    }

    /// Return the caller-visible parameter whose elements a `for` loop's variable is a view into, resolved before the
    /// loop's own bindings are defined (so `for items in items:` reads the parameter).
    ///
    /// Only a loop over a list parameter itself, over a field of it (or of `self` in a `mut self` method), or over the
    /// variable of an enclosing such loop iterates the list in place, and only when its elements are not `int`,
    /// `float` or `bool`. Any other iterable (a
    /// call such as `list(items)` or `enumerate(items)`, a method such as `items.clone()` or `table.values()`, an
    /// element such as `items[0]`) yields copies, and a change through its variable does not reach the parameter.
    pub(in crate::typechecker) fn loop_view_param(&self, iter: &Spanned<Expr>) -> Option<ViewSource> {
        let mut node = iter;
        while let Expr::Paren(inner) | Expr::Field(inner, _) = &node.node {
            node = inner;
        }
        let yields_views = self
            .type_info
            .expr_type(iter.span)
            .is_some_and(list_of_changeable_elements);
        if !yields_views {
            return None;
        }
        self.view_root_source(&node.node)
    }

    /// Enter the body of `for <pattern> in ...`, whose bindings are already defined; returns the state to restore with
    /// [`Self::exit_for_loop_body`].
    ///
    /// Every variable the pattern binds is a loop variable. When `source` describes the place the loop iterates in
    /// place ([`Self::loop_view_param`]), each variable is a view into its elements, so a change through it is a change
    /// to that place, and to the caller-visible parameter it belongs to, if any. Otherwise each of `derived`, from
    /// [`Self::read_only_derived_loop_views`], makes the variable it names a view into the read-only place a derived
    /// iterable reads in place.
    pub(in crate::typechecker) fn enter_for_loop_body(
        &mut self,
        pattern: &Pattern,
        source: Option<ViewSource>,
        derived: Vec<(String, ViewSource)>,
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
        let previous_variables = self.mutation.loop_variables.len();
        let destructured = !matches!(pattern, Pattern::Binding(_));
        self.mutation
            .loop_variables
            .extend(bindings.iter().map(|(_, span)| (*span, destructured)));
        let Some(body) = &mut self.mutation.current else {
            return (0, previous_variables);
        };
        let previous_views = body.views.len();
        if let Some(source) = source {
            body.views.extend(bindings.into_iter().map(|(name, span)| ParamView {
                name,
                span,
                source: source.clone(),
            }));
        } else {
            for (name, source) in derived {
                if let Some((_, span)) = bindings.iter().find(|(bound, _)| *bound == name) {
                    body.views.push(ParamView {
                        name,
                        span: *span,
                        source,
                    });
                }
            }
        }
        (previous_views, previous_variables)
    }

    /// Return a view, for each variable of `for <pattern> in iter` that a derived iterable reads in place, into the
    /// place its item belongs to, when that place does not permit a change; resolved before the loop's own bindings are
    /// defined (#1561).
    ///
    /// A loop whose body changes its items reads them in place out of the item of `enumerate(place)`, each operand of
    /// `zip(left, right)`, the values of `place.values()` on a dict, and a list reached through an element such as
    /// `groups[0]`, `pair[0]` or `boxes[0].rows`, as a loop over a place itself does ([`Self::loop_view_param`]). A
    /// change through the variable then changes the place, so one through a place declared without `mut` is refused
    /// like a change through a pattern binding of it. A writable place adds no view: a caller-visible parameter
    /// iterated through a derived iterable is not counted as changed.
    pub(in crate::typechecker) fn read_only_derived_loop_views(
        &self,
        iter: &Spanned<Expr>,
        pattern: &Pattern,
    ) -> Vec<(String, ViewSource)> {
        self.derived_iterable_views(iter, pattern)
            .into_iter()
            .filter(|(_, source, _)| matches!(source.access, ViewAccess::ReadOnly(_)))
            .map(|(name, source, _)| (name, source))
            .collect()
    }

    /// Return a view, for each variable of `for <pattern> in iter` that a derived iterable reads in place, into the
    /// place its item belongs to, whatever that place permits, with the root name of that place; see
    /// [`Self::read_only_derived_loop_views`].
    pub(super) fn derived_iterable_views(
        &self,
        iter: &Spanned<Expr>,
        pattern: &Pattern,
    ) -> Vec<(String, ViewSource, String)> {
        let mut node = iter;
        while let Expr::Paren(inner) = &node.node {
            node = inner;
        }
        // Each in-place source with the part of the pattern that binds its items (`None` for the whole pattern).
        let mut sources: Vec<(Option<usize>, &Spanned<Expr>)> = Vec::new();
        match &node.node {
            Expr::Call(callee, _, args) => {
                let Expr::Ident(name) = &callee.node else {
                    return Vec::new();
                };
                if self.has_non_builtin_call_root_binding(name) {
                    return Vec::new();
                }
                let operands = args
                    .iter()
                    .filter_map(|arg| match arg {
                        CallArg::Positional(value) => Some(value),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                match builtins::from_str(name) {
                    Some(BuiltinFnId::Enumerate) => sources.extend(operands.first().map(|operand| (Some(1), *operand))),
                    Some(BuiltinFnId::Zip) => {
                        sources.extend(
                            operands
                                .iter()
                                .take(2)
                                .enumerate()
                                .map(|(part, operand)| (Some(part), *operand)),
                        );
                    }
                    _ => {}
                }
                sources.retain(|(_, operand)| {
                    self.type_info
                        .expr_type(operand.span)
                        .is_some_and(list_of_changeable_elements)
                });
            }
            Expr::MethodCall(receiver, method, _, args)
                if args.is_empty()
                    && dict_methods::from_str(method) == Some(DictMethodId::Values)
                    && self
                        .type_info
                        .expr_type(receiver.span)
                        .is_some_and(dict_of_changeable_values) =>
            {
                sources.push((None, receiver));
            }
            // A place reached through an element (`groups[0]`, `boxes[0].rows`); one reached through fields alone is
            // the place `loop_view_param` already reads.
            Expr::Index(_, _) | Expr::Field(_, _)
                if self
                    .type_info
                    .expr_type(node.span)
                    .is_some_and(list_of_changeable_elements) =>
            {
                sources.push((None, node));
            }
            _ => {}
        }
        let mut views = Vec::new();
        for (part, place) in sources {
            let Some(source) = self.in_place_source(place) else {
                continue;
            };
            let bound = match (part, pattern) {
                (Some(part), Pattern::Tuple(items)) => items.get(part).map(|item| &item.node),
                _ => Some(pattern),
            };
            let mut names = Vec::new();
            if let Some(bound) = bound {
                collect_pattern_bindings(bound, &mut names);
            }
            // A place with no named root is rooted at `self` (`in_place_source` reached a source).
            let root = place_root_name(place).unwrap_or(keywords::as_str(KeywordId::SelfKw));
            for name in names {
                if !views
                    .iter()
                    .any(|(viewed, _, _): &(String, ViewSource, String)| *viewed == name)
                {
                    views.push((name, source.clone(), root.to_string()));
                }
            }
        }
        views
    }

    /// Return a view for each name a list comprehension's `for` clause binds when the comprehension reads its items in
    /// place from a place that does not permit a change (#1561).
    ///
    /// A list comprehension whose element or filter changes its items reads them in place, as a `for` loop does: the
    /// items of a list place a plain name binds, and the items a derived iterable (`enumerate`, `zip`, `values()`, a
    /// list reached through an element) reads in place ([`Self::read_only_derived_loop_views`]). A change through the
    /// name to a place declared without `mut` is refused like one through a loop variable. An iterable that is no place
    /// binds items of its own.
    pub(in crate::typechecker) fn read_only_comprehension_item_views(
        &self,
        iter: &Spanned<Expr>,
        pattern: &Pattern,
    ) -> Vec<(String, ViewSource)> {
        let direct = match pattern {
            Pattern::Binding(name)
                if self
                    .type_info
                    .expr_type(iter.span)
                    .is_some_and(list_of_changeable_elements) =>
            {
                self.in_place_source(iter)
                    .filter(|source| matches!(source.access, ViewAccess::ReadOnly(_)))
                    .map(|source| vec![(name.clone(), source)])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if direct.is_empty() {
            self.read_only_derived_loop_views(iter, pattern)
        } else {
            direct
        }
    }

    /// Return a view for each name a generator expression's `for` clause binds from the items of a place (#1561).
    ///
    /// A generator reads the items of a list place, directly or through a derived iterable, as copies it holds for its
    /// lazy evaluation, so a change through such a name would change a copy: it is refused whatever the place permits.
    /// An iterable that is no place yields items the generator owns, which it may change.
    pub(in crate::typechecker) fn generator_item_views(
        &self,
        iter: &Spanned<Expr>,
        pattern: &Pattern,
    ) -> Vec<(String, ViewSource)> {
        let copied =
            |root: &str| ViewSource::with_access(ViewAccess::ReadOnly(ReadOnlyPlace::GeneratorItems(root.to_string())));
        let direct = self
            .type_info
            .expr_type(iter.span)
            .is_some_and(list_of_changeable_elements)
            && self.in_place_source(iter).is_some();
        if direct {
            let root = place_root_name(iter).unwrap_or(keywords::as_str(KeywordId::SelfKw));
            let mut names = Vec::new();
            collect_pattern_bindings(pattern, &mut names);
            return names.into_iter().map(|name| (name, copied(root))).collect();
        }
        self.derived_iterable_views(iter, pattern)
            .into_iter()
            .map(|(name, _, root)| (name, copied(&root)))
            .collect()
    }

    /// Make each named binding, already defined, a view described by its source until [`Self::exit_pattern_views`]
    /// restores the returned state.
    pub(in crate::typechecker) fn enter_item_views(&mut self, views: Vec<(String, ViewSource)>) -> usize {
        let bound = views
            .into_iter()
            .filter_map(|(name, source)| {
                let span = self.lookup_symbol(&name)?.span;
                Some(ParamView { name, span, source })
            })
            .collect::<Vec<_>>();
        let Some(body) = &mut self.mutation.current else {
            return 0;
        };
        let previous = body.views.len();
        body.views.extend(bound);
        previous
    }

    /// Return what a change reaches through the place `place` names when a loop reads it in place: the source of its
    /// root binding, followed through parentheses, fields, and the elements of a list, a dict or a tuple. A place
    /// reached any other way, such as through a call, is no place.
    pub(super) fn in_place_source(&self, place: &Spanned<Expr>) -> Option<ViewSource> {
        let mut node = place;
        loop {
            match &node.node {
                Expr::Paren(inner) | Expr::Field(inner, _) => node = inner,
                Expr::Index(inner, _) => {
                    let element_place = self.type_info.expr_type(inner.span).is_some_and(|ty| match ty {
                        ResolvedType::Generic(name, _) => matches!(
                            collection_type_id(name),
                            Some(CollectionTypeId::List | CollectionTypeId::Dict | CollectionTypeId::Tuple)
                        ),
                        ResolvedType::Tuple(_) => true,
                        _ => false,
                    });
                    if !element_place {
                        return None;
                    }
                    node = inner;
                }
                _ => break,
            }
        }
        self.view_root_source(&node.node)
    }

    /// Leave a loop body entered with [`Self::enter_for_loop_body`].
    pub(in crate::typechecker) fn exit_for_loop_body(&mut self, (previous_views, previous_variables): (usize, usize)) {
        self.mutation.loop_variables.truncate(previous_variables);
        self.exit_pattern_views(previous_views);
    }

    /// Return what the bindings of a `match`, `if let` or `while let` pattern are views into when the pattern
    /// destructures `scrutinee`, resolved before the pattern's own bindings are defined.
    ///
    /// A scrutinee that is a place (a binding, a field, a list element, through parentheses) makes each name the
    /// pattern binds part of that place, so a change through the name changes the place: it needs the scrutinee
    /// matched in place when the place is writable, and is refused when it is not. A place reached through a dict value
    /// is bound as a copy, so it is read-only here. Any other scrutinee (a call, a literal) binds values of its own.
    pub(in crate::typechecker) fn pattern_view_param(&self, scrutinee: &Spanned<Expr>) -> Option<ViewSource> {
        let mut node = scrutinee;
        let mut through_dict_value = false;
        let mut through_other_index = false;
        loop {
            match &node.node {
                Expr::Paren(inner) | Expr::Field(inner, _) => node = inner,
                Expr::Index(inner, _) => {
                    match self.type_info.expr_type(inner.span) {
                        Some(ResolvedType::Generic(name, _))
                            if collection_type_id(name) == Some(CollectionTypeId::List) => {}
                        Some(ResolvedType::Generic(name, _))
                            if collection_type_id(name) == Some(CollectionTypeId::Dict) =>
                        {
                            through_dict_value = true;
                        }
                        _ => through_other_index = true,
                    }
                    node = inner;
                }
                _ => break,
            }
        }
        let mut source = if through_other_index {
            ViewSource::with_access(ViewAccess::Owned)
        } else {
            self.view_root_source(&node.node)
                .unwrap_or_else(|| ViewSource::with_access(ViewAccess::Owned))
        };
        if through_dict_value && matches!(source.access, ViewAccess::Writable) {
            source.access = ViewAccess::ReadOnly(ReadOnlyPlace::DictValue);
        }
        source.scrutinees.push(scrutinee.span);
        Some(source)
    }

    /// Make each name `pattern` binds, already defined, a view described by `source` until
    /// [`Self::exit_pattern_views`] restores the returned state; `source` comes from [`Self::pattern_view_param`].
    pub(in crate::typechecker) fn enter_pattern_views(
        &mut self,
        pattern: &Pattern,
        source: Option<ViewSource>,
    ) -> usize {
        let mut names = Vec::new();
        if source.is_some() {
            collect_pattern_bindings(pattern, &mut names);
        }
        let bindings = names
            .into_iter()
            .filter_map(|name| {
                let span = self.lookup_symbol(&name)?.span;
                Some((name, span))
            })
            .collect::<Vec<_>>();
        let Some(body) = &mut self.mutation.current else {
            return 0;
        };
        let previous = body.views.len();
        if let Some(source) = source {
            body.views.extend(bindings.into_iter().map(|(name, span)| ParamView {
                name,
                span,
                source: source.clone(),
            }));
        }
        previous
    }

    /// Leave the views entered with [`Self::enter_pattern_views`] or [`Self::enter_for_loop_body`].
    pub(in crate::typechecker) fn exit_pattern_views(&mut self, previous: usize) {
        if let Some(body) = &mut self.mutation.current {
            body.views.truncate(previous);
        }
    }

    /// Record a read of the binding `name`, which advances a `Generator` it holds.
    ///
    /// Iterating a generator, calling one of its methods and passing it to a call all advance it, so a read of a
    /// caller-visible parameter, or of a view into one, whose value is a generator is a change to the parameter.
    pub(in crate::typechecker) fn note_mut_param_read(&mut self, name: &str, span: Span) {
        let Some(SymbolKind::Variable(info)) = self.lookup_symbol(name).map(|symbol| &symbol.kind) else {
            return;
        };
        if !advances_when_read(&info.ty) {
            return;
        }
        if let Some(source) = self.view_source_of(name) {
            self.note_param_change(source, name, span, true);
        }
    }

    /// Record the iteration of `iterable`, whose type is `iterable_ty`, which advances a generator it evaluates to: a
    /// generator field of a caller-visible parameter, or of a view into one, changes the parameter.
    pub(in crate::typechecker) fn note_mut_param_iteration(
        &mut self,
        iterable: &Spanned<Expr>,
        iterable_ty: &ResolvedType,
    ) {
        if advances_when_read(iterable_ty) {
            self.note_place_change(iterable, true);
        }
    }
}
