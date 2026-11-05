//! Collection of functions and their type parameter bounds.

use super::*;

impl TypeChecker {
    /// Register a top-level function declaration.
    pub(super) fn collect_function(&mut self, func: &FunctionDecl, span: Span) {
        let existing_module_function_id = self.current_module_function_symbols.get(&func.name).copied();
        let type_params: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();
        let (type_param_bounds, type_param_bound_details) = self.collect_function_type_param_bounds(&func.type_params);

        let params: Vec<_> = func
            .params
            .iter()
            .map(|p| {
                let surface_type_name = self.alias_surface_type_name(&p.node.ty.node);
                let ty = self.resolve_type_checked(&p.node.ty);
                let is_mut = self.def_param_shows_changes_to_caller(&p.node, &ty);
                CallableParam::named_with_default(p.node.name.clone(), ty, p.node.kind, p.node.default.is_some())
                    .with_mut(is_mut)
                    .with_surface_type_name(surface_type_name)
            })
            .collect();
        let return_type = self.resolve_type_checked(&func.return_type);
        let binding = FunctionBindingInfo {
            is_async: func.is_async(),
            params: params.clone(),
            return_type: return_type.clone(),
            identity: Some(self.symbols.module_declaration_identity(
                &func.name,
                SemanticSourceTargetKind::Function,
                span,
            )),
        };
        self.record_caller_visible_mut_params(binding.identity.as_ref(), &func.params, &params);
        let mut info = FunctionInfo {
            params,
            return_type,
            is_async: func.is_async(),
            type_params,
            type_param_bounds,
            type_param_bound_details,
            emitted_name: None,
        };

        let duplicates_type_token_signature = existing_module_function_id
            .and_then(|existing_id| self.symbols.get(existing_id))
            .is_some_and(|existing_symbol| match &existing_symbol.kind {
                SymbolKind::Function(existing) => type_token_overload_signatures_identical(existing, &info),
                SymbolKind::FunctionOverloads(overloads) => overloads
                    .iter()
                    .any(|overload| type_token_overload_signatures_identical(&overload.info, &info)),
                _ => false,
            });
        if duplicates_type_token_signature {
            self.errors
                .push(errors::duplicate_type_token_overload(&func.name, span));
        }

        let existing_identity = existing_module_function_id
            .and_then(|existing_id| self.symbols.identity_of(existing_id))
            .cloned();
        if let Some(existing_id) = existing_module_function_id
            && let Some(existing_symbol) = self.symbols.get_mut(existing_id)
        {
            self.local_function_decls.insert(func.name.clone(), func.clone());
            self.type_info
                .declarations
                .function_bindings
                .insert(func.name.clone(), binding.clone());
            self.type_info
                .declarations
                .function_bindings_by_span
                .insert(span, binding.clone());
            info.emitted_name = Some(overloaded_function_emitted_name(&func.name, &info));
            match &mut existing_symbol.kind {
                SymbolKind::Function(existing_info) => {
                    existing_info.emitted_name = Some(overloaded_function_emitted_name(&func.name, existing_info));
                    let overloads = vec![
                        FunctionOverloadInfo {
                            info: existing_info.clone(),
                            span: existing_symbol.span,
                            identity: existing_identity,
                        },
                        FunctionOverloadInfo {
                            info,
                            span,
                            identity: binding.identity.clone(),
                        },
                    ];
                    self.type_info
                        .record_function_overloads(func.name.clone(), overloads.clone());
                    existing_symbol.kind = SymbolKind::FunctionOverloads(overloads);
                    // Each overload keeps its own span-keyed identity; the combined binding names no single
                    // declaration.
                    self.symbols.clear_identity(existing_id);
                    return;
                }
                SymbolKind::FunctionOverloads(overloads) => {
                    overloads.push(FunctionOverloadInfo {
                        info,
                        span,
                        identity: binding.identity.clone(),
                    });
                    self.type_info
                        .record_function_overloads(func.name.clone(), overloads.clone());
                    return;
                }
                _ => {}
            }
        }

        let symbol_id = self.symbols.define(Symbol {
            name: func.name.clone(),
            kind: SymbolKind::Function(info),
            span,
            scope: 0,
        });
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        // Local declarations replace marker semantics only when registration accepted them.
        self.testing_marker_import_bindings.remove(&func.name);
        self.local_function_decls.insert(func.name.clone(), func.clone());
        self.type_info
            .declarations
            .function_bindings
            .insert(func.name.clone(), binding.clone());
        self.type_info
            .declarations
            .function_bindings_by_span
            .insert(span, binding);
        self.current_module_function_symbols
            .insert(func.name.clone(), symbol_id);
    }

    /// Collect the name and structured views of a function's declared bounds in their existing two-pass order.
    /// The structured view retains source/module provenance and checked type arguments for later call validation.
    pub(super) fn collect_function_type_param_bounds(
        &mut self,
        params: &[TypeParam],
    ) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<TypeBoundInfo>>) {
        let type_param_bounds = params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| self.resolve_generic_bound_name(&bound.name, Span::default()))
                        .collect(),
                )
            })
            .collect();
        let type_param_bound_details = params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: self.resolve_generic_bound_name(&bound.name, Span::default()),
                            source_name: self.trait_bound_source_name(&bound.name),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(|type_arg| self.resolve_type_checked(type_arg))
                                .collect(),
                            module_path: self.trait_bound_module_path(&bound.name),
                            implementation_type_params: Vec::new(),
                            inferred: false,
                        })
                        .collect(),
                )
            })
            .collect();

        (type_param_bounds, type_param_bound_details)
    }
}

/// Return whether two overloads expose the same alias-normalized type-token dispatch surface.
///
/// Return types do not participate in overload selection. Parameter names and defaults describe call syntax rather
/// than dispatch identity, while parameter kinds and `mut` markers do change how arguments bind. Type annotations
/// have already passed through transparent-alias expansion before this comparison.
fn type_token_overload_signatures_identical(left: &FunctionInfo, right: &FunctionInfo) -> bool {
    left.params
        .iter()
        .any(|param| matches!(&param.ty, ResolvedType::TypeToken(_)))
        && left.params.len() == right.params.len()
        && left
            .params
            .iter()
            .zip(&right.params)
            .all(|(left, right)| left.ty == right.ty && left.kind == right.kind && left.is_mut == right.is_mut)
        && left.type_params == right.type_params
        && left.type_param_bounds == right.type_param_bounds
}
