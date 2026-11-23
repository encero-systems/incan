//! Trait dispatch, clone reads, compiler-generated member identities, value-enum methods, and reflection methods.

use super::*;

impl TypeChecker {
    /// Preserve the selected trait owner and receiver contract for downstream lowering.
    pub(super) fn resolved_trait_dispatch(
        &self,
        trait_name: &str,
        type_args: Vec<ResolvedType>,
        origin_module_path: Option<&[String]>,
        implementation_type_params: Vec<ImplementationTypeParamInfo>,
        receiver_is_mutable: bool,
    ) -> crate::typechecker::ResolvedMethodDispatch {
        let module_path = origin_module_path
            .map(<[String]>::to_vec)
            .or_else(|| incan_lang::lang::stdlib::trait_method_module_segments(trait_name));
        crate::typechecker::ResolvedMethodDispatch::Trait {
            trait_name: trait_name.to_string(),
            module_path,
            type_args,
            implementation_type_params,
            receiver_is_mutable,
        }
    }

    /// Return whether an expression names an enum type rather than an enum value.
    pub(super) fn is_enum_type_name_expr(&self, expr: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &expr.node else {
            return false;
        };
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Enum(_))))
    }

    /// Build the canonical target for one member defined by a compiler-owned surface registry.
    ///
    /// Builtin members have no source declaration span, and [`CanonicalSymbolId`] has no separate member-owner slot.
    /// Their registry declaration key is therefore owner-qualified (`List.append`, `str.upper`, and so on). The owner
    /// and member are selected from typed compiler registries before this is called; source spelling alone is never
    /// treated as semantic evidence.
    pub(super) fn compiler_builtin_member_identity(owner: &str, member: &str) -> CanonicalSymbolId {
        crate::symbols::canonical_builtin_member_identity(owner, member)
    }

    /// Copy a referenced scalar or shared function pointer through the builtin Clone contract. Capturing callable
    /// values and exclusive function references do not acquire pointer-copy semantics from their source type.
    pub(super) fn clone_reference_value(
        &mut self,
        receiver: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let (ResolvedType::Ref(inner) | ResolvedType::RefMut(inner)) = receiver else {
            return None;
        };
        let scalar = matches!(
            inner.as_ref(),
            ResolvedType::Int
                | ResolvedType::Float
                | ResolvedType::Numeric(_)
                | ResolvedType::Bool
                | ResolvedType::Unit
        );
        let pointer =
            matches!(receiver, ResolvedType::Ref(_)) && matches!(inner.as_ref(), ResolvedType::Function(_, _));
        if !(scalar || pointer) || method != trait_bounds::rust::CLONE_METHOD {
            return None;
        }
        if !self.validate_rust_method_type_arg_arity(method, &[], type_args, span) {
            return Some(ResolvedType::Unknown);
        }
        if !args.is_empty() {
            self.errors
                .push(errors::builtin_arity("reference clone", 0, args.len(), span));
            return Some(ResolvedType::Unknown);
        }
        self.type_info.record_resolved_identity(
            span,
            Self::compiler_builtin_member_identity(core_traits::as_str(TraitId::Clone), method),
        );
        self.type_info.record_resolved_builtin_method_call(
            span,
            BuiltinMethodId::Clone(incan_lang::lang::surface::clone_methods::CloneMethodId::Clone),
        );
        Some(inner.as_ref().clone())
    }

    /// Retain the accepted inferred Clone requirement only for a rigid parameter's zero-argument fallback. Source
    /// and explicit bound methods have already resolved before this boundary; no legacy fact is changed here.
    pub(super) fn record_body_ir_inferred_clone(
        &mut self,
        receiver: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) {
        if method != trait_bounds::rust::CLONE_METHOD
            || !type_args.is_empty()
            || !args.is_empty()
            || self.rigid_type_param_name(receiver).is_none()
        {
            return;
        }
        self.type_info.traits.inferred_method_identities.insert(
            span,
            Self::compiler_builtin_member_identity(core_traits::as_str(TraitId::Clone), method),
        );
        self.type_info.record_resolved_builtin_method_call(
            span,
            BuiltinMethodId::Clone(incan_lang::lang::surface::clone_methods::CloneMethodId::Clone),
        );
    }

    /// Build a compiler-synthesized member identity from the canonical identity of its owning declaration.
    pub(in crate::typechecker) fn synthetic_member_identity(
        owner: &CanonicalSymbolId,
        member: &str,
        kind: SemanticSourceTargetKind,
    ) -> CanonicalSymbolId {
        owner.generated_member(member, kind)
    }

    /// Resolve an owner declaration first, then mint its compiler-synthesized member target.
    pub(super) fn synthetic_member_identity_for_named_owner(
        &self,
        owner_name: &str,
        member: &str,
        kind: SemanticSourceTargetKind,
    ) -> Option<CanonicalSymbolId> {
        let owner_id = self.symbols.lookup(owner_name)?;
        let owner = self.symbols.identity_of(owner_id)?;
        Some(Self::synthetic_member_identity(owner, member, kind))
    }

    /// Record one compiler-generated member under its checked lexical or provider-owned type identity. Returned
    /// submodule types retain their admitted identity even when no lexical binding uses their internal spelling.
    /// A selected field or result uses its declaration's nominal scope before any same-named consumer binding. An
    /// internal carrier uses the checker-retained nominal identity table rather than a reconstructed source spelling.
    pub(in crate::typechecker::check_expr) fn record_compiler_generated_member_identity(
        &mut self,
        owner_name: &str,
        member: &str,
        receiver: Option<Span>,
        span: Span,
    ) {
        let declared_owner = receiver
            .and_then(|receiver| self.type_info.resolved_identity(receiver))
            .and_then(|declaration| self.declaration_nominal_scopes.get(declaration))
            .and_then(|scope| scope.get(owner_name));
        let Some(owner) = declared_owner
            .or_else(|| {
                self.type_info
                    .declarations
                    .named_type_identities
                    .get(owner_name)
                    .filter(|identity| {
                        incan_semantics_core::body_ir::nominal_carrier_key(identity).as_deref() == Some(owner_name)
                    })
            })
            .or_else(|| {
                self.symbols
                    .lookup(owner_name)
                    .and_then(|symbol| self.symbols.identity_of(symbol))
            })
            .or_else(|| self.public_library_type_identities.get(owner_name)?.canonical.as_ref())
            .cloned()
        else {
            return;
        };
        let identity = Self::synthetic_member_identity(&owner, member, SemanticSourceTargetKind::Method);
        self.type_info.record_resolved_identity(span, identity.clone());
        self.type_info
            .record_compiler_generated_member_identity(identity, owner);
    }

    /// Name the RFC 088 `iter()` member that starts an iterator over a list; the list method registry does not list it,
    /// because the iterator surface owns it.
    pub(super) fn iterable_iter_member(method: &str) -> Option<&'static str> {
        let iter = iterator_methods::IteratorMethodId::Iter;
        (iterator_methods::from_str(method) == Some(iter)).then(|| iterator_methods::as_str(iter))
    }

    /// Build the `Option[Enum]` return type for generated `from_value(...)`.
    pub(super) fn value_enum_from_value_return_type(enum_name: &str) -> ResolvedType {
        option_ty(ResolvedType::Named(enum_name.to_string()))
    }

    /// Typecheck generated value-enum helpers reached through member-access call syntax.
    pub(super) fn check_value_enum_generated_method_call(
        &mut self,
        call: ValueEnumGeneratedCall<'_>,
    ) -> Option<ResolvedType> {
        match call.method {
            "value" if !call.base_is_type_name => {
                if !call.type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(call.span));
                }
                if !call.args.is_empty() {
                    self.errors.push(errors::builtin_arity(
                        &format!("{}.value()", call.enum_name),
                        0,
                        call.args.len(),
                        call.span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                Some(call.value_enum.value_type.resolved_type())
            }
            "from_value" if call.base_is_type_name => {
                if !call.type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(call.span));
                }
                if call.args.len() != 1 {
                    self.errors.push(errors::builtin_arity(
                        &format!("{}.from_value()", call.enum_name),
                        1,
                        call.args.len(),
                        call.span,
                    ));
                    return Some(ResolvedType::Unknown);
                }
                let expected = call.value_enum.value_type.resolved_type();
                if let Some((arg_ty, arg)) = call.arg_types.first().zip(call.args.first()) {
                    let expr = match arg {
                        CallArg::Positional(expr)
                        | CallArg::Named(_, expr)
                        | CallArg::PositionalUnpack(expr)
                        | CallArg::KeywordUnpack(expr) => expr,
                    };
                    if !self.types_compatible(arg_ty, &expected) {
                        self.errors.push(errors::type_mismatch(
                            &expected.to_string(),
                            &arg_ty.to_string(),
                            expr.span,
                        ));
                    }
                }
                Some(Self::value_enum_from_value_return_type(call.enum_name))
            }
            _ => None,
        }
    }

    /// Resolve metadata fields from their published declaration and retain the exact selected member identity.
    /// Generated `__fields__()` results need no explicit import; the SDK remains the single field schema authority.
    pub(super) fn resolve_surface_type_field_type(
        &mut self,
        type_name: &str,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let name = surface_types::as_str(SurfaceTypeId::FieldInfo);
        let module = vec![stdlib::STDLIB_ROOT.to_owned(), stdlib::STDLIB_REFLECTION.to_owned()];
        if type_name == name {
            if self.lookup_symbol(type_name).is_some() && self.surface_type_identity(SurfaceTypeId::FieldInfo).is_none()
            {
                return None;
            }
        } else {
            // Generated metadata keeps its selected owner even when a local declaration shadows the SDK name.
            // Only the retained checked identity authorizes this implicit field schema lookup.
            let selected = self
                .type_info
                .declarations
                .body_ir_native_type_identities
                .get(type_name)?;
            if self.loaf_member_identity(&module, name).as_ref() != Some(selected) {
                return None;
            }
        }
        let (library, path) = self.loaf_module(&module)?;
        let (SymbolKind::Type(TypeInfo::Class(info)), _) =
            self.lookup_pub_library_module_symbol_member(&library, &path, name)?
        else {
            return None;
        };
        let selected = info.fields.get(field)?;
        if let Some(identity) = selected.identity.clone() {
            self.type_info.record_resolved_identity(span, identity);
        }
        Some(selected.ty.clone())
    }

    /// Return the surface-level result type for built-in reflection magic methods.
    ///
    /// These methods are compiler-provided rather than declared in user-visible source, so the typechecker must model
    /// their return types directly.
    pub(super) fn field_overlay_value_type_for_nominal(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        let (type_name, type_args) = match ty {
            ResolvedType::Named(name) => (name.as_str(), None),
            ResolvedType::Generic(name, args) => (name.as_str(), Some(args.as_slice())),
            _ => return None,
        };
        let type_info = self.lookup_semantic_type_info(type_name)?;
        let (type_params, fields): (&[String], Vec<&FieldInfo>) = match type_info {
            TypeInfo::Model(model) => (
                model.type_params.as_slice(),
                model.fields.values().filter(|field| !field.is_type_private).collect(),
            ),
            TypeInfo::Class(class) => (
                class.type_params.as_slice(),
                class.fields.values().filter(|field| !field.is_type_private).collect(),
            ),
            _ => return None,
        };
        if fields.is_empty() {
            return Some(ResolvedType::Unit);
        }
        let subst = type_args.map(|args| type_param_subst_map(type_params, args));
        let mut value_types: Vec<ResolvedType> = fields
            .into_iter()
            .map(|field| match &subst {
                Some(subst) => substitute_resolved_type(&field.ty, subst),
                None => field.ty.clone(),
            })
            .collect();
        value_types.sort_by_key(|ty| ty.to_string());
        value_types.dedup();
        if value_types.len() == 1 {
            value_types.pop()
        } else {
            Some(union_ty(value_types))
        }
    }

    /// Return the declared surface type for a compiler-provided reflection magic method.
    ///
    /// These methods do not have user-visible source declarations, so method-call checking uses this helper to expose
    /// their synthesized return types while preserving nominal receiver-specific field value typing.
    pub(super) fn reflection_magic_method_return_type(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
    ) -> Option<ResolvedType> {
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => Some(ResolvedType::Str),
            Some(magic_methods::MagicMethodId::Fields) => Some(self.reflection_metadata_type()),
            Some(magic_methods::MagicMethodId::FieldValue) => {
                self.field_overlay_value_type_for_nominal(receiver_ty).map(option_ty)
            }
            Some(magic_methods::MagicMethodId::FieldItems) => self
                .field_overlay_value_type_for_nominal(receiver_ty)
                .map(|value_ty| list_ty(ResolvedType::Tuple(vec![ResolvedType::Str, value_ty]))),
            _ => None,
        }
    }

    /// Return the receiver-independent reflection result type available through an inferred generic capability.
    pub(super) fn generic_reflection_magic_method_return_type(&mut self, method: &str) -> Option<ResolvedType> {
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => Some(ResolvedType::Str),
            Some(magic_methods::MagicMethodId::Fields) => Some(self.reflection_metadata_type()),
            Some(magic_methods::MagicMethodId::FieldValue) => Some(option_ty(ResolvedType::Str)),
            Some(magic_methods::MagicMethodId::FieldItems) => {
                Some(list_ty(ResolvedType::Tuple(vec![ResolvedType::Str, ResolvedType::Str])))
            }
            _ => None,
        }
    }

    /// Retain the published metadata carrier selected by a generated reflection call, without requiring a source
    /// import. The SDK declaration supplies its identity and field layout; no consumer recreates the record by its
    /// spelling.
    pub(super) fn reflection_metadata_type(&mut self) -> ResolvedType {
        let name = surface_types::as_str(SurfaceTypeId::FieldInfo);
        let module = vec![stdlib::STDLIB_ROOT.to_owned(), stdlib::STDLIB_REFLECTION.to_owned()];
        if let Some(identity) = self.loaf_member_identity(&module, name) {
            let SymbolOrigin::Package { library, module_path } = &identity.origin else {
                return ResolvedType::FrozenList(Box::new(ResolvedType::Named(name.to_owned())));
            };
            let mut path = module_path.clone();
            path.push(identity.declaration_name.clone());
            let carrier = crate::typechecker::canonical_public_library_type_name(library, &path.join("::"));
            self.type_info
                .declarations
                .body_ir_native_type_identities
                .insert(carrier.clone(), identity);
            return ResolvedType::FrozenList(Box::new(ResolvedType::Named(carrier)));
        }
        ResolvedType::FrozenList(Box::new(ResolvedType::Named(name.to_owned())))
    }

    /// Validate a reflection magic-method call.
    pub(super) fn validate_reflection_magic_call(
        &mut self,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) {
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let expected_arity = match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName)
            | Some(magic_methods::MagicMethodId::Fields)
            | Some(magic_methods::MagicMethodId::FieldItems) => 0,
            Some(magic_methods::MagicMethodId::FieldValue) => 1,
            _ => return,
        };
        if args.len() != expected_arity {
            self.errors
                .push(errors::builtin_arity(method, expected_arity, args.len(), span));
        }
    }

    /// Report whether a nominal type is allowed to use a given reflection magic method.
    ///
    /// Support is intentionally method-specific: `__class_name__()` is limited to models and classes, while
    /// `__fields__()` also applies to newtypes.
    pub(super) fn nominal_type_supports_reflection_magic(&self, ty: &ResolvedType, method: &str) -> bool {
        let type_name = match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name.as_str(),
            _ => return false,
        };
        match magic_methods::from_str(method) {
            Some(magic_methods::MagicMethodId::ClassName) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_))
                )
            }
            Some(magic_methods::MagicMethodId::Fields) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
                )
            }
            Some(magic_methods::MagicMethodId::FieldValue | magic_methods::MagicMethodId::FieldItems) => {
                matches!(
                    self.lookup_semantic_type_info(type_name),
                    Some(TypeInfo::Model(_) | TypeInfo::Class(_))
                )
            }
            _ => false,
        }
    }
}
