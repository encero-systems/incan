//! Rust argument boundary matching, borrow mutability, and boundary value conversions.

use super::*;

impl TypeChecker {
    /// Return how a rusttype boundary matches an argument type.
    pub(super) fn rusttype_boundary_match(
        &self,
        arg_ty: &ResolvedType,
        target_ty: &ResolvedType,
    ) -> Option<RustArgCoercionKind> {
        if let ResolvedType::Named(type_name) = arg_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
        {
            if self.types_compatible(&newtype.underlying, target_ty) {
                return Some(RustArgCoercionKind::RustTypeUnwrap);
            }
            if newtype.has_interop {
                return Some(RustArgCoercionKind::RustTypeInterop);
            }
        }

        // RFC 041 (`from S ...`) edges convert non-rusttype arguments into a rusttype surface. Type checking marks
        // these as interop-capable when the target is a rusttype with declared interop, and lowering picks the concrete
        // adapter edge.
        if let ResolvedType::Named(type_name) = target_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
            && newtype.has_interop
        {
            return Some(RustArgCoercionKind::RustTypeInterop);
        }

        None
    }

    /// Return whether a Rust type display names a generic type parameter.
    pub(super) fn is_rust_generic_type_param_display(rust_ty: &str) -> bool {
        let normalized = Self::compact_rust_display(rust_ty);
        if let Some(tail) = normalized.strip_prefix("impl").map(str::trim_start) {
            return !tail.is_empty()
                && (tail.contains("::") || tail.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()));
        }
        let mut chars = normalized.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_uppercase() || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return false;
        }
        !matches!(
            normalized.as_str(),
            "Box" | "HashMap" | "HashSet" | "Option" | "Result" | "Self" | "String" | "Vec"
        )
    }

    /// Return whether a borrowed Rust parameter is a dynamically-dispatched trait object.
    ///
    /// Trait-object compatibility is established from checked Rust metadata, but Rust still requires the reference
    /// syntax at the call site. Recording it here keeps typechecking and generated-Rust ownership in agreement.
    pub(super) fn rust_display_is_trait_object(rust_ty: &str) -> bool {
        rust_ty.trim_start().starts_with("dyn ")
    }

    /// Render an Incan type into the canonical boundary vocabulary used by interop coercion policy lookup.
    ///
    /// Returns `None` for shapes not covered by the builtin boundary coercion matrix.
    pub(super) fn incan_boundary_type_display(arg_ty: &ResolvedType) -> Option<String> {
        match arg_ty {
            ResolvedType::Int => Some("int".to_string()),
            ResolvedType::Float => Some("float".to_string()),
            ResolvedType::Numeric(id) => Some(numerics::as_str(*id).to_string()),
            ResolvedType::Bool => Some("bool".to_string()),
            ResolvedType::Str => Some("str".to_string()),
            ResolvedType::FrozenStr => Some("FrozenStr".to_string()),
            ResolvedType::Bytes => Some("bytes".to_string()),
            ResolvedType::FrozenBytes => Some("FrozenBytes".to_string()),
            ResolvedType::Unit => Some("unit".to_string()),
            ResolvedType::FrozenList(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenList[{elem_display}]"))
            }
            ResolvedType::FrozenDict(key, value) => {
                let key_display = Self::incan_boundary_type_display(key)?;
                let value_display = Self::incan_boundary_type_display(value)?;
                Some(format!("FrozenDict[{key_display}, {value_display}]"))
            }
            ResolvedType::FrozenSet(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenSet[{elem_display}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Option[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 =>
            {
                let ok = Self::incan_boundary_type_display(&args[0])?;
                let err = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Result[{ok}, {err}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("List[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                let key = Self::incan_boundary_type_display(&args[0])?;
                let value = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Dict[{key}, {value}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Set) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Set[{inner}]"))
            }
            ResolvedType::Tuple(elems) => {
                let mut rendered = Vec::with_capacity(elems.len());
                for elem in elems {
                    rendered.push(Self::incan_boundary_type_display(elem)?);
                }
                Some(format!("Tuple[{}]", rendered.join(", ")))
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::incan_boundary_type_display(inner),
            _ => None,
        }
    }

    /// Whether a Rust type display string belongs to the builtin boundary coercion matrix.
    ///
    /// This intentionally includes fully-qualified std/core/alloc spellings that rust-analyzer may emit.
    pub(super) fn is_builtin_rust_boundary_display(rust_ty: &str) -> bool {
        matches!(
            rust_ty,
            "bool"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "String"
                | "std::string::String"
                | "&str"
                | "Vec<u8>"
                | "std::vec::Vec<u8>"
                | "&[u8]"
                | "()"
        ) || rust_ty.starts_with("Option<")
            || rust_ty.starts_with("std::option::Option<")
            || rust_ty.starts_with("core::option::Option<")
            || rust_ty.starts_with("Result<")
            || rust_ty.starts_with("std::result::Result<")
            || rust_ty.starts_with("core::result::Result<")
            || rust_ty.starts_with("Vec<")
            || rust_ty.starts_with("std::vec::Vec<")
            || rust_ty.starts_with("alloc::vec::Vec<")
            || rust_ty.starts_with("HashMap<")
            || rust_ty.starts_with("std::collections::HashMap<")
            || rust_ty.starts_with("std::collections::hash_map::HashMap<")
            || rust_ty.starts_with("HashSet<")
            || rust_ty.starts_with("std::collections::HashSet<")
            || rust_ty.starts_with("std::collections::hash_set::HashSet<")
            || (rust_ty.starts_with('(') && rust_ty.ends_with(')'))
    }

    /// Classify whether an Incan argument type can satisfy a Rust parameter boundary.
    ///
    /// This first tries builtin coercion-matrix matches, then resolved-type compatibility, then rusttype-specific
    /// boundary adapters. Nested borrows keep their token boundaries throughout type resolution.
    /// Exact canonical carrier equality also recognizes transparent aliases nested inside owned
    /// containers without changing their passing mode.
    pub(super) fn rust_arg_boundary_match(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> RustArgBoundaryMatch {
        if matches!(arg_ty, ResolvedType::Function(..))
            && let Some(signature) = self.rust_boxed_callable_signature(rust_param_ty)
        {
            return if self.rust_trait_carriers_match(arg_ty, &signature) {
                RustArgBoundaryMatch::Exact
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        let display = Self::rust_display_without_lifetimes(rust_param_ty);
        let normalized = Self::compact_rust_display(&display);
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str())
            && Self::rust_display_is_trait_object(inner)
        {
            let target_inner_ty = self.resolved_type_from_rust_display(inner);
            return if self.types_compatible(arg_ty, &target_inner_ty)
                || self.rust_type_implements_trait_object(arg_ty, inner)
            {
                RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: is_mut })
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        // A by-value trait object reaches an argument through a specialized generic — `Arc::new(v)` checked
        // against `Arc<dyn TableProvider>` asks whether `v` is a `dyn TableProvider`. Rust admits that by
        // unsized coercion when `v`'s type implements the trait; the borrowed `&dyn Trait` branch above already
        // proves exactly that from metadata, so reuse it rather than fall through to a display-string check.
        if Self::rust_display_is_trait_object(display.as_str()) {
            return if self.rust_type_implements_trait_object(arg_ty, display.as_str()) {
                RustArgBoundaryMatch::Exact
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        if let Some(target_ty) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return if self.types_compatible(arg_ty, &target_ty) {
                RustArgBoundaryMatch::Exact
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        if Self::rust_display_type_var_name(normalized.as_str()).is_some() {
            return RustArgBoundaryMatch::Exact;
        }
        let borrowed_shared = matches!(Self::rust_display_borrow_kind(display.as_str()), Some((false, _)));
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_normalized = Self::compact_rust_display(inner);
            let target_inner_ty = self.resolved_type_from_rust_display(inner);
            if matches!(arg_ty, ResolvedType::RefMut(existing) if self.types_compatible(existing, &target_inner_ty))
                || (!is_mut
                    && matches!(arg_ty, ResolvedType::Ref(existing) if self.types_compatible(existing, &target_inner_ty)))
            {
                return RustArgBoundaryMatch::Exact;
            }
            if Self::is_rust_generic_type_param_display(inner_normalized.as_str())
                && !is_mut
                && !matches!(arg_ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            {
                return RustArgBoundaryMatch::Exact;
            }
            if !is_mut && self.types_compatible(arg_ty, &target_inner_ty) {
                return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: false });
            }
            if is_mut {
                if self.types_compatible(arg_ty, &target_inner_ty) {
                    return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: true });
                }
                if let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
                    && let Some(CoercionPolicy::Exact) =
                        admitted_builtin_coercion(incan_display.as_str(), inner_normalized.as_str())
                {
                    return RustArgBoundaryMatch::Exact;
                }
            }
        }
        if let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
            && Self::is_builtin_rust_boundary_display(normalized.as_str())
        {
            if Self::rust_display_borrow_kind(display.as_str()).is_none() {
                let target_ty = self.resolved_type_from_rust_display(display.as_str());
                if self.types_compatible(arg_ty, &target_ty) || self.rust_trait_carriers_match(arg_ty, &target_ty) {
                    return RustArgBoundaryMatch::Exact;
                }
            }
            return match admitted_builtin_coercion(incan_display.as_str(), normalized.as_str()) {
                Some(CoercionPolicy::Exact) => RustArgBoundaryMatch::Exact,
                Some(policy) => RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy)),
                None => RustArgBoundaryMatch::NoMatch,
            };
        }
        // Keep token boundaries inside native arguments: compacting `&mut T` into `&mutT` changes its referent.
        let target_ty = self.resolved_type_from_rust_display(display.as_str());
        let should_try_exact_type_match = !borrowed_shared || Self::incan_boundary_type_display(arg_ty).is_none();
        if should_try_exact_type_match
            && (self.types_compatible(arg_ty, &target_ty) || self.rust_trait_carriers_match(arg_ty, &target_ty))
        {
            return RustArgBoundaryMatch::Exact;
        }
        if let Some(kind) = self.rusttype_boundary_match(arg_ty, &target_ty) {
            return RustArgBoundaryMatch::Coercion(kind);
        }
        if let Some(incan_name) = Self::incan_boundary_type_display(arg_ty)
            && let Some(policy) = admitted_builtin_coercion(incan_name.as_str(), normalized.as_str())
        {
            return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy));
        }
        RustArgBoundaryMatch::NoMatch
    }

    /// Return whether complete Rust metadata proves that an argument type implements a trait-object boundary.
    ///
    /// Each bound of the object is its own obligation: implementing the principal trait never grants `Send` or `Sync`.
    /// A bound with type arguments, a higher-ranked binder or a modifier is not proven here.
    pub(super) fn rust_type_implements_trait_object(&self, arg_ty: &ResolvedType, trait_object_display: &str) -> bool {
        let Ok(syn::Type::TraitObject(object)) = syn::parse_str::<syn::Type>(trait_object_display) else {
            return false;
        };
        let Some((actual_path, _, _, _)) = self.rust_identity_for_type(arg_ty) else {
            return false;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(actual_path.as_str()) else {
            return false;
        };
        !object.bounds.is_empty()
            && object.bounds.iter().all(|bound| {
                let syn::TypeParamBound::Trait(bound) = bound else {
                    return matches!(bound, syn::TypeParamBound::Lifetime(_));
                };
                if bound.lifetimes.is_some()
                    || !matches!(bound.modifier, syn::TraitBoundModifier::None)
                    || bound
                        .path
                        .segments
                        .iter()
                        .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
                {
                    return false;
                }
                let path = bound.path.to_token_stream().to_string().replace(' ', "");
                let definition = self.rust_definition_for_path(&path);
                Self::rust_type_metadata_implements_trait(&metadata, &path, definition.as_deref())
            })
    }

    /// Reject an immutable source binding when a planned Rust boundary borrow is exclusive, and record the exclusive
    /// use of a caller-visible `mut` parameter as a change to it.
    pub(in crate::typechecker) fn validate_rust_borrow_mutability(
        &mut self,
        kind: RustArgCoercionKind,
        arg_expr: &Spanned<Expr>,
    ) -> bool {
        if matches!(
            kind,
            RustArgCoercionKind::Borrow { mutable: true } | RustArgCoercionKind::TraitObjectBorrow { mutable: true }
        ) {
            self.note_mut_param_exclusive_use(arg_expr);
        }
        if matches!(
            kind,
            RustArgCoercionKind::Borrow { mutable: true } | RustArgCoercionKind::TraitObjectBorrow { mutable: true }
        ) && let Expr::Ident(name) = &arg_expr.node
            && !self.mutable_bindings.contains(name)
        {
            self.errors
                .push(errors::mutable_rust_borrow_requires_mut(name, arg_expr.span));
            return false;
        }
        true
    }

    /// Require write access to the receiver of a Rust method that takes `&mut self`.
    ///
    /// A receiver that already is an exclusive reference, such as the payload `Option.as_mut()` binds, lends its
    /// referent again: the method changes the referent, never the binding, so the binding needs no `mut`. Any other
    /// receiver is borrowed exclusively, which a binding declared without `mut` does not allow.
    pub(in crate::typechecker) fn validate_exclusive_rust_receiver(&mut self, receiver: &Spanned<Expr>, span: Span) {
        if matches!(self.type_info.expr_type(receiver.span), Some(ResolvedType::RefMut(_))) {
            return;
        }
        self.refuse_write_through_read_only_binding(receiver, span);
        self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, receiver);
    }

    /// Validate one expression used at a Rust value boundary and record the exact coercion lowering must preserve.
    ///
    /// This is the shared Rust-boundary argument plan for free functions, methods, and named-field Rust constructors.
    /// Callers provide the Rust display from metadata; this method resolves owner-relative displays, prewarms identity
    /// metadata, and records the chosen coercion in the same artifact map lowering already consumes.
    pub(in crate::typechecker) fn validate_rust_boundary_value(
        &mut self,
        owner_path: &str,
        rust_type_display: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
        record_exact_builtin_coercion: bool,
    ) {
        self.validate_rust_boundary_value_with_target(
            owner_path,
            rust_type_display,
            arg_expr,
            arg_ty,
            record_exact_builtin_coercion,
            None,
        );
    }

    /// Preserve an instantiated checked parameter alongside its display when validating a generic argument. An exact
    /// structural match needs no conversion; all other arguments still require the ordinary native boundary plan.
    /// The optional target never authorizes a new borrow, numeric widening, or nominal-name equivalence.
    pub(super) fn validate_rust_boundary_value_with_target(
        &mut self,
        owner_path: &str,
        rust_type_display: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
        record_exact_builtin_coercion: bool,
        instantiated: Option<&ResolvedType>,
    ) {
        self.record_rust_callable_parameter_displays(rust_type_display, owner_path, arg_expr, arg_ty);
        let param_display = self.rust_display_for_owner_path(rust_type_display, owner_path);
        let normalized = Self::compact_rust_display(&param_display);
        let target_ty =
            self.resolved_rust_boundary_target_from_param_display_for_owner_path(rust_type_display, owner_path);
        if self.retain_rust_range_argument(arg_expr, &target_ty) {
            return;
        }
        self.prewarm_rust_type_identity_metadata(arg_ty);
        self.prewarm_rust_boundary_type_identity_metadata(&target_ty);
        if self.retain_native_list_conversion(arg_expr, arg_ty, &target_ty) {
            return;
        }
        let boundary = match self.rust_arg_boundary_match(arg_ty, param_display.as_str()) {
            RustArgBoundaryMatch::NoMatch
                if instantiated.is_some_and(|target| {
                    self.rust_inference_type(arg_ty, &[]) == self.rust_inference_type(target, &[])
                }) =>
            {
                RustArgBoundaryMatch::Exact
            }
            matched => matched,
        };
        match boundary {
            RustArgBoundaryMatch::Exact => {
                if Self::rust_display_type_var_name(normalized.as_str()).is_some() {
                    // A borrowed Rust result passed directly into a generic Rust parameter must keep its native
                    // reference type. Owning it first changes generic selection and can add an unnecessary clone
                    // (for example `String::from(match_.as_str())`). Record the consumption as well as clearing an
                    // existing coercion because return and argument artifacts can be discovered in either order.
                    // The Rust bound remains the final authority.
                    self.type_info.rust.native_return_consumers.insert(arg_expr.span);
                    self.type_info.rust.return_coercions.remove(&arg_expr.span);
                }
                if record_exact_builtin_coercion
                    && let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
                    && admitted_builtin_coercion(incan_display.as_str(), normalized.as_str())
                        == Some(CoercionPolicy::Exact)
                {
                    self.type_info.rust.arg_coercions.insert(
                        arg_expr.span,
                        RustArgCoercionInfo {
                            rust_target_type: normalized,
                            target_type: target_ty,
                            kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
                        },
                    );
                }
            }
            RustArgBoundaryMatch::Coercion(kind) => {
                if !self.validate_rust_borrow_mutability(kind, arg_expr) {
                    return;
                }
                self.type_info.rust.arg_coercions.insert(
                    arg_expr.span,
                    RustArgCoercionInfo {
                        rust_target_type: normalized,
                        target_type: target_ty,
                        kind,
                    },
                );
            }
            RustArgBoundaryMatch::NoMatch => {
                self.errors.push(errors::type_mismatch(
                    rust_type_display,
                    &arg_ty.to_string(),
                    arg_expr.span,
                ));
            }
        }
    }

    /// Retain an explicit owned-list mapping only when native From metadata proves each element conversion.
    /// Canonically identical or open element types keep ordinary argument inference; borrowed collections are not
    /// converted.
    pub(super) fn retain_native_list_conversion(
        &mut self,
        expression: &Spanned<Expr>,
        source: &ResolvedType,
        target: &ResolvedType,
    ) -> bool {
        // ---- Owned collection shape ----
        let (ResolvedType::Generic(source_name, source_args), ResolvedType::Generic(target_name, target_args)) =
            (source, target)
        else {
            return false;
        };
        if collection_type_id(source_name) != Some(CollectionTypeId::List)
            || collection_type_id(target_name) != Some(CollectionTypeId::List)
        {
            return false;
        }
        let ([source], [target]) = (source_args.as_slice(), target_args.as_slice()) else {
            return false;
        };

        // ---- Written literal elements or one checked mapping ----
        if self.retain_native_literal_conversions(expression, target) {
            return true;
        }
        if self.rust_trait_carriers_match(source, target)
            || matches!(source, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
        {
            return false;
        }
        let Some(conversion) = self.native_element_conversion(source, target) else {
            return false;
        };
        self.type_info
            .rust
            .element_adjustments
            .insert(expression.span, conversion);
        true
    }

    /// Preserve native conversions for a contextually typed list literal before its expected element type hides
    /// the written values' types. Mixed compatible values keep their ordinary storage, and a spread stays outside
    /// this fixed-element mapping. Aliases of the same native carrier require no conversion, and frozen text uses the
    /// language's ordinary owned-string boundary.
    pub(super) fn retain_native_literal_conversions(
        &mut self,
        expression: &Spanned<Expr>,
        target: &ResolvedType,
    ) -> bool {
        let Expr::List(entries) = &expression.node else {
            return false;
        };

        // ---- Complete literal conversion selection ----
        let mut conversions = Vec::new();
        for entry in entries {
            let crate::ast::ListEntry::Element(value) = entry else {
                return false;
            };
            let Some(actual) = self.type_info.expr_type(value.span) else {
                return false;
            };
            let source = if *actual == ResolvedType::FrozenStr {
                &ResolvedType::Str
            } else {
                actual
            };
            if self.rust_trait_carriers_match(source, target) {
                continue;
            }
            let Some(conversion) = self.native_element_conversion(source, target) else {
                return false;
            };
            conversions.push((value.span, vec![conversion]));
        }

        // ---- Retain only a fully selected literal ----
        if conversions.is_empty() {
            return false;
        }
        self.type_info.rust.argument_adjustments.extend(conversions);
        true
    }

    /// Preserve direct Rust-return arguments when the receiving Rust callable has no inspected signature.
    ///
    /// Without parameter metadata the compiler cannot justify converting a borrowed Rust result into its owned Incan
    /// surface representation before handing it straight back to Rust. Retain the native result and leave the unknown
    /// boundary to rustc. The expression marker also makes this independent of return/argument traversal order.
    pub(in crate::typechecker) fn preserve_unresolved_rust_call_argument_returns(&mut self, args: &[CallArg]) {
        for arg in args {
            let expr = Self::call_arg_expr(arg);
            let key = expr.span;
            self.type_info.rust.native_return_consumers.insert(key);
            self.type_info.rust.return_coercions.remove(&key);
        }
    }
}
