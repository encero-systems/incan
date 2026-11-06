//! Checked type resolution and source type-annotation name validation.

use super::*;

impl TypeChecker {
    /// Resolve a type annotation and emit diagnostics for reserved or invalid type spellings.
    pub(super) fn resolve_type_checked(&mut self, ty: &Spanned<Type>) -> ResolvedType {
        self.resolve_type_checked_for_input(ty, false)
    }

    /// A supported source-function parameter or local declaration may defer its outer ownership diagnostic until its
    /// body is checked. Every other annotation role, nested mode, malformed wrapper and returned dependency stays
    /// refused.
    pub(super) fn resolve_type_checked_for_input(&mut self, ty: &Spanned<Type>, defer_input: bool) -> ResolvedType {
        self.resolve_type_checked_deferring(
            ty,
            |mode| defer_input && mode != incan_lang::lang::types::ownership::OwnershipMode::Reference,
            false,
        )
    }

    /// Resolve an annotation whose outer ownership mode, when `defer` selects it, is checked by its declaration's
    /// contract instead of being refused here. With `carried`, the mode of one `Result` or `Option` payload, or of one
    /// tuple element, is deferred the same way, for a view carried in the result. Other nested modes are always
    /// refused.
    pub(super) fn resolve_type_checked_deferring(
        &mut self,
        ty: &Spanned<Type>,
        defer: impl Fn(incan_lang::lang::types::ownership::OwnershipMode) -> bool,
        carried: bool,
    ) -> ResolvedType {
        self.resolve_qualified_type_annotations(ty);
        if self.validate_source_type_names {
            if let Type::Generic(name, arguments) = &ty.node
                && arguments.len() == 1
                && self.explicit_ownership_mode(name).is_some_and(&defer)
            {
                self.refuse_unimplemented_ownership_annotation(&arguments[0]);
            } else if carried
                && let Some(arguments) = carried_view_components(&ty.node)
                && let Some(index) = arguments.iter().position(|argument| {
                    matches!(&argument.node, Type::Generic(name, payload)
                        if payload.len() == 1 && self.explicit_ownership_mode(name).is_some_and(&defer))
                })
            {
                // One component carries the view; every other component stays an ordinary type.
                for (position, argument) in arguments.iter().enumerate() {
                    match &argument.node {
                        Type::Generic(_, payload) if position == index => {
                            self.refuse_unimplemented_ownership_annotation(&payload[0]);
                        }
                        _ => self.refuse_unimplemented_ownership_annotation(argument),
                    }
                }
            } else {
                self.refuse_unimplemented_ownership_annotation(ty);
            }
            self.validate_source_type_annotation_names(&ty.node, ty.span);
            self.record_type_reference_identities(ty);
        }
        self.validate_stdlib_type_usage(ty);
        self.refuse_marked_copied_scalars(ty);
        if let Type::Simple(name) = &ty.node
            && Self::reserved_numeric_type_name(name)
        {
            self.errors.push(CompileError::type_error(
                format!(
                    "`{name}` is reserved for numeric types; use `decimal[p, s]`/`numeric[p, s]` for decimals or an exact-width integer type"
                ),
                ty.span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some(decimal_ty) = self.resolve_decimal_type_checked(ty) {
            return decimal_ty;
        }
        if self.report_bare_builtin_collection_annotations(ty) {
            return ResolvedType::Unknown;
        }
        if let Type::Simple(name) = &ty.node
            && let Some(sym) = self.lookup_symbol(name.as_str())
            && let SymbolKind::RustItem(info) = &sym.kind
            && info.binding == RustImportBindingKind::CrateRoot
        {
            self.errors
                .push(errors::rust_crate_root_used_as_type(name.as_str(), &info.path, ty.span));
            return ResolvedType::Unknown;
        }
        let resolved = resolve_type_with_rust_arg_renderer(
            &ty.node,
            &self.symbols,
            &|arg| self.render_provider_aware_rust_arg(arg),
            &|arg| self.qualify_native_source_argument(arg),
            &|segments| self.qualified_type_annotation_resolved_type(segments),
        );
        let resolved = self.normalize_union_member_identity(resolved);
        let resolved = self.expand_type_aliases(resolved);
        // Only the checking pass sees every declaration's derives; collection may resolve a name declared further on.
        if self.validate_source_type_names {
            self.refuse_unhashable_collection_keys(ty, &resolved);
        }
        resolved
    }

    /// Return the nominal type a module-qualified spelling was proven to name, for the shared type resolver.
    ///
    /// The proof was recorded by [`Self::resolve_qualified_type_annotations`]; an unproven spelling stays `None` so
    /// the resolver yields `Unknown` and the validation walk reports it.
    pub(super) fn qualified_type_annotation_resolved_type(&self, segments: &[String]) -> Option<ResolvedType> {
        self.type_info
            .qualified_type_reference(&segments.join("."))
            .map(|reference| self.qualified_reference_member_spelling(reference.resolved.clone(), &reference.identity))
    }

    /// Reject unbound names in source-authored annotations after declaration collection is complete.
    pub(super) fn validate_source_type_annotation_names(&mut self, ty: &Type, span: Span) {
        match ty {
            Type::Simple(name) => self.validate_source_type_annotation_name(name, span),
            Type::Generic(name, args) => {
                self.validate_source_type_annotation_name(name, span);
                for arg in args {
                    self.validate_source_type_annotation_names(&arg.node, arg.span);
                }
            }
            Type::Dotted(segments) => self.validate_source_dotted_type_annotation(segments, span),
            Type::DottedGeneric(segments, args) => {
                self.validate_source_dotted_type_annotation(segments, span);
                for arg in args {
                    self.validate_source_type_annotation_names(&arg.node, arg.span);
                }
            }
            Type::ConstrainedPrimitive(name, _) => self.validate_source_type_annotation_name(name, span),
            Type::Function(params, ret) => {
                for param in params {
                    self.validate_source_type_annotation_names(&param.node, param.span);
                }
                self.validate_source_type_annotation_names(&ret.node, ret.span);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.validate_source_type_annotation_names(&inner.node, inner.span);
            }
            Type::Tuple(items) => {
                for item in items {
                    self.validate_source_type_annotation_names(&item.node, item.span);
                }
            }
            Type::Qualified(segments) => self.validate_source_qualified_type_annotation(segments, span),
            Type::IntLiteral(_) | Type::Unit | Type::SelfType | Type::Infer => {}
        }
    }

    /// Require an imported Rust root and public metadata for a source-authored qualified type. Compiler-selected
    /// dependencies may have restricted metadata, which must not grant an explicit type annotation.
    pub(super) fn validate_source_qualified_type_annotation(&mut self, segments: &[String], span: Span) {
        let resolved = resolve_type(&Type::Qualified(segments.to_vec()), &self.symbols);
        if let ResolvedType::RustPath(path) = &resolved {
            self.admit_source_rust_type(&segments.join("::"), path, span);
            return;
        }
        if resolved != ResolvedType::Unknown {
            return;
        }
        if let Some(root) = segments.first() {
            self.emit_unknown_source_type_annotation_name(root, span);
        }
    }

    /// Reject a module-qualified type annotation (`mod.Type`) the checker could not resolve to a declaration.
    ///
    /// An unbound root is an unknown symbol like any other. A bound root that is not a module, or a module that
    /// declares no such type, is refused with its own message: the spelling would otherwise resolve to `Unknown`
    /// and reach lowering as a dotted name, which the Rust emitter cannot spell (#1437). A spelling that resolved
    /// has its proof recorded under [`DeclarationArtifacts::qualified_type_references`] and needs nothing here.
    ///
    /// The C interop namespace is the one dotted root that is not a module: `c.i32` or `c.Owned[T]` names a
    /// vocabulary carrier that the checked-binding facet interprets, and `from std.interop import c` binds `c` as
    /// that namespace's marker type. Those spellings are accepted here exactly as they were before qualified module
    /// types resolved, so a safe facade over a binding keeps checking.
    pub(super) fn validate_source_dotted_type_annotation(&mut self, segments: &[String], span: Span) {
        let Some(root) = segments.first() else {
            return;
        };
        if self.symbols.lookup(root).is_none() {
            self.emit_unknown_source_type_annotation_name(root, span);
            return;
        }
        let spelling = segments.join(".");
        if self.type_info.qualified_type_reference(&spelling).is_some() {
            return;
        }
        if self
            .import_binding_path(root)
            .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        {
            return;
        }
        let key = (spelling.clone(), span);
        if !self.unknown_source_type_names_emitted.insert(key) {
            return;
        }
        match self.resolve_qualified_name(segments) {
            Err(failure) => self.report_namespace_resolution_failure(failure, span),
            Ok(_) => self.errors.push(CompileError::type_error(
                format!("`{spelling}` does not name a type or trait"),
                span,
            )),
        }
    }

    /// Render a checked module path the way an import statement spells it, for a diagnostic.
    ///
    /// The path is the binding's resolved one rather than the alias the author wrote, so the message names the
    /// module that was actually searched (`beta`, not `m`) and its hint is a spelling that imports from it. A
    /// public library carries its `pub::` marker and dots below the library name, as the import reference documents.
    pub(super) fn module_import_spelling(module_path: &[String]) -> String {
        match module_path {
            [root, library, rest @ ..] if root == PUBLIC_LIBRARY_NAMESPACE => {
                let mut spelling = format!("{root}::{library}");
                if !rest.is_empty() {
                    spelling.push('.');
                    spelling.push_str(&rest.join("."));
                }
                spelling
            }
            _ => module_path.join("."),
        }
    }

    /// Emit one diagnostic when a simple source annotation name has no declaration in the active scope.
    pub(super) fn validate_source_type_annotation_name(&mut self, name: &str, span: Span) {
        if name == "_"
            || name == "Type"
            || Self::reserved_numeric_type_name(name)
            || runtime_errors::from_str(name).is_some()
        {
            return;
        }
        if !matches!(
            resolve_type(&Type::Simple(name.to_string()), &self.symbols),
            ResolvedType::TypeVar(_)
        ) {
            return;
        }
        self.emit_unknown_source_type_annotation_name(name, span);
    }

    /// Emit at most one unknown-symbol diagnostic for one source annotation occurrence.
    ///
    /// Inside a callable the diagnostic suggests declaring the name as a type parameter of that callable; elsewhere
    /// it keeps the import-or-define remedy.
    pub(super) fn emit_unknown_source_type_annotation_name(&mut self, name: &str, span: Span) {
        let key = (name.to_string(), span);
        if !self.unknown_source_type_names_emitted.insert(key) {
            return;
        }
        let error = match &self.annotation_owner {
            Some(owner) => {
                errors::unknown_type_parameter_candidate(name, &owner.name, &owner.declared_type_params, span)
            }
            None => errors::unknown_symbol(name, span),
        };
        self.errors.push(error);
    }

    /// Make `owner` the callable whose annotations are being checked, returning the previous owner to restore.
    pub(super) fn enter_annotation_owner(&mut self, name: &str, type_params: &[TypeParam]) -> Option<AnnotationOwner> {
        self.annotation_owner.replace(AnnotationOwner {
            name: name.to_string(),
            declared_type_params: type_params.iter().map(|param| param.name.clone()).collect(),
        })
    }

    /// Return whether a simple type name is reserved for a parameterized numeric family.
    pub(super) fn reserved_numeric_type_name(name: &str) -> bool {
        numerics::decimal_constructor_from_str(name).is_some()
    }

    /// Resolve and validate a parameterized decimal type annotation.
    pub(super) fn resolve_decimal_type_checked(&mut self, ty: &Spanned<Type>) -> Option<ResolvedType> {
        let Type::Generic(name, args) = &ty.node else {
            return None;
        };
        numerics::decimal_constructor_from_str(name.as_str())?;
        if args.len() != 2 {
            self.errors.push(CompileError::type_error(
                format!("{name}[...] expects exactly 2 integer parameters: precision and scale"),
                ty.span,
            ));
            return Some(ResolvedType::Unknown);
        }
        let Some(precision) = decimal_type_int_arg(&args[0]) else {
            self.errors.push(CompileError::type_error(
                "Decimal precision must be an integer literal".to_string(),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let Some(scale) = decimal_type_int_arg(&args[1]) else {
            self.errors.push(CompileError::type_error(
                "Decimal scale must be an integer literal".to_string(),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        };
        if !(1..=38).contains(&precision) {
            self.errors.push(CompileError::type_error(
                format!("Decimal precision must be between 1 and 38, found {precision}"),
                args[0].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if scale < 0 || scale > precision {
            self.errors.push(CompileError::type_error(
                format!("Decimal scale must be between 0 and precision {precision}, found {scale}"),
                args[1].span,
            ));
            return Some(ResolvedType::Unknown);
        }
        // Every registered decimal constructor is one type identity. In particular, RFC 009 defines
        // `decimal128[p, s]` as an alias rather than a distinct nominal type.
        let canonical = numerics::decimal_constructor_info_for(
            incan_lang::lang::types::numerics::DecimalTypeConstructorId::Decimal,
        )
        .canonical;
        Some(ResolvedType::Generic(
            canonical.to_string(),
            vec![
                ResolvedType::TypeVar(precision.to_string()),
                ResolvedType::TypeVar(scale.to_string()),
            ],
        ))
    }
}

/// The components of a result annotation that can carry a returned view: a `Result` or `Option` annotation's
/// payloads, or a tuple annotation's elements.
pub(super) fn carried_view_components(ty: &Type) -> Option<&[Spanned<Type>]> {
    match ty {
        Type::Generic(carrier, arguments)
            if matches!(
                incan_lang::lang::types::collections::from_str(carrier),
                Some(CollectionTypeId::Result | CollectionTypeId::Option)
            ) =>
        {
            Some(arguments)
        }
        _ => written_tuple_elements(ty),
    }
}

/// Extract a decimal precision or scale argument from a type-position integer literal.
pub(super) fn decimal_type_int_arg(arg: &Spanned<Type>) -> Option<i64> {
    match &arg.node {
        Type::IntLiteral(value) => Some(value.value),
        _ => None,
    }
}
