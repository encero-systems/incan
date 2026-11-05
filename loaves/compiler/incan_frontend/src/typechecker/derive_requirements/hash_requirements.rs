//! Automatic-derive member refusals, hashable collection keys and type arguments, and literal argument bindings.

use super::*;

impl TypeChecker {
    // ========================================================================
    // #1754: automatic derives of a field's type
    // ========================================================================

    /// Refuse one member whose type cannot satisfy the automatic `Clone` and `Debug` derives its declaration carries.
    ///
    /// `owner_kind` is the declaration keyword (`model`, `class`, `enum`) and `owner_name` its name. The refusal names
    /// the innermost type that lacks the derives, so `list[JoinHandle[int]]` points at `JoinHandle[int]`. Direct native
    /// fields without an admitted owned Clone template receive a missing-proof refusal instead of a negative trait
    /// fact; conditional candidates retain native predicate validation.
    pub(in crate::typechecker) fn refuse_member_without_automatic_derives(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        member: DerivedMember<'_>,
        member_ty: &ResolvedType,
        span: Span,
    ) {
        // A lifetime-bearing native member has no lifetime contract to derive anything for (DD-0006).
        if self.refuse_lifetime_bearing_rust_type(member_ty, span) {
            return;
        }
        if self.native_field_clone_candidate(member_ty) == Some(false)
            && self.native_derive_support(member_ty, DeriveId::Clone) != DeriveSupport::Supported
        {
            self.errors.push(errors::native_field_clone_not_retained(
                owner_kind,
                owner_name,
                member,
                &member_ty.to_string(),
                span,
            ));
            return;
        }
        let Some((holder, missing)) = self.first_missing_derives(member_ty, &[DeriveId::Clone, DeriveId::Debug]) else {
            return;
        };
        self.errors.push(errors::member_type_lacks_automatic_derives(
            owner_kind,
            owner_name,
            member,
            &member_ty.to_string(),
            &holder.to_string(),
            &missing,
            span,
        ));
    }

    /// Find the first of `required` that `ty` is known to lack, and every one of `required` that type inside it lacks.
    pub(super) fn first_missing_derives(
        &self,
        ty: &ResolvedType,
        required: &[DeriveId],
    ) -> Option<(ResolvedType, Vec<&'static str>)> {
        let holder = required
            .iter()
            .find_map(|derive| match self.derive_support(ty, *derive) {
                DeriveSupport::Missing(holder) => Some(holder),
                DeriveSupport::Supported | DeriveSupport::Unknown => None,
            })?;
        let missing = required
            .iter()
            .filter(|derive| matches!(self.derive_support(&holder, **derive), DeriveSupport::Missing(_)))
            .map(|derive| derives::as_str(*derive))
            .collect::<Vec<_>>();
        Some((holder, missing))
    }

    // ========================================================================
    // #1758: set elements and dict keys implement Eq and Hash
    // ========================================================================

    /// Refuse the set element and dict key types an annotation names that lack `Eq` or `Hash` (#1758).
    ///
    /// The annotation and its resolved type are walked together so each refusal points at the element or key type as
    /// written; the interop `HashMap[K, V]` keys like a dict. A spelling that no longer lines up with its resolved type
    /// (a type alias) is not descended into: the alias's own declaration is where its target is checked.
    pub(in crate::typechecker) fn refuse_unhashable_collection_keys(
        &mut self,
        annotation: &Spanned<Type>,
        resolved: &ResolvedType,
    ) {
        match (&annotation.node, resolved) {
            (Type::Generic(_, args), ResolvedType::Generic(name, resolved_args))
                if args.len() == resolved_args.len() =>
            {
                let role = match collection_type_id(name) {
                    Some(CollectionTypeId::Set) => Some(HashedCollectionRole::SetElement),
                    Some(CollectionTypeId::Dict) => Some(HashedCollectionRole::DictKey),
                    _ if self.surface_type_named(name) == Some(SurfaceTypeId::HashMap) => {
                        Some(HashedCollectionRole::DictKey)
                    }
                    _ => None,
                };
                if let (Some(role), Some(key), Some(key_ty)) = (role, args.first(), resolved_args.first()) {
                    self.refuse_unhashable_collection_member(role, key_ty, key.span);
                }
                for (arg, resolved_arg) in args.iter().zip(resolved_args) {
                    self.refuse_unhashable_collection_keys(arg, resolved_arg);
                }
            }
            (Type::Generic(_, args), ResolvedType::FrozenList(element) | ResolvedType::FrozenSet(element)) => {
                if let [arg] = args.as_slice() {
                    self.refuse_unhashable_collection_keys(arg, element);
                }
            }
            (Type::Generic(_, args), ResolvedType::FrozenDict(key, value)) => {
                if let [key_arg, value_arg] = args.as_slice() {
                    self.refuse_unhashable_collection_keys(key_arg, key);
                    self.refuse_unhashable_collection_keys(value_arg, value);
                }
            }
            (Type::Tuple(items), ResolvedType::Tuple(resolved_items)) if items.len() == resolved_items.len() => {
                for (item, resolved_item) in items.iter().zip(resolved_items) {
                    self.refuse_unhashable_collection_keys(item, resolved_item);
                }
            }
            (Type::Function(params, ret), ResolvedType::Function(resolved_params, resolved_ret))
                if params.len() == resolved_params.len() =>
            {
                for (param, resolved_param) in params.iter().zip(resolved_params) {
                    self.refuse_unhashable_collection_keys(param, &resolved_param.ty);
                }
                self.refuse_unhashable_collection_keys(ret, resolved_ret);
            }
            _ => {}
        }
    }

    /// Collect the names of `type_params` that appear in `ty`.
    pub(in crate::typechecker) fn collect_named_type_params(
        ty: &ResolvedType,
        type_params: &[String],
        out: &mut Vec<String>,
    ) {
        match ty {
            ResolvedType::Named(name) | ResolvedType::TypeVar(name) => {
                if type_params.contains(name) && !out.contains(name) {
                    out.push(name.clone());
                }
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for arg in args {
                    Self::collect_named_type_params(arg, type_params, out);
                }
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::collect_named_type_params(inner, type_params, out),
            ResolvedType::FrozenDict(key, value) => {
                Self::collect_named_type_params(key, type_params, out);
                Self::collect_named_type_params(value, type_params, out);
            }
            _ => {}
        }
    }

    /// Refuse one set element or dict key type that lacks `Eq` or `Hash`, once per source location (#1758).
    pub(in crate::typechecker) fn refuse_unhashable_collection_member(
        &mut self,
        role: HashedCollectionRole,
        member_ty: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.first_missing_derives(member_ty, &[DeriveId::Eq, DeriveId::Hash]) else {
            return;
        };
        let error = errors::collection_member_lacks_hash_derives(
            role,
            &member_ty.to_string(),
            &holder.to_string(),
            &missing,
            self.hash_remedy(&holder),
            span,
        );
        self.push_error_once(error);
    }

    /// Whether a destination annotation's element or key type lacks `Eq` or `Hash`, so its annotation was refused.
    ///
    /// A literal checked against such a destination is not refused again for the same element or key type; one
    /// checked against a type parameter or a hashable type still is, for its own elements.
    pub(in crate::typechecker) fn annotation_reports_unhashable_member(
        &self,
        expected_member: Option<&ResolvedType>,
    ) -> bool {
        expected_member.is_some_and(|ty| {
            self.first_missing_derives(ty, &[DeriveId::Eq, DeriveId::Hash])
                .is_some()
        })
    }

    /// Refuse a generic call whose type argument lacks `Eq` or `Hash` where the callee hashes that type parameter
    /// (#1758).
    ///
    /// The callee's hashed parameters are part of its signature, inferred when it was collected
    /// ([`Self::infer_hash_key_type_params`]), so this runs at the call whatever the declaration order, for functions
    /// and methods of this module and of source modules it imports; a compiled library's callee carries them as `Eq`
    /// and `Hash` bounds instead, which the ordinary bound check applies. `bindings` are the call's bindings closed by
    /// its literal arguments ([`Self::bindings_closed_by_literal_arguments`]). A binding to the caller's own type
    /// parameter is not refused: the caller's signature carries the requirement on to its callers.
    pub(in crate::typechecker) fn refuse_unhashable_type_arguments(
        &mut self,
        callee_name: &str,
        callee: Option<&CanonicalSymbolId>,
        type_params: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let Some(required) = callee
            .and_then(|identity| self.hash_key_inference.type_params.get(identity))
            .cloned()
        else {
            return;
        };
        for type_param in required.iter().filter(|type_param| type_params.contains(type_param)) {
            let Some(bound) = bindings
                .get(type_param)
                .filter(|bound| !Self::is_open_binding(bound, type_params))
            else {
                continue;
            };
            self.refuse_unhashable_type_argument(callee_name, type_param, bound, span);
        }
    }

    /// Refuse a caller's own type parameter passed where a compiled library's callee hashes its type parameter, unless
    /// the caller declares the callee's inferred `Eq` and `Hash` bounds on it (#1758).
    ///
    /// A callee of this checker's own modules passes its requirement on to the caller's signature instead
    /// ([`Self::infer_hash_key_type_params`]); a library's cannot, since the caller's generated signature carries only
    /// the bounds its declaration spells. `placeholder` is the caller's type parameter the call binds.
    pub(in crate::typechecker) fn refuse_type_parameter_without_hash_bounds(
        &mut self,
        callee_name: &str,
        type_param: &str,
        placeholder: &str,
        inferred: &[&TypeBoundInfo],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let missing = inferred
            .iter()
            .filter(|bound| !self.active_type_param_satisfies_bound_info(placeholder, bound, bindings))
            .filter_map(|bound| derives::from_str(&bound.name))
            .map(derives::as_str)
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }
        let error = errors::type_argument_lacks_hash_derives(
            callee_name,
            type_param,
            placeholder,
            placeholder,
            &missing,
            HashRemedy::TypeParameter,
            span,
        );
        self.push_error_once(error);
    }

    /// Refuse one type argument bound to a hashed type parameter when it is known to lack `Eq` or `Hash` (#1758).
    ///
    /// Shared by the callees whose requirement is inferred in this checker and by those whose signature carries it as
    /// inferred `Eq` and `Hash` bounds (a compiled library's): either way an unknown answer admits the call.
    pub(in crate::typechecker) fn refuse_unhashable_type_argument(
        &mut self,
        callee_name: &str,
        type_param: &str,
        argument: &ResolvedType,
        span: Span,
    ) {
        let Some((holder, missing)) = self.first_missing_derives(argument, &[DeriveId::Eq, DeriveId::Hash]) else {
            return;
        };
        let error = errors::type_argument_lacks_hash_derives(
            callee_name,
            type_param,
            &argument.to_string(),
            &holder.to_string(),
            &missing,
            self.hash_remedy(&holder),
            span,
        );
        self.push_error_once(error);
    }

    /// Return a generic call's type-parameter bindings with each open one closed from a literal argument, for the
    /// checks of what the call instantiates its callee with.
    ///
    /// A literal argument checked against a parameter type that names a type parameter takes that placeholder as its
    /// own type (`[Tag.A]` against `list[T]` is a `list[T]`), so the binding stays open; the literal's elements say
    /// what the call instantiates the parameter with (`Tag`). A binding that is already closed is kept.
    pub(in crate::typechecker) fn bindings_closed_by_literal_arguments(
        &self,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        bindings: &HashMap<String, ResolvedType>,
    ) -> HashMap<String, ResolvedType> {
        let mut literal_bindings = HashMap::new();
        for (expr, param) in Self::arguments_with_parameters(params, args) {
            if let Some(param) = param {
                self.literal_argument_bindings(&param.ty, expr, type_params, &mut literal_bindings);
            }
        }
        let mut closed = bindings.clone();
        for (type_param, ty) in literal_bindings {
            let open = closed
                .get(&type_param)
                .is_none_or(|bound| Self::is_open_binding(bound, type_params));
            if open {
                closed.insert(type_param, ty);
            }
        }
        closed
    }

    /// Return the declaration identity of the function a call by name reaches, unless it is an overload set, whose
    /// bindings belong to one overload.
    pub(in crate::typechecker) fn called_function_identity(&self, callee_name: &str) -> Option<CanonicalSymbolId> {
        let symbol_id = self.symbols.lookup(callee_name)?;
        match &self.symbols.get(symbol_id)?.kind {
            SymbolKind::Function(_) => self.symbols.identity_of(symbol_id).cloned(),
            _ => None,
        }
    }

    /// Whether a binding is still open: unresolved, or one of the callee's own type parameters.
    pub(super) fn is_open_binding(bound: &ResolvedType, type_params: &[String]) -> bool {
        match bound {
            ResolvedType::Unknown | ResolvedType::CallSiteInfer => true,
            ResolvedType::TypeVar(name) | ResolvedType::Named(name) => type_params.contains(name),
            _ => false,
        }
    }

    /// Pair each argument with the parameter it binds: positional arguments in order over the ordinary parameters a
    /// call can bind positionally, named arguments by name. An unpacked argument ends the pairing, since it hides which
    /// parameters the arguments after it bind.
    pub(in crate::typechecker) fn arguments_with_parameters<'a, 'p>(
        params: &'p [CallableParam],
        args: &'a [CallArg],
    ) -> Vec<(&'a Spanned<Expr>, Option<&'p CallableParam>)> {
        Self::arguments_with_parameter_slots(params, args)
            .into_iter()
            .map(|(expression, slot)| (expression, slot.map(|slot| &params[slot])))
            .collect()
    }

    /// Keep the canonical source parameter slot while pairing arguments in evaluation order. Unpacking ends this
    /// projection, as in the derive caller; ownership consumers must refuse a truncated result rather than infer
    /// the hidden bindings. A named argument's evaluation position never changes its declaration slot.
    pub(in crate::typechecker) fn arguments_with_parameter_slots<'a>(
        params: &[CallableParam],
        args: &'a [CallArg],
    ) -> Vec<(&'a Spanned<Expr>, Option<usize>)> {
        let mut positional_params = params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Normal && !param.is_partial_preset)
            .map(|(slot, _)| slot);
        let mut paired = Vec::new();
        for arg in args {
            match arg {
                CallArg::Positional(expr) => paired.push((expr, positional_params.next())),
                CallArg::Named(name, expr) => paired.push((
                    expr,
                    params.iter().position(|param| param.name() == Some(name.node.as_str())),
                )),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => break,
            }
        }
        paired
    }

    /// Recover type-parameter bindings from a literal argument's elements, following the parameter type's shape.
    ///
    /// Only a collection or tuple literal is read: any other argument's type is already what inference bound.
    pub(super) fn literal_argument_bindings(
        &self,
        param_ty: &ResolvedType,
        expr: &Spanned<Expr>,
        type_params: &[String],
        out: &mut HashMap<String, ResolvedType>,
    ) {
        match (param_ty, &expr.node) {
            (_, Expr::Paren(inner)) => self.literal_argument_bindings(param_ty, inner, type_params, out),
            (ResolvedType::Generic(name, args), Expr::List(entries))
                if collection_type_id(name) == Some(CollectionTypeId::List) =>
            {
                if let (Some(element_ty), Some(ListEntry::Element(first))) = (args.first(), entries.first()) {
                    self.literal_element_bindings(element_ty, first, type_params, out);
                }
            }
            (ResolvedType::Generic(name, args), Expr::Set(elements))
                if collection_type_id(name) == Some(CollectionTypeId::Set) =>
            {
                if let (Some(element_ty), Some(first)) = (args.first(), elements.first()) {
                    self.literal_element_bindings(element_ty, first, type_params, out);
                }
            }
            (ResolvedType::Generic(name, args), Expr::Dict(entries))
                if collection_type_id(name) == Some(CollectionTypeId::Dict) =>
            {
                if let ([key_ty, value_ty], Some(DictEntry::Pair(key, value))) = (args.as_slice(), entries.first()) {
                    self.literal_element_bindings(key_ty, key, type_params, out);
                    self.literal_element_bindings(value_ty, value, type_params, out);
                }
            }
            (ResolvedType::Tuple(items), Expr::Tuple(elements)) if items.len() == elements.len() => {
                for (item_ty, element) in items.iter().zip(elements) {
                    self.literal_element_bindings(item_ty, element, type_params, out);
                }
            }
            _ => {}
        }
    }

    /// Recover a binding from one element of a literal: its own type where the element type is a type parameter, or
    /// the bindings of a nested literal.
    pub(super) fn literal_element_bindings(
        &self,
        element_ty: &ResolvedType,
        element: &Spanned<Expr>,
        type_params: &[String],
        out: &mut HashMap<String, ResolvedType>,
    ) {
        match element_ty {
            ResolvedType::TypeVar(name) | ResolvedType::Named(name) if type_params.contains(name) => {
                if let Some(ty) = self.type_info.expr_type(element.span)
                    && !Self::is_open_binding(ty, type_params)
                {
                    out.entry(name.clone()).or_insert_with(|| ty.clone());
                }
            }
            _ => self.literal_argument_bindings(element_ty, element, type_params, out),
        }
    }

    /// Choose the remedy for the type that lacks `Eq` or `Hash`: a declared type adds derives, unless it defines
    /// `__eq__`, whose custom equality provides neither; a builtin cannot take derives.
    pub(super) fn hash_remedy(&self, ty: &ResolvedType) -> HashRemedy {
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = ty else {
            return HashRemedy::Builtin;
        };
        let methods = match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(info)) => &info.methods,
            Some(TypeInfo::Class(info)) => &info.methods,
            Some(TypeInfo::Enum(info)) => &info.methods,
            Some(TypeInfo::Newtype(info)) => &info.methods,
            Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None => return HashRemedy::Builtin,
        };
        if methods.contains_key("__eq__") {
            HashRemedy::CustomEquality
        } else {
            HashRemedy::AddDerives
        }
    }

    /// Push an error unless the same message was already reported at the same span.
    ///
    /// An annotation or expression can be checked more than once while its declaration is checked (a field and its
    /// default, for example); the refusal belongs to the source location, not to each pass over it.
    pub(super) fn push_error_once(&mut self, error: CompileError) {
        if !self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message)
        {
            self.errors.push(error);
        }
    }
}
