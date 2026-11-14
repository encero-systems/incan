//! Const and static declarations and registry explicit entries.

use super::*;

impl TypeChecker {
    /// Check a const declaration, routing RFC 024 `__derives__` metadata through derive-specific validation.
    pub(super) fn check_const(&mut self, konst: &ConstDecl, span: Span) {
        if konst.name == "__derives__" {
            self.check_derives_metadata(konst);
            return;
        }
        // RFC 008: const-eval (with cycle detection + category classification).
        self.check_and_resolve_const(konst, span);
    }

    /// Validate RFC 024 module-level `__derives__` metadata against the module's local trait declarations.
    pub(super) fn check_derives_metadata(&mut self, konst: &ConstDecl) {
        let Expr::List(entries) = &konst.value.node else {
            self.errors.push(CompileError::type_error(
                "`__derives__` must be a list of trait names".to_string(),
                konst.value.span,
            ));
            return;
        };
        if entries.is_empty() {
            self.errors.push(CompileError::type_error(
                "`__derives__` must list at least one trait".to_string(),
                konst.value.span,
            ));
            return;
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            let crate::ast::ListEntry::Element(expr) = entry else {
                self.errors.push(CompileError::type_error(
                    "`__derives__` entries must be trait names, not spreads".to_string(),
                    konst.value.span,
                ));
                continue;
            };
            let Expr::Ident(name) = &expr.node else {
                self.errors.push(CompileError::type_error(
                    "`__derives__` entries must be trait names".to_string(),
                    expr.span,
                ));
                continue;
            };
            if !seen.insert(name.clone()) {
                self.errors.push(CompileError::type_error(
                    format!("Duplicate trait '{name}' in `__derives__`"),
                    expr.span,
                ));
            }
            if self.lookup_trait_info(name).is_none() {
                self.errors.push(CompileError::type_error(
                    format!("`__derives__` entry '{name}' is not a trait"),
                    expr.span,
                ));
            }
        }
    }

    /// Validate a module static declaration and record its final type for later lowering.
    pub(super) fn check_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let expected_ty = self.resolve_type_checked(&static_decl.ty);
        let previous_registry_entry_context = self.checking_registry_entry_static_initializer;
        self.checking_registry_entry_static_initializer = Self::registry_entry_type_arguments(&expected_ty).is_some()
            && matches!(&static_decl.value.node, Expr::MethodCall(_, method, _, _) if method == "entry");
        let value_ty = self.check_expr_with_expected(&static_decl.value, Some(&expected_ty));
        self.checking_registry_entry_static_initializer = previous_registry_entry_context;
        if !self.types_compatible(&value_ty, &expected_ty)
            && !self.record_validated_newtype_coercion_if_possible(&value_ty, &expected_ty, static_decl.value.span)
        {
            self.errors.push(errors::type_mismatch(
                &expected_ty.to_string(),
                &value_ty.to_string(),
                static_decl.value.span,
            ));
        }

        if let Some(symbol_id) = self.symbols.lookup_local(&static_decl.name)
            && let Some(symbol) = self.symbols.get_mut(symbol_id)
            && let SymbolKind::Static(info) = &mut symbol.kind
        {
            info.ty = expected_ty.clone();
        }

        if let Some((key_type, descriptor_type)) = Self::registry_type_arguments(&expected_ty) {
            match self.registry_definition_subjects(static_decl) {
                Ok(subjects) => {
                    self.type_info.registry.definitions.insert(
                        static_decl.name.clone(),
                        RegistryDefinitionInfo {
                            key_type: key_type.clone(),
                            descriptor_type: descriptor_type.clone(),
                            subjects,
                            is_public: static_decl.visibility == Visibility::Public,
                        },
                    );
                }
                Err(message) => self
                    .errors
                    .push(CompileError::type_error(message, static_decl.value.span)),
            }
        }

        self.check_registry_explicit_entry_static(static_decl, &expected_ty, span);

        let _ = span;
    }

    /// Return the typed key and descriptor arguments when `ty` is `Registry[K, T]`.
    pub(in crate::typechecker) fn registry_type_arguments(ty: &ResolvedType) -> Option<(&ResolvedType, &ResolvedType)> {
        let ResolvedType::Generic(name, arguments) = ty else {
            return None;
        };
        let [key, descriptor] = arguments.as_slice() else {
            return None;
        };
        (name == "Registry").then_some((key, descriptor))
    }

    /// Return the typed key and descriptor arguments when `ty` is `RegistryEntry[K, T]`.
    pub(super) fn registry_entry_type_arguments(ty: &ResolvedType) -> Option<(&ResolvedType, &ResolvedType)> {
        let ResolvedType::Generic(name, arguments) = ty else {
            return None;
        };
        let [key, descriptor] = arguments.as_slice() else {
            return None;
        };
        (name == "RegistryEntry").then_some((key, descriptor))
    }

    /// Validate a real static `registry.entry(...)` value that describes the current compilation unit or package.
    ///
    /// Unlike a declaration decorator this entry's subject is supplied explicitly, but its key and descriptor are
    /// still compiler-checked structural facts. The source static remains the runtime value; this records the
    /// non-runtime projection without executing any user code.
    pub(super) fn check_registry_explicit_entry_static(
        &mut self,
        static_decl: &StaticDecl,
        expected_ty: &ResolvedType,
        span: Span,
    ) {
        let Some((entry_key_type, entry_descriptor_type)) = Self::registry_entry_type_arguments(expected_ty) else {
            return;
        };
        let Expr::MethodCall(receiver, method, _, arguments) = &static_decl.value.node else {
            return;
        };
        if method != "entry" {
            return;
        }
        let Expr::Ident(registry_name) = &receiver.node else {
            self.errors.push(CompileError::type_error(
                "RegistryEntry statics must call a module static registry's entry(...) method".to_string(),
                receiver.span,
            ));
            return;
        };
        let Some(definition) = self.type_info.registry.definitions.get(registry_name).cloned() else {
            self.errors.push(CompileError::type_error(
                format!("registry `{registry_name}` must be initialized by Registry.define(...) before entry(...)"),
                receiver.span,
            ));
            return;
        };
        if definition.key_type != *entry_key_type || definition.descriptor_type != *entry_descriptor_type {
            self.errors.push(CompileError::type_error(
                format!(
                    "RegistryEntry `{}` must use the same K and T as registry `{registry_name}`",
                    static_decl.name
                ),
                static_decl.ty.span,
            ));
            return;
        }

        let Some((key, subject, descriptor)) = self.registry_entry_arguments(arguments, static_decl.value.span) else {
            return;
        };
        let (subject_kind, subject_constructor_identity, checked_constructor_identity) =
            match self.registry_explicit_subject(subject) {
                Ok(subject) => subject,
                Err(message) => {
                    self.errors.push(CompileError::type_error(message, subject.span));
                    return;
                }
            };
        if !definition.subjects.contains(&subject_kind) {
            self.errors.push(CompileError::type_error(
                format!("registry `{registry_name}` does not enable SubjectKind.{subject_kind}"),
                subject.span,
            ));
            return;
        }
        if !self.registry_descriptor_type_is_valid(entry_descriptor_type) {
            self.errors.push(CompileError::type_error(
                format!(
                    "registry.entry descriptor type `{entry_descriptor_type}` must be a model with @derive(Descriptor)"
                ),
                descriptor.span,
            ));
            return;
        }

        let Some((key_value, descriptor_value)) =
            self.registry_entry_structural_values(key, descriptor, entry_key_type, entry_descriptor_type)
        else {
            return;
        };
        if self.registry_has_key(registry_name, &key_value) {
            self.errors.push(CompileError::type_error(
                format!("duplicate checked registry key in `{registry_name}`"),
                key.span,
            ));
            return;
        }
        let Some(entry_method_identity) = self
            .type_info
            .resolved_identity(static_decl.value.span)
            .filter(|identity| {
                identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method
                    && identity.declaration_name == "entry"
            })
            .cloned()
        else {
            self.errors.push(CompileError::type_error(
                "registry.entry must resolve to the source-owned Registry.entry declaration".to_string(),
                static_decl.value.span,
            ));
            return;
        };
        self.type_info
            .registry
            .explicit_entries
            .push(RegistryExplicitEntryInfo {
                registry_name: registry_name.clone(),
                key: key_value,
                descriptor: descriptor_value,
                subject_kind,
                entry_method_identity,
                subject_constructor_identity,
                checked_constructor_identity,
                entry_name: static_decl.name.clone(),
                declaration_span: span,
                key_span: key.span,
                subject_span: subject.span,
                descriptor_span: descriptor.span,
            });
    }

    /// Bind the explicit entry's required named key, subject and descriptor without evaluating expressions. Shape
    /// errors stop at their first source occurrence, preserving duplicate, unknown-name and missing-value diagnostics.
    pub(super) fn registry_entry_arguments<'a>(
        &mut self,
        arguments: &'a [CallArg],
        span: Span,
    ) -> Option<(&'a Spanned<Expr>, &'a Spanned<Expr>, &'a Spanned<Expr>)> {
        let mut key = None;
        let mut subject = None;
        let mut descriptor = None;
        for argument in arguments {
            let CallArg::Named(name, value) = argument else {
                self.errors.push(CompileError::type_error(
                    "registry.entry for checked compilation-unit or package entries requires named key, subject, and descriptor arguments".to_string(),
                    span,
                ));
                return None;
            };
            let target = match name.node.as_str() {
                "key" => &mut key,
                "subject" => &mut subject,
                "descriptor" => &mut descriptor,
                _ => {
                    self.errors.push(CompileError::type_error(
                        format!(
                            "registry.entry does not accept checked registry argument `{}`",
                            name.node
                        ),
                        value.span,
                    ));
                    return None;
                }
            };
            if target.replace(value).is_some() {
                self.errors.push(CompileError::type_error(
                    format!("registry.entry received duplicate `{}` argument", name.node),
                    value.span,
                ));
                return None;
            }
        }
        let (Some(key), Some(subject), Some(descriptor)) = (key, subject, descriptor) else {
            self.errors.push(CompileError::type_error(
                "registry.entry for checked compilation-unit or package entries requires key, subject, and descriptor"
                    .to_string(),
                span,
            ));
            return None;
        };
        Some((key, subject, descriptor))
    }

    /// Check both explicit entry values before reporting the first carrier mismatch, then extract their structural
    /// facts in key/descriptor order. Description decorators retain their separate sequential checking rule.
    pub(super) fn registry_entry_structural_values(
        &mut self,
        key: &Spanned<Expr>,
        descriptor: &Spanned<Expr>,
        key_type: &ResolvedType,
        descriptor_type: &ResolvedType,
    ) -> Option<(SemanticRegistryValue, SemanticRegistryValue)> {
        let key_actual = self.check_expr_with_expected(key, Some(key_type));
        let descriptor_actual = self.check_expr_with_expected(descriptor, Some(descriptor_type));
        if !self.types_compatible(&key_actual, key_type) {
            self.errors.push(errors::type_mismatch(
                &key_type.to_string(),
                &key_actual.to_string(),
                key.span,
            ));
            return None;
        }
        if !self.types_compatible(&descriptor_actual, descriptor_type) {
            self.errors.push(errors::type_mismatch(
                &descriptor_type.to_string(),
                &descriptor_actual.to_string(),
                descriptor.span,
            ));
            return None;
        }
        let key_value = self.checked_registry_structural_value(key)?;
        let descriptor_value = self.checked_registry_structural_value(descriptor)?;
        Some((key_value, descriptor_value))
    }

    /// Resolve the bounded explicit registry subject and the exact source artifacts that lowering may substitute.
    ///
    /// The public placeholder and compiler-reserved materializer are both ordinary Incan methods, so this frontend
    /// boundary retains their canonical identities instead of asking lowering to reconstruct either artifact from a
    /// method spelling. The expression has already been typechecked before this validator runs.
    pub(super) fn registry_explicit_subject(
        &self,
        subject: &Spanned<Expr>,
    ) -> Result<(SemanticRegistrySubjectKind, CanonicalSymbolId, CanonicalSymbolId), String> {
        let Expr::MethodCall(receiver, method, _, arguments) = &subject.node else {
            return Err(
                "registry.entry subject must be RegistrySubject.current_unit() or RegistrySubject.package()"
                    .to_string(),
            );
        };
        if !matches!(&receiver.node, Expr::Ident(name) if name == "RegistrySubject") || !arguments.is_empty() {
            return Err(
                "registry.entry subject must be RegistrySubject.current_unit() or RegistrySubject.package()"
                    .to_string(),
            );
        }
        let (subject_kind, checked_constructor_name) = match method.as_str() {
            "current_unit" => (SemanticRegistrySubjectKind::CompilationUnit, "_checked_current_unit"),
            "package" => (SemanticRegistrySubjectKind::Package, "_checked_package"),
            _ => Err(
                "registry.entry subject must be RegistrySubject.current_unit() or RegistrySubject.package()"
                    .to_string(),
            )?,
        };
        let subject_constructor_identity = self
            .type_info
            .resolved_identity(subject.span)
            .filter(|identity| {
                identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method
                    && identity.declaration_name == *method
            })
            .cloned()
            .ok_or_else(|| {
                "registry.entry subject must resolve to the compiler-owned RegistrySubject constructor".to_string()
            })?;
        let checked_constructor_identity = self
            .method_info_for_owner("RegistrySubject", checked_constructor_name)
            .and_then(|method| method.identity.clone())
            .filter(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method)
            .ok_or_else(|| {
                format!(
                    "registry.entry cannot resolve compiler-owned RegistrySubject.{checked_constructor_name} materializer"
                )
            })?;
        Ok((subject_kind, subject_constructor_identity, checked_constructor_identity))
    }

    /// Return whether a checked description or explicit entry already owns `key` in `registry_name`.
    pub(super) fn registry_has_key(&self, registry_name: &str, key: &SemanticRegistryValue) -> bool {
        self.type_info
            .registry
            .descriptions
            .iter()
            .any(|entry| entry.registry_name == registry_name && &entry.key == key)
            || self
                .type_info
                .registry
                .explicit_entries
                .iter()
                .any(|entry| entry.registry_name == registry_name && &entry.key == key)
    }

    /// Return whether `key` is already claimed by a described registry's canonical definition.
    ///
    /// Local aliases remain relevant to runtime lowering, but duplicate validation must compare the source owner so
    /// importing the same registry under two names cannot create two checked descriptions for one registry key.
    ///
    /// `explicit_entries` only ever records module-local `registry_name` bindings (`.entry()` requires a local
    /// `Registry.define(...)`), and `RegistryArtifacts::definitions` is a single binding-name-keyed map with no
    /// module qualification. Comparing an `Imported` registry's remote-module binding name against that flat local
    /// namespace by bare string equality would risk a false "duplicate key" match against an unrelated,
    /// coincidentally same-named local registry in a different module, so this cross-check is intentionally
    /// restricted to `Local` until registry identity carries a canonical module path.
    pub(super) fn registry_description_has_key(
        &self,
        registry: &RegistryDescriptionRegistry,
        key: &SemanticRegistryValue,
    ) -> bool {
        self.type_info
            .registry
            .descriptions
            .iter()
            .any(|entry| &entry.registry == registry && &entry.key == key)
            || self.type_info.registry.explicit_entries.iter().any(|entry| {
                matches!(registry, RegistryDescriptionRegistry::Local { binding } if binding == &entry.registry_name)
                    && &entry.key == key
            })
    }

    /// Extract the subject contract from a static `Registry.define(subjects=[...])` initializer.
    pub(in crate::typechecker) fn registry_definition_subjects(
        &self,
        static_decl: &StaticDecl,
    ) -> Result<Vec<SemanticRegistrySubjectKind>, String> {
        let Expr::MethodCall(receiver, method, _, arguments) = &static_decl.value.node else {
            return Err("Registry statics must be initialized with Registry.define(subjects=[...])".to_string());
        };
        if !matches!(&receiver.node, Expr::Ident(name) if name == "Registry") || method != "define" {
            return Err("Registry statics must be initialized with Registry.define(subjects=[...])".to_string());
        }
        let subjects = match arguments.as_slice() {
            [CallArg::Positional(value)] => value,
            [CallArg::Named(name, value)] if name.node == "subjects" => value,
            _ => {
                return Err("Registry.define requires exactly one `subjects` argument".to_string());
            }
        };
        let Expr::List(entries) = &subjects.node else {
            return Err("Registry.define subjects must be a literal list of SubjectKind values".to_string());
        };

        let mut result = Vec::new();
        for entry in entries {
            let ListEntry::Element(value) = entry else {
                return Err("Registry.define subjects cannot use list spreads".to_string());
            };
            let Expr::Field(base, variant) = &value.node else {
                return Err("Registry.define subjects must use SubjectKind.<kind> values".to_string());
            };
            if !matches!(&base.node, Expr::Ident(name) if name == "SubjectKind") {
                return Err("Registry.define subjects must use SubjectKind.<kind> values".to_string());
            }
            let subject = match variant.as_str() {
                "Function" => SemanticRegistrySubjectKind::Function,
                "Method" => SemanticRegistrySubjectKind::Method,
                "CompilationUnit" => SemanticRegistrySubjectKind::CompilationUnit,
                "Package" => SemanticRegistrySubjectKind::Package,
                _ => return Err(format!("unknown Registry subject kind `{variant}`")),
            };
            if result.contains(&subject) {
                return Err(format!("duplicate Registry subject kind `{variant}`"));
            }
            result.push(subject);
        }
        if result.is_empty() {
            return Err("Registry.define requires at least one allowed subject kind".to_string());
        }
        Ok(result)
    }
}
