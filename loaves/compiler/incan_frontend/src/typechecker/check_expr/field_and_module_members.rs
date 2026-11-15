//! F-string parts, field visibility, and imported module function and constant members.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

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
        let owner = field.owner.as_deref().map_or(type_name, |owner| owner);
        self.type_private_member_is_inaccessible(owner, field.identity.as_ref())
    }

    /// Report a private field under the source name of the class or model that declares it. The receiver's type
    /// may name the declaration's generated carrier, which no diagnostic shows.
    pub(super) fn private_field_error(
        &self,
        type_name: &str,
        field: &FieldInfo,
        name: &str,
        span: Span,
    ) -> CompileError {
        let suggest_public = field
            .identity
            .as_ref()
            .is_none_or(|identity| identity.origin == self.symbols.declaration_origin());
        errors::private_field(
            field.owner.as_deref().map_or(type_name, |owner| owner),
            name,
            span,
            field.made_private.as_ref().and_then(|facts| facts.message.as_deref()),
            field
                .made_private
                .as_ref()
                .and_then(|facts| facts.replacement.as_deref()),
            suggest_public,
        )
    }

    /// Resolve an expression receiver like `math` to the module namespace its checked binding exposes.
    pub(in crate::typechecker) fn imported_module_for_expr(
        &self,
        expr: &Spanned<Expr>,
    ) -> Option<(String, Vec<String>)> {
        let segments = crate::typechecker::collect::dotted_path_segments(&expr.node)?;
        let (head, tail) = segments.split_first()?;
        let mut module_path = self.module_namespace_path(head)?;
        for child in tail {
            let mut candidate = module_path.clone();
            candidate.push(child.clone());
            if let Some(resolved) = self.resolved_source_module_path(&ImportPath::simple(candidate.clone())) {
                module_path = resolved;
            } else if self.loaf_module(&candidate).is_some_and(|(library, public_path)| {
                self.loaves
                    .manifest(&library)
                    .is_some_and(|manifest| Self::manifest_public_module_exists(manifest, &public_path))
            }) {
                module_path = candidate;
            } else {
                return None;
            }
        }
        Some((segments.join("."), module_path))
    }

    /// Return the lexical head and its source span from an identifier/field namespace path.
    pub(super) fn dotted_namespace_head(expr: &Spanned<Expr>) -> Option<(&str, Span)> {
        match &expr.node {
            Expr::Ident(name) => Some((name, expr.span)),
            Expr::Field(base, _) => Self::dotted_namespace_head(base),
            _ => None,
        }
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
        self.resolve_module_namespace_member(module_path, member)
            .ok()
            .and_then(|resolved| resolved.canonical)
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
        let resolved = self.resolve_module_namespace_member(module_path, member).ok()?;
        let ResolvedNamespaceMemberKind::Symbol(kind) = resolved.kind else {
            return None;
        };
        let source = resolved.source_module_path?;
        matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)).then_some((kind, source))
    }

    /// Resolve any published member through the same Loaf or source-module registry used by an import by name.
    ///
    /// The caller decides which declaration kinds its expression form accepts. Keeping publication and canonical
    /// source-path selection here prevents qualified calls and values from maintaining a function-only namespace.
    pub(super) fn resolve_imported_module_member_with_source(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        let resolved = self.resolve_module_namespace_member(module_path, member).ok()?;
        let ResolvedNamespaceMemberKind::Symbol(kind) = resolved.kind else {
            return None;
        };
        Some((kind, resolved.source_module_path?))
    }

    /// Resolve a constant reached through an imported Loaf or source module.
    pub(in crate::typechecker) fn resolve_imported_module_constant_member(
        &mut self,
        module_path: &[String],
        member: &str,
    ) -> Option<(VariableInfo, Option<incan_semantics_core::CanonicalSymbolId>)> {
        let resolved = self.resolve_module_namespace_member(module_path, member).ok()?;
        let ResolvedNamespaceMemberKind::Symbol(kind) = resolved.kind else {
            return None;
        };
        let identity = resolved.canonical;
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
