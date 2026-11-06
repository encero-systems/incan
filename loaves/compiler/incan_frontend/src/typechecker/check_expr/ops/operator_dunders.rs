//! Operator dunder resolution and derive-backed and user-defined operator methods.

use super::*;

impl TypeChecker {
    /// Resolve a synthetic protocol hook, retaining the exact trait provenance selected by the source expression.
    ///
    /// A source method can return its owning trait without making that trait name directly visible to the consumer.
    /// Ordinary operator lookup therefore has no unqualified trait symbol to consult. The source call's recorded
    /// dispatch remains authoritative: use its canonical module only when the returned receiver is that same trait.
    pub(super) fn resolve_protocol_operator_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        source_dispatch: Option<&ResolvedMethodDispatch>,
    ) -> Option<ResolvedType> {
        if let Some(resolved) = self.resolve_operator_dunder(receiver_ty, method, args, arg_types, span, None) {
            return Some(resolved);
        }

        let (receiver_name, receiver_args) = match receiver_ty {
            ResolvedType::Named(name) => (name, Vec::new()),
            ResolvedType::Generic(name, type_args) => (name, type_args.clone()),
            _ => return None,
        };
        let Some(ResolvedMethodDispatch::Trait {
            trait_name,
            module_path: Some(module_path),
            ..
        }) = source_dispatch
        else {
            return None;
        };
        let same_builtin_trait = builtin_traits::from_qualified_str(receiver_name)
            .is_some_and(|id| builtin_traits::from_qualified_str(trait_name) == Some(id));
        if receiver_name != trait_name && !same_builtin_trait {
            return None;
        }

        let adoption = TypeBoundInfo {
            name: trait_name.clone(),
            source_name: None,
            type_args: receiver_args,
            module_path: Some(module_path.clone()),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.resolve_named_method(
            &std::collections::HashMap::new(),
            None,
            Some(std::slice::from_ref(&adoption)),
            method,
            MemberBindingSurface::Instance,
            &[],
            args,
            arg_types,
            span,
            receiver_ty,
            None,
        )
    }

    /// Resolve a user-defined index assignment (`base[index] = value`) through `__setitem__`, if available.
    pub(in crate::typechecker) fn resolve_index_set_dunder(
        &mut self,
        base_ty: &ResolvedType,
        index: &Spanned<Expr>,
        index_ty: &ResolvedType,
        value: &Spanned<Expr>,
        value_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__setitem__";
        let args = vec![CallArg::Positional(index.clone()), CallArg::Positional(value.clone())];
        let arg_types = vec![index_ty.clone(), value_ty.clone()];
        let expected_return_ty = ResolvedType::Unit;
        let ret = self.resolve_operator_dunder(base_ty, method, &args, &arg_types, span, Some(&expected_return_ty))?;
        if !self.types_compatible(&ret, &expected_return_ty) {
            self.errors.push(errors::type_mismatch(
                &expected_return_ty.to_string(),
                &ret.to_string(),
                span,
            ));
        }
        self.type_info
            .record_resolved_operator_call(span, method, ResolvedOperatorKind::IndexAssign);
        Some(ret)
    }

    /// Resolve compound assignment as an in-place hook first, then the ordinary binary hook.
    pub(in crate::typechecker) fn resolve_compound_assignment_operator(
        &mut self,
        receiver_ty: &ResolvedType,
        op: CompoundOp,
        value: &Spanned<Expr>,
        value_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let args = vec![CallArg::Positional(value.clone())];
        let arg_types = vec![value_ty.clone()];
        let in_place = compound_in_place_dunder(op);
        if let Some(ret) =
            self.resolve_operator_dunder(receiver_ty, in_place, &args, &arg_types, span, Some(receiver_ty))
        {
            self.type_info
                .record_resolved_operator_call(span, in_place, ResolvedOperatorKind::Binary);
            return Some(ret);
        }
        let binary = binary_operator_dunder(compound_binary_op(op))?;
        let ret = self.resolve_operator_dunder(receiver_ty, binary, &args, &arg_types, span, Some(receiver_ty))?;
        self.type_info
            .record_resolved_operator_call(span, binary, ResolvedOperatorKind::Binary);
        Some(ret)
    }

    /// Resolve one operator dunder on a user type or generic placeholder.
    ///
    /// Also resolves the `message()` an `Error` adopter displays through (`check_expr/error_display.rs`), which, like a
    /// dunder, is a method call the program never wrote.
    pub(in crate::typechecker::check_expr) fn resolve_operator_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        if self.is_generic_placeholder_type(receiver_ty) {
            let placeholder_name = self.generic_placeholder_name(receiver_ty)?.to_string();
            return self.resolve_generic_placeholder_method(
                &placeholder_name,
                method,
                &[],
                args,
                arg_types,
                span,
                receiver_ty,
                expected_return_ty,
            );
        }
        match receiver_ty {
            ResolvedType::Generic(type_name, _type_args) => {
                if let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned() {
                    self.resolve_operator_dunder_on_type_info(
                        &type_info,
                        method,
                        args,
                        arg_types,
                        span,
                        receiver_ty,
                        expected_return_ty,
                    )
                } else {
                    self.resolve_trait_receiver_method(
                        receiver_ty,
                        method,
                        MemberBindingSurface::Instance,
                        &[],
                        args,
                        arg_types,
                        span,
                        expected_return_ty,
                    )
                }
            }
            ResolvedType::Named(type_name) => {
                if let Some(type_info) = self.lookup_semantic_type_info(type_name).cloned() {
                    self.resolve_operator_dunder_on_type_info(
                        &type_info,
                        method,
                        args,
                        arg_types,
                        span,
                        receiver_ty,
                        expected_return_ty,
                    )
                } else {
                    self.resolve_trait_receiver_method(
                        receiver_ty,
                        method,
                        MemberBindingSurface::Instance,
                        &[],
                        args,
                        arg_types,
                        span,
                        expected_return_ty,
                    )
                }
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Resolve an operator dunder against concrete type metadata and its adopted traits.
    pub(super) fn resolve_operator_dunder_on_type_info(
        &mut self,
        type_info: &TypeInfo,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        match type_info {
            TypeInfo::Model(model) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                self.resolve_named_method(
                    &model.methods,
                    Some(&model.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Class(class) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                self.resolve_named_method(
                    &class.methods,
                    Some(&class.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Enum(en) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                self.resolve_named_method(
                    &en.methods,
                    Some(&en.method_overloads),
                    Some(&trait_adoptions),
                    method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Newtype(newtype) => {
                let resolved_method = self.resolve_newtype_method_name(newtype, method);
                self.resolve_named_method(
                    &newtype.methods,
                    Some(&newtype.method_overloads),
                    Some(&newtype.trait_adoptions),
                    resolved_method,
                    MemberBindingSurface::Instance,
                    &[],
                    args,
                    arg_types,
                    span,
                    receiver_ty,
                    expected_return_ty,
                )
            }
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Return whether a type can participate in user-defined operator dispatch.
    pub(in crate::typechecker) fn is_user_operator_receiver(&self, ty: &ResolvedType) -> bool {
        if self.is_generic_placeholder_type(ty) {
            return true;
        }
        match ty {
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => {
                matches!(
                    self.lookup_semantic_type_info(name),
                    Some(TypeInfo::Class(_) | TypeInfo::Model(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
                ) || self.lookup_semantic_trait_info(name).is_some()
            }
            _ => false,
        }
    }

    /// Return whether a type has Rust-backed comparison derives for this operator: the operator's derive declared or
    /// implied (`Ord` implies `PartialEq`), spelled in `@rust.derive(...)`, or an enum's automatic `PartialEq`.
    pub(super) fn type_has_derive_backed_comparison_operator(&self, ty: &ResolvedType, op: BinaryOp) -> bool {
        comparison_operator_derive(op).is_some_and(|derive| self.comparison_is_derived(ty, derive))
    }

    /// Refuse a comparison whose operand type is known not to implement the operator's trait (#1870).
    ///
    /// Called once the operator's dunders have not resolved it: `==`, `!=` and list membership need `PartialEq` and the
    /// orderings need `PartialOrd`, of the operand and of every type inside it, as the generated comparison does. For
    /// membership `ty` is the list's element type. A type the derive relation cannot decide is accepted. Returns
    /// whether the comparison was refused.
    pub(super) fn refuse_comparison_without_derive(&mut self, ty: &ResolvedType, op: BinaryOp, span: Span) -> bool {
        let Some(derive) = comparison_operator_derive(op) else {
            return false;
        };
        let DeriveSupport::Missing(holder) = self.derive_support(ty, derive) else {
            return false;
        };
        // The catalog derive that provides the operator; `Eq` and `Ord` bring `PartialEq` and `PartialOrd` with them.
        let providing_derive = if derive == DeriveId::PartialEq {
            DeriveId::Eq
        } else {
            DeriveId::Ord
        };
        // Membership compares items with `==`, so its dunder is `__eq__`.
        let dunder_op = if matches!(op, BinaryOp::In | BinaryOp::NotIn) {
            BinaryOp::Eq
        } else {
            op
        };
        let dunder = self
            .is_user_operator_receiver(&holder)
            .then(|| binary_operator_dunder(dunder_op))
            .flatten();
        self.errors.push(errors::operator_not_provided(
            &ty.to_string(),
            &holder.to_string(),
            &op.to_string(),
            derives::as_str(providing_derive),
            dunder,
            span,
        ));
        true
    }

    /// Return whether a type directly declares an operator method, excluding derived trait defaults.
    pub(super) fn type_has_inherent_operator_method(&self, ty: &ResolvedType, method: &str) -> bool {
        match ty {
            ResolvedType::Generic(type_name, _) | ResolvedType::Named(type_name) => {
                let Some(type_info) = self.lookup_semantic_type_info(type_name) else {
                    return false;
                };
                match type_info {
                    TypeInfo::Model(model) => {
                        model.methods.contains_key(method) || model.method_overloads.contains_key(method)
                    }
                    TypeInfo::Class(class) => {
                        class.methods.contains_key(method) || class.method_overloads.contains_key(method)
                    }
                    TypeInfo::Enum(en) => en.methods.contains_key(method) || en.method_overloads.contains_key(method),
                    TypeInfo::Newtype(newtype) => {
                        let resolved = self.resolve_newtype_method_name(newtype, method);
                        newtype.methods.contains_key(resolved) || newtype.method_overloads.contains_key(resolved)
                    }
                    TypeInfo::Builtin | TypeInfo::TypeAlias => false,
                }
            }
            _ => false,
        }
    }

    /// Return whether a type exposes any method in a candidate operator method set.
    pub(super) fn type_has_any_operator_method(&mut self, ty: &ResolvedType, methods: &[&str], span: Span) -> bool {
        methods
            .iter()
            .any(|method| self.type_has_operator_method(ty, method, span))
    }

    /// Return whether a type exposes one operator method directly or through an adopted trait.
    pub(super) fn type_has_operator_method(&mut self, ty: &ResolvedType, method: &str, span: Span) -> bool {
        match ty {
            ResolvedType::Generic(type_name, _) | ResolvedType::Named(type_name) => {
                let Some(type_info) = self.lookup_semantic_type_info(type_name) else {
                    return false;
                };
                match type_info {
                    TypeInfo::Model(model) => {
                        if model.methods.contains_key(method) || model.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Class(class) => {
                        if class.methods.contains_key(method) || class.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions =
                            self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Enum(en) => {
                        if en.methods.contains_key(method) || en.method_overloads.contains_key(method) {
                            return true;
                        }
                        let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, method, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Newtype(newtype) => {
                        let resolved = self.resolve_newtype_method_name(&newtype, method);
                        if newtype.methods.contains_key(resolved) || newtype.method_overloads.contains_key(resolved) {
                            return true;
                        }
                        let trait_adoptions = newtype.trait_adoptions.clone();
                        let resolved = resolved.to_owned();
                        trait_adoptions.iter().any(|adoption| {
                            self.trait_method_info_resolved_for_adoption(adoption, &resolved, span)
                                .is_some()
                        })
                    }
                    TypeInfo::Builtin | TypeInfo::TypeAlias => false,
                }
            }
            _ => false,
        }
    }
}
