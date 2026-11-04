//! F-string parts, field visibility, and imported module function and constant members.

use super::*;

impl TypeChecker {
    /// Type-check every interpolated expression of an f-string and refuse a `{value}` part with no printed form
    /// (#1748) or an unsupported format specifier.
    ///
    /// A `{value}` part displays under the rule `print`/`println` arguments and `str(...)` share (see
    /// [`Self::check_display_operand`]); an `Error` adopter with no `__str__` is recorded to render its `message()`
    /// (#1778). A `{value:?}` part asks for the value's structure through `Debug`, and a value with none is refused the
    /// same way (see [`Self::check_debug_operand`]). A part with any other format spec (`{value:x}`) names a specifier
    /// the language does not support and is refused outright.
    pub(super) fn check_fstring_parts(&mut self, parts: &[FStringPart]) {
        for part in parts {
            let FStringPart::Expr { expr, format } = part else {
                continue;
            };
            let ty = self.check_expr(expr);
            match format {
                FStringFormat::Display => {
                    self.check_display_operand(errors::DisplayPosition::Interpolation, expr, &ty);
                }
                FStringFormat::Debug => self.check_debug_operand(expr, &ty),
                FStringFormat::Unsupported(spec) => self
                    .errors
                    .push(errors::unsupported_fstring_format_specifier(spec, expr.span)),
            }
        }
    }

    /// Resolve a field by canonical name or alias, returning the canonical name and FieldInfo.
    ///
    /// - `allow_alias`: whether alias lookup is allowed (models only).
    /// - `allow_numeric_alias`: whether numeric spellings like `"1"` may match aliases.
    pub(super) fn resolve_field_info<'a>(
        &self,
        fields: &'a HashMap<String, FieldInfo>,
        field_name: &str,
        allow_alias: bool,
        allow_numeric_alias: bool,
    ) -> Option<(String, &'a FieldInfo)> {
        if let Some(info) = fields.get(field_name) {
            return Some((field_name.to_string(), info));
        }
        if !allow_alias {
            return None;
        }
        if !allow_numeric_alias && field_name.parse::<usize>().is_ok() {
            return None;
        }
        fields
            .iter()
            .find(|(_, info)| info.alias.as_deref() == Some(field_name))
            .map(|(name, info)| (name.clone(), info))
    }

    /// Keep type-private field access within the declaring type and module, by the rule methods share
    /// ([`Self::type_private_member_is_inaccessible`]).
    pub(super) fn private_field_is_inaccessible(&self, type_name: &str, field: &FieldInfo) -> bool {
        if !field.is_type_private {
            return false;
        }
        let owner = field.owner.as_deref().unwrap_or(type_name);
        self.type_private_member_is_inaccessible(owner, field.identity.as_ref())
    }

    /// Report a private field under the source name of the class or model that declares it. The receiver's type
    /// may name the declaration's generated carrier, which no diagnostic shows.
    pub(super) fn private_field_error(type_name: &str, field: &FieldInfo, name: &str, span: Span) -> CompileError {
        errors::private_field(field.owner.as_deref().unwrap_or(type_name), name, span)
    }

    /// Resolve an expression receiver like `math` to an imported module binding path.
    ///
    /// This is intentionally module-kind driven (`SymbolKind::Module`) instead of name-driven so member access does not
    /// require per-module hardcoded registries.
    pub(in crate::typechecker) fn imported_module_for_expr(
        &self,
        expr: &Spanned<Expr>,
    ) -> Option<(String, Vec<String>)> {
        let Expr::Ident(name) = &expr.node else {
            return None;
        };
        let sym = self.lookup_symbol(name)?;
        let SymbolKind::Module(info) = &sym.kind else {
            return None;
        };
        // Rust imports keep their dedicated metadata path and Python modules remain dynamic.
        if info.path.first().is_some_and(|seg| seg == "rust") || info.is_python {
            return None;
        }
        Some((name.clone(), info.path.clone()))
    }

    /// The declaring identity of a callable reached through a module binding (`math.sqrt`), from the Loaf that owns
    /// the module or from the program's own module graph.
    ///
    /// Both `import std.math` + `math.sqrt(..)` and `from std.math import sqrt` + `sqrt(..)` record the same
    /// declaration.
    pub(super) fn imported_module_callable_identity(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        if let Some((library, rest)) = self.loaf_module(module_path) {
            return self
                .resolve_pub_library_module_symbol_member(&library, &rest, member)
                .ok()
                .flatten()?
                .canonical;
        }
        self.dependency_member_identity(&ImportPath::simple(module_path.to_vec()), member)
    }

    /// Resolve a function member from a Loaf or source module binding.
    pub(super) fn resolve_imported_module_function_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<SymbolKind> {
        self.resolve_imported_module_function_member_with_source(module_path, member)
            .map(|(kind, _)| kind)
    }

    /// Resolve an imported callable together with the authored module path used by lowering and codegraph facts.
    ///
    /// A Loaf owns its public modules even when an export is missing: a member it does not publish is not answered
    /// from anywhere else. During library compilation, its own source functions already carry the package origin;
    /// that origin is local only when it matches the package the accepting checker is compiling.
    pub(super) fn resolve_imported_module_function_member_with_source(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        if let Some((library, rest)) = self.loaf_module(module_path) {
            let (kind, source_module_path) = self.lookup_pub_library_module_symbol_member(&library, &rest, member)?;
            return matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
                .then_some((kind, source_module_path));
        }
        let import_path = ImportPath::simple(module_path.to_vec());
        let kind = self.dependency_member_symbol_for_path(&import_path, member)?;
        if !matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)) {
            return None;
        }
        let identity = self.dependency_member_identity(&import_path, member)?;
        let source_module_path = match identity.origin {
            incan_semantics_core::SymbolOrigin::Module(path) => path,
            incan_semantics_core::SymbolOrigin::Package { library, module_path }
                if self.symbols.package_identity() == Some(library.as_str()) =>
            {
                module_path
            }
            _ => return None,
        };
        Some((kind, source_module_path))
    }

    /// Resolve a constant reached through an imported Loaf or source module.
    pub(in crate::typechecker) fn resolve_imported_module_constant_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(VariableInfo, Option<incan_semantics_core::CanonicalSymbolId>)> {
        let (kind, identity) = if let Some((library, rest)) = self.loaf_module(module_path) {
            let resolved = self
                .resolve_pub_library_module_symbol_member(&library, &rest, member)
                .ok()
                .flatten()?;
            (resolved.kind, resolved.canonical)
        } else {
            let path = ImportPath::simple(module_path.to_vec());
            (
                self.dependency_member_symbol_for_path(&path, member)?,
                self.dependency_member_identity(&path, member),
            )
        };
        match kind {
            SymbolKind::Variable(info) => Some((info, identity)),
            SymbolKind::Static(info) => Some((
                VariableInfo {
                    ty: info.ty,
                    is_mutable: false,
                    is_used: info.is_used,
                },
                identity,
            )),
            _ => None,
        }
    }

    /// Convert function symbol information into a resolved function type.
    pub(super) fn function_info_to_resolved_function_type(info: &FunctionInfo) -> ResolvedType {
        ResolvedType::Function(info.params.clone(), Box::new(info.return_type.clone()))
    }
}
