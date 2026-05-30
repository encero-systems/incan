//! Decorator resolution and validation helpers for the first pass.
//!
//! This keeps decorator path resolution and validation logic out of the main collection flow while preserving RFC 022
//! semantics.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, SymbolKind, SymbolTable, TypeInfo};
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives;
use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

#[derive(Clone, Copy)]
enum DecoratorValidationTarget {
    AllowsUserDefined,
    RejectsUserDefined(&'static str),
}

/// Resolve a decorator path to a module path.
pub(in crate::frontend::typechecker) fn resolve_decorator_path(dec: &Decorator, symbols: &SymbolTable) -> Vec<String> {
    decorator_resolution::resolve_decorator_path(dec, symbols)
}

/// Resolve a decorator path to a decorator id.
pub(in crate::frontend::typechecker) fn resolve_decorator_id(
    dec: &Decorator,
    symbols: &SymbolTable,
) -> Option<DecoratorId> {
    let resolved = resolve_decorator_path(dec, symbols);
    decorators::from_segments(&resolved)
}

/// Find decorators by name.
pub(super) fn decorators_named<'a>(
    decorators: &'a [Spanned<Decorator>],
    symbols: &SymbolTable,
    id: DecoratorId,
) -> impl Iterator<Item = &'a Spanned<Decorator>> {
    decorators
        .iter()
        .filter(move |d| resolve_decorator_id(&d.node, symbols) == Some(id))
}

/// Extract positional identifier names from decorator arguments.
pub(super) fn positional_idents(args: &[DecoratorArg]) -> impl Iterator<Item = (&str, Span)> + '_ {
    args.iter().filter_map(|arg| match arg {
        DecoratorArg::Positional(expr) => {
            if let Expr::Ident(name) = &expr.node {
                Some((name.as_str(), expr.span))
            } else {
                None
            }
        }
        _ => None,
    })
}

impl TypeChecker {
    /// Validate decorator paths for declarations that allow user-defined decorator candidates.
    pub(crate) fn validate_decorators_allowing_user_defined(&mut self, decorators: &[Spanned<Decorator>]) {
        self.validate_decorators_for_target(decorators, DecoratorValidationTarget::AllowsUserDefined);
    }

    /// Validate decorator paths for declarations that do not allow user-defined decorators.
    pub(crate) fn validate_decorators_rejecting_user_defined(
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
    fn validate_decorators_for_target(&mut self, decorators: &[Spanned<Decorator>], target: DecoratorValidationTarget) {
        for dec in decorators {
            let mut resolved = resolve_decorator_path(&dec.node, &self.symbols);
            let mut feature = self.surface_context.decorator_feature_for_path(&resolved);

            // ---- Fallback: import-alias resolution ----
            // The SymbolTable-based `DecoratorPrefixLookup` only handles Module symbols, so decorators imported as
            // functions (e.g. `from std.testing import parametrize` then `@parametrize(...)`) won't resolve via the
            // symbol table. Fall back to the import aliases collected from the program's `import` / `from ... import`
            // declarations, which correctly map `parametrize` → `["std", "testing", "parametrize"]`.
            if feature.is_none() && decorators::from_segments(&resolved).is_none() {
                let alias_resolved = decorator_resolution::resolve_decorator_path(&dec.node, &self.import_aliases);
                if alias_resolved != resolved {
                    resolved = alias_resolved;
                    feature = self.surface_context.decorator_feature_for_path(&resolved);
                }
            }

            let Some(id) = decorators::from_segments(&resolved) else {
                let is_stdlib_decorator_function = feature
                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
                    && resolved.len() >= 3
                    && self
                        .stdlib_cache
                        .lookup_function_meta(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
                        .is_some_and(|f| f.is_rust_extern && f.rust_module_path.is_some());
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

            if id == DecoratorId::RustAllow {
                self.validate_rust_allow_args(dec);
            }
        }
    }

    /// Validate RFC 057 `@rust.allow(...)` arguments.
    ///
    /// The decorator is intentionally item-scoped and accepts only explicit lint paths so generated code can emit
    /// targeted `#[allow(...)]` attributes without introducing broad crate- or module-level suppression.
    pub(crate) fn validate_rust_allow_args(&mut self, dec: &Spanned<Decorator>) {
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
    pub(crate) fn validate_rust_derives(
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
    fn validate_single_rust_derive_arg(&mut self, expr: &Expr, span: Span, traits: &[Spanned<TraitBound>]) {
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
    fn validate_rust_derive_trait_conflict(&mut self, derive_leaf: &str, traits: &[Spanned<TraitBound>], span: Span) {
        for trait_ref in traits {
            let trait_leaf = self
                .import_aliases
                .get(&trait_ref.node.name)
                .and_then(|segments| segments.last().map(String::as_str))
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
    fn rust_derive_leaf_for_ident(&self, name: &str) -> Option<&str> {
        self.import_aliases
            .get(name)
            .and_then(|segments| segments.last().map(String::as_str))
    }

    /// Return the Rust path imported for a local derive binding name.
    fn rust_import_path_for_local_name(&self, name: &str) -> Option<String> {
        if let Some(segments) = self.import_aliases.get(name)
            && segments.first().is_some_and(|segment| segment == "rust")
            && segments.len() >= 2
        {
            return Some(segments[1..].join("::"));
        }
        self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
            SymbolKind::RustItem(info) => Some(info.path.clone()),
            _ => None,
        })
    }

    /// Return whether a string Rust derive path names a crate available to generated Rust.
    fn rust_derive_path_has_declared_crate(&self, path: &str) -> bool {
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
    fn rust_path_leaf(path: &str) -> Option<&str> {
        if path.split("::").all(Self::is_valid_rust_path_segment) {
            return path.rsplit("::").next();
        }
        None
    }

    /// Return whether one Rust path segment is acceptable in a derive path string.
    fn is_valid_rust_path_segment(segment: &str) -> bool {
        let segment = segment.strip_prefix("r#").unwrap_or(segment);
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return the final source trait segment for conflict comparisons.
    fn trait_name_leaf(name: &str) -> &str {
        name.rsplit('.').next().unwrap_or(name)
    }

    /// Return whether a derive name is built into Rust and needs no dependency metadata.
    fn is_builtin_rust_derive(name: &str) -> bool {
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
    pub(crate) fn reject_rust_allow_on_unsupported_declaration(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        for dec in decorators {
            if self.decorator_id_with_import_aliases(&dec.node) == Some(DecoratorId::RustAllow) {
                self.errors
                    .push(errors::rust_allow_unsupported_attachment(kind, dec.span));
            }
        }
    }

    /// Return whether this decorator should be handled as an RFC 036 user-defined decorator candidate.
    ///
    /// Compiler-owned decorators and stdlib marker decorators keep their existing compiler semantics. Unknown paths in
    /// known compiler namespaces stay diagnostic-only rather than becoming user-defined decorators.
    pub(crate) fn is_user_defined_decorator_candidate(&mut self, dec: &Decorator) -> bool {
        if self.decorator_id_with_import_aliases(dec).is_some() {
            return false;
        }

        let resolved = decorator_resolution::resolve_decorator_path(dec, &self.import_aliases);
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
            && self
                .stdlib_cache
                .lookup_function_meta(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
                .is_some_and(|f| f.is_rust_extern && f.rust_module_path.is_some());
        !is_stdlib_decorator_function
    }

    /// Resolve a decorator identifier through import aliases.
    fn decorator_id_with_import_aliases(&self, dec: &Decorator) -> Option<DecoratorId> {
        let resolved = resolve_decorator_path(dec, &self.symbols);
        if let Some(id) = decorators::from_segments(&resolved) {
            return Some(id);
        }

        let alias_resolved = decorator_resolution::resolve_decorator_path(dec, &self.import_aliases);
        decorators::from_segments(&alias_resolved)
    }

    /// Validate one lint name passed to `@rust.allow`.
    fn validate_single_rust_allow_lint(&mut self, name: &str, span: Span, seen: &mut HashSet<String>) {
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
    fn is_valid_rust_lint_path(name: &str) -> bool {
        name.split("::").all(Self::is_valid_rust_lint_segment)
    }

    /// Return whether one Rust lint path segment is valid.
    fn is_valid_rust_lint_segment(segment: &str) -> bool {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    /// Return whether a Rust lint group is too broad for `@rust.allow`.
    fn is_broad_rust_lint_group(name: &str) -> bool {
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

    /// Validate @derive decorator arguments and report errors for unknown derives.
    pub(crate) fn validate_derives(&mut self, decorators: &[Spanned<Decorator>]) {
        let derive_items: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| {
                dec.node.args.iter().filter_map(|arg| match arg {
                    DecoratorArg::Positional(expr) => {
                        if let Expr::Ident(name) = &expr.node {
                            Some((name.clone(), expr.span))
                        } else {
                            None
                        }
                    }
                    DecoratorArg::Named(name, _) => {
                        // Named args not valid for derive, but report error on them.
                        Some((name.clone(), dec.span))
                    }
                })
            })
            .collect();

        for (name, span) in derive_items {
            self.validate_single_derive(&name, span);
        }
    }

    /// Extract derive names from @derive decorators.
    pub(crate) fn extract_derive_names(&self, decorators: &[Spanned<Decorator>]) -> Vec<String> {
        decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| positional_idents(&dec.node.args))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Extract `@requires` constraints from decorators as `(name, type)` pairs.
    pub(super) fn extract_requires(&mut self, decorators: &[Spanned<Decorator>]) -> Vec<(String, ResolvedType)> {
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
    fn validate_single_derive(&mut self, name: &str, span: Span) {
        if derives::from_str(name).is_some() {
            return;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        {
            return;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Module(_)))
            && let Some(module_path) = self.module_path_for_imported_name(name)
        {
            if self.lookup_derivable_traits(&module_path).is_some() {
                return;
            }
            self.errors.push(errors::derive_module_missing_derives(name, span));
            return;
        }

        if let Some((canonical, info)) = self.resolve_qualified_trait(name) {
            self.define_hidden_trait_symbol(&canonical, info, span);
            return;
        }

        // Allow custom derives imported from stdlib modules backed by rust.module(...).
        let resolved = self
            .import_aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![name.to_string()]);
        if resolved.len() >= 2
            && self.imported_trait_is_derivable(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
        {
            return;
        }

        // Check if the name refers to a type/function (wrong usage)
        if let Some(kind_name) = self.lookup_symbol_kind(name) {
            self.errors.push(errors::derive_wrong_kind(name, kind_name, span));
        } else {
            self.errors.push(errors::unknown_derive(name, span));
        }
    }

    /// Look up what kind of symbol a name refers to, if any.
    fn lookup_symbol_kind(&self, name: &str) -> Option<&'static str> {
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
