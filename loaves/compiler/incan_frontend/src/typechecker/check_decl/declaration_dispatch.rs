//! Declaration dispatch, capabilities, type aliases, partials, method partials, and test modules.

use super::*;

impl TypeChecker {
    /// Dispatch one declaration while routing a top-level function through its direct local-record producer. The
    /// ordinary declaration dispatcher remains the authority for every other declaration kind.
    pub(in crate::typechecker) fn check_record_declaration(
        &mut self,
        program: &Program,
        declaration: &Spanned<Declaration>,
    ) {
        let Declaration::Function(function) = &declaration.node else {
            self.check_declaration(declaration);
            return;
        };
        self.record_check_work("declaration", "top_level", declaration.span);
        self.validate_protected_type_param_bindings(&function.type_params, declaration.span);
        self.check_record_function(program, function, declaration.span);
    }

    // ========================================================================
    // Second pass: check declarations
    // ========================================================================

    /// Validate a declaration's body and semantics (second pass).
    ///
    /// Dispatches to `check_model`, `check_class`, etc. Expects symbols to already be registered via
    /// [`collect_declaration`](Self::collect_declaration).
    pub fn check_declaration(&mut self, decl: &Spanned<Declaration>) {
        if !matches!(
            decl.node,
            Declaration::Import(_) | Declaration::Alias(_) | Declaration::Docstring(_)
        ) {
            self.record_check_work("declaration", "top_level", decl.span);
        }
        match &decl.node {
            Declaration::Import(_) => {} // Already handled
            Declaration::Const(konst) => {
                self.check_declaration_owner(decl.span, |checker| checker.check_const(konst, decl.span))
            }
            Declaration::Static(static_decl) => {
                self.check_declaration_owner(decl.span, |checker| checker.check_static(static_decl, decl.span))
            }
            Declaration::Model(model) => self.check_model(model, decl.span),
            Declaration::Class(class) => self.check_class(class, decl.span),
            Declaration::Trait(tr) => {
                self.validate_protected_type_param_bindings(&tr.type_params, decl.span);
                self.check_trait(tr);
            }
            Declaration::Alias(_) => {} // Alias targets are validated during collection.
            Declaration::Partial(partial) => self.check_partial_decl(partial),
            Declaration::TypeAlias(alias) => {
                self.validate_protected_type_param_bindings(&alias.type_params, decl.span);
                self.check_type_alias(alias);
            }
            Declaration::Newtype(nt) => {
                self.validate_protected_type_param_bindings(&nt.type_params, decl.span);
                self.check_newtype(nt);
                self.record_declared_display_route(&nt.name, decl.span);
            }
            Declaration::Enum(en) => {
                self.validate_protected_type_param_bindings(&en.type_params, decl.span);
                self.check_enum(en);
                self.record_declared_display_route(&en.name, decl.span);
            }
            Declaration::Function(func) => {
                self.validate_protected_type_param_bindings(&func.type_params, decl.span);
                self.check_function(func, decl.span);
            }
            Declaration::TestModule(test_module) => self.check_test_module(test_module),
            Declaration::VocabBlock(block) if block.not_checked => {}
            Declaration::VocabBlock(_) => self.errors.push(CompileError::syntax(
                "raw vocabulary declarations must be desugared before type checking".to_string(),
                decl.span,
            )),
            Declaration::Capability(cap) => self.check_capability_decl(cap, decl.span),
            Declaration::Docstring(_) => {} // Docstrings don't need checking
        }
    }

    /// Observe an actual checking boundary. Module and source span distinguish physical inputs only within this
    /// invocation; they are diagnostics, never stable identities or cache keys.
    pub(in crate::typechecker) fn record_check_work(&self, action: &'static str, kind: &'static str, span: Span) {
        tracing::trace!(target: "incan::invocation", phase = "checking", action, kind,
            module = ?self.current_module_path, input = ?span, "compiler work");
    }

    /// Record how a declared model, class, enum or newtype displays when lowering has to give it a Rust `Display`: the
    /// `message()` call it displays through when it adopts `Error` and has no `Display` of its own.
    pub(super) fn record_declared_display_route(&mut self, type_name: &str, decl_span: Span) {
        let self_ty = self.trait_conformance_self_type(type_name);
        self.record_error_message_display_type(type_name, &self_ty, decl_span);
    }

    /// Reject protected builtin spellings in a declaration-owned generic parameter list.
    ///
    /// Generic parameters are inserted into the ordinary lexical symbol table, so they can displace a bare callable
    /// root even though their source role is type-only. [`TypeParam`] retains no individual source span; callers pass
    /// the nearest enclosing declaration or method span rather than fabricating a byte range.
    pub(super) fn validate_protected_type_param_bindings(&mut self, type_params: &[TypeParam], owner_span: Span) {
        for type_param in type_params {
            self.validate_protected_builtin_binding(&type_param.name, owner_span);
        }
    }

    /// Check one RFC 104 `capability` declaration.
    ///
    /// Collection records what the declaration wrote; this decides whether what it wrote means anything. The two are
    /// separate passes because capabilities may reference each other in any order within a module, so resolution
    /// cannot happen while the symbols are still being collected.
    ///
    /// Every `requires` entry is validated from the AST rather than from the collected symbol, because collection
    /// keeps only the entries that parsed as a reference. Re-walking the source list is what lets an entry that is
    /// not a reference at all be reported instead of silently disappearing.
    pub(super) fn check_capability_decl(&mut self, cap: &CapabilityDecl, span: Span) {
        self.validate_decorators_rejecting_user_defined(&cap.decorators, "capability");
        self.reject_registry_description_decorators(&cap.decorators, "capability");

        match cap.description.as_ref() {
            None => self
                .errors
                .push(errors::capability_description_required(&cap.name, span)),
            // Presence is not enough. Collection stores only compile-time text, so a clause it cannot read leaves
            // the declaration carrying no description while appearing to have one -- the same silent drop the
            // `requires` list had.
            Some(expr) if capability_description_text(&expr.node).is_none() => self
                .errors
                .push(errors::capability_description_must_be_text(&cap.name, expr.span)),
            Some(_) => {}
        }

        let mut requires = Vec::new();
        for entry in &cap.requires {
            let Some(path) = dotted_path_segments(&entry.node) else {
                self.errors
                    .push(errors::capability_requirement_not_a_reference(entry.span));
                continue;
            };
            if let Some(identity) = self.check_capability_requirement(&path, entry.span) {
                requires.push(identity);
            }
        }

        self.record_checked_capability_declaration(cap, requires);
    }

    /// Retain one validated capability declaration as a checked fact.
    ///
    /// The declaration has just been validated, so its resolved requirement identities are known here and nowhere
    /// else; discarding them would make every later consumer re-resolve a dotted reference to learn what the
    /// authority contract says. Reading the description and scope back from the collected symbol rather than the AST
    /// keeps one interpretation of the clauses instead of a second parse (RFC 104, #1027).
    pub(super) fn record_checked_capability_declaration(
        &mut self,
        cap: &CapabilityDecl,
        requires: Vec<CanonicalSymbolId>,
    ) {
        let Some(symbol_id) = self.symbols.lookup(&cap.name) else {
            return;
        };
        let Some(identity) = self.symbols.identity_of(symbol_id).cloned() else {
            return;
        };
        let Some(symbol) = self.symbols.get(symbol_id) else {
            return;
        };
        let SymbolKind::Capability(info) = &symbol.kind else {
            return;
        };
        let declaration = CapabilityDeclarationInfo {
            capability: identity.clone(),
            description: info.description.clone(),
            scope: info
                .scope
                .iter()
                .map(|(name, ty)| (name.clone(), ty.to_string()))
                .collect(),
            requires,
            is_public: info.is_public,
        };
        self.type_info.declarations.capabilities.insert(identity, declaration);
    }

    /// Resolve one `requires` entry and confirm it names a capability.
    ///
    /// A bare name resolves against the current module's own symbols; a dotted path resolves as a member of the
    /// module its leading segments name, the same way an import resolves against a definition. RFC 104 asks for
    /// exactly that equivalence, so that a misspelled capability is an unresolved-symbol error at the reference
    /// rather than a runtime denial reported somewhere else entirely.
    pub(super) fn check_capability_requirement(&mut self, path: &[String], span: Span) -> Option<CanonicalSymbolId> {
        let rendered = path.join(".");
        let (item_name, module_segments) = path.split_last()?;

        if module_segments.is_empty() {
            let symbol_id = self.symbols.lookup(item_name);
            match symbol_id.and_then(|id| self.symbols.get(id)).map(|symbol| &symbol.kind) {
                Some(SymbolKind::Capability(_)) => {
                    return symbol_id.and_then(|id| self.symbols.identity_of(id)).cloned();
                }
                Some(other) => {
                    let found = Self::symbol_kind_noun(other);
                    self.errors
                        .push(errors::capability_requirement_not_a_capability(&rendered, found, span));
                }
                None => self
                    .errors
                    .push(errors::capability_requirement_unresolved(&rendered, span)),
            }
            return None;
        }

        let module = ImportPath::simple(module_segments.to_vec());
        let resolved = self
            .dependency_member_identity(&module, item_name)
            .or_else(|| self.imported_module_capability_identity(module_segments, item_name));
        match resolved {
            Some(identity) if identity.kind == SemanticSourceTargetKind::Capability => Some(identity),
            Some(identity) => {
                let found = identity.kind.as_str();
                self.errors
                    .push(errors::capability_requirement_not_a_capability(&rendered, found, span));
                None
            }
            None => {
                self.errors
                    .push(errors::capability_requirement_unresolved(&rendered, span));
                None
            }
        }
    }

    /// Resolve a dotted capability reference that travels through a module imported into this file.
    ///
    /// `from std.runtime import host` binds `host` to the module `std.runtime.host`, so `host.fs.read` names the
    /// `read` capability of `std.runtime.host.fs`. Dependency resolution alone cannot answer that: it resolves
    /// package dependencies, while the reserved `host.*` namespace is declared by the compiler's own bundled stdlib
    /// source. RFC 104 asks for exactly one declaration mechanism, so the reference has to resolve through the
    /// import that brought the module into scope, the same way any other member reference does.
    pub(super) fn imported_module_capability_identity(
        &mut self,
        module_segments: &[String],
        item_name: &str,
    ) -> Option<CanonicalSymbolId> {
        let (root, nested) = module_segments.split_first()?;
        let mut module_path = self.module_namespace_path(root)?;
        module_path.extend(nested.iter().cloned());
        self.loaf_member_identity(&module_path, item_name)
    }

    /// Name a symbol kind for a diagnostic that reports the wrong kind was referenced.
    pub(super) fn symbol_kind_noun(kind: &SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Variable(_) => "variable",
            SymbolKind::Static(_) => "static",
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) => "function",
            SymbolKind::Type(_) => "type",
            SymbolKind::Trait(_) => "trait",
            SymbolKind::Module(_) => "module",
            SymbolKind::Variant(_) => "enum variant",
            SymbolKind::Field(_) => "field",
            SymbolKind::Property(_) => "property",
            SymbolKind::RustItem(_) => "rust import",
            SymbolKind::Capability(_) => "capability",
        }
    }

    /// Validate a transparent alias target in the scope of its declared type parameters.
    pub(super) fn check_type_alias(&mut self, alias: &TypeAliasDecl) {
        self.symbols.enter_scope(ScopeKind::Block);
        for param in &alias.type_params {
            self.symbols.define_with_target_kind(
                Symbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Builtin), // Type-var placeholder
                    span: param.span,
                    scope: 0,
                },
                SemanticSourceTargetKind::GenericBinder,
            );
        }
        self.validate_type_param_bound_type_names(&alias.type_params);
        self.refuse_unsupported_callable_markers(
            &alias.type_params,
            CallableMarkerOwner::Nominal {
                kind: "type",
                name: &alias.name,
            },
        );
        let _ = self.resolve_type_checked(&alias.target);
        self.symbols.exit_scope();
    }

    /// Validate preset expressions for a collected top-level partial declaration.
    pub(super) fn check_partial_decl(&mut self, partial: &PartialDecl) {
        let Some(sym) = self.lookup_symbol(&partial.name).cloned() else {
            for arg in &partial.args {
                self.validate_top_level_partial_preset(arg);
                self.check_expr(&arg.value);
            }
            return;
        };
        let SymbolKind::Function(info) = sym.kind else {
            for arg in &partial.args {
                self.validate_top_level_partial_preset(arg);
                self.check_expr(&arg.value);
            }
            return;
        };
        for arg in &partial.args {
            self.validate_top_level_partial_preset(arg);
            let expected = info
                .params
                .iter()
                .find(|param| param.name() == Some(arg.name.as_str()))
                .map(|param| &param.ty);
            let actual = self.check_expr_with_expected(&arg.value, expected);
            if let Some(param) = info.params.iter().find(|param| param.name() == Some(arg.name.as_str()))
                && !self.types_compatible(&actual, &param.ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param.ty.to_string(),
                    &actual.to_string(),
                    arg.value.span,
                ));
            }
        }
    }

    /// Emit the RFC 084 declaration-safety diagnostic for one module-level partial preset.
    pub(super) fn validate_top_level_partial_preset(&mut self, arg: &PartialArg) {
        if !self.is_declaration_safe_partial_preset(&arg.value) {
            self.errors
                .push(errors::unsafe_top_level_partial_preset(&arg.name, arg.value.span));
        }
    }

    /// Return whether a module-level partial preset can be represented without executing user code.
    ///
    /// RFC 084 admits a scalar literal (a negated number literal included), a const named by identifier or by a
    /// qualified path, a payload-free enum variant path, a list, dict, set or tuple literal of such values, and a model
    /// literal of a known model.
    pub(super) fn is_declaration_safe_partial_preset(&mut self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Literal(_) => true,
            Expr::Unary(UnaryOp::Neg, operand) => {
                matches!(operand.node, Expr::Literal(Literal::Int(_) | Literal::Float(_)))
            }
            Expr::Ident(_) | Expr::Field(_, _) => self.is_declaration_safe_const_or_variant_path(expr),
            Expr::Paren(inner) => self.is_declaration_safe_partial_preset(inner),
            Expr::Tuple(items) | Expr::Set(items) => {
                items.iter().all(|item| self.is_declaration_safe_partial_preset(item))
            }
            Expr::List(entries) => entries.iter().all(|entry| match entry {
                ListEntry::Element(value) => self.is_declaration_safe_partial_preset(value),
                ListEntry::Spread(_) => false,
            }),
            Expr::Dict(entries) => entries.iter().all(|entry| match entry {
                DictEntry::Pair(key, value) => {
                    self.is_declaration_safe_partial_preset(key) && self.is_declaration_safe_partial_preset(value)
                }
                DictEntry::Spread(_) => false,
            }),
            Expr::Call(callee, type_args, args) => {
                type_args.is_empty()
                    && self.is_model_literal_callee(callee)
                    && args.iter().all(|arg| match arg {
                        CallArg::Named(_, value) => self.is_declaration_safe_partial_preset(value),
                        CallArg::Positional(_) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
                    })
            }
            Expr::Constructor(name, args) => {
                self.is_model_literal_name(name)
                    && args.iter().all(|arg| match arg {
                        CallArg::Named(_, value) => self.is_declaration_safe_partial_preset(value),
                        CallArg::Positional(_) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
                    })
            }
            _ => false,
        }
    }

    /// Return whether a call-like preset target names a model literal constructor.
    pub(super) fn is_model_literal_callee(&self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => self.is_model_literal_name(name),
            _ => false,
        }
    }

    /// Return whether a name resolves to a model type that can be serialized as preset metadata.
    pub(super) fn is_model_literal_name(&self, name: &str) -> bool {
        self.lookup_symbol(name)
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Model(_))))
    }

    /// Return whether a top-level partial preset path names a const or a zero-argument enum variant.
    ///
    /// A const is one this module declares, one imported by name (its binding carries the const's identity), or one
    /// read through an imported module (`limits.LIMIT`).
    pub(super) fn is_declaration_safe_const_or_variant_path(&mut self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(name) => {
                self.const_decls.contains_key(name)
                    || self.symbols.lookup(name).is_some_and(|symbol_id| {
                        self.symbols
                            .identity_of(symbol_id)
                            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Const)
                            || self
                                .symbols
                                .get(symbol_id)
                                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Variant(_)))
                    })
            }
            Expr::Field(base, member) => {
                if let Expr::Ident(type_name) = &base.node
                    && let Some(symbol) = self.lookup_symbol(type_name)
                    && let SymbolKind::Type(TypeInfo::Enum(info)) = &symbol.kind
                {
                    return info.variants.iter().any(|variant| variant == member)
                        || info.variant_aliases.contains_key(member);
                }
                let Some((_, module_path)) = self.imported_module_for_expr(base) else {
                    return false;
                };
                self.resolve_imported_module_constant_member(&module_path, member)
                    .and_then(|(_, identity)| identity)
                    .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Const)
            }
            _ => false,
        }
    }

    /// Validate preset expressions for same-type method partial declarations.
    pub(super) fn check_method_partials(&mut self, owner: &str, partials: &[Spanned<MethodPartialDecl>]) {
        let active_partials = partials
            .iter()
            .filter(|partial| self.member_binding_is_active(partial.span))
            .collect::<Vec<_>>();
        for partial in active_partials {
            let Some(target) = self.method_info_for_owner(owner, &partial.node.target).cloned() else {
                for arg in &partial.node.args {
                    self.check_expr(&arg.value);
                }
                continue;
            };

            for arg in &partial.node.args {
                let expected = target
                    .params
                    .iter()
                    .find(|param| param.name() == Some(arg.name.as_str()))
                    .map(|param| param.ty.clone());
                let actual = match expected.as_ref() {
                    Some(expected_ty) => self.check_expr_with_expected(&arg.value, Some(expected_ty)),
                    None => self.check_expr(&arg.value),
                };
                if let Some(expected_ty) = expected
                    && !self.types_compatible(&actual, &expected_ty)
                {
                    self.errors.push(errors::type_mismatch(
                        &expected_ty.to_string(),
                        &actual.to_string(),
                        arg.value.span,
                    ));
                }
            }
        }
    }

    /// Return collected method metadata for an owner type or trait surface.
    pub(in crate::typechecker) fn method_info_for_owner(&self, owner: &str, method: &str) -> Option<&MethodInfo> {
        if let Some(info) = self.lookup_trait_info(owner) {
            return info.methods.get(method);
        }
        match self.lookup_type_info(owner)? {
            TypeInfo::Model(info) => info.methods.get(method),
            TypeInfo::Class(info) => info.methods.get(method),
            TypeInfo::Newtype(info) => info.methods.get(method),
            TypeInfo::Enum(info) => info.methods.get(method),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Typecheck an inline test module declaration.
    pub(super) fn check_test_module(&mut self, test_module: &TestModuleDecl) {
        self.symbols.enter_scope(ScopeKind::Block);
        let previous_validation_state = self.validate_source_type_names;
        self.validate_source_type_names = false;
        self.collect_declarations_for_check(&test_module.body);
        self.resolve_forward_type_references(&test_module.body);
        self.settle_method_scopes(&test_module.body);
        self.record_local_nominal_type_param_bounds(&test_module.body);
        self.infer_hash_key_type_params(&test_module.body, false);
        self.validate_source_type_names = previous_validation_state;
        if let Some(semantics) = self.testing_marker_semantics.clone() {
            self.collect_testing_fixture_names_from_decls(&test_module.body, &semantics);
        }
        for decl in &test_module.body {
            self.check_declaration(decl);
        }
        self.symbols.exit_scope();
    }
}
