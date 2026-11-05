//! Derive validation, derive argument identities, Rust derive paths and `requires` extraction.

use super::*;

impl TypeChecker {
    /// Validate @derive decorator arguments and report errors for unknown derives.
    pub fn validate_derives(&mut self, decorators: &[Spanned<Decorator>]) {
        let derive_items: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| {
                dec.node.args.iter().filter_map(|arg| match arg {
                    DecoratorArg::Positional(expr) => {
                        decorator_resolution::derive_argument_name(&expr.node).map(|name| (name, expr.span))
                    }
                    DecoratorArg::Named(name, _) => {
                        // Named args not valid for derive, but report error on them.
                        Some((name.clone(), dec.span))
                    }
                })
            })
            .collect();

        for (name, span) in derive_items {
            if self.validate_single_derive(&name, span) {
                self.record_derive_argument_identity(&name, span);
            }
        }
    }

    /// Record the compiler-proven target selected by one accepted `@derive(...)` argument.
    pub(super) fn record_derive_argument_identity(&mut self, name: &str, span: Span) {
        let identity = derives::from_str(name)
            .and_then(|id| {
                let canonical = derives::as_str(id);
                self.symbols
                    .lookup(canonical)
                    .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
                    .cloned()
                    .or_else(|| {
                        Some(CanonicalSymbolId {
                            namespace: SymbolNamespace::OrdinaryLexical,
                            origin: SymbolOrigin::Builtin,
                            declaration_name: canonical.to_string(),
                            kind: SemanticSourceTargetKind::Builtin,
                            scope_discriminant: None,
                            declaration_span: HirSourceSpan::new(0, 0),
                        })
                    })
            })
            .or_else(|| {
                self.symbols
                    .lookup(name)
                    .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
                    .cloned()
            });
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
    }

    /// Extract derive names from @derive decorators.
    pub fn extract_derive_names(&self, decorators: &[Spanned<Decorator>]) -> Vec<String> {
        decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| positional_derive_names(&dec.node.args))
            .map(|(name, _)| name)
            .collect()
    }

    /// Record canonical Rust derive paths applied directly to one local concrete type.
    ///
    /// Unlike `@derive(...)`, RFC 043 `@rust.derive(...)` is emitted verbatim and does not adopt an Incan trait.
    /// Its inspected probe expansion can select a candidate Rust generic ABI, so retain the exact resolved macro
    /// namespace without mixing it into [`TypeInfo`](crate::symbols::TypeInfo) derive names. Native rustc
    /// compilation remains authoritative for the real generated declaration.
    ///
    /// Every local nominal type also gets its [`LocalDeriveFacts`] here, which the derive relation reads: the builtin
    /// derives `@rust.derive(...)` spells (`Eq`, `"std::hash::Hash"`), and whether it names a derive macro the compiler
    /// cannot classify (#1754, #1758).
    pub fn record_local_rust_derive_paths(&mut self, type_name: &str, decorators: &[Spanned<Decorator>]) {
        let mut paths = Vec::new();
        let mut facts = LocalDeriveFacts::default();
        for decorator in decorators_named(decorators, &self.symbols, DecoratorId::RustDerive) {
            for argument in &decorator.node.args {
                let DecoratorArg::Positional(expression) = argument else {
                    continue;
                };
                let path = match &expression.node {
                    Expr::Ident(name) => self.rust_import_path_for_local_name(name),
                    Expr::Literal(Literal::String(path)) if self.rust_derive_path_has_declared_crate(path) => {
                        Some(path.clone())
                    }
                    _ => None,
                };
                match self.classify_rust_derive_arg(&expression.node) {
                    Some(derive) => {
                        if !facts.rust_builtin_derives.contains(&derive) {
                            facts.rust_builtin_derives.push(derive);
                        }
                    }
                    None if path.as_deref().is_some_and(|path| {
                        matches!(path, "serde::Serialize" | "serde::Deserialize")
                            || self.checked_native_derive_macro(path).is_some()
                    }) => {}
                    None => facts.has_unclassified_rust_derive = true,
                }
                if let Some(path) = path
                    && !paths.contains(&path)
                {
                    paths.push(path);
                }
            }
        }
        self.local_derive_facts.insert(type_name.to_string(), facts);
        if paths.is_empty() {
            self.local_rust_derive_paths.remove(type_name);
        } else {
            self.local_rust_derive_paths.insert(type_name.to_string(), paths);
        }
    }

    /// Return the builtin derive a `@rust.derive(...)` argument names, or `None` for any other derive macro.
    ///
    /// A bare builtin name (`Hash`) or string (`"Hash"`) is Rust's own derive, as is a `std::`/`core::` path whose leaf
    /// is one (`"std::hash::Hash"`); an identifier bound to such a Rust import counts the same way.
    pub(super) fn classify_rust_derive_arg(&self, expr: &Expr) -> Option<derives::DeriveId> {
        let (path, leaf) = match expr {
            Expr::Ident(name) => (
                self.rust_import_path_for_local_name(name),
                self.rust_derive_leaf_for_ident(name)
                    .unwrap_or(name.as_str())
                    .to_string(),
            ),
            Expr::Literal(Literal::String(path)) => (
                path.contains("::").then(|| path.clone()),
                Self::rust_path_leaf(path)?.to_string(),
            ),
            _ => return None,
        };
        Self::builtin_rust_derive_path(path.as_deref().unwrap_or(&leaf))
    }

    /// Classify a bare builtin or an exact standard Rust derive path without confusing a dependency's same-named macro.
    pub(in crate::typechecker) fn builtin_rust_derive_path(path: &str) -> Option<derives::DeriveId> {
        let path = path.strip_prefix("rust::").unwrap_or(path);
        if path.contains("::") && !path.starts_with("std::") && !path.starts_with("core::") {
            return None;
        }
        let leaf = path.rsplit("::").next()?;
        Self::is_builtin_rust_derive(leaf)
            .then(|| derives::from_str(leaf))
            .flatten()
    }

    /// Extract `@requires` constraints from decorators as `(name, type)` pairs.
    pub(in crate::typechecker::collect) fn extract_requires(
        &mut self,
        decorators: &[Spanned<Decorator>],
    ) -> Vec<(String, ResolvedType)> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut requires: Vec<(String, ResolvedType)> = Vec::new();

        for dec in decorators {
            if resolve_decorator_id(&dec.node, &self.symbols) != Some(DecoratorId::Requires) {
                continue;
            }
            for arg in &dec.node.args {
                if let DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) = arg {
                    if !seen.insert(name.clone()) {
                        self.errors.push(errors::duplicate_trait_requires_field(name, ty.span));
                        continue;
                    }
                    requires.push((name.clone(), self.resolve_type_checked(ty)));
                }
            }
        }
        requires
    }

    /// Validate a single derive name, reporting appropriate errors.
    pub(super) fn validate_single_derive(&mut self, name: &str, span: Span) -> bool {
        if derives::from_str(name).is_some() {
            return true;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        {
            return true;
        }

        // A qualified trait is a member of its module namespace, not a request for every derive published by the
        // module. Resolve it before the module-level `__derives__` shorthand so `json.Serialize` keeps its exact
        // trait identity while bare `json` still expands the module's declared derive set.
        let resolved = self.derive_trait_path(name);
        if resolved.len() >= 2
            && self.imported_trait_is_derivable(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
        {
            if let Some((canonical, info)) = self.resolve_qualified_trait(name) {
                self.define_hidden_trait_symbol(&canonical, info, span);
            }
            return true;
        }

        if let Some(module_path) = self.module_path_for_imported_name(name) {
            if self.lookup_derivable_traits(&module_path).is_some() {
                return true;
            }
            self.errors.push(errors::derive_module_missing_derives(name, span));
            return false;
        }

        // A module named through another module (`serde.json` after `from std import serde`) derives its traits as
        // the module imported by name does.
        if name.contains('.')
            && let Some(module_path) = self.module_path_for_imported_name(name)
            && self.lookup_derivable_traits(&module_path).is_some()
        {
            return true;
        }

        // Check if the name refers to a type/function (wrong usage)
        if let Some(kind_name) = self.lookup_symbol_kind(name) {
            self.errors.push(errors::derive_wrong_kind(name, kind_name, span));
        } else {
            self.errors.push(errors::unknown_derive(name, span));
        }
        false
    }

    /// Look up what kind of symbol a name refers to, if any.
    pub(super) fn lookup_symbol_kind(&self, name: &str) -> Option<&'static str> {
        let sym_id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(sym_id)?;

        match &sym.kind {
            SymbolKind::Type(TypeInfo::Model(_)) => Some("model"),
            SymbolKind::Type(TypeInfo::Class(_)) => Some("class"),
            SymbolKind::Type(TypeInfo::Enum(_)) => Some("enum"),
            SymbolKind::Function(_) => Some("function"),
            _ => None,
        }
    }
}
