//! Rust signature inference, native signature types, and retained Rust function values.

use super::*;

impl TypeChecker {
    /// Select the native argument carrier after an already approved boundary conversion, preserving the checked borrow
    /// or numeric conversion when a call is retained. A source range keeps ordinary storage; a half-open literal
    /// constructs Rust's public Range only at this boundary.
    pub(in crate::typechecker::check_expr) fn rust_argument_native_type(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        if let Some(conversion) = self.type_info.rust.element_adjustments.get(&expression.span) {
            return Some(ResolvedType::Generic(
                "List".into(),
                vec![conversion.return_type.clone()],
            ));
        }
        if let Some(coercion) = self.type_info.rust_arg_coercion(expression.span) {
            // Builtin boundary types retain unsized text and slices. The source-facing target intentionally
            // normalizes those to source str/List, which would change a second inference pass to &String/&Vec.
            if matches!(
                coercion.kind,
                RustArgCoercionKind::Builtin(_) | RustArgCoercionKind::Borrow { mutable: false }
            ) {
                return Some(self.native_rust_signature_type(&coercion.rust_target_type, ""));
            }
            return Some(coercion.target_type.clone());
        }
        match &expression.node {
            Expr::Paren(inner) => self.rust_argument_native_type(inner),
            Expr::Range { inclusive: false, .. } => self
                .type_info
                .calls
                .body_ir_constructor_types
                .get(&expression.span)
                .cloned()
                .or_else(|| Some(self.native_rust_signature_type("std::ops::Range<i64>", ""))),
            _ => self.rust_native_value_type(expression),
        }
    }

    /// Project declared Rust parameters and canonical native constructors into ordinary structural inference.
    /// Re-exported owners normalize once, and checked generic placeholders retain their declared binders.
    /// Source aliases expand before native inference; lowering receives their structural carrier, never an alias
    /// name embedded inside a native generic argument. Native aliases expand from retained templates; lifetime-only
    /// or defaulted owners with no effective arguments share their metadata self template's bare identity.
    /// Structured arguments already carry checked bindings, so only a rendered owner's base enters the constructor.
    /// Anonymous native binders keep the declaration's exact key even when a signature parser normalizes whitespace.
    pub(in crate::typechecker) fn rust_inference_type(&self, ty: &ResolvedType, parameters: &[String]) -> ResolvedType {
        map_resolved_type_leaves(ty, &|leaf| {
            if let Some(target) = self.rusttype_native_target(leaf) {
                return Some(self.rust_inference_type(&target, parameters));
            }
            if let ResolvedType::Named(name) | ResolvedType::TypeVar(name) | ResolvedType::RustPath(name) = leaf
                && let Some(parameter) = parameters.iter().find(|parameter| {
                    parameter
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .eq(name.chars().filter(|ch| !ch.is_whitespace()))
                })
            {
                return Some(ResolvedType::TypeVar(parameter.clone()));
            }
            if let Some(name) = self.generic_placeholder_name(leaf) {
                return Some(ResolvedType::TypeVar(name.to_owned()));
            }
            if let ResolvedType::Named(name) = leaf
                && matches!(self.lookup_semantic_type_info(name), Some(TypeInfo::Newtype(info)) if info.is_rusttype)
                && let Some(path) = self.rust_canonical_path_for_receiver_type(leaf)
            {
                return Some(self.rust_inference_type(&ResolvedType::RustPath(path), parameters));
            }
            if matches!(leaf, ResolvedType::Named(_) | ResolvedType::Generic(_, _)) {
                let expanded = self.expand_type_aliases(leaf.clone());
                if expanded != *leaf {
                    return Some(self.rust_inference_type(&expanded, parameters));
                }
            }
            if let ResolvedType::Generic(base, arguments) = leaf {
                let path = self.rust_canonical_path_for_receiver_type(leaf);
                let canonical = path
                    .as_ref()
                    .map(|path| self.type_info.rust.item_public_paths.get(path).unwrap_or(path))
                    .map_or(base.as_str(), String::as_str);
                let canonical = Self::rust_generic_base_and_args(canonical).map_or(canonical, |(base, _)| base);
                return Some(self.rust_inference_constructor(canonical, arguments, parameters));
            }
            if *leaf == ResolvedType::Bytes {
                return Some(ResolvedType::Generic(
                    "List".into(),
                    vec![ResolvedType::Numeric(numerics::NumericTypeId::U8)],
                ));
            }
            let ResolvedType::RustPath(path) = leaf else {
                return None;
            };
            if self
                .generic_placeholder_name(&ResolvedType::Named(path.clone()))
                .is_some()
            {
                return Some(ResolvedType::TypeVar(path.trim().to_owned()));
            }
            let source_alias = ResolvedType::Named(path.clone());
            let expanded = self.expand_type_aliases(source_alias.clone());
            if expanded != source_alias {
                return Some(self.rust_inference_type(&expanded, parameters));
            }
            let (base, arguments) = Self::rust_generic_base_and_args(path).unwrap_or((path, Vec::new()));
            if let Some(target) = self.rust_alias_expansion(base, &arguments) {
                return Some(self.rust_inference_type(&self.native_rust_signature_type(&target, ""), parameters));
            }
            if arguments.is_empty() {
                return None;
            }
            if let Some(structural) = self.resolved_structural_rust_param_display(path, |checker, argument| {
                checker.resolved_type_from_rust_display(argument)
            }) {
                return Some(self.rust_inference_type(&structural, parameters));
            }
            let base = self
                .type_info
                .rust
                .item_public_paths
                .get(base)
                .map_or(base, String::as_str);
            let arguments = arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .map(|argument| self.native_rust_signature_type(argument, ""))
                .collect::<Vec<_>>();
            Some(self.rust_inference_constructor(base, &arguments, parameters))
        })
    }

    /// Normalize native defaults before projecting an ordinary collection shape, preserving the checked public
    /// owner path for storage. Builtin tuple applications become structural tuples before shared inference matches
    /// Rust return signatures; a nondefault allocator remains an exact native argument.
    pub(in crate::typechecker) fn rust_inference_constructor(
        &self,
        base: &str,
        arguments: &[ResolvedType],
        parameters: &[String],
    ) -> ResolvedType {
        let displays = arguments
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>();
        let references = displays.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(target) = self.rust_alias_expansion(base, &references) {
            return self.rust_inference_type(&self.native_rust_signature_type(&target, ""), parameters);
        }
        let arguments = arguments
            .iter()
            .map(|argument| self.rust_inference_type(argument, parameters))
            .collect();
        if collection_type_id(base) == Some(CollectionTypeId::Tuple) {
            return ResolvedType::Tuple(arguments);
        }
        if !base.contains("::") {
            return ResolvedType::Generic(base.to_owned(), arguments);
        }
        let arguments = match self.rust_item_metadata_for_path(base).as_deref() {
            Some(incan_lang::interop::RustItemMetadata {
                kind: RustItemKind::Type(info),
                ..
            }) => self.rust_storage_owner_arguments(arguments, info, base),
            _ => arguments,
        };
        if arguments.is_empty() {
            return ResolvedType::RustPath(base.to_owned());
        }
        let Some(collection) = Self::rust_collection_id_from_display_base(base) else {
            return ResolvedType::Generic(base.to_owned(), arguments);
        };
        match Self::resolved_structural_rust_arguments(collection, arguments) {
            Ok(structural) => self.native_frozen_owner(base, "", structural),
            Err(arguments) => ResolvedType::Generic(base.to_owned(), arguments),
        }
    }

    /// The canonical Rust path a generic template names: its own path, or the item an imported Rust name stands for
    /// (`Box` imported from `std::boxed`), so a template written with the import unifies with a native owner.
    pub(in crate::typechecker::check_expr::calls) fn rust_template_canonical_path(&self, name: &str) -> Option<String> {
        if name.contains("::") {
            return Some(name.to_owned());
        }
        self.rust_canonical_path_for_nominal_receiver(name, None)
    }

    /// Expand a source collection into an exact native owner only when a native template requires its hidden
    /// default parameters. A whole type variable still binds the original source type through ordinary inference.
    pub(in crate::typechecker) fn rust_inference_owner_for_template(
        &self,
        base: &str,
        actual: &ResolvedType,
    ) -> Option<ResolvedType> {
        if !base.contains("::") {
            return None;
        }
        let mut arguments = self.rust_owner_type_args(actual, base)?;
        let metadata = self.rust_item_metadata_for_path(base)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        if !self.complete_rust_owner_arguments(&info, &mut arguments) {
            return None;
        }
        let expanded = ResolvedType::Generic(base.to_owned(), arguments);
        (&expanded != actual).then_some(expanded)
    }

    /// Infer declared function or impl arguments from bound inputs and the expected output, using the same
    /// structural unifier as Incan functions. References describe passing mode and do not prevent an owned argument
    /// from fixing the referent's type. An inconsistent binding remains unknown and cannot authorize a native call.
    pub(in crate::typechecker::check_expr) fn infer_rust_signature_bindings(
        &self,
        sig: &RustFunctionSig,
        owner_path: &str,
        parameters: &[String],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        expected: Option<&ResolvedType>,
    ) -> HashMap<String, ResolvedType> {
        let mut bound = HashMap::new();
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        for (index, (argument, actual)) in args.iter().zip(arg_types).enumerate() {
            let parameter = match argument {
                CallArg::Positional(_) => params.get(index),
                CallArg::Named(name, _) => params.iter().find(|param| param.name.as_ref() == Some(&name.node)),
                _ => None,
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let target = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                parameter.type_display.as_str(),
                owner_path,
            );
            let target = self.rust_inference_type(&target, parameters);
            let native_actual = self.rust_argument_native_type(Self::call_arg_expr(argument));
            let native_actual = native_actual.map(|native| {
                self.rust_range_bound_destination(sig, &parameter.type_display, Self::call_arg_expr(argument), &native)
                    .unwrap_or(native)
            });
            let actual = self.rust_inference_type(native_actual.as_ref().unwrap_or(actual), parameters);
            // Frozen text crosses into Rust in the owned-string form a `str` value has: the frozen carrier implements
            // no Rust trait, so a generic parameter bound to it could never satisfy its callee's bounds (#2590).
            let actual = if actual == ResolvedType::FrozenStr {
                ResolvedType::Str
            } else {
                actual
            };
            let (target, actual) = match (&target, &actual) {
                (ResolvedType::Ref(_) | ResolvedType::RefMut(_), ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => {
                    (&target, &actual)
                }
                (ResolvedType::Ref(inner) | ResolvedType::RefMut(inner), _) => (inner.as_ref(), &actual),
                _ => (&target, &actual),
            };
            self.infer_type_param_bindings(target, actual, &mut bound);
            if let Some(callable) = self.rust_bound_callable_type(sig, &parameter.type_display) {
                self.infer_type_param_bindings(&self.rust_inference_type(&callable, parameters), actual, &mut bound);
            }
        }
        if let Some(expected) = expected {
            let result = self.resolved_rust_return_type_from_sig(sig, owner_path);
            self.infer_type_param_bindings(
                &self.rust_inference_type(&result, parameters),
                &self.rust_inference_type(expected, parameters),
                &mut bound,
            );
        }
        self.infer_rust_predicate_bindings(sig, parameters, &mut bound);
        self.prefer_borrowed_bindings(sig, parameters, &mut bound);
        bound
    }

    /// Instantiate signature displays from inferred types before recording boundary and result facts.
    pub(super) fn specialize_inferred_rust_signature(
        &self,
        sig: &RustFunctionSig,
        owner: &str,
        bound: &HashMap<String, ResolvedType>,
    ) -> RustFunctionSig {
        let substitutions = bound
            .iter()
            .filter(|(_, ty)| !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
            .map(|(name, ty)| (name.as_str(), render_resolved_type_as_rust_arg(ty)))
            .collect();
        self.rust_signature_projections(Self::specialize_rust_signature(sig, &substitutions, owner))
    }

    /// Instantiate a checked Rust signature structurally so Incan argument identities survive the foreign boundary.
    /// Rendering an Incan model into a Rust display must not replace its retained declaration with a bare name.
    pub(in crate::typechecker::check_expr) fn instantiate_rust_signature_type(
        &self,
        ty: &ResolvedType,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
    ) -> ResolvedType {
        substitute_resolved_type(&self.rust_inference_type(ty, parameters), bindings)
    }

    /// Retain unsized native text, slices, and trait-object principals structurally before generic substitution in
    /// parameters and results. `[]` is an internal generic constructor unavailable as a source identifier. A generic
    /// `&T` instantiated with source text borrows its owned String carrier instead of being reclassified as an
    /// unsized string.
    ///
    /// A display that names a nominal carrier key resolves through the active module's bindings, which the cache key
    /// does not hold, so it bypasses the cache.
    pub(crate) fn native_rust_signature_type(&self, display: &str, owner: &str) -> ResolvedType {
        if display.contains(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return self.resolve_native_rust_signature_type(display, owner);
        }
        let key = (display.to_owned(), owner.to_owned());
        if let Some(resolved) = self.native_queries.native_signature_types.borrow().get(&key) {
            return resolved.clone();
        }
        let resolved = self.resolve_native_rust_signature_type(display, owner);
        self.native_queries
            .native_signature_types
            .borrow_mut()
            .insert(key, resolved.clone());
        resolved
    }

    /// Resolve one native signature display for `native_rust_signature_type`, which caches the result; nested
    /// displays recurse through the cached entry point. Metadata-proven aliases of unsized text retain the native
    /// referent before ordinary source projection can turn them into owned string values.
    pub(super) fn resolve_native_rust_signature_type(&self, display: &str, owner: &str) -> ResolvedType {
        let display = self.rust_display_for_owner_path(display, owner);
        let display = Self::rust_display_without_lifetimes(&display);
        // Parsed qualified owners render namespace punctuation with surrounding whitespace. Normalize that
        // punctuation before metadata lookup while preserving word boundaries in `dyn`, `mut`, and `as`.
        let display = display.split("::").map(str::trim).collect::<Vec<_>>().join("::");
        let display = display.trim();
        // Rust's associated values can parenthesize a dynamic referent with an explicit object lifetime.
        // Parentheses change no carrier; the object parser retains its principal and associated equalities.
        if display.starts_with('(')
            && let Ok(syn::Type::Paren(parenthesized)) = syn::parse_str::<syn::Type>(display)
        {
            return self.native_rust_signature_type(&parenthesized.elem.to_token_stream().to_string(), owner);
        }
        // Qualified projections must keep the `as` token separated until the associated-type query normalizes them.
        // Compact path rendering would merge its owner and trait names into an unresolvable nominal identity.
        if display.starts_with('<') {
            return ResolvedType::RustPath(display.to_owned());
        }
        if display == "str" || self.rust_alias_expansion(display, &[]).as_deref() == Some("str") {
            return ResolvedType::RustPath("str".into());
        }
        if display.starts_with("dyn ")
            && let Some(object) = self.native_rust_object_type(display)
        {
            return object;
        }
        // The impossible source identifier `[]` is the structural native slice constructor, never a Vec alias.
        if let Some(element) = display.strip_prefix('[').and_then(|inner| inner.strip_suffix(']'))
            && !element.contains(';')
        {
            return ResolvedType::Generic("[]".into(), vec![self.native_rust_signature_type(element, owner)]);
        }
        if let Some((mutable, inner)) = Self::rust_display_borrow_kind(display) {
            let inner = Box::new(self.native_rust_signature_type(inner, owner));
            return if mutable {
                ResolvedType::RefMut(inner)
            } else {
                ResolvedType::Ref(inner)
            };
        }
        if let Some(inner) = display.strip_prefix('(').and_then(|inner| inner.strip_suffix(')'))
            && inner.contains(',')
        {
            return ResolvedType::Tuple(
                Self::split_top_level_generic_args(inner)
                    .into_iter()
                    .map(|element| self.native_rust_signature_type(element, owner))
                    .collect(),
            );
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display, |checker, argument| {
            checker.native_rust_signature_type(argument, owner)
        }) {
            return self.native_frozen_owner(display, owner, structural);
        }
        let ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(display, owner);
        let ty = map_resolved_type_leaves(&ty, &|leaf| {
            matches!(leaf, ResolvedType::Ref(inner) if **inner == ResolvedType::Str)
                .then(|| ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into()))))
        });
        self.native_frozen_owner(display, owner, self.native_signature_storage_owner(ty))
    }

    /// Preserve a metadata-proven frozen nominal's owner alongside its inferred arguments. Source frozen views are
    /// useful for expression checking but cannot unify with a native impl Self template after erasing its owner.
    pub(in crate::typechecker::check_expr) fn native_frozen_owner(
        &self,
        display: &str,
        owner: &str,
        ty: ResolvedType,
    ) -> ResolvedType {
        let arguments = match &ty {
            ResolvedType::FrozenList(element) | ResolvedType::FrozenSet(element) => vec![*element.clone()],
            ResolvedType::FrozenDict(key, value) => vec![*key.clone(), *value.clone()],
            _ => return ty,
        };
        match self.rust_result_item_path(display, owner) {
            Some(path) => ResolvedType::Generic(path, arguments),
            None => ty,
        }
    }

    /// Canonicalize metadata-defaulted native storage at signature intake, as receiver selection already does. A
    /// payload layout and a call signature must not retain different arities for the same defaulted native owner.
    pub(in crate::typechecker::check_expr) fn native_signature_storage_owner(&self, ty: ResolvedType) -> ResolvedType {
        let normalized = map_resolved_type_leaves(&ty, &|leaf| self.short_native_storage_owner(leaf));
        let normalized = self.short_native_storage_owner(&normalized).unwrap_or(normalized);
        if normalized != ty {
            return self.native_signature_storage_owner(normalized);
        }
        normalized
    }

    /// Shorten one exact native owner from its metadata defaults. A changed display may reveal a structural
    /// collection or nested owner, so the caller walks the resulting structure before publishing its storage type.
    pub(super) fn short_native_storage_owner(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        let path = match ty {
            ResolvedType::RustPath(path) if path.contains('<') => self.rust_path_base_and_args(path).0,
            ResolvedType::Generic(base, _) if base.contains("::") => base.clone(),
            _ => return None,
        };
        if let Some(metadata) = self.known_rust_metadata_for_path(&path)
            && let RustItemKind::Type(info) = &metadata.kind
        {
            return Some(self.rust_storage_owner(ty, &info, &path));
        }
        None
    }

    /// Retain a declaration-ordered inference result for Body IR; conflicting constraints are source errors.
    pub(super) fn record_inferred_rust_arguments(
        &mut self,
        path: &str,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        for parameter in parameters {
            if bindings.get(parameter) == Some(&ResolvedType::Unknown) {
                self.errors
                    .push(errors::call_site_type_inference_unresolved(path, parameter, span));
            }
        }
        if parameters.is_empty() {
            return;
        }
        let arguments = parameters
            .iter()
            .map(|parameter| bindings.get(parameter).cloned())
            .collect::<Option<Vec<_>>>();
        if let Some(arguments) = arguments {
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert(span, arguments);
        }
    }

    /// Retain a safe call whose Rust signature and native instantiation were fully selected. Async calls retain a
    /// future of the declared output, which the existing native constructor boxes without polling its body.
    pub(super) fn retain_checked_rust_function(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        if signature.is_unsafe {
            return;
        }
        self.retain_native_callback_parameters(path, signature, &signature.type_params);
        let arguments = signature
            .type_params
            .iter()
            .map(|parameter| bindings.get(parameter).cloned())
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments else {
            return;
        };
        let mut parameters = self.type_info.call_site_callable_params(span).unwrap_or(&[]).to_vec();
        for (parameter, declared) in parameters.iter_mut().zip(&signature.params) {
            let template = self.native_rust_signature_type(&declared.type_display, path);
            parameter.ty = self.instantiate_rust_signature_type(&template, &signature.type_params, bindings);
        }
        let result = self.instantiate_rust_signature_type(
            &self.native_rust_signature_type(&signature.return_type, path),
            &signature.type_params,
            bindings,
        );
        let result = if signature.return_type.contains(" as ") {
            let selected = self.specialize_inferred_rust_signature(signature, path, bindings);
            self.native_rust_signature_type(&selected.return_type, path)
        } else {
            result
        };
        let result = if signature.is_async {
            ResolvedType::Generic("Awaitable".into(), vec![result])
        } else {
            result
        };
        self.type_info.rust.checked_calls.insert(
            span,
            crate::typechecker::type_info::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                signature_loans: Self::native_signature_loan_identities(signature),
                receiver_adjustments: Vec::new(),
                path: path.trim_start_matches("rust::").to_owned(),
                type_arguments: arguments,
                parameters,
                return_type: result,
                result_item_path: self.rust_result_item_path(&signature.return_type, path),
                has_receiver: false,
            },
        );
    }

    /// Retain a safe monomorphic free-function read using the same signature selection as its direct application.
    pub(in crate::typechecker::check_expr) fn retain_rust_function_value(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        span: Span,
    ) -> Option<ResolvedType> {
        if signature.is_async || signature.is_unsafe || !signature.type_params.is_empty() {
            return None;
        }
        self.record_rust_call_site_params(span, &signature.params, path);
        self.retain_checked_rust_function(path, signature, &HashMap::new(), span);
        self.publish_rust_function_value(span)
    }

    /// Keep the selected callable's native parameters and result under its function-value role, so the read never
    /// supplies an application's result provenance.
    pub(in crate::typechecker::check_expr) fn publish_rust_function_value(
        &mut self,
        span: Span,
    ) -> Option<ResolvedType> {
        let key = span;
        let selected = self.type_info.rust.checked_calls.remove(&key)?;
        let ty = ResolvedType::Function(selected.parameters.clone(), Box::new(selected.return_type.clone()));
        self.type_info.rust.checked_function_values.insert(key, selected);
        Some(ty)
    }

    /// Record an ownership conversion for borrowed Rust scalar-like returns that Incan exposes as owned values.
    pub(in crate::typechecker) fn record_rust_return_coercion_from_display(
        &mut self,
        rust_return_type: &str,
        incan_ret: &ResolvedType,
        span: Span,
    ) {
        if self.type_info.rust.native_return_consumers.contains(&span) {
            return;
        }
        let normalized = rust_return_type.replace(' ', "");
        let is_borrowed_str = normalized == "&str" || (normalized.starts_with("&'") && normalized.ends_with("str"));
        let target = if is_borrowed_str && matches!(incan_ret, ResolvedType::Str) {
            Some(("String", ResolvedType::Str))
        } else {
            let is_borrowed_bytes =
                normalized == "&[u8]" || (normalized.starts_with("&'") && normalized.ends_with("[u8]"));
            if is_borrowed_bytes && matches!(incan_ret, ResolvedType::Bytes) {
                Some(("Vec<u8>", ResolvedType::Bytes))
            } else {
                None
            }
        };
        let Some((rust_target_type, target_type)) = target else {
            return;
        };
        self.type_info.rust.return_coercions.insert(
            span,
            RustArgCoercionInfo {
                rust_target_type: rust_target_type.to_string(),
                target_type,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
            },
        );
    }
}
