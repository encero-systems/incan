//! Registry description and provider operation decorators and registry structural values.

use super::*;

impl TypeChecker {
    /// Validate compiler-recognized typed declaration descriptions before the callable body opens its local scope.
    pub(super) fn check_registry_description_decorators(
        &mut self,
        decorators: &[Spanned<Decorator>],
        declaration_name: &str,
        decl_span: Span,
        subject_kind: SemanticRegistrySubjectKind,
    ) {
        for decorator in decorators {
            if self.decorator_id(&decorator.node) != Some(DecoratorId::Describe) {
                continue;
            }
            let [
                DecoratorArg::Positional(registry),
                DecoratorArg::Positional(key),
                DecoratorArg::Positional(descriptor),
            ] = decorator.node.args.as_slice()
            else {
                self.errors.push(CompileError::type_error(
                    "@describe requires exactly three positional arguments: registry, key, descriptor".to_string(),
                    decorator.span,
                ));
                continue;
            };
            let Expr::Ident(registry_name) = &registry.node else {
                self.errors.push(CompileError::type_error(
                    "@describe registry must be a module static Registry binding".to_string(),
                    registry.span,
                ));
                continue;
            };
            let Some(static_info) = self.lookup_static_info(registry_name).cloned() else {
                self.errors.push(CompileError::type_error(
                    format!("@describe registry `{registry_name}` must be a module static Registry binding"),
                    registry.span,
                ));
                continue;
            };
            let Some((key_type, descriptor_type)) = Self::registry_type_arguments(&static_info.ty) else {
                self.errors.push(CompileError::type_error(
                    format!("@describe registry `{registry_name}` must have type Registry[K, T]"),
                    registry.span,
                ));
                continue;
            };
            let Some((definition, canonical_registry)) =
                self.registry_definition_for_description(registry_name, registry.span)
            else {
                continue;
            };
            if definition.key_type != *key_type || definition.descriptor_type != *descriptor_type {
                self.errors.push(CompileError::type_error(
                    format!("@describe registry `{registry_name}` has inconsistent Registry.define type arguments"),
                    registry.span,
                ));
                continue;
            }
            if !definition.subjects.contains(&subject_kind) {
                self.errors.push(CompileError::type_error(
                    format!("@describe cannot attach a {subject_kind} to registry `{registry_name}` because SubjectKind.{subject_kind} is not enabled"),
                    decorator.span,
                ));
                continue;
            }
            if !self.registry_descriptor_type_is_valid(descriptor_type) {
                self.errors.push(CompileError::type_error(
                    format!("@describe descriptor type `{descriptor_type}` must be a model with @derive(Descriptor)"),
                    descriptor.span,
                ));
                continue;
            }

            let key_actual = self.check_expr_with_expected(key, Some(key_type));
            if !self.types_compatible(&key_actual, key_type) {
                self.errors.push(errors::type_mismatch(
                    &key_type.to_string(),
                    &key_actual.to_string(),
                    key.span,
                ));
                continue;
            }
            let descriptor_actual = self.check_expr_with_expected(descriptor, Some(descriptor_type));
            if !self.types_compatible(&descriptor_actual, descriptor_type) {
                self.errors.push(errors::type_mismatch(
                    &descriptor_type.to_string(),
                    &descriptor_actual.to_string(),
                    descriptor.span,
                ));
                continue;
            }
            let Some(key_value) = self.checked_registry_structural_value(key) else {
                continue;
            };
            let Some(descriptor_value) = self.checked_registry_structural_value(descriptor) else {
                continue;
            };
            if self.registry_description_has_key(&canonical_registry, &key_value) {
                self.errors.push(CompileError::type_error(
                    format!("duplicate @describe key in registry `{registry_name}`"),
                    key.span,
                ));
                continue;
            }
            self.type_info.registry.descriptions.push(RegistryDescriptionInfo {
                registry_name: registry_name.clone(),
                registry: canonical_registry,
                key: key_value,
                descriptor: descriptor_value,
                subject_kind,
                declaration_name: declaration_name.to_string(),
                declaration_span: decl_span,
                decorator_span: decorator.span,
                key_span: key.span,
                descriptor_span: descriptor.span,
            });
        }
    }

    /// Resolve a description's initialized registry while retaining whether its owner is local or imported.
    /// Local definitions keep precedence; imported definitions retain their provider module, binding and visibility.
    pub(super) fn registry_definition_for_description(
        &mut self,
        name: &str,
        span: Span,
    ) -> Option<(RegistryDefinitionInfo, RegistryDescriptionRegistry)> {
        if let Some(definition) = self.type_info.registry.definitions.get(name).cloned() {
            return Some((
                definition,
                RegistryDescriptionRegistry::Local {
                    binding: name.to_string(),
                },
            ));
        }
        if let Some(imported) = self.type_info.registry.imported_definitions.get(name).cloned() {
            let public = imported.definition.is_public;
            return Some((
                imported.definition,
                RegistryDescriptionRegistry::Imported {
                    module_path: imported.owner_module_path,
                    binding: imported.owner_binding,
                    public,
                },
            ));
        }
        self.errors.push(CompileError::type_error(
            format!("@describe registry `{name}` must be initialized by Registry.define(...) before it is described"),
            span,
        ));
        None
    }

    /// Read a checked structural registry value, reporting its existing refusal at the expression's source span.
    /// Callers retain control over whether another value is checked after failure.
    pub(super) fn checked_registry_structural_value(&mut self, value: &Spanned<Expr>) -> Option<SemanticRegistryValue> {
        match self.registry_structural_value(value) {
            Ok(value) => Some(value),
            Err(message) => {
                self.errors.push(CompileError::type_error(message, value.span));
                None
            }
        }
    }

    /// Record the checked capability requirement for one provider operation.
    ///
    /// `@provider_operation(capability.path)` is deliberately a declaration attachment rather than a convention on a
    /// callable or module name. The typechecker resolves the capability at the producer declaration site and carries
    /// the resulting canonical identities into provider metadata; later consumers never need to reinterpret the
    /// decorator or recover meaning from a backend projection.
    pub(super) fn check_provider_operation_decorators(&mut self, func: &FunctionDecl, decl_span: Span) {
        let decorators = func
            .decorators
            .iter()
            .filter(|decorator| self.decorator_id(&decorator.node) == Some(DecoratorId::ProviderOperation))
            .collect::<Vec<_>>();
        if decorators.is_empty() {
            return;
        }
        if decorators.len() > 1 {
            for decorator in decorators {
                self.errors.push(CompileError::type_error(
                    "a provider function may declare only one @provider_operation capability".to_string(),
                    decorator.span,
                ));
            }
            return;
        }
        let decorator = decorators[0];
        let [DecoratorArg::Positional(capability)] = decorator.node.args.as_slice() else {
            self.errors.push(CompileError::type_error(
                "@provider_operation requires exactly one capability reference".to_string(),
                decorator.span,
            ));
            return;
        };
        let Some(path) = dotted_path_segments(&capability.node) else {
            self.errors.push(CompileError::type_error(
                "@provider_operation capability must be a checked capability reference, not a value expression"
                    .to_string(),
                capability.span,
            ));
            return;
        };
        let Some(required_capability) = self.provider_operation_capability_identity(&path, capability.span) else {
            return;
        };
        self.type_info
            .record_resolved_identity(capability.span, required_capability.clone());
        let Some(module_path) = self.current_module_path.clone() else {
            self.errors.push(CompileError::type_error(
                "@provider_operation requires a compiler-known module identity".to_string(),
                decorator.span,
            ));
            return;
        };
        let operation = CanonicalSymbolId::module_declaration(
            module_path,
            func.name.clone(),
            SemanticSourceTargetKind::Function,
            decl_span,
        );
        self.type_info.declarations.provider_operations.insert(
            operation.clone(),
            ProviderOperationDeclarationInfo {
                operation,
                required_capability,
                runtime_requirements: Vec::new(),
            },
        );
    }

    /// Resolve an operation decorator's capability reference to the declaration identity it actually names.
    pub(super) fn provider_operation_capability_identity(
        &mut self,
        path: &[String],
        span: Span,
    ) -> Option<CanonicalSymbolId> {
        let rendered = path.join(".");
        let (name, module) = path.split_last()?;
        if module.is_empty() {
            let Some(symbol_id) = self.symbols.lookup(name) else {
                self.errors.push(CompileError::type_error(
                    format!("@provider_operation capability `{rendered}` is unresolved"),
                    span,
                ));
                return None;
            };
            let symbol = self.symbols.get(symbol_id)?;
            if !matches!(symbol.kind, SymbolKind::Capability(_)) {
                self.errors.push(CompileError::type_error(
                    format!("@provider_operation reference `{rendered}` does not name a capability"),
                    span,
                ));
                return None;
            }
            let Some(identity) = self.symbols.identity_of(symbol_id).cloned() else {
                self.errors.push(CompileError::type_error(
                    format!("@provider_operation capability `{rendered}` has no compiler-proven identity"),
                    span,
                ));
                return None;
            };
            return Some(identity);
        }
        let (root, nested) = module.split_first()?;
        let Some(mut module_path) = self.module_namespace_path(root) else {
            self.errors.push(CompileError::type_error(
                format!("@provider_operation capability `{rendered}` is unresolved"),
                span,
            ));
            return None;
        };
        module_path.extend(nested.iter().cloned());
        let identity = self
            .loaf_member_identity(&module_path, name)
            .or_else(|| self.dependency_member_identity(&ImportPath::simple(module_path.clone()), name));
        match identity {
            Some(identity) if identity.kind == SemanticSourceTargetKind::Capability => Some(identity),
            Some(_) => {
                self.errors.push(CompileError::type_error(
                    format!("@provider_operation reference `{rendered}` does not name a capability"),
                    span,
                ));
                None
            }
            None => {
                self.errors.push(CompileError::type_error(
                    format!("@provider_operation capability `{rendered}` is unresolved"),
                    span,
                ));
                None
            }
        }
    }

    /// Reject `@describe` where RFC 113 has no stable source subject yet.
    ///
    /// The decorator resolver deliberately recognizes compiler-owned decorators on broad syntactic targets. Keeping
    /// this target check beside the actual registry collector prevents an unsupported declaration from being silently
    /// accepted merely because it is a known decorator, while leaving future model/trait/module support open to a
    /// concrete subject-identity contract.
    pub(super) fn reject_registry_description_decorators(&mut self, decorators: &[Spanned<Decorator>], target: &str) {
        for decorator in decorators {
            if self.decorator_id(&decorator.node) == Some(DecoratorId::Describe) {
                self.errors.push(CompileError::type_error(
                    format!("@describe is currently supported only on concrete functions and methods, not a {target}"),
                    decorator.span,
                ));
            }
        }
    }

    /// Return whether a type is an RFC 113 descriptor model.
    pub(super) fn registry_descriptor_type_is_valid(&self, ty: &ResolvedType) -> bool {
        let ResolvedType::Named(name) = ty else {
            return false;
        };
        let mut visiting = HashSet::new();
        self.registry_descriptor_named_type_is_valid(name, &mut visiting)
            .is_ok()
    }

    /// Convert one expression to an RFC 113 structural snapshot without evaluating user code.
    pub(in crate::typechecker) fn registry_structural_value(
        &self,
        expr: &Spanned<Expr>,
    ) -> Result<SemanticRegistryValue, String> {
        self.registry_structural_value_with_consts(expr, &mut HashSet::new())
    }

    /// Convert one expression to a checked structural snapshot, expanding local const declarations through the
    /// compiler's already-parsed declaration graph. An imported `pub const` is recorded as the value its declaring
    /// module computed for it. This is not runtime evaluation: only the RFC 113 literal/model subset is traversed, and
    /// cycles are rejected explicitly.
    pub(super) fn registry_structural_value_with_consts(
        &self,
        expr: &Spanned<Expr>,
        expanding_consts: &mut HashSet<String>,
    ) -> Result<SemanticRegistryValue, String> {
        if let Expr::Ident(name) = &expr.node
            && !self.const_decls.contains_key(name)
            && let Some(value) = self.imported_registry_const(expr.span, name)
        {
            return value.clone();
        }
        match &expr.node {
            Expr::Literal(Literal::Int(value)) => Ok(SemanticRegistryValue::Int(value.value)),
            Expr::Literal(Literal::Float(value)) => Ok(SemanticRegistryValue::Float(value.repr.clone())),
            Expr::Literal(Literal::Decimal(value)) => Ok(SemanticRegistryValue::Float(value.repr.clone())),
            Expr::Literal(Literal::String(value)) => Ok(SemanticRegistryValue::String(value.clone())),
            Expr::Literal(Literal::Bytes(value)) => Ok(SemanticRegistryValue::Bytes(value.clone())),
            Expr::Literal(Literal::Bool(value)) => Ok(SemanticRegistryValue::Bool(*value)),
            Expr::Literal(Literal::None) => Ok(SemanticRegistryValue::None),
            Expr::Paren(inner) => self.registry_structural_value_with_consts(inner, expanding_consts),
            Expr::List(entries) => entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => self.registry_structural_value_with_consts(value, expanding_consts),
                    ListEntry::Spread(_) => Err("@describe structural values cannot use list spreads".to_string()),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(SemanticRegistryValue::List),
            Expr::Dict(entries) => entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) => Ok((
                        self.registry_structural_value_with_consts(key, expanding_consts)?,
                        self.registry_structural_value_with_consts(value, expanding_consts)?,
                    )),
                    DictEntry::Spread(_) => Err("@describe structural values cannot use dict spreads".to_string()),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(SemanticRegistryValue::Dict),
            Expr::Ident(name) if self.const_decls.contains_key(name) => {
                let Some(konst) = self.const_decls.get(name) else {
                    return Err(format!("unknown const `{name}` in registry descriptor"));
                };
                if !expanding_consts.insert(name.clone()) {
                    return Err(format!("registry descriptor const `{name}` is cyclic"));
                }
                let value = self.registry_structural_value_with_consts(&konst.0.value, expanding_consts);
                expanding_consts.remove(name);
                value
            }
            Expr::Ident(name) if self.lookup_type_info(name).is_some() => Ok(SemanticRegistryValue::Type(name.clone())),
            Expr::Field(_, _) => self
                .registry_structural_path(expr)
                .ok_or_else(|| "@describe structural references must name a const or enum variant".to_string())
                .map(SemanticRegistryValue::ConstRef),
            Expr::Call(callee, _, arguments) => {
                let Expr::Ident(name) = &callee.node else {
                    return Err("@describe structural calls must construct a newtype or Descriptor model".to_string());
                };
                if name == constructors::as_str(ConstructorId::Some) {
                    return self.registry_structural_some(arguments, expanding_consts);
                }
                if matches!(self.lookup_type_info(name), Some(TypeInfo::Model(_))) {
                    return self.registry_structural_model_with_consts(name, arguments, expanding_consts);
                }
                if !matches!(self.lookup_type_info(name), Some(TypeInfo::Newtype(_))) {
                    return Err("@describe structural calls must construct a newtype or Descriptor model".to_string());
                }
                let [CallArg::Positional(value)] = arguments.as_slice() else {
                    return Err("@describe newtype keys require exactly one positional structural value".to_string());
                };
                Ok(SemanticRegistryValue::Newtype {
                    name: name.clone(),
                    value: Box::new(self.registry_structural_value_with_consts(value, expanding_consts)?),
                })
            }
            Expr::Constructor(name, arguments) if name == constructors::as_str(ConstructorId::Some) => {
                self.registry_structural_some(arguments, expanding_consts)
            }
            Expr::Constructor(name, arguments) => {
                self.registry_structural_model_with_consts(name, arguments, expanding_consts)
            }
            _ => Err("@describe values must be literals, safe const references, collections, newtypes, or Descriptor model literals".to_string()),
        }
    }

    /// The structural value recorded for the imported `pub const` an identifier names, found by the const's canonical
    /// identity rather than its spelling. A name that is not an imported source const has none.
    fn imported_registry_const(&self, span: Span, name: &str) -> Option<&Result<SemanticRegistryValue, String>> {
        let identity = self
            .type_info
            .resolved_identity(span)
            .or_else(|| self.symbols.lookup(name).and_then(|id| self.symbols.identity_of(id)))?;
        self.dependency_registry_consts.get(identity)
    }

    /// Snapshot `Some(value)` as a structural option without evaluating user code.
    pub(super) fn registry_structural_some(
        &self,
        arguments: &[CallArg],
        expanding_consts: &mut HashSet<String>,
    ) -> Result<SemanticRegistryValue, String> {
        let [CallArg::Positional(value)] = arguments else {
            return Err("@describe Some(...) requires exactly one positional structural value".to_string());
        };
        Ok(SemanticRegistryValue::Option(Box::new(
            self.registry_structural_value_with_consts(value, expanding_consts)?,
        )))
    }

    /// Snapshot a descriptor model constructor while preserving expanded local const values.
    pub(super) fn registry_structural_model_with_consts(
        &self,
        name: &str,
        arguments: &[CallArg],
        expanding_consts: &mut HashSet<String>,
    ) -> Result<SemanticRegistryValue, String> {
        if !matches!(self.lookup_type_info(name), Some(TypeInfo::Model(_))) {
            return Err("@describe structural constructors must be models".to_string());
        }
        let mut fields = Vec::new();
        for argument in arguments {
            let CallArg::Named(field, value) = argument else {
                return Err("@describe Descriptor model literals require named fields without spreads".to_string());
            };
            fields.push((
                field.node.clone(),
                self.registry_structural_value_with_consts(value, expanding_consts)?,
            ));
        }
        fields.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(SemanticRegistryValue::Model {
            name: name.to_string(),
            fields,
        })
    }

    /// Return a safe const or enum-variant path for a structural reference expression.
    pub(super) fn registry_structural_path(&self, expr: &Spanned<Expr>) -> Option<Vec<String>> {
        /// Collect only plain identifier/member segments, rejecting every computed expression form.
        fn collect(expr: &Expr, out: &mut Vec<String>) -> bool {
            match expr {
                Expr::Ident(name) => {
                    out.push(name.clone());
                    true
                }
                Expr::Field(base, field) => {
                    if !collect(&base.node, out) {
                        return false;
                    }
                    out.push(field.clone());
                    true
                }
                _ => false,
            }
        }
        let mut path = Vec::new();
        if !collect(&expr.node, &mut path) {
            return None;
        }
        let first = path.first()?;
        if self.const_decls.contains_key(first) || matches!(self.lookup_type_info(first), Some(TypeInfo::Enum(_))) {
            Some(path)
        } else {
            None
        }
    }
}
