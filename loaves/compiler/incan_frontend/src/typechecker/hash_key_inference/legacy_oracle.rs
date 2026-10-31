//! Original syntax-rescanning transfer, retained only as the cold influence oracle.

use super::{
    BTreeSet, CallArg, CollectionTypeId, DictEntry, Expr, HashKeyCallable, HashMap, ParamKind, Pattern, ResolvedType,
    ScanTarget, SymbolKind, TypeChecker, any_expr_in_body, collection_type_id,
};

impl TypeChecker {
    /// Return the type parameters of `target` its body hashes, given what is known of the other callables so far.
    pub(super) fn legacy_hashed_type_params(
        &self,
        target: &ScanTarget<'_>,
        targets: &[ScanTarget<'_>],
        requirements: &HashMap<HashKeyCallable, BTreeSet<String>>,
    ) -> Vec<String> {
        let mut env = target
            .params
            .iter()
            .map(|(name, _, ty)| (name.clone(), ty.clone()))
            .collect::<HashMap<_, _>>();
        self.bind_statement_locals(target.body, &mut env);
        let mut hashed = Vec::new();

        // ---- A dict or set type whose key or element mentions the type parameter ----
        for ty in env.values().chain(std::iter::once(&target.return_type)) {
            Self::collect_hashed_key_type_params(ty, &target.type_params, &mut hashed);
        }

        // ---- An index write into a dict ----
        let mut index_writes = Vec::new();
        Self::collect_index_writes(target.body, &mut index_writes);
        for write in index_writes {
            if Self::scan_type(&write.object, &env).is_some_and(|ty| Self::is_hashed_collection(&ty)) {
                Self::collect_direct_type_param(Self::scan_type(&write.index, &env), &target.type_params, &mut hashed);
            }
        }

        let owner = match &target.callable {
            HashKeyCallable::Method { owner, .. } => Some(owner.as_str()),
            HashKeyCallable::Function(_) => None,
        };
        any_expr_in_body(target.body, |expr| {
            match expr {
                Expr::Call(callee, _, args) => {
                    if let (Expr::Ident(name), [CallArg::Positional(source)]) = (&callee.node, args.as_slice())
                        && collection_type_id(name) == Some(CollectionTypeId::Set)
                        && let Some(item) = Self::scan_type(source, &env).as_ref().and_then(Self::scan_item_type)
                    {
                        Self::collect_named_type_params(&item, &target.type_params, &mut hashed);
                    }
                    if let Expr::Ident(name) = &callee.node {
                        self.legacy_passed_hashed_type_params(
                            &HashKeyCallable::Function(name.clone()),
                            args,
                            &env,
                            target,
                            targets,
                            requirements,
                            &mut hashed,
                        );
                    }
                }
                Expr::MethodCall(receiver, method, _, args) => {
                    // `s.add(x)`, `d.insert(x, v)`, `d.get(x)` and the other keyed methods of a set or dict hash
                    // their first argument.
                    if Self::scan_type(receiver, &env).is_some_and(|ty| Self::is_hashed_collection(&ty))
                        && let Some(CallArg::Positional(key)) = args.first()
                    {
                        Self::collect_direct_type_param(Self::scan_type(key, &env), &target.type_params, &mut hashed);
                    }
                    if let (Expr::SelfExpr, Some(owner)) = (&receiver.node, owner) {
                        let callable = HashKeyCallable::Method {
                            owner: owner.to_string(),
                            method: method.clone(),
                        };
                        self.legacy_passed_hashed_type_params(
                            &callable,
                            args,
                            &env,
                            target,
                            targets,
                            requirements,
                            &mut hashed,
                        );
                    }
                }
                Expr::Set(elements) => {
                    for element in elements {
                        Self::collect_direct_type_param(
                            Self::scan_type(element, &env),
                            &target.type_params,
                            &mut hashed,
                        );
                    }
                }
                Expr::Dict(entries) => {
                    for entry in entries {
                        if let DictEntry::Pair(key, _) = entry {
                            Self::collect_direct_type_param(
                                Self::scan_type(key, &env),
                                &target.type_params,
                                &mut hashed,
                            );
                        }
                    }
                }
                Expr::DictComp(comp) => {
                    let mut comp_env = env.clone();
                    if let (Pattern::Binding(name), Some(item)) = (
                        &comp.pattern.node,
                        Self::scan_type(&comp.iter, &env)
                            .as_ref()
                            .and_then(Self::scan_item_type),
                    ) {
                        comp_env.insert(name.clone(), item);
                    }
                    Self::collect_direct_type_param(
                        Self::scan_type(&comp.key, &comp_env),
                        &target.type_params,
                        &mut hashed,
                    );
                }
                _ => {}
            }
            false
        });
        hashed
    }

    /// Add the type parameters `target` passes to a callable at the positions that callable hashes.
    ///
    /// A callable of the same module is found by name among the scan targets; one declared elsewhere is found through
    /// its identity in the requirements recorded when its module was collected.
    #[allow(clippy::too_many_arguments)]
    fn legacy_passed_hashed_type_params(
        &self,
        callee: &HashKeyCallable,
        args: &[CallArg],
        env: &HashMap<String, ResolvedType>,
        target: &ScanTarget<'_>,
        targets: &[ScanTarget<'_>],
        requirements: &HashMap<HashKeyCallable, BTreeSet<String>>,
        hashed: &mut Vec<String>,
    ) {
        let (required, params) = if let Some(scanned) = targets.iter().find(|candidate| &candidate.callable == callee) {
            let Some(required) = requirements.get(callee) else {
                return;
            };
            let params = scanned
                .params
                .iter()
                .map(|(name, kind, ty)| (name.clone(), *kind, ty.clone()))
                .collect::<Vec<_>>();
            (required.clone(), params)
        } else {
            let HashKeyCallable::Function(name) = callee else {
                return;
            };
            let Some(symbol_id) = self.symbols.lookup(name) else {
                return;
            };
            let Some(identity) = self.symbols.identity_of(symbol_id) else {
                return;
            };
            let (Some(required), Some(SymbolKind::Function(info))) = (
                self.hash_key_inference.type_params.get(identity),
                self.symbols.get(symbol_id).map(|symbol| &symbol.kind),
            ) else {
                return;
            };
            let params = info
                .params
                .iter()
                .map(|param| (param.name.clone().unwrap_or_default(), param.kind, param.ty.clone()))
                .collect::<Vec<_>>();
            (required.clone(), params)
        };
        let mut positional = params.iter().filter(|(_, kind, _)| *kind == ParamKind::Normal);
        for arg in args {
            let (param, expr) = match arg {
                CallArg::Positional(expr) => (positional.next(), expr),
                CallArg::Named(name, expr) => (params.iter().find(|(param, _, _)| *param == name.node), expr),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => return,
            };
            let (Some((_, _, param_ty)), Some(arg_ty)) = (param, Self::scan_type(expr, env)) else {
                continue;
            };
            for required_param in &required {
                if let Some(bound) = Self::scan_binding(param_ty, &arg_ty, required_param) {
                    Self::collect_named_type_params(&bound, &target.type_params, hashed);
                }
            }
        }
    }
}

impl TypeChecker {
    /// Run the frozen pre-split declaration/check sequence for comparisons. This oracle is compiled only into the
    /// frontend test harness and is never an input or fallback for a record activation.
    pub(super) fn legacy_check_program(
        &mut self,
        program: &crate::ast::Program,
    ) -> Result<(), Vec<crate::diagnostics::CompileError>> {
        self.legacy_body_checking = true;
        let preserve_dependencies = std::mem::take(&mut self.dependency_semantics_pending);
        self.reset_program_check(program, preserve_dependencies);
        self.legacy_collect_program_declarations(program);
        self.prepare_program_body_facts();
        self.check_program_declaration_bodies(program);
        self.legacy_record_program_lowering_facts(program);
        let result = self.finish_program_check(program);
        self.clear_nominal_identity_cache();
        result
    }

    /// Collect fresh legacy headers without extracted influence scans or early publication.
    pub(super) fn legacy_collect_program_declarations(&mut self, program: &crate::ast::Program) {
        self.prepare_program_declaration_names(program);
        if !self.pending_trait_supertraits.is_empty() {
            self.resolve_pending_trait_supertraits();
        }
        self.collect_declarations_for_check(&program.declarations);
        self.resolve_forward_type_references(&program.declarations);
        self.record_local_nominal_type_param_bounds(&program.declarations);
        self.legacy_infer_hash_key_type_params(&program.declarations);
        self.resolve_pending_trait_supertraits();
        self.finalize_supertrait_graph();
        self.merge_supertrait_requires_into_traits();
        self.validate_static_dependencies();
        self.collect_testing_fixture_names(program);
        self.legacy_plan_closure_holding_callables(program);
    }

    /// Install the legacy requirements exactly where the original declaration pass did, without the new influence
    /// extraction. Imported requirement inputs, name grouping and publication order are unchanged.
    fn legacy_infer_hash_key_type_params(&mut self, declarations: &[crate::ast::Spanned<crate::ast::Declaration>]) {
        let targets = self.hash_key_scan_targets(declarations);
        let mut requirements = HashMap::<HashKeyCallable, BTreeSet<String>>::new();
        loop {
            let mut changed = false;
            for target in &targets {
                for parameter in self.legacy_hashed_type_params(target, &targets, &requirements) {
                    changed |= requirements
                        .entry(target.callable.clone())
                        .or_default()
                        .insert(parameter);
                }
            }
            if !changed {
                break;
            }
        }
        let mut ordered = requirements.into_iter().collect::<Vec<_>>();
        ordered.sort();
        for (callable, type_params) in ordered {
            if let Some(identity) = self.hash_key_callable_identity(&callable) {
                self.hash_key_inference
                    .type_params
                    .entry(identity)
                    .or_default()
                    .extend(type_params.iter().cloned());
            }
            self.hash_key_inference.local_requirements.push((callable, type_params));
        }
    }
}

impl TypeChecker {
    /// Record checked layout, trait and import facts for later stages without publishing a nominal snapshot yet.
    /// Native module validation precedes exporting inferred bounds and declaration identities.
    fn legacy_record_program_lowering_facts(&mut self, program: &crate::ast::Program) {
        self.type_info
            .c_abi
            .resolve_checked_facades(self.current_module_path.as_deref());
        self.record_trait_metadata_for_lowering();
        self.record_standard_registry_types();
        self.record_trait_adoption_arguments_for_lowering(program);
        self.record_body_ir_bare_adoption_arguments(program);
        self.record_model_field_visibilities_for_lowering(program);
        self.record_class_layouts_for_lowering(program);
        self.record_body_ir_class_inheritance(program);
        self.record_method_decorator_receiver_slots(program);

        // ---- RFC 023: validate rust.module() and @rust.extern rules ----
        self.validate_rust_module_and_extern(program);
        self.validate_source_type_names = false;
        // The module is checked; its inferred hashed type parameters now join its exported signatures (#1758).
        self.export_inferred_hash_key_bounds();

        // ---- RFC 120: export the minted declaration identities for later stages ----
        self.export_module_declaration_bindings();
        self.export_nested_declaration_identities();
        self.export_checked_import_bindings();
        self.export_type_alias_targets();
        self.record_binding_collision_diagnostics();
    }
}
