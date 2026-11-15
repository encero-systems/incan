//! Rust field types and Rust callable aliases for closures and method arguments.

use super::*;

impl TypeChecker {
    /// Resolve a source-facing Rust field spelling to the metadata field it names.
    ///
    /// Rust raw identifier fields should be written with the Rust source name at Incan field-use sites. For example, a
    /// Rust field declared as `r#type` is accessed as `obj.type` and constructed with `TypeName(type=...)`; emission
    /// rawifies the keyword identifier back to `r#type`. An ordinary Rust field declared as `type_` remains available
    /// only as `obj.type_`.
    pub(in crate::typechecker::check_expr) fn rust_field_for_source_name<'a>(
        info: &'a RustTypeInfo,
        source_name: &str,
    ) -> Option<&'a RustFieldInfo> {
        if !info.metadata_completeness.field_is_public(source_name) {
            return None;
        }
        info.fields.iter().find(|field| field.name == source_name)
    }

    /// Resolve the type of one field on a value typed by a Rust path, as the field is written in Incan source.
    ///
    /// The path is the value's own `RustPath` spelling; the metadata lookup normalizes the `rust::` prefix and any
    /// type arguments, exactly as a field read does. A fast cache entry can carry only nominal metadata, so a missing
    /// field upgrades to complete semantic metadata before the checker rejects it. Field access and field assignment
    /// resolve through this one lookup, so a field that can be read can also be assigned. Retain the native field
    /// carrier before projecting its source type so subsequent method lookup consumes this same selection.
    pub(in crate::typechecker) fn rust_path_field_type(
        &mut self,
        path: &str,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        // Field reads pass the value's path straight to the metadata lookup, which owns the `rust::` and generic
        // normalization; a bare `demo::Holder` has no `<...>` to strip and must resolve exactly like a read does.
        let mut metadata = self.rust_item_metadata_for_path(path)?;
        if matches!(&metadata.kind, RustItemKind::Type(info) if Self::rust_field_for_source_name(info, field).is_none())
        {
            metadata = self.rust_item_metadata_for_complete_type(path)?;
        }
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let rust_field = Self::rust_field_for_source_name(info, field)?;
        self.retain_rust_layout(path, info);
        let base = Self::rust_generic_base_and_args(path).map_or(path, |(base, _)| base);
        if let Some(owner) = incan_semantics_core::body_ir::rust_item_identity(base) {
            self.type_info.record_resolved_identity(
                span,
                Self::synthetic_member_identity(&owner, &rust_field.name, SemanticSourceTargetKind::Field),
            );
        }
        let (source_type, native) = self.resolved_rust_field_types(path, rust_field);
        self.type_info.rust.field_values.insert(span, native);
        Some(source_type)
    }

    /// Resolve source and native field types together from metadata against the instantiated owner.
    ///
    /// Rust field metadata carries both a structural shape and the source display. Field access should use the display
    /// because it preserves exact numeric widths and owner-relative paths that matter when a Rust field is copied back
    /// into another Rust boundary. The structural shape remains useful for broad semantic classification and older
    /// metadata entries whose display cannot be resolved. Native method lookup keeps the metadata-selected item even
    /// when the source type projects to a collection.
    pub(super) fn resolved_rust_field_types(
        &self,
        owner_path: &str,
        field: &RustFieldInfo,
    ) -> (ResolvedType, RustCheckedFieldValue) {
        let mut display = self.rust_display_for_owner_path(field.type_display.as_str(), owner_path);
        if let Some((base, arguments)) = Self::rust_generic_base_and_args(owner_path)
            && let Some(metadata) = self.rust_item_metadata_for_path(base)
            && let RustItemKind::Type(info) = &metadata.kind
        {
            let arguments = arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .collect::<Vec<_>>();
            display = Self::substitute_rust_display_params(&display, &info.type_params, &arguments);
        }
        let resolved = self.resolved_type_from_rust_display(display.as_str());
        let source = if matches!(resolved, ResolvedType::Unknown) {
            self.resolved_type_from_rust_shape(&field.type_shape)
        } else {
            resolved
        };
        let native = RustCheckedFieldValue {
            native_type: self.native_rust_signature_type(&display, owner_path),
            item_path: self.rust_result_item_path(&display, owner_path),
        };
        (source, native)
    }

    /// Return the target display for a Rust type alias when the expected destination type names one.
    pub(super) fn rust_callable_alias_target_display(&self, expected_ty: &ResolvedType) -> Option<String> {
        let ResolvedType::RustPath(path) = expected_ty else {
            return None;
        };
        self.rust_callable_alias_target_display_for_path(path, &mut std::collections::HashSet::new())
    }

    /// Follow Rust type-alias chains until they expose a callable trait object target.
    ///
    /// This is intentionally metadata-driven rather than crate-specific. DataFusion's
    /// `ScalarFunctionImplementation -> Arc<dyn Fn(...)>` chain is one motivating surface, but the compiler must not
    /// special-case DataFusion or require regression tests to compile that heavyweight crate.
    ///
    /// Use blocking metadata reads here so contextual closure typing does not depend on whether a transitive alias was
    /// already imported elsewhere or happened to be warmed by an earlier arm in the same expression.
    pub(super) fn rust_callable_alias_target_display_for_path(
        &self,
        path: &str,
        seen: &mut std::collections::HashSet<String>,
    ) -> Option<String> {
        let canonical_path = Self::normalize_rust_namespace_path(path).to_string();
        if !seen.insert(canonical_path.clone()) {
            return None;
        }
        if let Some(metadata) = self.rust_item_metadata_for_path(path)
            && let RustItemKind::Type(type_info) = &metadata.kind
            && let Some(target) = type_info.alias_target.as_ref()
        {
            let display = self.rust_display_for_owner_path(target, canonical_path.as_str());
            let display = self.canonicalize_reexported_callable_alias_paths(display, &metadata);
            if Self::rust_display_has_callable_fn_bound(display.as_str()) {
                return Some(display);
            }
            let (target_base, _) = self.rust_path_base_and_args(display.as_str());
            if target_base != canonical_path
                && let Some(expanded) = self.rust_callable_alias_target_display_for_path(target_base.as_str(), seen)
            {
                return Some(expanded);
            }
            return None;
        }
        Some(self.rust_display_for_owner_path(path, path))
            .filter(|display| Self::rust_display_has_callable_fn_bound(display.as_str()))
    }

    /// Replace definition-module paths inside a public callable alias with verified public re-exports.
    ///
    /// Rust aliases can be publicly re-exported from a crate root while their source RHS still names the private
    /// defining module. Inferred closure parameters retain that RHS spelling for ABI accuracy, but generated consumer
    /// Rust must use a path visible from the consumer crate. Each replacement is checked against Rust metadata
    /// before it is made, so a neighboring private type is never guessed to be public merely because the alias itself
    /// was re-exported.
    pub(super) fn canonicalize_reexported_callable_alias_paths(
        &self,
        display: String,
        alias: &RustItemMetadata,
    ) -> String {
        let Some(definition_path) = alias.definition_path.as_deref() else {
            return display;
        };
        let Some((canonical_module, _)) = alias.canonical_path.rsplit_once("::") else {
            return display;
        };
        let Some((definition_module, _)) = definition_path.rsplit_once("::") else {
            return display;
        };
        if canonical_module == definition_module {
            return display;
        }

        let definition_prefix = format!("{definition_module}::");
        let canonical_prefix = format!("{canonical_module}::");
        let mut rendered = String::with_capacity(display.len());
        let mut remaining = display.as_str();
        while let Some(index) = remaining.find(definition_prefix.as_str()) {
            let (before, candidate) = remaining.split_at(index);
            rendered.push_str(before);
            let previous = rendered.chars().next_back();
            if previous.is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric() || ch == ':') {
                rendered.push_str(definition_prefix.as_str());
                remaining = &candidate[definition_prefix.len()..];
                continue;
            }

            let after_prefix = &candidate[definition_prefix.len()..];
            let item_len = after_prefix
                .char_indices()
                .take_while(|(_, ch)| *ch == '_' || *ch == '#' || ch.is_ascii_alphanumeric())
                .map(|(index, ch)| index + ch.len_utf8())
                .last()
                .unwrap_or(0);
            if item_len == 0 {
                rendered.push_str(definition_prefix.as_str());
                remaining = after_prefix;
                continue;
            }

            let item = &after_prefix[..item_len];
            let definition_item_path = format!("{definition_prefix}{item}");
            let canonical_item_path = format!("{canonical_prefix}{item}");
            let has_public_reexport = self
                .rust_item_metadata_for_path(canonical_item_path.as_str())
                .is_some_and(|metadata| {
                    metadata.visibility == RustVisibility::Public
                        && metadata.definition_path.as_deref() == Some(definition_item_path.as_str())
                });
            if has_public_reexport {
                rendered.push_str(canonical_item_path.as_str());
            } else {
                rendered.push_str(definition_item_path.as_str());
            }
            remaining = &after_prefix[item_len..];
        }
        rendered.push_str(remaining);
        rendered
    }

    /// Read a boxed native callback's metadata signature before a direct function pointer is boxed at its call
    /// boundary. Capturing callable objects still need their own auto-trait proof and are not admitted by this
    /// projection.
    pub(in crate::typechecker::check_expr) fn rust_boxed_callable_signature(
        &self,
        display: &str,
    ) -> Option<ResolvedType> {
        let (owner, arguments) = Self::rust_generic_base_and_args(display)?;
        if !incan_lang::interop::is_box_carrier_path(owner) || arguments.len() != 1 {
            return None;
        }
        let signature = self.rust_callable_alias_signature(&ResolvedType::RustPath(display.to_owned()))?;
        Some(ResolvedType::Function(
            signature
                .params
                .iter()
                .map(|parameter| {
                    CallableParam::positional(self.native_rust_signature_type(&parameter.rust_display, ""))
                })
                .collect(),
            Box::new(signature.return_ty),
        ))
    }

    /// Parse a Rust callable alias target such as `Arc<dyn Fn(&[T]) -> Result<U, E> + Send + Sync>`.
    pub(super) fn rust_callable_alias_signature(
        &self,
        expected_ty: &ResolvedType,
    ) -> Option<RustCallableAliasSignature> {
        let target_display = self.rust_callable_alias_target_display(expected_ty)?;
        let ty = syn::parse_str::<SynType>(&target_display).ok()?;
        let fn_bound = Self::rust_callable_fn_bound(&ty)?;

        let params = fn_bound
            .inputs
            .iter()
            .map(|input| {
                let rust_display = Self::compact_rust_display(&input.to_token_stream().to_string());
                RustCallableAliasParam {
                    resolved_ty: self.resolved_param_type_from_rust_display(&rust_display),
                    rust_display,
                }
            })
            .collect::<Vec<_>>();
        let return_ty = match &fn_bound.output {
            ReturnType::Default => ResolvedType::Unit,
            ReturnType::Type(_, ty) => {
                let rust_display = Self::compact_rust_display(&ty.to_token_stream().to_string());
                self.resolved_type_from_rust_display(&rust_display)
            }
        };
        Some(RustCallableAliasSignature { params, return_ty })
    }

    /// Return whether a Rust display type contains a callable trait-object target.
    pub(super) fn rust_display_has_callable_fn_bound(display: &str) -> bool {
        let Ok(ty) = syn::parse_str::<SynType>(display) else {
            return false;
        };
        Self::rust_callable_fn_bound(&ty).is_some()
    }

    /// Return the `Fn(...) -> ...` bound carried by a Rust callable trait-object target.
    pub(super) fn rust_callable_fn_bound(ty: &SynType) -> Option<&syn::ParenthesizedGenericArguments> {
        let trait_object = Self::rust_callable_trait_object(ty)?;
        trait_object.bounds.iter().find_map(|bound| {
            let TypeParamBound::Trait(trait_bound) = bound else {
                return None;
            };
            let segment = trait_bound.path.segments.last()?;
            if !matches!(segment.ident.to_string().as_str(), "Fn" | "FnMut" | "FnOnce") {
                return None;
            }
            let PathArguments::Parenthesized(args) = &segment.arguments else {
                return None;
            };
            Some(args)
        })
    }

    /// Find the Rust trait-object type wrapped by a callable alias target.
    pub(super) fn rust_callable_trait_object(ty: &SynType) -> Option<&syn::TypeTraitObject> {
        match ty {
            SynType::TraitObject(trait_object) => Some(trait_object),
            SynType::Group(group) => Self::rust_callable_trait_object(&group.elem),
            SynType::Paren(paren) => Self::rust_callable_trait_object(&paren.elem),
            SynType::Path(path) => {
                let segment = path.path.segments.last()?;
                let PathArguments::AngleBracketed(args) = &segment.arguments else {
                    return None;
                };
                args.args.iter().find_map(|arg| match arg {
                    GenericArgument::Type(inner) => Self::rust_callable_trait_object(inner),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// Check a closure expression against a Rust callable alias.
    pub(super) fn check_closure_with_rust_callable_alias(
        &mut self,
        expr: &Spanned<Expr>,
        signature: &RustCallableAliasSignature,
    ) -> ResolvedType {
        let Expr::Closure(params, body) = &expr.node else {
            return self.check_expr(expr);
        };
        if params.len() != signature.params.len() {
            self.errors.push(errors::builtin_arity(
                "closure",
                signature.params.len(),
                params.len(),
                expr.span,
            ));
            return ResolvedType::Unknown;
        }

        self.symbols.enter_scope(ScopeKind::Function);

        let prev_in_async_body = self.in_async_body;
        self.in_async_body = false;
        let prev_return_error_type = self.current_return_error_type.take();

        let param_types = params
            .iter()
            .zip(signature.params.iter())
            .map(|(param, expected)| {
                let ty = expected.resolved_ty.clone();
                self.validate_protected_builtin_binding(&param.node.name, param.span);
                self.symbols.define(Symbol {
                    name: param.node.name.clone(),
                    kind: SymbolKind::Variable(VariableInfo {
                        ty: ty.clone(),
                        is_mutable: false,
                        is_used: false,
                    }),
                    span: param.span,
                    scope: 0,
                });
                CallableParam::named(param.node.name.clone(), ty, param.node.kind)
            })
            .collect::<Vec<_>>();

        self.enter_mut_param_closure();
        let return_ty = self.check_expr_with_expected(body, Some(&signature.return_ty));
        self.exit_mut_param_closure();
        if !matches!(return_ty, ResolvedType::Unknown) && !self.types_compatible(&return_ty, &signature.return_ty) {
            self.errors.push(errors::type_mismatch(
                &signature.return_ty.to_string(),
                &return_ty.to_string(),
                body.span,
            ));
        }

        self.current_return_error_type = prev_return_error_type;
        self.in_async_body = prev_in_async_body;
        self.symbols.exit_scope();

        self.type_info.rust.closure_param_type_displays.insert(
            expr.span,
            signature
                .params
                .iter()
                .map(|param| param.rust_display.clone())
                .collect(),
        );

        let closure_ty = ResolvedType::Function(param_types, Box::new(signature.return_ty.clone()));
        self.record_expr_type(expr.span, closure_ty.clone());
        closure_ty
    }

    /// Check a method argument against a Rust callable alias.
    pub(super) fn check_method_arg_with_rust_callable_alias(
        &mut self,
        arg: &CallArg,
        signature: Option<&RustCallableAliasSignature>,
    ) -> ResolvedType {
        match arg {
            CallArg::Positional(expr)
            | CallArg::Named(_, expr)
            | CallArg::PositionalUnpack(expr)
            | CallArg::KeywordUnpack(expr) => {
                if let Some(signature) = signature
                    && matches!(expr.node, Expr::Closure(_, _))
                {
                    return self.check_closure_with_rust_callable_alias(expr, signature);
                }
                self.check_expr(expr)
            }
        }
    }
}
