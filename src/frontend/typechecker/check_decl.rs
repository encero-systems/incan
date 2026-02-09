//! Second-pass declaration checking: validate models, classes, traits, enums, functions, methods.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;

use super::TypeChecker;
use incan_core::lang::derives::{self, DeriveId};
use incan_core::lang::magic_methods;
use std::collections::{HashMap, HashSet};

impl TypeChecker {
    fn collect_trait_method_names(&self, traits: &[Spanned<Ident>]) -> HashSet<String> {
        let mut names = HashSet::new();
        for trait_ref in traits {
            if let Some(id) = self.symbols.lookup(trait_ref.node.as_str()) {
                if let Some(sym) = self.symbols.get(id) {
                    if let SymbolKind::Trait(trait_info) = &sym.kind {
                        names.extend(trait_info.methods.keys().cloned());
                    }
                }
            }
        }
        names
    }

    fn validate_field_metadata(
        &mut self,
        type_name: &str,
        fields: &[Spanned<FieldDecl>],
        method_names: &HashSet<String>,
    ) {
        let canonical_names: HashSet<String> = fields.iter().map(|f| f.node.name.clone()).collect();
        let mut builtin_member_names: HashSet<&'static str> = HashSet::new();
        for info in magic_methods::MAGIC_METHODS {
            builtin_member_names.insert(info.canonical);
            builtin_member_names.extend(info.aliases);
        }
        let mut seen_aliases: HashMap<String, Span> = HashMap::new();

        for field in fields {
            let Some(alias) = field.node.metadata.alias.as_ref() else {
                continue;
            };

            if alias.trim().is_empty() {
                self.errors.push(errors::empty_alias(field.span));
                continue;
            }

            if canonical_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_canonical(type_name, alias, field.span));
            }

            if method_names.contains(alias) {
                self.errors
                    .push(errors::alias_collides_with_method(type_name, alias, field.span));
            }
            if builtin_member_names.contains(alias.as_str()) {
                self.errors
                    .push(errors::alias_collides_with_builtin(type_name, alias, field.span));
            }

            if let Some(prev_span) = seen_aliases.get(alias) {
                self.errors
                    .push(errors::duplicate_alias(type_name, alias, *prev_span, field.span));
            } else {
                seen_aliases.insert(alias.clone(), field.span);
            }
        }
    }
    fn method_sig_string_named(&self, method_name: &str, m: &MethodInfo) -> String {
        let recv = match m.receiver {
            Some(Receiver::Mutable) => "mut self",
            Some(Receiver::Immutable) => "self",
            None => "",
        };
        let mut parts: Vec<String> = Vec::new();
        if !recv.is_empty() {
            parts.push(recv.to_string());
        }
        for (name, ty) in &m.params {
            parts.push(format!("{name}: {ty}"));
        }
        let async_kw = if m.is_async { "async " } else { "" };
        format!(
            "{async_kw}def {name}({params}) -> {ret}",
            name = method_name,
            params = parts.join(", "),
            ret = m.return_type
        )
    }

    fn method_sigs_compatible(&self, expected: &MethodInfo, found: &MethodInfo) -> bool {
        if expected.receiver != found.receiver {
            return false;
        }
        if expected.is_async != found.is_async {
            return false;
        }
        if expected.params.len() != found.params.len() {
            return false;
        }
        for ((_, e_ty), (_, f_ty)) in expected.params.iter().zip(found.params.iter()) {
            if !self.types_compatible(e_ty, f_ty) {
                return false;
            }
        }
        self.types_compatible(&expected.return_type, &found.return_type)
    }

    // ========================================================================
    // Second pass: check declarations
    // ========================================================================

    /// Validate a declaration's body and semantics (second pass).
    ///
    /// Dispatches to `check_model`, `check_class`, etc. Expects symbols to
    /// already be registered via [`collect_declaration`](Self::collect_declaration).
    pub(crate) fn check_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(_) => {} // Already handled
            Declaration::Const(konst) => self.check_const(konst, decl.span),
            Declaration::Model(model) => self.check_model(model),
            Declaration::Class(class) => self.check_class(class),
            Declaration::Trait(tr) => self.check_trait(tr),
            Declaration::Newtype(nt) => self.check_newtype(nt),
            Declaration::Enum(en) => self.check_enum(en),
            Declaration::Function(func) => self.check_function(func),
            Declaration::Docstring(_) => {} // Docstrings don't need checking
        }
    }

    fn check_const(&mut self, konst: &ConstDecl, span: Span) {
        // RFC 008: const-eval (with cycle detection + category classification).
        self.check_and_resolve_const(konst, span);
    }

    fn check_model(&mut self, model: &ModelDecl) {
        self.symbols.enter_scope(ScopeKind::Model);

        self.validate_decorators(&model.decorators);
        // Validate @derive decorators
        self.validate_derives(&model.decorators);
        let derives = self.extract_derive_names(&model.decorators);
        let has_validate = derives
            .iter()
            .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));

        // Define type parameters
        for param in &model.type_params {
            self.symbols.define(Symbol {
                name: param.clone(),
                kind: SymbolKind::Type(TypeInfo::Builtin), // Type var placeholder
                span: Span::default(),
                scope: 0,
            });
        }

        // Check traits exist and are satisfied (models can adopt storage-free traits, RFC 000).
        // Note: do this after defining type params so `@requires(field: T)` can resolve `T`.
        for trait_ref in &model.traits {
            let trait_name = trait_ref.node.as_str();
            if let Some(id) = self.symbols.lookup(trait_name) {
                if let Some(sym) = self.symbols.get(id) {
                    if let SymbolKind::Trait(trait_info) = &sym.kind {
                        self.check_trait_conformance_model(model, trait_info.clone(), trait_name, trait_ref.span);
                    }
                }
            } else {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }

        let mut method_names = HashSet::new();
        if let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) {
            method_names.extend(info.methods.keys().cloned());
        }
        method_names.extend(self.collect_trait_method_names(&model.traits));
        self.validate_field_metadata(&model.name, &model.fields, &method_names);

        // Define fields in scope
        for field in &model.fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.symbols.define(Symbol {
                name: field.node.name.clone(),
                kind: SymbolKind::Field(FieldInfo {
                    ty,
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                }),
                span: field.span,
                scope: 0,
            });

            // Check default expression type
            if let Some(default) = &field.node.default {
                let default_ty = self.check_expr(default);
                let field_ty = self.resolve_type_checked(&field.node.ty);
                if !self.types_compatible(&default_ty, &field_ty) {
                    self.errors.push(errors::type_mismatch(
                        &field_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
            }
        }

        // Check methods
        for method in &model.methods {
            self.check_method(&method.node, &model.name);
        }

        if has_validate {
            self.check_validate_derive_model(model);
        }

        self.symbols.exit_scope();
    }

    fn check_validate_derive_model(&mut self, model: &ModelDecl) {
        // Validate that validate() exists and has the expected signature.
        let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) else {
            return;
        };

        let Some(validate) = info.methods.get("validate") else {
            self.errors.push(errors::validate_derive_missing_validate_method(
                &model.name,
                Span::default(),
            ));
            return;
        };

        let expected = "def validate(self) -> Result[Self, E]";
        let found_sig = self.method_sig_string_named("validate", validate);

        // Receiver must exist and be immutable.
        if validate.receiver != Some(Receiver::Immutable) || validate.is_async || !validate.params.is_empty() {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
            return;
        }

        // Return type must be Result[Self, E] (allow Result[ModelName, E] too).
        let ok_matches_self = |ok: &ResolvedType| {
            matches!(ok, ResolvedType::SelfType)
                || matches!(ok, ResolvedType::Named(n) if n == &model.name)
                || matches!(ok, ResolvedType::TypeVar(n) if n == &model.name)
        };

        if validate.return_type.is_result() {
            let ok_ty = validate
                .return_type
                .result_ok_type()
                .cloned()
                .unwrap_or(ResolvedType::Unknown);
            if !ok_matches_self(&ok_ty) {
                self.errors.push(errors::validate_derive_invalid_validate_signature(
                    &model.name,
                    expected,
                    &found_sig,
                    Span::default(),
                ));
            }
        } else {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
        }
    }
    fn check_trait_conformance_model(
        &mut self,
        model: &ModelDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        adoption_span: Span,
    ) {
        // Check required fields (including types)
        for (field_name, field_ty) in &trait_info.requires {
            let found = model.fields.iter().find(|f| &f.node.name == field_name);
            match found {
                None => {
                    self.errors
                        .push(errors::missing_field(&model.name, field_name, adoption_span));
                }
                Some(f) => {
                    let actual_ty = self.resolve_type_checked(&f.node.ty);
                    if !self.types_compatible(&actual_ty, field_ty) {
                        self.errors.push(errors::type_mismatch(
                            &field_ty.to_string(),
                            &actual_ty.to_string(),
                            f.node.ty.span,
                        ));
                    }
                }
            }
        }

        // Check required methods (those without body)
        for (method_name, method_info) in &trait_info.methods {
            if !method_info.has_body {
                // Prefer symbol-table method info so we can validate signatures.
                let model_info = self
                    .symbols
                    .lookup(&model.name)
                    .and_then(|id| self.symbols.get(id))
                    .and_then(|sym| match &sym.kind {
                        SymbolKind::Type(TypeInfo::Model(info)) => Some(info.clone()),
                        _ => None,
                    });

                if let Some(mi) = model_info {
                    match mi.methods.get(method_name) {
                        None => self
                            .errors
                            .push(errors::missing_trait_method(trait_name, method_name, adoption_span)),
                        Some(found) => {
                            if !self.method_sigs_compatible(method_info, found) {
                                let expected_sig = self.method_sig_string_named(method_name, method_info);
                                let found_sig = self.method_sig_string_named(method_name, found);
                                self.errors.push(errors::trait_method_signature_mismatch(
                                    trait_name,
                                    &model.name,
                                    method_name,
                                    &expected_sig,
                                    &found_sig,
                                    adoption_span,
                                ));
                            }
                        }
                    }
                } else {
                    // Fallback: previous behavior (name-only)
                    let found = model.methods.iter().any(|m| &m.node.name == method_name);
                    if !found {
                        self.errors
                            .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                    }
                }
            }
        }
    }

    fn check_class(&mut self, class: &ClassDecl) {
        self.symbols.enter_scope(ScopeKind::Class);

        self.validate_decorators(&class.decorators);
        // Validate @derive decorators
        self.validate_derives(&class.decorators);

        // Check base class exists
        if let Some(base) = &class.extends {
            if self.symbols.lookup(base).is_none() {
                self.errors.push(errors::unknown_symbol(base, Span::default()));
            }
        }

        // Check traits exist and are satisfied
        for trait_ref in &class.traits {
            let trait_name = trait_ref.node.as_str();
            if let Some(id) = self.symbols.lookup(trait_name) {
                if let Some(sym) = self.symbols.get(id) {
                    if let SymbolKind::Trait(trait_info) = &sym.kind {
                        self.check_trait_conformance(class, trait_info.clone(), trait_name, trait_ref.span);
                    }
                }
            } else {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }

        // RFC 021: Field aliases are NOT supported on class declarations.
        // Reject any field metadata on class fields.
        for field in &class.fields {
            if field.node.metadata.alias.is_some() {
                self.errors.push(errors::alias_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
            if field.node.metadata.description.is_some() {
                self.errors.push(errors::description_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
        }

        // Define fields
        for field in &class.fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.symbols.define(Symbol {
                name: field.node.name.clone(),
                kind: SymbolKind::Field(FieldInfo {
                    ty,
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                }),
                span: field.span,
                scope: 0,
            });

            if let Some(default) = &field.node.default {
                let default_ty = self.check_expr(default);
                let field_ty = self.resolve_type_checked(&field.node.ty);
                if !self.types_compatible(&default_ty, &field_ty) {
                    self.errors.push(errors::type_mismatch(
                        &field_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
            }
        }

        // Check methods
        for method in &class.methods {
            self.check_method(&method.node, &class.name);
        }

        self.symbols.exit_scope();
    }

    fn check_trait_conformance(
        &mut self,
        class: &ClassDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        adoption_span: Span,
    ) {
        // Use the effective members view (own + inherited) from the symbol table.
        let class_info = self
            .symbols
            .lookup(&class.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Class(info)) => Some(info.clone()),
                _ => None,
            });

        // Check required fields (presence + type compatibility).
        for (field_name, field_ty) in &trait_info.requires {
            match class_info.as_ref().and_then(|ci| ci.fields.get(field_name)) {
                None => {
                    self.errors
                        .push(errors::missing_field(&class.name, field_name, adoption_span));
                }
                Some(found) => {
                    if !self.types_compatible(&found.ty, field_ty) {
                        self.errors.push(errors::trait_required_field_type_mismatch(
                            trait_name,
                            &class.name,
                            field_name,
                            &field_ty.to_string(),
                            &found.ty.to_string(),
                            adoption_span,
                        ));
                    }
                }
            }
        }

        // Check required methods (those without body)
        for (method_name, method_info) in &trait_info.methods {
            if !method_info.has_body {
                match class_info.as_ref().and_then(|ci| ci.methods.get(method_name)) {
                    None => self
                        .errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span)),
                    Some(found) => {
                        if !self.method_sigs_compatible(method_info, found) {
                            let expected_sig = self.method_sig_string_named(method_name, method_info);
                            let found_sig = self.method_sig_string_named(method_name, found);
                            self.errors.push(errors::trait_method_signature_mismatch(
                                trait_name,
                                &class.name,
                                method_name,
                                &expected_sig,
                                &found_sig,
                                adoption_span,
                            ));
                        }
                    }
                }
            }
        }
    }

    fn check_trait(&mut self, tr: &TraitDecl) {
        self.symbols.enter_scope(ScopeKind::Trait);

        self.validate_decorators(&tr.decorators);
        let requires_map: HashMap<String, ResolvedType> = self
            .symbols
            .lookup(&tr.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Trait(info) => Some(info.requires.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .fold(HashMap::new(), |mut acc, (name, ty)| {
                acc.entry(name).or_insert(ty);
                acc
            });
        let prev_trait_requires = self.current_trait_requires.take();
        let prev_trait_name = self.current_trait_name.take();
        let prev_missing_emitted = self.current_trait_missing_requires_emitted.take();
        self.current_trait_requires = Some(requires_map);
        self.current_trait_name = Some(tr.name.clone());

        for method in &tr.methods {
            if method.node.body.is_some() {
                let prev_method_seen = self.current_trait_missing_requires_emitted.take();
                self.current_trait_missing_requires_emitted = Some(std::collections::HashSet::new());
                // Trait default methods are checked against `Self` (the eventual adopter type),
                // not against the trait name itself. This allows default bodies to reference
                // adopter fields (validated at adoption sites via `@requires`).
                self.check_method_with_self_ty(&method.node, ResolvedType::SelfType);
                self.current_trait_missing_requires_emitted = prev_method_seen;
            }
        }

        self.current_trait_requires = prev_trait_requires;
        self.current_trait_name = prev_trait_name;
        self.current_trait_missing_requires_emitted = prev_missing_emitted;
        self.symbols.exit_scope();
    }

    fn check_newtype(&mut self, nt: &NewtypeDecl) {
        // Check underlying type exists
        let underlying = self.resolve_type_checked(&nt.underlying);
        if matches!(underlying, ResolvedType::Unknown) {
            self.errors.push(errors::unknown_symbol(
                &format!("{:?}", nt.underlying.node),
                nt.underlying.span,
            ));
        }

        // Check methods (reuse the standard method-checking logic so parameters are in scope).
        for method in &nt.methods {
            if method.node.body.is_some() {
                self.check_method(&method.node, &nt.name);
            }
        }
    }

    fn check_enum(&mut self, en: &EnumDecl) {
        self.validate_decorators(&en.decorators);
        // Check variant field types exist
        for variant in &en.variants {
            for field_ty in &variant.node.fields {
                let resolved = self.resolve_type_checked(field_ty);
                if matches!(resolved, ResolvedType::Unknown) {
                    self.errors
                        .push(errors::unknown_symbol(&format!("{:?}", field_ty.node), field_ty.span));
                }
            }
        }
    }

    fn check_function(&mut self, func: &FunctionDecl) {
        self.symbols.enter_scope(ScopeKind::Function);

        self.validate_decorators(&func.decorators);
        // Define parameters
        for param in &func.params {
            let ty = self.resolve_type_checked(&param.node.ty);
            self.symbols.define(Symbol {
                name: param.node.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span: param.span,
                scope: 0,
            });
        }

        let return_type = self.resolve_type_checked(&func.return_type);
        self.symbols.set_return_type(return_type.clone());

        // Set error type for ? checking
        self.current_return_error_type = return_type.result_err_type().cloned();

        // Check body
        for stmt in &func.body {
            self.check_statement(stmt);
        }

        self.current_return_error_type = None;
        self.symbols.exit_scope();
    }

    pub(crate) fn check_method(&mut self, method: &MethodDecl, owner: &str) {
        self.validate_decorators(&method.decorators);
        self.check_method_with_self_ty(method, ResolvedType::Named(owner.to_string()));
    }

    fn check_method_with_self_ty(&mut self, method: &MethodDecl, self_ty: ResolvedType) {
        self.symbols.enter_scope(ScopeKind::Method {
            receiver: method.receiver,
        });

        // Define self if present
        if let Some(receiver) = method.receiver {
            let is_mutable = matches!(receiver, Receiver::Mutable);
            if is_mutable {
                self.mutable_bindings.insert("self".to_string());
            }
            self.symbols.define(Symbol {
                name: "self".to_string(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty: self_ty,
                    is_mutable,
                    is_used: true,
                }),
                span: Span::default(),
                scope: 0,
            });
        }

        // Define parameters
        for param in &method.params {
            let ty = self.resolve_type_checked(&param.node.ty);
            self.symbols.define(Symbol {
                name: param.node.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span: param.span,
                scope: 0,
            });
        }

        let return_type = self.resolve_type_checked(&method.return_type);
        self.symbols.set_return_type(return_type.clone());

        // Set error type for ? checking
        self.current_return_error_type = return_type.result_err_type().cloned();

        // Check body
        if let Some(body) = &method.body {
            for stmt in body {
                self.check_statement(stmt);
            }
        }

        self.current_return_error_type = None;
        self.mutable_bindings.remove("self");
        self.symbols.exit_scope();
    }
}
