//! Rust receiver paths, contextual Rust receivers, and Rust owner type arguments and signatures.

use super::*;

impl TypeChecker {
    /// Return the receiver's Rust item path, keeping instantiated arguments on the checked receiver type.
    /// Method identity never contains generic display syntax; impl inference reads arguments from the receiver.
    /// Native slice displays use their structural `[]` owner while retaining their element type separately.
    pub(in crate::typechecker) fn rust_canonical_path_for_receiver_type(&self, ty: &ResolvedType) -> Option<String> {
        // ---- Rusttype backing owner ----
        if let Some(target) = self.rusttype_native_target(ty) {
            return self.rust_canonical_path_for_receiver_type(&target);
        }

        // ---- Native and source-projected receivers ----
        match ty {
            ResolvedType::Generic(base, arguments) if base == "[]" && arguments.len() == 1 => Some(base.clone()),
            ResolvedType::Generic(base, _) if base.contains("::") => Some(base.clone()),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                self.rust_canonical_path_for_receiver_type(inner).or_else(|| {
                    match inner.as_ref() {
                        // A source str owns String storage; a native unsized str keeps its own RustPath.
                        ResolvedType::Str => Some("std::string::String".to_owned()),
                        ResolvedType::Generic(name, _) => {
                            incan_lang::interop::RustCollectionFamily::for_type_name(name)
                                .map(|family| family.canonical_path().to_owned())
                        }
                        _ => None,
                    }
                })
            }
            ResolvedType::RustPath(path) => {
                if path.starts_with("dyn ") {
                    let object = self.native_rust_signature_type(path, "");
                    return (object != *ty)
                        .then(|| self.rust_canonical_path_for_receiver_type(&object))
                        .flatten();
                }
                if let ResolvedType::Generic(base, _) = self.native_rust_signature_type(path, "")
                    && base == "[]"
                {
                    return Some(base);
                }
                Some(Self::rust_generic_base_and_args(path).map_or_else(|| path.clone(), |(base, _)| base.to_owned()))
            }
            ResolvedType::Named(name) => self.rust_canonical_path_for_nominal_receiver(name, None),
            ResolvedType::Generic(name, args) => {
                if name.contains("::") {
                    return Some(name.trim_start_matches("::").to_owned());
                }
                self.rust_canonical_path_for_nominal_receiver(name, Some(args.as_slice()))
            }
            _ => None,
        }
    }

    /// Return the import binding a bare receiver expression names, when it is an imported Rust item.
    ///
    /// `Trait.method(...)` and `Type.method(...)` spell the receiver as the import binding itself; a value receiver
    /// such as `handle.method(...)`, including a local that happens to be a bare identifier, has no binding to report.
    pub(super) fn rust_import_binding_for_receiver_expr<'a>(&self, receiver: &'a Spanned<Expr>) -> Option<&'a str> {
        match &receiver.node {
            Expr::Ident(name) => self
                .lookup_symbol(name)
                .filter(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
                .map(|_| name.as_str()),
            Expr::Paren(inner) => self.rust_import_binding_for_receiver_expr(inner),
            _ => None,
        }
    }

    /// Return metadata attached to the exact imported Rust receiver expression.
    ///
    /// Multiple compiled providers may describe the same canonical Rust path. The selected public import remains the
    /// semantic authority for its reexport rather than allowing a later path-only lookup to select another provider.
    pub(super) fn rust_metadata_for_receiver_expr(&self, receiver: &Spanned<Expr>) -> Option<Arc<RustItemMetadata>> {
        match &receiver.node {
            Expr::Ident(name) => {
                let SymbolKind::RustItem(info) = &self.lookup_symbol(name)?.kind else {
                    return None;
                };
                info.metadata.clone()
            }
            Expr::Paren(inner) => self.rust_metadata_for_receiver_expr(inner),
            _ => None,
        }
    }

    /// Prefer exact receiver metadata when it can authoritatively answer one method lookup.
    pub(super) fn rust_metadata_for_receiver_method(
        &self,
        rust_path: &str,
        method: &str,
        receiver_metadata: Option<&Arc<RustItemMetadata>>,
    ) -> Option<Arc<RustItemMetadata>> {
        if let Some(metadata) = receiver_metadata {
            let is_authoritative = match &metadata.kind {
                RustItemKind::Type(type_info) => {
                    type_info.methods.iter().any(|candidate| candidate.name == method)
                        || type_info.metadata_completeness.has_methods()
                }
                RustItemKind::Constant { .. } => false,
                _ => true,
            };
            if is_authoritative {
                return Some(metadata.clone());
            }
        }
        self.rust_item_metadata_for_method_call(rust_path, method)
    }

    /// Return the canonical Rust item path for a nominal receiver. Owner arguments remain on the receiver type;
    /// generic rusttype templates must not put their argument display into method or variant identities.
    pub(in crate::typechecker) fn rust_canonical_path_for_nominal_receiver(
        &self,
        name: &str,
        type_args: Option<&[ResolvedType]>,
    ) -> Option<String> {
        if let Some(TypeInfo::Newtype(newtype)) = self.lookup_semantic_type_info(name)
            && newtype.is_rusttype
        {
            let underlying = if let Some(args) = type_args {
                let subst = type_param_subst_map(&newtype.type_params, args);
                substitute_resolved_type(&newtype.underlying, &subst)
            } else {
                newtype.underlying.clone()
            };
            if let Some(path) = self.rust_path_for_rusttype_underlying(&underlying) {
                if let Some((base, _)) = Self::rust_generic_base_and_args(&path) {
                    return Some(base.to_owned());
                }
                return Some(path);
            }
        }

        let id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(id)?;
        let SymbolKind::RustItem(info) = &sym.kind else {
            return None;
        };
        match info.binding {
            RustImportBindingKind::CrateRoot => None,
            RustImportBindingKind::RootedPath | RustImportBindingKind::FromImport => Some(info.path.clone()),
        }
    }

    /// Whether a contextual type specializes the native owner, including its registered source collection projection.
    pub(super) fn contextual_type_matches_rust_owner(expected: &ResolvedType, rust_path: &str) -> bool {
        // Metadata records the ancestral spelling of an owner (`alloc::boxed::Box`); the source imported the `std`
        // one. Every re-export of one item is an alias of it, so the owner match must happen in the ancestral
        // namespace or a `Self`-returning call like `Box.new(x)` never reconciles with its expected `Box<T>`.
        let owner = incan_lang::interop::ancestral_rust_path(rust_path);
        match expected {
            ResolvedType::RustPath(path) => path
                .split('<')
                .next()
                .is_some_and(|base| incan_lang::interop::ancestral_rust_path(base) == owner),
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => {
                incan_lang::interop::ancestral_rust_path(name) == owner
                    || RustCollectionFamily::for_canonical_path(rust_path)
                        .is_some_and(|family| RustCollectionFamily::for_type_name(name) == Some(family))
            }
            _ => false,
        }
    }

    /// Find the concrete receiver occupying a `Self` position in one expected result.
    pub(super) fn contextual_rust_receiver_type(
        actual: &ResolvedType,
        expected: &ResolvedType,
        rust_path: &str,
    ) -> Option<ResolvedType> {
        match actual {
            ResolvedType::RustPath(path)
                if path == "Self" && Self::contextual_type_matches_rust_owner(expected, rust_path) =>
            {
                Some(expected.clone())
            }
            ResolvedType::RustPath(path)
                if path.split('<').next().is_some_and(|base| {
                    incan_lang::interop::ancestral_rust_path(base)
                        == incan_lang::interop::ancestral_rust_path(rust_path)
                }) && Self::contextual_type_matches_rust_owner(expected, rust_path) =>
            {
                Some(expected.clone())
            }
            ResolvedType::Generic(_name, _) | ResolvedType::Named(_name)
                if Self::contextual_type_matches_rust_owner(actual, rust_path)
                    && Self::contextual_type_matches_rust_owner(expected, rust_path) =>
            {
                Some(expected.clone())
            }
            ResolvedType::Ref(inner) => match expected {
                ResolvedType::Ref(expected_inner) => {
                    Self::contextual_rust_receiver_type(inner, expected_inner, rust_path)
                }
                _ => None,
            },
            ResolvedType::RefMut(inner) => match expected {
                ResolvedType::RefMut(expected_inner) => {
                    Self::contextual_rust_receiver_type(inner, expected_inner, rust_path)
                }
                _ => None,
            },
            ResolvedType::Generic(name, args) => match expected {
                ResolvedType::Generic(expected_name, expected_args)
                    if name == expected_name && args.len() == expected_args.len() =>
                {
                    args.iter()
                        .zip(expected_args)
                        .find_map(|(actual, expected)| Self::contextual_rust_receiver_type(actual, expected, rust_path))
                }
                _ => None,
            },
            ResolvedType::Tuple(items) => match expected {
                ResolvedType::Tuple(expected_items) if items.len() == expected_items.len() => items
                    .iter()
                    .zip(expected_items)
                    .find_map(|(actual, expected)| Self::contextual_rust_receiver_type(actual, expected, rust_path)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Read native owner argument slots from a checked specialization or its registered source collection carrier.
    /// References retain passing mode elsewhere; a different owner returns None, and omitted defaults are filled
    /// separately from the declaration's metadata before unification or signature construction.
    pub(in crate::typechecker) fn rust_owner_type_args(
        &self,
        receiver: &ResolvedType,
        rust_path: &str,
    ) -> Option<Vec<ResolvedType>> {
        if let Some(target) = self.rusttype_native_target(receiver) {
            return self.rust_owner_type_args(&target, rust_path);
        }
        let native_vec =
            incan_lang::interop::ancestral_rust_path(rust_path) == incan_lang::interop::ancestral_rust_path("Vec");
        match receiver {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.rust_owner_type_args(inner, rust_path),
            ResolvedType::Bytes if native_vec => Some(vec![ResolvedType::Numeric(numerics::NumericTypeId::U8)]),
            ResolvedType::Generic(name, args)
                if native_vec && collection_type_id(name) == Some(CollectionTypeId::List) =>
            {
                Some(args.clone())
            }
            ResolvedType::Generic(name, args)
                if matches!(
                    collection_type_id(name),
                    Some(CollectionTypeId::Dict | CollectionTypeId::Set)
                ) && RustCollectionFamily::for_canonical_path(rust_path)
                    .is_some_and(|family| RustCollectionFamily::for_type_name(name) == Some(family)) =>
            {
                Some(args.clone())
            }
            // The receiver may name the owner through any re-export (`alloc::boxed::Box`, a prelude `Box`) while
            // the import spelled `std::boxed::Box`; every spelling of one item must yield its type arguments.
            ResolvedType::Generic(name, args)
                if Self::contextual_type_matches_rust_owner(receiver, rust_path)
                    || self
                        .rust_canonical_path_for_nominal_receiver(name, None)
                        .is_some_and(|path| {
                            incan_lang::interop::ancestral_rust_path(&path)
                                == incan_lang::interop::ancestral_rust_path(rust_path)
                        }) =>
            {
                Some(args.clone())
            }
            ResolvedType::RustPath(display) => {
                if rust_path == "[]"
                    && let ResolvedType::Generic(base, arguments) = self.native_rust_signature_type(display, "")
                    && base == "[]"
                {
                    return Some(arguments);
                }
                let normalized = display.strip_prefix("rust::").unwrap_or(display);
                let (base, args) = Self::rust_generic_base_and_args(normalized).unwrap_or((normalized, Vec::new()));
                if incan_lang::interop::ancestral_rust_path(base) != incan_lang::interop::ancestral_rust_path(rust_path)
                {
                    return None;
                }
                Some(
                    args.into_iter()
                        .filter(|arg| !Self::rust_generic_arg_is_lifetime(arg))
                        .map(|arg| self.native_rust_signature_type(arg, rust_path))
                        .collect(),
                )
            }
            _ => None,
        }
    }

    /// Substitute exact native binders before identifier tokens. Anonymous `impl Trait` parameters carry their full
    /// display as the metadata binder name; matching that whole type preserves the same inference contract as named
    /// parameters without rewriting identifiers inside the trait bound. Every other display goes through the one
    /// tokenizer, [`TypeChecker::substitute_rust_binders`], with `Self` replaced by the receiver.
    pub(super) fn substitute_rust_owner_display(
        display: &str,
        substitutions: &HashMap<&str, String>,
        receiver_display: &str,
    ) -> String {
        if display.trim() != "Self"
            && let Some(replacement) = substitutions.get(display.trim())
        {
            return replacement.clone();
        }
        let binder = |word: &str| substitutions.get(word).map(String::as_str);
        Self::substitute_rust_binders(display, binder, Some(receiver_display))
    }

    /// Apply checked Rust type substitutions to one inspected signature.
    pub(in crate::typechecker::check_expr) fn specialize_rust_signature(
        sig: &RustFunctionSig,
        substitutions: &HashMap<&str, String>,
        receiver_display: &str,
    ) -> RustFunctionSig {
        RustFunctionSig {
            receiver_contract: sig.receiver_contract,
            bounds: sig
                .bounds
                .iter()
                .map(|bound| incan_lang::interop::RustTraitBound {
                    self_type: Self::substitute_rust_owner_display(&bound.self_type, substitutions, receiver_display),
                    path: bound.path.clone(),
                    type_arguments: bound
                        .type_arguments
                        .iter()
                        .map(|argument| Self::substitute_rust_owner_display(argument, substitutions, receiver_display))
                        .collect(),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(|binding| incan_lang::interop::RustAssociatedTypeBinding {
                            name: binding.name.clone(),
                            value_path: Self::substitute_rust_owner_display(
                                &binding.value_path,
                                substitutions,
                                receiver_display,
                            ),
                        })
                        .collect(),
                })
                .collect(),
            type_params: sig.type_params.clone(),
            params: sig
                .params
                .iter()
                .map(|param| incan_lang::interop::RustParam {
                    name: param.name.clone(),
                    type_display: Self::substitute_rust_owner_display(
                        param.type_display.as_str(),
                        substitutions,
                        receiver_display,
                    ),
                })
                .collect(),
            return_type: Self::substitute_rust_owner_display(sig.return_type.as_str(), substitutions, receiver_display),
            is_async: sig.is_async,
            is_unsafe: sig.is_unsafe,
        }
    }

    /// Whether written owner arguments leave only defaulted type parameters, independently of impl method binders.
    pub(super) fn rust_owner_arity_admits(owner: &incan_lang::interop::RustTypeInfo, written: usize) -> bool {
        written <= owner.type_params.len()
            && (written..owner.type_params.len())
                .all(|index| owner.type_param_defaults.get(index).is_some_and(Option::is_some))
    }

    /// Fill omitted owner parameters from metadata, substituting earlier arguments into each dependent default.
    pub(in crate::typechecker) fn complete_rust_owner_arguments(
        &self,
        owner: &incan_lang::interop::RustTypeInfo,
        arguments: &mut Vec<ResolvedType>,
    ) -> bool {
        if !Self::rust_owner_arity_admits(owner, arguments.len()) {
            return false;
        }
        while arguments.len() < owner.type_params.len() {
            let Some(default) = self.owner_default(owner, arguments.len(), arguments, "") else {
                return false;
            };
            arguments.push(default);
        }
        true
    }

    /// Return the declared default of the owner type parameter at `index`, or `None` when it has none.
    ///
    /// This is the one conversion of a native type-parameter default: the earlier arguments (`earlier`, a prefix of
    /// the owner's arguments) are substituted into a dependent default such as `U = T` in `Pair<T, U = T>`, and the
    /// result converts through `native_rust_signature_type` anchored at `anchor`.
    pub(in crate::typechecker) fn owner_default(
        &self,
        owner: &incan_lang::interop::RustTypeInfo,
        index: usize,
        earlier: &[ResolvedType],
        anchor: &str,
    ) -> Option<ResolvedType> {
        let default = owner.type_param_defaults.get(index)?.as_deref()?;
        let count = earlier.len().min(index).min(owner.type_params.len());
        let displays = earlier[..count]
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>();
        let displays = displays.iter().map(String::as_str).collect::<Vec<_>>();
        let default = Self::substitute_rust_display_params(default, &owner.type_params[..count], &displays);
        Some(self.native_rust_signature_type(&default, anchor))
    }

    /// Specialize one inspected associated-function signature with its receiver's concrete type arguments.
    pub(super) fn specialize_rust_owner_signature(
        &self,
        sig: &RustFunctionSig,
        rust_path: &str,
        type_params: &[String],
        type_args: &[ResolvedType],
    ) -> RustFunctionSig {
        let receiver_display = if type_args.is_empty() {
            rust_path.to_string()
        } else {
            format!(
                "{rust_path}<{}>",
                type_args
                    .iter()
                    .map(render_resolved_type_as_rust_arg)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let substitutions = type_params
            .iter()
            .zip(type_args)
            .map(|(param, arg)| (param.as_str(), render_resolved_type_as_rust_arg(arg)))
            .collect::<HashMap<_, _>>();
        self.rust_signature_projections(Self::specialize_rust_signature(
            sig,
            &substitutions,
            receiver_display.as_str(),
        ))
    }

    /// Specialize one inspected method signature with explicit, non-placeholder method type arguments.
    pub(super) fn specialize_rust_method_signature(
        &self,
        sig: &RustFunctionSig,
        rust_path: &str,
        type_args: &[ResolvedType],
    ) -> RustFunctionSig {
        let substitutions = sig
            .type_params
            .iter()
            .zip(type_args)
            .filter(|(_, arg)| !matches!(arg, ResolvedType::CallSiteInfer | ResolvedType::Unknown))
            .map(|(param, arg)| (param.as_str(), render_resolved_type_as_rust_arg(arg)))
            .collect::<HashMap<_, _>>();
        Self::specialize_rust_signature(sig, &substitutions, rust_path)
    }
}
