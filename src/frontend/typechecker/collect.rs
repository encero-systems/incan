//! First-pass collection: register types, functions, and imports into the symbol table.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::freeze_const_type;

use super::TypeChecker;

mod decl_helpers;
pub(super) mod decorators;
mod stdlib_imports;

use self::decl_helpers::{collect_fields, collect_methods, inject_json_methods, inject_validate_methods};

impl TypeChecker {
    // ========================================================================
    // First pass: collect declarations
    // ========================================================================

    /// Register a declaration in the symbol table (first pass).
    ///
    /// Dispatches to `collect_import`, `collect_model`, etc. to populate the [`SymbolTable`] with type, function,
    /// and trait definitions. Bodies are **not** validated here; that happens in
    /// [`check_declaration`](Self::check_declaration) in the second pass.
    pub(crate) fn collect_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(import) => self.collect_import(import, decl.span),
            Declaration::Const(konst) => {
                self.validate_root_namespace(&konst.name, decl.span);
                self.collect_const(konst, decl.span);
            }
            Declaration::Static(static_decl) => {
                self.validate_root_namespace(&static_decl.name, decl.span);
                self.collect_static(static_decl, decl.span);
            }
            Declaration::Model(model) => {
                self.validate_root_namespace(&model.name, decl.span);
                self.collect_model(model, decl.span);
            }
            Declaration::Class(class) => {
                self.validate_root_namespace(&class.name, decl.span);
                self.collect_class(class, decl.span);
            }
            Declaration::Trait(tr) => {
                self.validate_root_namespace(&tr.name, decl.span);
                self.collect_trait(tr, decl.span);
            }
            Declaration::TypeAlias(a) => {
                self.validate_root_namespace(&a.name, decl.span);
                // Register the alias name as a known type so other declarations can reference it.
                self.symbols.define(Symbol {
                    name: a.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::TypeAlias),
                    span: decl.span,
                    scope: 0,
                });
            }
            Declaration::Newtype(nt) => {
                self.validate_root_namespace(&nt.name, decl.span);
                self.collect_newtype(nt, decl.span);
            }
            Declaration::Enum(en) => {
                self.validate_root_namespace(&en.name, decl.span);
                self.collect_enum(en, decl.span);
            }
            Declaration::Function(func) => {
                self.validate_root_namespace(&func.name, decl.span);
                self.collect_function(func, decl.span);
            }
            Declaration::Docstring(_) => {} // Docstrings don't need collection
        }
    }

    /// Register a module-level const binding (first pass).
    ///
    /// Note: the initializer is validated in the second pass.
    fn collect_const(&mut self, konst: &ConstDecl, span: Span) {
        // Remember for const-eval (cycle detection / evaluation).
        self.const_decls.insert(konst.name.clone(), (konst.clone(), span));

        // Best-effort type from annotation; refined during const-eval in second pass.
        let ty = konst
            .ty
            .as_ref()
            .map(|t| {
                // `const` implies deep immutability; map common container annotations to frozen equivalents.
                let resolved = self.resolve_type_checked(t);
                freeze_const_type(resolved)
            })
            .unwrap_or(ResolvedType::Unknown);

        // Define as an immutable variable-like symbol for name resolution.
        self.symbols.define(Symbol {
            name: konst.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
    }

    /// Register a module-level static binding (first pass).
    fn collect_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let decl_index = self.static_decls.len();
        self.static_decl_positions.insert(static_decl.name.clone(), decl_index);
        self.static_decls.push((static_decl.clone(), span));

        let ty = self.resolve_type_checked(&static_decl.ty);
        self.type_info.static_bindings.insert(
            static_decl.name.clone(),
            crate::frontend::typechecker::StaticBindingInfo { is_imported: false },
        );

        self.symbols.define(Symbol {
            name: static_decl.name.clone(),
            kind: SymbolKind::Static(StaticInfo {
                ty,
                is_public: matches!(static_decl.visibility, Visibility::Public),
                is_imported: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
    }

    /// Register a model declaration with its fields, methods, and derived traits.
    fn collect_model(&mut self, model: &ModelDecl, span: Span) {
        let fields = collect_fields(&model.fields, self);
        let mut methods = collect_methods(&model.methods, self, Some(&model.name), &model.type_params);

        // Inject JSON methods based on derives
        let derives = self.extract_derive_names(&model.decorators);
        inject_json_methods(&mut methods, &model.name, &derives);
        let field_order: Vec<Ident> = model.fields.iter().map(|f| f.node.name.clone()).collect();
        inject_validate_methods(&mut methods, &model.name, &fields, &field_order, &derives);

        self.symbols.define(Symbol {
            name: model.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                traits: model.traits.iter().map(|t| t.node.name.clone()).collect(),
                derives,
                fields,
                methods,
            })),
            span,
            scope: 0,
        });
    }

    /// Register a class declaration, inheriting from parent if present.
    fn collect_class(&mut self, class: &ClassDecl, span: Span) {
        let (mut fields, mut methods) = self.inherit_from_parent(&class.extends);

        // Add own fields (can override inherited ones)
        fields.extend(collect_fields(&class.fields, self));

        // Add own methods (can override inherited ones)
        methods.extend(collect_methods(
            &class.methods,
            self,
            Some(&class.name),
            &class.type_params,
        ));

        // Inject JSON methods based on derives
        let derives = self.extract_derive_names(&class.decorators);
        inject_json_methods(&mut methods, &class.name, &derives);

        self.symbols.define(Symbol {
            name: class.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                extends: class.extends.clone(),
                traits: class.traits.iter().map(|t| t.node.name.clone()).collect(),
                derives,
                fields,
                methods,
            })),
            span,
            scope: 0,
        });
    }

    /// Inherit fields and methods from a parent class if present.
    fn inherit_from_parent(
        &self,
        extends: &Option<String>,
    ) -> (HashMap<String, FieldInfo>, HashMap<String, MethodInfo>) {
        let Some(parent_name) = extends else {
            return (HashMap::new(), HashMap::new());
        };
        let Some(TypeInfo::Class(parent_info)) = self.lookup_type_info(parent_name) else {
            return (HashMap::new(), HashMap::new());
        };
        (parent_info.fields.clone(), parent_info.methods.clone())
    }

    /// Register a trait declaration with its method signatures, supertraits, and requirements.
    fn collect_trait(&mut self, tr: &TraitDecl, span: Span) {
        let methods = collect_methods(&tr.methods, self, None, &tr.type_params);
        let requires = self.extract_requires(&tr.decorators);
        if !tr.traits.is_empty() {
            self.pending_trait_supertraits
                .push((tr.name.clone(), tr.traits.clone()));
        }

        self.symbols.define(Symbol {
            name: tr.name.clone(),
            kind: SymbolKind::Trait(TraitInfo {
                type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                supertraits: Vec::new(),
                methods,
                requires,
            }),
            span,
            scope: 0,
        });
    }

    /// Resolve one `with` supertrait bound to `(trait_name, type_arguments)` after validation (RFC 042).
    fn resolve_trait_supertrait_bound(&mut self, bound: &Spanned<TraitBound>) -> Option<(String, Vec<ResolvedType>)> {
        let ty = trait_bound_to_ast_type(bound);
        let spanned = Spanned::new(ty, bound.span);
        let resolved = self.resolve_type_checked(&spanned);
        let (trait_name, args) = match resolved {
            ResolvedType::Named(n) => (n, Vec::new()),
            ResolvedType::Generic(n, args) => (n, args),
            _ => {
                self.errors.push(errors::supertrait_bound_invalid(bound.span));
                return None;
            }
        };
        let Some(trait_info) = self.lookup_trait_info(&trait_name) else {
            self.errors
                .push(errors::supertrait_bound_not_trait(&trait_name, bound.span));
            return None;
        };
        let expected_arity = trait_info.type_params.len();
        if args.len() != expected_arity {
            self.errors.push(errors::supertrait_bound_arity_mismatch(
                &trait_name,
                expected_arity,
                args.len(),
                bound.span,
            ));
            return None;
        }
        Some((trait_name, args))
    }

    /// Resolve queued trait `with` bounds now that all types and traits exist in the symbol table (RFC 042).
    pub(crate) fn resolve_pending_trait_supertraits(&mut self) {
        let pending = std::mem::take(&mut self.pending_trait_supertraits);
        for (trait_name, bounds) in pending {
            let mut supertraits: Vec<(String, Vec<ResolvedType>)> = Vec::new();
            for bound in &bounds {
                if let Some(entry) = self.resolve_trait_supertrait_bound(bound) {
                    supertraits.push(entry);
                }
            }
            let Some(sym_id) = self.symbols.lookup(&trait_name) else {
                continue;
            };
            let Some(sym) = self.symbols.get_mut(sym_id) else {
                continue;
            };
            let SymbolKind::Trait(info) = &mut sym.kind else {
                continue;
            };
            info.supertraits = supertraits;
        }
    }

    /// After all declarations are collected: detect supertrait cycles and fill `supertrait_closure`.
    pub(crate) fn finalize_supertrait_graph(&mut self) {
        self.supertrait_closure.clear();
        let edges = self.supertrait_name_adjacency();
        let trait_names: Vec<String> = self
            .symbols
            .all_symbols()
            .iter()
            .filter_map(|sym| matches!(sym.kind, SymbolKind::Trait(_)).then_some(sym.name.clone()))
            .collect();
        if let Some(cycle) = find_supertrait_cycle_path(&edges) {
            let span = cycle
                .first()
                .and_then(|name| self.lookup_symbol(name))
                .map(|sym| sym.span)
                .unwrap_or_default();
            self.errors.push(errors::supertrait_cycle(&cycle, span));
            for name in trait_names {
                self.supertrait_closure.insert(name, Vec::new());
            }
            return;
        }
        for name in trait_names {
            let closure = self.expand_supertraits_transitively(&name);
            self.supertrait_closure.insert(name, closure);
        }
    }

    fn supertrait_name_adjacency(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                let names: Vec<String> = info.supertraits.iter().map(|(n, _)| n.clone()).collect();
                map.insert(sym.name.clone(), names);
            }
        }
        map
    }

    /// Transitive supertraits of `trait_name`, with type arguments substituted along each edge.
    fn expand_supertraits_transitively(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let mut seen = HashSet::new();
        let mut work: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let Some(root) = self.lookup_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());
        while let Some((sup_name, sup_args)) = work.pop() {
            let key = format!(
                "{sup_name}<{}>",
                sup_args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((sup_name.clone(), sup_args.clone()));
            let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                continue;
            };
            let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
            for (ss_name, ss_args) in &sup_info.supertraits {
                let mapped: Vec<ResolvedType> = ss_args.iter().map(|t| substitute_resolved_type(t, &subst)).collect();
                work.push((ss_name.clone(), mapped));
            }
        }
        result
    }

    /// Merge `@requires` fields from transitive supertraits into each trait symbol (RFC 042).
    ///
    /// Uses each trait's **explicit** `@requires` from a snapshot taken before merging, so order does not matter.
    /// Incompatible requirements for the same field name emit [`errors::supertrait_requires_conflict`].
    pub(crate) fn merge_supertrait_requires_into_traits(&mut self) {
        let mut explicit: HashMap<String, Vec<(String, ResolvedType)>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                explicit.insert(sym.name.clone(), info.requires.clone());
            }
        }

        let trait_names: Vec<String> = explicit.keys().cloned().collect();
        let mut updates: Vec<(String, Vec<(String, ResolvedType)>)> = Vec::new();

        for tname in trait_names {
            let span = self
                .symbols
                .lookup(&tname)
                .and_then(|id| self.symbols.get(id))
                .map(|s| s.span)
                .unwrap_or_default();

            let mut merged: HashMap<String, ResolvedType> =
                explicit.get(&tname).cloned().unwrap_or_default().into_iter().collect();

            let closure = self.supertrait_closure.get(tname.as_str()).cloned().unwrap_or_default();

            for (sup_name, sup_args) in closure {
                let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                    continue;
                };
                let sup_req = explicit.get(&sup_name).cloned().unwrap_or_default();
                let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
                for (field, ty) in sup_req {
                    let inst = substitute_resolved_type(&ty, &subst);
                    match merged.get(&field) {
                        None => {
                            merged.insert(field, inst);
                        }
                        Some(existing) => {
                            if existing == &inst {
                                continue;
                            }
                            if self.types_compatible(existing, &inst) || self.types_compatible(&inst, existing) {
                                continue;
                            }
                            self.errors.push(errors::supertrait_requires_conflict(
                                &tname,
                                &field,
                                &existing.to_string(),
                                &inst.to_string(),
                                span,
                            ));
                        }
                    }
                }
            }

            let mut req_vec: Vec<(String, ResolvedType)> = merged.into_iter().collect();
            req_vec.sort_by(|a, b| a.0.cmp(&b.0));
            updates.push((tname, req_vec));
        }

        for (tname, requires) in updates {
            if let Some(id) = self.symbols.lookup(&tname)
                && let Some(sym) = self.symbols.get_mut(id)
                && let SymbolKind::Trait(info) = &mut sym.kind
            {
                info.requires = requires;
            }
        }
    }

    /// Register a newtype declaration with its underlying type and methods.
    fn collect_newtype(&mut self, nt: &NewtypeDecl, span: Span) {
        let underlying = self.resolve_type_checked(&nt.underlying);
        let method_rebindings = nt
            .rebindings
            .iter()
            .filter_map(|rebinding| {
                Self::rebinding_target_method_name(&rebinding.node.target.node)
                    .map(|target| (rebinding.node.name.clone(), target))
            })
            .collect();

        // Define a placeholder symbol FIRST so methods can reference the newtype name
        self.symbols.define(Symbol {
            name: nt.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: nt.type_params.iter().map(|tp| tp.name.clone()).collect(),
                is_rusttype: nt.is_rusttype,
                has_interop: !nt.interop_edges.is_empty(),
                underlying: underlying.clone(),
                method_rebindings,
                methods: HashMap::new(), // Empty for now
            })),
            span,
            scope: 0,
        });

        // Now collect methods - they can reference the newtype name
        let methods = collect_methods(&nt.methods, self, Some(&nt.name), &nt.type_params);

        // Update the symbol with the collected methods
        if let Some(sym_id) = self.symbols.lookup(&nt.name)
            && let Some(sym) = self.symbols.get_mut(sym_id)
            && let SymbolKind::Type(TypeInfo::Newtype(info)) = &mut sym.kind
        {
            info.methods = methods;
        }
    }

    /// Extract the effective target method name for a `alias = target` rebinding declaration.
    ///
    /// We accept both:
    /// - `alias = method_name`
    /// - `alias = TypeOrValue.method_name` (last segment is the target method)
    fn rebinding_target_method_name(target: &Expr) -> Option<String> {
        match target {
            Expr::Ident(name) => Some(name.clone()),
            Expr::Field(_, member) => Some(member.clone()),
            _ => None,
        }
    }

    /// Register an enum declaration and define symbols for each variant.
    fn collect_enum(&mut self, en: &EnumDecl, span: Span) {
        let variants: Vec<_> = en.variants.iter().map(|v| v.node.name.clone()).collect();
        let derives = self.extract_derive_names(&en.decorators);

        self.symbols.define(Symbol {
            name: en.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                variants: variants.clone(),
                derives,
            })),
            span,
            scope: 0,
        });

        // Also define each variant as a symbol
        for variant in &en.variants {
            let fields: Vec<_> = variant
                .node
                .fields
                .iter()
                .map(|f| self.resolve_type_checked(f))
                .collect();
            self.symbols.define(Symbol {
                name: variant.node.name.clone(),
                kind: SymbolKind::Variant(VariantInfo {
                    enum_name: en.name.clone(),
                    fields,
                }),
                span: variant.span,
                scope: 0,
            });
        }
    }

    /// Register a top-level function declaration.
    fn collect_function(&mut self, func: &FunctionDecl, span: Span) {
        // Local declaration shadows any imported marker binding with the same name.
        self.testing_marker_import_bindings.remove(&func.name);
        self.local_function_decls.insert(func.name.clone(), func.clone());
        let type_params: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();
        let type_param_bounds: HashMap<String, Vec<String>> = func
            .type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds.iter().map(|bound| bound.name.clone()).collect(),
                )
            })
            .collect();

        let params: Vec<_> = func
            .params
            .iter()
            .map(|p| (p.node.name.clone(), self.resolve_type_checked(&p.node.ty)))
            .collect();
        let return_type = self.resolve_type_checked(&func.return_type);

        self.symbols.define(Symbol {
            name: func.name.clone(),
            kind: SymbolKind::Function(FunctionInfo {
                params,
                return_type,
                is_async: func.is_async(),
                type_params,
                type_param_bounds,
            }),
            span,
            scope: 0,
        });
    }
}

fn trait_bound_to_ast_type(bound: &Spanned<TraitBound>) -> Type {
    if bound.node.type_args.is_empty() {
        Type::Simple(bound.node.name.clone())
    } else {
        Type::Generic(bound.node.name.clone(), bound.node.type_args.clone())
    }
}

/// Returns one simple cycle (trait names), if the directed supertrait graph has a cycle.
fn find_supertrait_cycle_path(edges: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut nodes: HashSet<String> = HashSet::new();
    for (k, vs) in edges {
        nodes.insert(k.clone());
        for v in vs {
            nodes.insert(v.clone());
        }
    }
    let mut color: HashMap<String, u8> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for start in nodes {
        if color.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        stack.clear();
        if let Some(cycle) = dfs_supertrait_cycle(&start, edges, &mut color, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn dfs_supertrait_cycle(
    n: &str,
    edges: &HashMap<String, Vec<String>>,
    color: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    match color.get(n).copied().unwrap_or(0) {
        1 => {
            let idx = stack.iter().position(|x| x == n)?;
            Some(stack[idx..].to_vec())
        }
        2 => None,
        _ => {
            color.insert(n.to_string(), 1);
            stack.push(n.to_string());
            for succ in edges.get(n).into_iter().flatten() {
                if let Some(c) = dfs_supertrait_cycle(succ, edges, color, stack) {
                    return Some(c);
                }
            }
            stack.pop();
            color.insert(n.to_string(), 2);
            None
        }
    }
}
