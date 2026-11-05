//! Copy and clone classification, fresh values, and call-site `Self` substitution.

use super::*;

impl TypeChecker {
    /// Check if a type is copyable.
    pub(in crate::typechecker) fn is_copy_type(&self, ty: &ResolvedType) -> bool {
        matches!(
            ty,
            ResolvedType::Never
                | ResolvedType::Int
                | ResolvedType::Float
                | ResolvedType::Numeric(_)
                | ResolvedType::Bool
                | ResolvedType::Unit
                | ResolvedType::Ref(_)
                | ResolvedType::RefMut(_)
        )
    }

    /// Check if a type is cloneable.
    ///
    /// The derive relation answers wherever it knows the type, including every stdlib surface type the registry
    /// records (so `Mutex[T]` clones and `JoinHandle[T]` does not). An unknown answer keeps this check's own policy:
    /// a type parameter by its active bounds, a Rust-origin type refused, an unresolved type admitted.
    pub(in crate::typechecker) fn is_clone_type(&self, ty: &ResolvedType) -> bool {
        match self.derive_support(ty, DeriveId::Clone) {
            DeriveSupport::Supported => return true,
            DeriveSupport::Missing(_) => return false,
            DeriveSupport::Unknown => {}
        }
        match ty {
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit => true,
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => self.is_clone_type(inner),
            ResolvedType::FrozenDict(k, v) => self.is_clone_type(k) && self.is_clone_type(v),
            ResolvedType::Tuple(items) => items.iter().all(|t| self.is_clone_type(t)),
            ResolvedType::Generic(name, args) => {
                if let Some(id) = surface_types::from_str(name.as_str()) {
                    return match id {
                        SurfaceTypeId::Vec => args.first().is_none_or(|t| self.is_clone_type(t)),
                        SurfaceTypeId::HashMap => {
                            let key_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                            let val_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                            key_ok && val_ok
                        }
                        _ => false,
                    };
                }
                match collection_type_id(name.as_str()) {
                    Some(CollectionTypeId::List) | Some(CollectionTypeId::Set) | Some(CollectionTypeId::Option) => {
                        args.first().is_none_or(|t| self.is_clone_type(t))
                    }
                    Some(CollectionTypeId::Dict) => {
                        let key_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                        let val_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                        key_ok && val_ok
                    }
                    Some(CollectionTypeId::Result) => {
                        let ok_ok = args.first().is_none_or(|t| self.is_clone_type(t));
                        let err_ok = args.get(1).is_none_or(|t| self.is_clone_type(t));
                        ok_ok && err_ok
                    }
                    // A generator is a one-shot runtime iterator; it has no copy.
                    Some(CollectionTypeId::Generator) => false,
                    _ => args.iter().all(|t| self.is_clone_type(t)),
                }
            }
            ResolvedType::Named(name) => {
                if let Some(id) = surface_types::from_str(name.as_str()) {
                    return matches!(id, SurfaceTypeId::Html);
                }
                matches!(
                    self.lookup_type_info(name),
                    Some(TypeInfo::Builtin)
                        | Some(TypeInfo::Class(_))
                        | Some(TypeInfo::Model(_))
                        | Some(TypeInfo::Newtype(_))
                        | Some(TypeInfo::Enum(_))
                )
            }
            ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::TypeToken(_)
            | ResolvedType::Trait(_)
            | ResolvedType::Function(_, _)
            | ResolvedType::SelfType => true,
            ResolvedType::TypeVar(name) => self.active_type_param_has_builtin_bound(name, TraitId::Clone),
            ResolvedType::CallSiteInfer => false,
            // RFC 041: provenance is known, but Incan does not yet query Rust for `Copy`/`Clone`; do not assume.
            ResolvedType::RustPath(_) => false,
            ResolvedType::Unknown => true,
        }
    }

    /// Return whether an active type parameter has a builtin bound.
    pub(super) fn active_type_param_has_builtin_bound(&self, type_param: &str, trait_id: TraitId) -> bool {
        let expected = core_traits::as_str(trait_id);
        self.innermost_type_param_bounds(type_param).is_some_and(|bounds| {
            bounds
                .iter()
                .any(|bound| bound.name == expected || Self::type_bound_source_name(bound) == expected)
        })
    }

    /// Return whether `expr` builds a new value without taking anything out of an existing place, so storing it moves
    /// the value and needs no copy (#1821).
    ///
    /// A literal and an f-string are new values. A call, a method call and a constructor are new values only when
    /// every argument, and a method call's receiver, is itself a new value by this rule or has a `Copy` or `Clone`
    /// type: `spawn(work(n))` qualifies, while `Some(handle)`, `maybe.unwrap()` and `holder.get()` over a place whose
    /// type is not `Clone` do not, since building them would move or copy out of that place. A name, a field, an
    /// element, `self` and every other shape read a place the program may use again; the checker does not decide
    /// whether a read is the place's last use, so those are never new values. Parentheses and `?` yield the value of
    /// the expression they wrap. The call's own callee is not a part of the value and is not inspected.
    pub(super) fn is_fresh_value(&self, expr: &Spanned<Expr>) -> bool {
        let arguments_are_fresh_or_copyable = |args: &[CallArg]| {
            args.iter().all(|arg| match arg {
                CallArg::Positional(value) | CallArg::Named(_, value) => self.is_fresh_or_copyable_value(value),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
            })
        };
        match &expr.node {
            Expr::Literal(_) | Expr::FString(_) => true,
            Expr::Paren(inner) | Expr::Try(inner) => self.is_fresh_value(inner),
            Expr::Call(_, _, args) | Expr::Constructor(_, args) => arguments_are_fresh_or_copyable(args),
            Expr::MethodCall(receiver, _, _, args) => {
                self.is_fresh_or_copyable_value(receiver) && arguments_are_fresh_or_copyable(args)
            }
            _ => false,
        }
    }

    /// Return whether `expr` is a new value ([`Self::is_fresh_value`]) or has a checked type that is `Copy` or `Clone`,
    /// so a value built from it takes nothing out of a place the program may use again.
    ///
    /// A part whose type the checker did not record counts as neither, which keeps the enclosing append's `Clone`
    /// requirement.
    pub(super) fn is_fresh_or_copyable_value(&self, expr: &Spanned<Expr>) -> bool {
        self.is_fresh_value(expr)
            || self
                .type_info
                .expr_type(expr.span)
                .is_some_and(|ty| self.is_copy_type(ty) || self.is_clone_type(ty))
    }

    /// [`ResolvedType::SelfType`] in a trait method signature means the receiver type for this call site.
    pub(super) fn concrete_type_for_trait_self(&self, receiver: &ResolvedType) -> ResolvedType {
        match receiver {
            ResolvedType::Generic(name, args) => ResolvedType::Generic(name.clone(), args.clone()),
            ResolvedType::Named(name) => {
                let n_params = self
                    .lookup_semantic_type_info(name)
                    .map(|info| match info {
                        TypeInfo::Model(m) => m.type_params.len(),
                        TypeInfo::Class(c) => c.type_params.len(),
                        TypeInfo::Enum(e) => e.type_params.len(),
                        TypeInfo::Newtype(n) => n.type_params.len(),
                        _ => 0,
                    })
                    .unwrap_or(0);
                if n_params > 0 {
                    ResolvedType::Generic(name.clone(), vec![ResolvedType::Unknown; n_params])
                } else {
                    receiver.clone()
                }
            }
            _ => receiver.clone(),
        }
    }

    /// Replace every [`ResolvedType::SelfType`] in `ty` using the **call-site** receiver.
    ///
    /// For method **bodies**, `TypeChecker::concretize_self_type_in_annotation` in `check_decl.rs` maps `Self` to the
    /// owner's `self_ty` while checking the implementation. At a **call site**, `Self` means the instantiated
    /// receiver (for example `DataFrame[Order]` when calling on `x: DataFrame[Order]`).
    pub(in crate::typechecker) fn substitute_self_in_resolved_type(
        &self,
        ty: ResolvedType,
        receiver: &ResolvedType,
    ) -> ResolvedType {
        match ty {
            ResolvedType::SelfType => self.concrete_type_for_trait_self(receiver),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name,
                args.into_iter()
                    .map(|a| self.substitute_self_in_resolved_type(a, receiver))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|a| self.substitute_self_in_resolved_type(a, receiver))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => {
                ResolvedType::FrozenList(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::FrozenSet(inner) => {
                ResolvedType::FrozenSet(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::FrozenDict(k, v) => ResolvedType::FrozenDict(
                Box::new(self.substitute_self_in_resolved_type(*k, receiver)),
                Box::new(self.substitute_self_in_resolved_type(*v, receiver)),
            ),
            ResolvedType::Ref(inner) => {
                ResolvedType::Ref(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            ResolvedType::RefMut(inner) => {
                ResolvedType::RefMut(Box::new(self.substitute_self_in_resolved_type(*inner, receiver)))
            }
            other => other,
        }
    }

    /// Substitute call-site `Self` and the selected receiver owner's binders inside a backend trait-dispatch target.
    ///
    /// The selected trait instantiation is carried separately from the method signature. Both must receive the same
    /// receiver substitution so a source target such as `Index[list[str], Self]` emits `Index<Vec<String>, Box>` at
    /// the call site rather than an illegal bare Rust `Self`. Adoption arguments belong to the receiver declaration,
    /// so `Shelf[T] with Sliceable[T]` called on `Shelf[str]` must retain `str`, not its owner's free `T`.
    pub(super) fn method_dispatch_substituting_call_site_self(
        &self,
        dispatch: crate::typechecker::ResolvedMethodDispatch,
        receiver: &ResolvedType,
    ) -> crate::typechecker::ResolvedMethodDispatch {
        let owner_bindings = self.receiver_owner_type_bindings(receiver);
        match dispatch {
            crate::typechecker::ResolvedMethodDispatch::Trait {
                trait_name,
                module_path,
                type_args,
                implementation_type_params,
                receiver_is_mutable,
            } => crate::typechecker::ResolvedMethodDispatch::Trait {
                trait_name,
                module_path,
                type_args: type_args
                    .into_iter()
                    .map(|ty| {
                        substitute_resolved_type(&self.substitute_self_in_resolved_type(ty, receiver), &owner_bindings)
                    })
                    .collect(),
                implementation_type_params: implementation_type_params
                    .into_iter()
                    .map(|type_param| ImplementationTypeParamInfo {
                        name: type_param.name,
                        bounds: type_param
                            .bounds
                            .into_iter()
                            .map(|bound| ImplementationTraitBoundInfo {
                                trait_path: bound.trait_path,
                                type_args: bound
                                    .type_args
                                    .into_iter()
                                    .map(|ty| self.substitute_self_in_resolved_type(ty, receiver))
                                    .collect(),
                                associated_types: bound
                                    .associated_types
                                    .into_iter()
                                    .map(|(name, ty)| (name, self.substitute_self_in_resolved_type(ty, receiver)))
                                    .collect(),
                                origin: bound.origin,
                            })
                            .collect(),
                    })
                    .collect(),
                receiver_is_mutable,
            },
        }
    }

    /// Read a nominal receiver's declared parameter binding without inferring any arguments from operand carriers.
    /// Trait receivers and incomplete owner applications supply no additional binding scope.
    pub(super) fn receiver_owner_type_bindings(
        &self,
        receiver: &ResolvedType,
    ) -> std::collections::HashMap<String, ResolvedType> {
        let ResolvedType::Generic(name, arguments) = receiver else {
            return std::collections::HashMap::new();
        };
        let parameters = match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(owner)) => &owner.type_params,
            Some(TypeInfo::Class(owner)) => &owner.type_params,
            Some(TypeInfo::Newtype(owner)) => &owner.type_params,
            Some(TypeInfo::Enum(owner)) => &owner.type_params,
            _ => return std::collections::HashMap::new(),
        };
        if parameters.len() != arguments.len() {
            return std::collections::HashMap::new();
        }
        type_param_subst_map(parameters, arguments)
    }

    /// Build formal parameter types and return type for a method call, replacing [`ResolvedType::SelfType`] with the
    /// instantiated receiver.
    ///
    /// [`MethodInfo`] in the symbol table stores `Self` literally. At a call site, `Self` means the concrete receiver
    /// type (for example the `T` of `List[T]` when calling on a `List[int]` value). Both inherent and trait dispatch
    /// use this before [`Self::validate_method_call_args`] and before RFC 054 explicit type-argument substitution and
    /// inference (`check_generic_method_call` in the `calls` submodule).
    ///
    /// Returns `(params, return_type)` with `Self` resolved; method-level type parameters may still appear as
    /// [`ResolvedType::TypeVar`] until inference completes.
    pub(in crate::typechecker::check_expr) fn method_types_substituting_call_site_self(
        &self,
        method_info: &MethodInfo,
        receiver_ty: &ResolvedType,
    ) -> (Vec<CallableParam>, ResolvedType) {
        let params = method_info
            .params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: self.substitute_self_in_resolved_type(param.ty.clone(), receiver_ty),
                kind: param.kind,
                has_default: param.has_default,
                is_partial_preset: param.is_partial_preset,
                is_mut: param.is_mut,
            })
            .collect();
        let return_type = self.substitute_self_in_resolved_type(method_info.return_type.clone(), receiver_ty);
        (params, return_type)
    }
}
