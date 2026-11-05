//! Native attribute applications, decorator validation, Rust `allow` and derive arguments, and decorator ids.

use super::*;

impl TypeChecker {
    /// Select an exported derive macro only from a declared crate's inspected facts. Forwarding to rustc does not
    /// predict its expansion or grant any generated methods, layout, or implementation to the checker.
    pub(in crate::typechecker) fn checked_native_derive_macro(&self, path: &str) -> Option<String> {
        if !self.rust_derive_path_has_declared_crate(path) {
            return None;
        }
        let metadata = self.rust_item_metadata_for_path(path)?;
        let derivable = match &metadata.kind {
            incan_lang::interop::RustItemKind::Macro(info) => info.is_derive(),
            incan_lang::interop::RustItemKind::Trait(info) => info.derive_macro.is_some(),
            _ => false,
        };
        derivable.then(|| metadata.canonical_path.clone())
    }

    /// Return whether the Loaf owning this resolved path declares it as an extern-backed decorator function.
    ///
    /// The path names a public namespace, so a function declared in one of the namespace's source modules counts the
    /// same way an import of that name would find it.
    pub(in crate::typechecker) fn provider_declares_decorator_function(&self, resolved: &[String]) -> bool {
        self.provider_decorator_function(resolved).is_some()
    }

    /// The extern-backed decorator function the Loaf owning this resolved path declares, from its published API.
    pub(super) fn provider_decorator_function(&self, resolved: &[String]) -> Option<&ApiFunction> {
        let (function_name, module_path) = resolved.split_last()?;
        let (library, provider_module_path) = self.loaf_module(module_path)?;
        let manifest = self.loaves.manifest(&library)?;
        Self::api_modules_for_public_namespace(manifest, &provider_module_path)
            .into_iter()
            .flat_map(|module| module.declarations.iter())
            .find_map(|declaration| match declaration {
                ApiDeclaration::Function(function)
                    if function.name == *function_name
                        && function
                            .decorators
                            .iter()
                            .any(|decorator| decorator.path.as_slice() == ["rust", "extern"]) =>
                {
                    Some(function)
                }
                _ => None,
            })
    }

    /// Record the native attribute macros a function's decorators apply (#2256).
    ///
    /// A decorator that resolves to a provider function whose published facts name an exported attribute macro is not
    /// a call: rustc applies the macro to the declaration. The selection rests on that published fact alone, never on
    /// the decorator's spelling. Arguments are retained as written literals; any other expression is refused, because
    /// an attribute macro receives tokens and the checker will not guess an evaluation for them.
    pub(in crate::typechecker) fn record_native_attribute_applications(
        &mut self,
        decorators: &[Spanned<Decorator>],
        declaration: Span,
    ) {
        let mut applications = Vec::new();
        for decorator in decorators {
            let resolved = resolve_decorator_path(&decorator.node, &self.symbols);
            let Some(path) = self
                .provider_decorator_function(&resolved)
                .and_then(|function| function.native_attribute.clone())
            else {
                continue;
            };
            let mut arguments = Vec::new();
            for argument in &decorator.node.args {
                match self.native_attribute_argument(argument) {
                    Ok(argument) => arguments.push(argument),
                    Err(refusal) => {
                        self.errors.push(refusal.error(&path, decorator.span));
                        return;
                    }
                }
            }
            applications.push(NativeAttribute { path, arguments });
        }
        if !applications.is_empty() {
            self.type_info
                .declarations
                .native_attribute_applications
                .insert(declaration, applications);
        }
    }

    /// One written decorator argument as the literal value an attribute macro receives.
    pub(super) fn native_attribute_argument(
        &self,
        argument: &DecoratorArg,
    ) -> Result<NativeAttributeArgument, NativeAttributeRefusal> {
        let (name, value) = match argument {
            DecoratorArg::Positional(value) => (None, value),
            DecoratorArg::Named(name, DecoratorArgValue::Expr(value)) => (Some(name.clone()), value),
            DecoratorArg::Named(_, DecoratorArgValue::Type(_)) => return Err(NativeAttributeRefusal::Expression),
        };
        Ok(NativeAttributeArgument {
            name,
            value: self.native_attribute_value(&value.node)?,
        })
    }

    /// A literal, a scalar constant's value, a name (see [`Self::native_attribute_name`]), or a list of them.
    pub(super) fn native_attribute_value(
        &self,
        expression: &Expr,
    ) -> Result<NativeAttributeValue, NativeAttributeRefusal> {
        match expression {
            Expr::Literal(Literal::String(text)) => Ok(NativeAttributeValue::Text(text.clone())),
            Expr::Literal(Literal::Int(value)) => Ok(NativeAttributeValue::Int(value.value)),
            Expr::Literal(Literal::Float(value)) => Ok(NativeAttributeValue::Float(value.value)),
            Expr::Literal(Literal::Bool(value)) => Ok(NativeAttributeValue::Bool(*value)),
            Expr::Ident(name) => self.native_attribute_name(name),
            Expr::List(entries) => entries
                .iter()
                .map(|entry| match entry {
                    ListEntry::Element(value) => self.native_attribute_value(&value.node),
                    _ => Err(NativeAttributeRefusal::Expression),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(NativeAttributeValue::List),
            _ => Err(NativeAttributeRefusal::Expression),
        }
    }

    /// One written name as an attribute macro value.
    ///
    /// A constant with a scalar value gives that value. A name no value binds stands for its own spelling, the one a
    /// hand-written attribute uses for a method set such as `methods=[GET]`; so does a scalar-typed value whose
    /// constant value this module does not hold, such as an imported `GET`. A name bound to any other value, such as
    /// a list constant, has no literal form and is refused rather than passed on as its own name.
    pub(super) fn native_attribute_name(&self, name: &str) -> Result<NativeAttributeValue, NativeAttributeRefusal> {
        match self.type_info.const_value(name) {
            Some(ConstValue::FrozenStr(text)) => return Ok(NativeAttributeValue::Text(text.clone())),
            Some(ConstValue::Int(value)) => return Ok(NativeAttributeValue::Int(*value)),
            Some(ConstValue::Float(value)) => return Ok(NativeAttributeValue::Float(*value)),
            Some(ConstValue::Bool(value)) => return Ok(NativeAttributeValue::Bool(*value)),
            Some(ConstValue::FrozenBytes(_)) => return Err(NativeAttributeRefusal::NonScalarBinding(name.to_owned())),
            None => {}
        }
        let bound_type = self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
            SymbolKind::Variable(info) => Some(&info.ty),
            SymbolKind::Static(info) => Some(&info.ty),
            _ => None,
        });
        match bound_type {
            Some(ty) if !is_scalar_attribute_type(ty) => Err(NativeAttributeRefusal::NonScalarBinding(name.to_owned())),
            _ => Ok(NativeAttributeValue::Text(name.to_owned())),
        }
    }

    /// Validate decorator paths for declarations that allow user-defined decorator candidates.
    pub fn validate_decorators_allowing_user_defined(&mut self, decorators: &[Spanned<Decorator>]) {
        self.validate_decorators_for_target(decorators, DecoratorValidationTarget::AllowsUserDefined);
    }

    /// Validate decorator paths for declarations that do not allow user-defined decorators.
    pub fn validate_decorators_rejecting_user_defined(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        self.validate_decorators_for_target(decorators, DecoratorValidationTarget::RejectsUserDefined(kind));
    }

    /// Validate decorator paths, preserving compiler-owned decorator diagnostics while deciding whether unknown
    /// non-compiler decorators are accepted as user-defined candidates or rejected for this target.
    ///
    /// When a decorator doesn't resolve to a known `DecoratorId`, the error message is contextual:
    /// - If the leading segment is a known namespace (e.g. `rust`, `std`), the error mentions the namespace and lists
    ///   available decorators within it.
    /// - Otherwise, supported function-like targets keep it for RFC 036 typechecking, while unsupported targets emit a
    ///   user-defined decorator target diagnostic.
    pub(super) fn validate_decorators_for_target(
        &mut self,
        decorators: &[Spanned<Decorator>],
        target: DecoratorValidationTarget,
    ) {
        for dec in decorators {
            let resolved = resolve_decorator_path(&dec.node, &self.symbols);
            let feature = self.surface_context.decorator_feature_for_path(&resolved);

            let Some(id) = decorators::from_segments(&resolved) else {
                let is_stdlib_decorator_function = feature
                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
                    && resolved.len() >= 3
                    && self.provider_declares_decorator_function(&resolved);
                if is_stdlib_decorator_function {
                    continue;
                }

                let path = if resolved.is_empty() {
                    dec.node.name.clone()
                } else {
                    resolved.join(".")
                };

                // ---- Namespace-aware error (e.g. "@rust.blah" → "unknown in `rust` namespace") ----
                if let Some(first) = resolved.first()
                    && decorators::is_known_decorator_namespace(first)
                {
                    let known = decorators::decorators_in_namespace(first);
                    let known_display: Vec<_> = known.iter().map(|d| format!("@{d}")).collect();
                    let hint = if known_display.is_empty() {
                        format!("No decorators are currently defined in the `{first}` namespace")
                    } else {
                        format!("Known `{first}` decorators: {}", known_display.join(", "))
                    };
                    self.errors
                        .push(errors::unknown_decorator(&path, dec.span).with_hint(&hint));
                } else if let DecoratorValidationTarget::RejectsUserDefined(kind) = target {
                    self.errors
                        .push(errors::user_defined_decorator_unsupported_target(&path, kind, dec.span));
                } else {
                    continue;
                }
                continue;
            };

            self.type_info.record_resolved_identity(
                dec.span,
                CanonicalSymbolId {
                    namespace: SymbolNamespace::OrdinaryLexical,
                    origin: SymbolOrigin::Builtin,
                    declaration_name: decorators::as_str(id).to_string(),
                    kind: SemanticSourceTargetKind::Builtin,
                    scope_discriminant: None,
                    declaration_span: HirSourceSpan::new(0, 0),
                },
            );

            if id == DecoratorId::RustAllow {
                self.validate_rust_allow_args(dec);
            }
        }
    }

    /// Validate RFC 057 `@rust.allow(...)` arguments.
    ///
    /// The decorator is intentionally item-scoped and accepts only explicit lint paths so generated code can emit
    /// targeted `#[allow(...)]` attributes without introducing broad crate- or module-level suppression.
    pub fn validate_rust_allow_args(&mut self, dec: &Spanned<Decorator>) {
        let mut seen = HashSet::new();
        let mut positional_count = 0usize;

        for arg in &dec.node.args {
            match arg {
                DecoratorArg::Positional(expr) => {
                    positional_count += 1;
                    let Expr::Literal(Literal::String(name)) = &expr.node else {
                        self.errors
                            .push(errors::rust_allow_requires_positional_string(expr.span));
                        continue;
                    };
                    self.validate_single_rust_allow_lint(name, expr.span, &mut seen);
                }
                DecoratorArg::Named(name, _) => {
                    self.errors.push(errors::rust_allow_rejects_named_args(name, dec.span));
                }
            }
        }

        if positional_count == 0 {
            self.errors
                .push(errors::rust_allow_requires_positional_string(dec.span));
        }
    }

    /// Validate direct RFC 043 `@rust.derive(...)` passthrough on concrete type declarations.
    pub fn validate_rust_derives(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
        is_rusttype: bool,
        traits: &[Spanned<TraitBound>],
    ) {
        let rust_derives: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::RustDerive).collect();
        if rust_derives.is_empty() {
            return;
        }

        if kind != "model" && kind != "class" && kind != "enum" && kind != "newtype" {
            for dec in rust_derives {
                self.errors
                    .push(errors::rust_derive_unsupported_attachment(kind, dec.span));
            }
            return;
        }

        if is_rusttype {
            for dec in rust_derives {
                self.errors.push(errors::rust_derive_unsupported_rusttype(dec.span));
            }
            return;
        }

        for dec in rust_derives {
            let mut positional_count = 0usize;
            for arg in &dec.node.args {
                match arg {
                    DecoratorArg::Positional(expr) => {
                        positional_count += 1;
                        self.validate_single_rust_derive_arg(&expr.node, expr.span, traits);
                    }
                    DecoratorArg::Named(name, _) => {
                        self.errors.push(errors::rust_derive_rejects_named_args(name, dec.span));
                    }
                }
            }
            if positional_count == 0 {
                self.errors.push(errors::rust_derive_requires_positional_arg(dec.span));
            }
        }
    }

    /// Validate one positional `@rust.derive(...)` argument.
    pub(super) fn validate_single_rust_derive_arg(&mut self, expr: &Expr, span: Span, traits: &[Spanned<TraitBound>]) {
        match expr {
            Expr::Ident(name) => {
                let leaf = self
                    .rust_derive_leaf_for_ident(name)
                    .unwrap_or(name.as_str())
                    .to_string();
                self.validate_rust_derive_trait_conflict(&leaf, traits, span);
                if Self::is_builtin_rust_derive(&leaf) || self.rust_import_path_for_local_name(name).is_some() {
                    return;
                }
                self.errors.push(errors::rust_derive_unresolved(name, span));
            }
            Expr::Literal(Literal::String(path)) => {
                let Some(leaf) = Self::rust_path_leaf(path) else {
                    self.errors.push(errors::rust_derive_invalid_arg(span));
                    return;
                };
                self.validate_rust_derive_trait_conflict(leaf, traits, span);
                if Self::is_builtin_rust_derive(leaf) && !path.contains("::") {
                    return;
                }
                if self.rust_derive_path_has_declared_crate(path) {
                    return;
                }
                self.errors.push(errors::rust_derive_unresolved(path, span));
            }
            _ => self.errors.push(errors::rust_derive_invalid_arg(span)),
        }
    }

    /// Reject derive names that would duplicate an explicit trait adoption.
    pub(super) fn validate_rust_derive_trait_conflict(
        &mut self,
        derive_leaf: &str,
        traits: &[Spanned<TraitBound>],
        span: Span,
    ) {
        for trait_ref in traits {
            let trait_leaf = self
                .rust_derive_leaf_for_ident(&trait_ref.node.name)
                .unwrap_or_else(|| Self::trait_name_leaf(&trait_ref.node.name));
            if trait_leaf == derive_leaf {
                self.errors.push(errors::rust_derive_conflicts_with_trait_adoption(
                    derive_leaf,
                    &trait_ref.node.name,
                    span,
                ));
            }
        }
    }

    /// Resolve an imported derive binding to its final path segment.
    pub(super) fn rust_derive_leaf_for_ident(&self, name: &str) -> Option<&str> {
        self.import_binding_path(name)
            .and_then(|segments| segments.last().map(String::as_str))
            .or_else(|| {
                self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
                    SymbolKind::RustItem(info) => info.path.rsplit("::").next(),
                    _ => None,
                })
            })
    }

    /// Return the Rust path imported for a local derive binding name.
    pub(in crate::typechecker) fn rust_import_path_for_local_name(&self, name: &str) -> Option<String> {
        self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
            SymbolKind::RustItem(info) => Some(info.path.clone()),
            _ => None,
        })
    }

    /// Return whether a string Rust derive path names a crate available to generated Rust.
    pub(super) fn rust_derive_path_has_declared_crate(&self, path: &str) -> bool {
        let segments: Vec<_> = path.split("::").collect();
        if segments.is_empty() || !segments.iter().all(|segment| Self::is_valid_rust_path_segment(segment)) {
            return false;
        }
        let Some(crate_name) = segments.first() else {
            return false;
        };
        if matches!(*crate_name, "std" | "core" | "alloc") {
            return true;
        }
        self.declared_crate_names
            .as_ref()
            .is_some_and(|declared| declared.contains(*crate_name))
    }

    /// Return the leaf segment for a syntactically valid Rust path string.
    pub(super) fn rust_path_leaf(path: &str) -> Option<&str> {
        if path.split("::").all(Self::is_valid_rust_path_segment) {
            return path.rsplit("::").next();
        }
        None
    }

    /// Return whether one Rust path segment is acceptable in a derive path string.
    pub(super) fn is_valid_rust_path_segment(segment: &str) -> bool {
        let segment = segment.strip_prefix("r#").unwrap_or(segment);
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return the final source trait segment for conflict comparisons.
    pub(super) fn trait_name_leaf(name: &str) -> &str {
        name.rsplit('.').next().unwrap_or(name)
    }

    /// Return whether a derive name is built into Rust and needs no dependency metadata.
    pub(super) fn is_builtin_rust_derive(name: &str) -> bool {
        matches!(
            derives::from_str(name),
            Some(
                derives::DeriveId::Clone
                    | derives::DeriveId::Copy
                    | derives::DeriveId::Debug
                    | derives::DeriveId::Default
                    | derives::DeriveId::Eq
                    | derives::DeriveId::Hash
                    | derives::DeriveId::Ord
                    | derives::DeriveId::PartialEq
                    | derives::DeriveId::PartialOrd
            )
        )
    }

    /// Reject RFC 057 `@rust.allow(...)` on declarations that do not own a supported Rust item boundary.
    ///
    /// Parser syntax allows decorators on several declaration forms. This helper keeps the semantic support matrix
    /// explicit so adding a new declaration kind does not silently inherit Rust lint suppression behavior.
    pub fn reject_rust_allow_on_unsupported_declaration(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        for dec in decorators {
            if self.decorator_id(&dec.node) == Some(DecoratorId::RustAllow) {
                self.errors
                    .push(errors::rust_allow_unsupported_attachment(kind, dec.span));
            }
        }
    }

    /// Return whether this decorator should be handled as an RFC 036 user-defined decorator candidate.
    ///
    /// Compiler-owned decorators and stdlib marker decorators keep their existing compiler semantics. Unknown paths in
    /// known compiler namespaces stay diagnostic-only rather than becoming user-defined decorators.
    pub fn is_user_defined_decorator_candidate(&mut self, dec: &Decorator) -> bool {
        if self.decorator_id(dec).is_some() {
            return false;
        }

        let resolved = decorator_resolution::resolve_decorator_path(dec, &self.symbols);
        if resolved
            .first()
            .is_some_and(|first| decorators::is_known_decorator_namespace(first))
        {
            return false;
        }

        let feature = self.surface_context.decorator_feature_for_path(&resolved);
        let is_stdlib_decorator_function = feature
            == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
            && resolved.len() >= 3
            && self.provider_declares_decorator_function(&resolved);
        !is_stdlib_decorator_function
    }

    /// Resolve a decorator identifier through checked source bindings.
    pub fn decorator_id(&self, dec: &Decorator) -> Option<DecoratorId> {
        let resolved = resolve_decorator_path(dec, &self.symbols);
        decorators::from_segments(&resolved)
    }

    /// Validate one lint name passed to `@rust.allow`.
    pub(super) fn validate_single_rust_allow_lint(&mut self, name: &str, span: Span, seen: &mut HashSet<String>) {
        if name.is_empty() || name.trim() != name || !Self::is_valid_rust_lint_path(name) {
            self.errors.push(errors::rust_allow_invalid_lint_name(name, span));
            return;
        }

        if Self::is_broad_rust_lint_group(name) {
            self.errors.push(errors::rust_allow_broad_lint_group(name, span));
            return;
        }

        if !seen.insert(name.to_string()) {
            self.errors.push(errors::rust_allow_duplicate_lint(name, span));
        }
    }

    /// Return whether a Rust lint path has valid syntax.
    pub(super) fn is_valid_rust_lint_path(name: &str) -> bool {
        name.split("::").all(Self::is_valid_rust_lint_segment)
    }

    /// Return whether one Rust lint path segment is valid.
    pub(super) fn is_valid_rust_lint_segment(segment: &str) -> bool {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return whether a Rust lint group is too broad for `@rust.allow`.
    pub(super) fn is_broad_rust_lint_group(name: &str) -> bool {
        matches!(
            name,
            "warnings"
                | "unused"
                | "clippy::all"
                | "clippy::pedantic"
                | "clippy::nursery"
                | "clippy::restriction"
                | "clippy::cargo"
        )
    }
}
