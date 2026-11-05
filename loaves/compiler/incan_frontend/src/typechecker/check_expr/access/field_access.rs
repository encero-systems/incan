//! Field access on nominal, imported-module and Rust receivers, including properties.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Resolve a declared field on a nominal user-defined type, applying generic substitutions when available.
    ///
    /// This keeps field access on `Named(Type)` and `Generic(Type[...])` owners on the same path instead of letting
    /// generic owners fall through to "missing field" diagnostics despite having declared fields. The selected field's
    /// declaring scope supplies Body IR nominal owners before its source spellings leave this boundary.
    pub(in crate::typechecker) fn resolve_nominal_field_type(
        &mut self,
        type_name: &str,
        type_args: Option<&[ResolvedType]>,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        if let Some(surface_ty) = self.resolve_surface_type_field_type(type_name, field, span) {
            return Some(surface_ty);
        }
        let type_info = self.lookup_semantic_type_info(type_name)?;

        let field_info = match type_info {
            TypeInfo::Model(model) => {
                // `.0`, `.1`, ... is tuple-index syntax in the language surface.
                // RFC 021: Non-identifier aliases like `alias="1"` are valid as wire names, but are not usable via
                // member access / named-arg / pattern syntax.
                //
                // Therefore numeric field spellings do NOT participate in alias lookup on models.
                if field.parse::<usize>().is_ok() {
                    self.errors.push(errors::missing_field(type_name, field, span));
                    return Some(ResolvedType::Unknown);
                }
                let (_, info) = self.resolve_field_info(&model.fields, field, true, false)?;
                let info = info.clone();
                let subst = type_param_subst_map(&model.type_params, type_args.unwrap_or_default());
                if let Some(identity) = info.identity.clone() {
                    self.type_info.record_resolved_identity(span, identity);
                }
                if self.private_field_is_inaccessible(type_name, &info) {
                    self.errors
                        .push(Self::private_field_error(type_name, &info, field, span));
                    return Some(ResolvedType::Unknown);
                }
                self.retain_declared_nominal_field(span, &info.ty, &subst, info.identity.as_ref());
                substitute_resolved_type(&info.ty, &subst)
            }
            TypeInfo::Class(class) => {
                // RFC 021: No alias-aware resolution for classes (models only)
                let (_, info) = self.resolve_field_info(&class.fields, field, false, true)?;
                let info = info.clone();
                let subst = type_param_subst_map(&class.type_params, type_args.unwrap_or_default());
                if let Some(identity) = info.identity.clone() {
                    self.type_info.record_resolved_identity(span, identity);
                }
                if self.private_field_is_inaccessible(type_name, &info) {
                    self.errors
                        .push(Self::private_field_error(type_name, &info, field, span));
                    return Some(ResolvedType::Unknown);
                }
                self.retain_declared_nominal_field(span, &info.ty, &subst, info.identity.as_ref());
                substitute_resolved_type(&info.ty, &subst)
            }
            TypeInfo::Enum(enum_info) => {
                let resolved = Self::resolve_nominal_enum_field_type(type_name, type_args, field, enum_info)?;
                let identity = enum_info.variant_identities.get(field).cloned();
                if let Some(identity) = identity {
                    self.type_info.record_resolved_identity(span, identity);
                }
                return Some(resolved);
            }
            TypeInfo::Newtype(nt) if nt.is_rusttype => {
                if let ResolvedType::RustPath(path) = &nt.underlying {
                    let path = path.clone();
                    if let Some(sig) = self.rust_associated_function_signature(&path, field) {
                        return Some(self.resolved_function_type_from_rust_sig_for_path(&sig, false, &path));
                    }
                    return self.rust_path_field_type(&path, field, span);
                }
                return None;
            }
            TypeInfo::Newtype(nt) if field == conventions::NEWTYPE_TUPLE_FIELD => {
                let underlying = nt.underlying.clone();
                // The wrapped slot belongs to the newtype's declaration, including one a library returned without
                // the consumer importing it.
                let owner = self
                    .symbols
                    .lookup(type_name)
                    .and_then(|symbol| self.symbols.identity_of(symbol))
                    .or_else(|| self.public_library_type_identities.get(type_name)?.canonical.as_ref())
                    .cloned();
                if let Some(owner) = owner {
                    self.type_info.record_resolved_identity(span, owner);
                }
                return Some(underlying);
            }
            _ => return None,
        };

        Some(field_info)
    }

    /// Resolve a declared enum variant or value conversion constructor without mutating checking state.
    ///
    /// The caller records a variant's identity after this borrow ends, retaining only the selected facts rather than
    /// cloning the enum's complete declaration metadata.
    pub(super) fn resolve_nominal_enum_field_type(
        type_name: &str,
        type_args: Option<&[ResolvedType]>,
        field: &str,
        info: &EnumInfo,
    ) -> Option<ResolvedType> {
        let canonical = info.variant_aliases.get(field).map(String::as_str).unwrap_or(field);
        if info.variants.iter().any(|variant| variant == canonical) {
            let enum_ty = if let Some(arguments) = type_args {
                ResolvedType::Generic(type_name.to_string(), arguments.to_vec())
            } else {
                ResolvedType::Named(type_name.to_string())
            };
            let mut fields = info.variant_fields.get(canonical).cloned().unwrap_or_default();
            if let Some(arguments) = type_args {
                let substitutions = type_param_subst_map(&info.type_params, arguments);
                fields = fields
                    .into_iter()
                    .map(|ty| substitute_resolved_type(&ty, &substitutions))
                    .collect();
            }
            if fields.is_empty() {
                return Some(enum_ty);
            }
            return Some(ResolvedType::Function(
                fields.into_iter().map(CallableParam::positional).collect(),
                Box::new(enum_ty),
            ));
        }
        if field == "from_value"
            && let Some(value_enum) = &info.value_enum
        {
            return Some(ResolvedType::Function(
                vec![CallableParam::positional(value_enum.value_type.resolved_type())],
                Box::new(Self::value_enum_from_value_return_type(type_name)),
            ));
        }
        None
    }

    /// Resolve a declared computed property on a nominal user-defined type, applying generic substitutions.
    pub(super) fn resolve_nominal_property_type(
        &mut self,
        type_name: &str,
        type_args: Option<&[ResolvedType]>,
        property: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let (properties, type_params, class_visibility) = match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => (&model.properties, &model.type_params, false),
            TypeInfo::Class(class) => (&class.properties, &class.type_params, true),
            _ => return None,
        };
        let info = properties.get(property)?;
        let identity = info.identity.clone();
        let owner = info.owner.as_deref().unwrap_or(type_name);
        let inaccessible = class_visibility
            && matches!(info.visibility, Visibility::Private)
            && self.current_method_owner.as_deref() != Some(owner);
        let return_type = if let Some(args) = type_args {
            let subst = type_param_subst_map(type_params, args);
            substitute_resolved_type(&info.return_type, &subst)
        } else {
            info.return_type.clone()
        };
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(span, identity);
        }
        if inaccessible {
            self.errors.push(errors::private_property(type_name, property, span));
            return Some(ResolvedType::Unknown);
        }
        self.type_info
            .record_computed_property_access(span, type_name, property);
        Some(return_type)
    }

    /// Return whether the receiver has a computed property with this name.
    pub(in crate::typechecker::check_expr) fn receiver_has_computed_property(
        &mut self,
        receiver_ty: &ResolvedType,
        property: &str,
        span: Span,
    ) -> bool {
        match receiver_ty {
            ResolvedType::Named(type_name) => self
                .resolve_nominal_property_type(type_name, None, property, span)
                .is_some(),
            ResolvedType::Generic(type_name, type_args) => self
                .resolve_nominal_property_type(type_name, Some(type_args.as_slice()), property, span)
                .is_some(),
            ResolvedType::TypeVar(name) => self
                .resolve_generic_placeholder_property(name, property, span)
                .is_some(),
            _ => false,
        }
    }

    /// Resolve a computed property on an active generic placeholder bound (`T with Trait[...]`).
    pub(super) fn resolve_generic_placeholder_property(
        &mut self,
        placeholder_name: &str,
        property: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let active_bounds = self
            .innermost_type_param_bounds(placeholder_name)
            .map(<[TypeBoundInfo]>::to_vec)
            .unwrap_or_default();
        for bound in &active_bounds {
            if let Some(info) = self.trait_property_info_resolved_for_adoption(bound, property, span) {
                if let Some(identity) = info.identity.clone() {
                    self.type_info.record_resolved_identity(span, identity);
                }
                self.type_info
                    .record_computed_property_access(span, &bound.name, property);
                return Some(info.return_type);
            }
        }
        None
    }

    /// Type-check a field access against its retained value owner before consulting source-facing inference. Foreign
    /// collection elements keep their declaring fields even when a caller binds the same type name.
    pub(in crate::typechecker::check_expr) fn check_field(
        &mut self,
        base: &Spanned<Expr>,
        field: &str,
        span: Span,
    ) -> ResolvedType {
        if let Some(result) = self.check_c_binding_enum_constant(base, field, span) {
            return result;
        }
        let base_ty = self.check_type_receiver_expr(base);
        let base_ty = self.retained_source_field_receiver(base.span).unwrap_or(base_ty);

        // Field lookup follows explicit references without changing the checked receiver's ownership facts.
        let mut field_owner = &base_ty;
        while let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = field_owner {
            field_owner = inner;
        }
        let base_ty = field_owner;

        if let Some(ty) = self.check_imported_module_field(base, field, span) {
            return ty;
        }
        if matches!(base_ty, ResolvedType::Unknown) {
            return ResolvedType::Unknown;
        }
        if let Some(result) = self.resolve_checked_extractor_field(base, base_ty, field, span) {
            return result;
        }
        if let Some(result) = self.resolve_rust_deref_field(base, base_ty, field, span) {
            return result;
        }
        if let ResolvedType::RustPath(path) = base_ty {
            let namespace = self.rust_namespace_receiver(base);
            return self.check_rust_path_field(path, field, span, namespace);
        }

        if let ResolvedType::Generic(name, args) = &base_ty
            && surface_types::from_str(name.as_str()).is_some_and(surface_types::field_access_reads_wrapped_value)
            && args.len() == 1
        {
            if field == "value" {
                return args[0].clone();
            }
            return self.check_nominal_field(base, &args[0], field, span);
        }

        // In a declared type's own method, a `Self` value (`other: Self`) is that type, so its fields resolve as they
        // do on `self`, as its methods already do (#1561); a trait default keeps `Self` open and reads fields through
        // `@requires`.
        if matches!(base_ty, ResolvedType::SelfType)
            && let Some(owner_ty) = self.current_method_owner_type()
        {
            return self.check_nominal_field(base, &owner_ty, field, span);
        }

        self.check_nominal_field(base, base_ty, field, span)
    }

    /// Retain a source declaration's exact field owner without replacing a source surface wrapper with its native
    /// storage type.
    pub(super) fn retained_source_field_receiver(&self, span: Span) -> Option<ResolvedType> {
        let mut retained = self.body_ir_checked_value_type(span)?.clone();
        let mut referent = &mut retained;
        while let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = referent {
            referent = inner;
        }
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = referent else {
            return None;
        };
        if !matches!(
            self.lookup_semantic_type_info(name),
            Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        if let Some(key) = self.source_nominal_carrier_key(name) {
            *name = key;
        }
        Some(retained)
    }

    /// Resolve imported module members from checked symbol metadata before treating a receiver as a value.
    pub(super) fn check_imported_module_field(
        &mut self,
        base: &Spanned<Expr>,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let (module_name, module_path, resolved) =
            if let Some((module_name, module_path)) = self.imported_module_for_expr(base) {
                let resolved = self.resolve_module_namespace_member(&module_path, field);
                (module_name, module_path, resolved)
            } else {
                let mut segments = crate::typechecker::collect::dotted_path_segments(&base.node)?;
                let module_path = self.module_namespace_path(segments.first()?)?;
                let module_name = segments.join(".");
                segments.push(field.to_string());
                let resolved = self.resolve_qualified_name(&segments);
                (module_name, module_path, resolved)
            };
        self.record_module_namespace_reference(base.span, &module_path);
        if let Some((head, head_span)) = Self::dotted_namespace_head(base) {
            let _ = self.check_ident(head, head_span);
        }
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(failure) => {
                self.report_namespace_resolution_failure(failure, span);
                return Some(ResolvedType::Unknown);
            }
        };
        let callable = format!("{module_name}.{field}");
        if let Some(identity) = resolved.canonical.clone() {
            self.type_info.record_resolved_identity(span, identity);
        }
        match resolved.kind {
            ResolvedNamespaceMemberKind::SurfaceType(surface) => {
                self.type_info.expressions.ident_kinds.insert(span, IdentKind::TypeName);
                Some(ResolvedType::Named(surface_types::as_str(surface).to_string()))
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variable(info)) => Some(info.ty),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Static(info)) => Some(info.ty),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Field(info)) => Some(info.ty),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Property(info)) => Some(info.return_type),
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Variant(info)) => {
                let result = ResolvedType::Named(info.enum_name);
                if info.fields.is_empty() {
                    Some(result)
                } else {
                    Some(ResolvedType::Function(
                        info.fields.into_iter().map(CallableParam::positional).collect(),
                        Box::new(result),
                    ))
                }
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Type(type_info)) => {
                let canonical_name = if let Some(library) = resolved.public_library.as_deref() {
                    let root_len = self.loaf_binding_root(library).len();
                    let mut source_type_path = resolved
                        .source_module_path
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .skip(root_len)
                        .cloned()
                        .collect::<Vec<_>>();
                    source_type_path.push(resolved.source_name.clone());
                    canonical_public_library_type_name(library, &source_type_path.join("::"))
                } else {
                    resolved.source_name.clone()
                };
                self.type_info.expressions.ident_kinds.insert(span, IdentKind::TypeName);
                if let (Some(source), Some(source_kind)) = (
                    resolved.source_module_path,
                    Self::source_target_kind_for_type_info(&type_info),
                ) {
                    self.record_source_target(span, source, resolved.source_name, source_kind);
                }
                Some(ResolvedType::Named(canonical_name))
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Function(info)) => {
                if self.decorator_callable_span == Some(span) {
                    self.decorator_callable_type_params = info.type_params.clone();
                }
                if let Some(source) = resolved.source_module_path {
                    self.record_source_target(span, source, resolved.source_name, "function");
                }
                if !info.type_params.is_empty() {
                    if !self.is_generic_partial_target_span(span) && self.decorator_callable_span != Some(span) {
                        self.errors
                            .push(errors::generic_function_reference(callable.as_str(), span));
                        return Some(ResolvedType::Unknown);
                    }
                    if self.is_generic_partial_target_span(span) {
                        self.generic_partial_target = Some(GenericPartialTarget::of_function(&callable, &info));
                    }
                }
                Some(Self::function_info_to_resolved_function_type(&info))
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::FunctionOverloads(_)) => {
                if let Some(source) = resolved.source_module_path {
                    self.record_source_target(span, source, resolved.source_name, "function");
                }
                self.errors.push(CompileError::type_error(
                    format!(
                        "Cannot use overloaded function '{callable}' as a value; call it directly so an overload can be selected"
                    ),
                    span,
                ));
                Some(ResolvedType::Unknown)
            }
            ResolvedNamespaceMemberKind::Symbol(SymbolKind::Module(_)) => {
                self.type_info.expressions.ident_kinds.insert(span, IdentKind::Module);
                Some(ResolvedType::Unknown)
            }
            ResolvedNamespaceMemberKind::Method(info) => {
                Some(ResolvedType::Function(info.params, Box::new(info.return_type)))
            }
            ResolvedNamespaceMemberKind::Symbol(_) => {
                self.errors.push(CompileError::type_error(
                    format!("Qualified member '{callable}' is not a value"),
                    span,
                ));
                Some(ResolvedType::Unknown)
            }
        }
    }

    /// Prove a native namespace from checked import/type roles along a name or field path, excluding runtime receiver
    /// expressions and preserving parentheses.
    pub(super) fn rust_namespace_receiver(&self, expression: &Spanned<Expr>) -> bool {
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_namespace_receiver(inner);
        }
        if !matches!(
            self.type_info.ident_kind(expression.span),
            Some(IdentKind::RustImport | IdentKind::TypeName)
        ) {
            return false;
        }
        match &expression.node {
            Expr::Ident(_) => true,
            Expr::Field(base, _) => self.rust_namespace_receiver(base),
            _ => false,
        }
    }

    /// Resolve a Rust field or associated item only from admitted metadata; executable function values require the
    /// receiver's checked namespace role.
    pub(super) fn check_rust_path_field(
        &mut self,
        path: &str,
        field: &str,
        span: Span,
        namespace: bool,
    ) -> ResolvedType {
        let Some(meta) = self.rust_item_metadata_for_path(path) else {
            self.errors.push(CompileError::type_error(
                format!("Rust item `rust::{path}::{field}` has no admitted metadata"),
                span,
            ));
            return ResolvedType::Unknown;
        };
        match &meta.kind {
            RustItemKind::Module(module) => {
                if let Some(child) = module.children.iter().find(|c| c.name == field) {
                    return match child.kind_hint {
                        incan_lang::interop::RustModuleChildKind::Module
                        | incan_lang::interop::RustModuleChildKind::Type
                        | incan_lang::interop::RustModuleChildKind::Trait => {
                            if namespace {
                                self.type_info
                                    .expressions
                                    .ident_kinds
                                    .insert(span, IdentKind::RustImport);
                            }
                            ResolvedType::RustPath(format!("{path}::{field}"))
                        }
                        incan_lang::interop::RustModuleChildKind::Other => {
                            ResolvedType::RustPath(format!("{path}::{field}"))
                        }
                        incan_lang::interop::RustModuleChildKind::Function => {
                            let selected = format!("{path}::{field}");
                            let Some(metadata) = self.rust_item_metadata_for_path(&selected) else {
                                return ResolvedType::Unknown;
                            };
                            let RustItemKind::Function(signature) = &metadata.kind else {
                                return ResolvedType::Unknown;
                            };
                            let ty = self.resolved_function_type_from_rust_sig_for_owner_path(
                                signature,
                                false,
                                &metadata.canonical_path,
                            );
                            if namespace {
                                return self
                                    .retain_rust_function_value(&metadata.canonical_path, signature, span)
                                    .unwrap_or(ty);
                            }
                            ty
                        }
                        incan_lang::interop::RustModuleChildKind::Constant => {
                            let selected = format!("{path}::{field}");
                            let Some(metadata) = self.rust_item_metadata_for_path(&selected) else {
                                return ResolvedType::Unknown;
                            };
                            let RustItemKind::Constant { type_display } = &metadata.kind else {
                                return ResolvedType::Unknown;
                            };
                            self.type_info.rust.checked_constants.insert(span, selected);
                            self.native_rust_signature_type(&type_display, path)
                        }
                    };
                }
                // Module membership from rust-analyzer is authoritative.
                self.errors
                    .push(errors::missing_field(rust_receiver_display(path).as_str(), field, span));
                ResolvedType::Unknown
            }
            RustItemKind::Type(owner) => {
                let selected = format!("{path}::{field}");
                if let Some(metadata) = self.rust_item_metadata_for_path(&selected)
                    && let RustItemKind::Constant { type_display } = &metadata.kind
                {
                    self.type_info.rust.checked_constants.insert(span, selected);
                    return self.native_rust_signature_type(type_display, path);
                }
                if let Some(method) = owner
                    .methods
                    .iter()
                    .find(|method| method.name == field && !Self::rust_signature_has_receiver(&method.signature))
                {
                    let ty = self.resolved_function_type_from_rust_sig_for_path(&method.signature, false, path);
                    if !namespace {
                        return ty;
                    }
                    return self
                        .retain_rust_associated_function_value(path, owner, method, &ty, span)
                        .unwrap_or(ty);
                }
                if let Some(params) = self.rust_variant_callable_params(path, field) {
                    if params.is_empty() {
                        let selected = format!("{path}::{field}");
                        if let Some(metadata) = self.rust_item_metadata_for_path(&selected)
                            && let RustItemKind::Constant { type_display } = &metadata.kind
                        {
                            self.type_info
                                .rust
                                .checked_constants
                                .insert(span, metadata.canonical_path.clone());
                            return self.native_rust_signature_type(&type_display, path);
                        }
                        self.retain_rust_variant_construction(path, field, &[], &params, span);
                        return ResolvedType::RustPath(path.to_string());
                    }
                    return ResolvedType::Function(params, Box::new(ResolvedType::RustPath(path.to_string())));
                }
                // No admitted metadata names the item: refuse it rather than leave the access untyped.
                self.rust_path_field_type(path, field, span).unwrap_or_else(|| {
                    self.errors
                        .push(errors::rust_associated_item_metadata_unavailable(path, field, span));
                    ResolvedType::Unknown
                })
            }
            RustItemKind::Unsupported { description } => {
                self.errors
                    .push(errors::rust_item_shape_not_supported(path, description.as_str(), span));
                ResolvedType::Unknown
            }
            // Function, Trait, Constant: metadata coverage is incomplete, stay permissive.
            _ => ResolvedType::Unknown,
        }
    }

    /// Retain a safe associated function's concrete native signature as a value without evaluating its owner type.
    /// Uninstantiated owner, impl, method, and const parameters grant no executable function-value representation.
    pub(super) fn retain_rust_associated_function_value(
        &mut self,
        path: &str,
        owner: &incan_lang::interop::RustTypeInfo,
        method: &incan_lang::interop::RustMethodSig,
        ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        if owner.has_const_params
            || !owner.type_params.is_empty()
            || !method.impl_type_params.is_empty()
            || !method.signature.type_params.is_empty()
        {
            return None;
        }
        let ResolvedType::Function(_, result) = ty else {
            return None;
        };
        self.record_rust_call_site_params(span, &method.signature.params, path);
        if let Err(refusal) = self.retain_checked_rust_method(method, &[], owner, path, &[], &[], None, result, span) {
            self.errors.push(refusal);
            return None;
        }
        self.publish_rust_function_value(span)
    }

    /// Retain callable-value metadata only for Body IR, excluding the checker's existing type and trait name roles.
    /// Parentheses preserve the identifier's role; legacy resolution and return types remain unchanged.
    pub(super) fn retain_body_ir_callable_name_read(&mut self, base: &Spanned<Expr>, span: Span) {
        let mut receiver = base;
        while let Expr::Paren(inner) = &receiver.node {
            receiver = inner;
        }
        if matches!(
            self.type_info.ident_kind(receiver.span),
            Some(IdentKind::TypeName | IdentKind::Trait)
        ) {
            return;
        }
        self.type_info.expressions.body_ir_callable_name_reads.insert(span);
    }

    /// Resolve a nominal value's field without changing the receiver's ownership or reference facts.
    pub(super) fn check_nominal_field(
        &mut self,
        base: &Spanned<Expr>,
        ty: &ResolvedType,
        field: &str,
        span: Span,
    ) -> ResolvedType {
        let checker = self;

        if field == "__name__" && checker.is_generic_placeholder_type(ty) {
            checker.retain_body_ir_callable_name_read(base, span);
            if let Some(owner_name) = checker.generic_placeholder_name(ty)
                && let Some(identity) = checker.synthetic_member_identity_for_named_owner(
                    owner_name,
                    field,
                    SemanticSourceTargetKind::Field,
                )
            {
                checker.type_info.record_resolved_identity(span, identity);
            }
            return ResolvedType::Str;
        }
        match ty {
            ResolvedType::Unknown => ResolvedType::Unknown,
            // Trait default methods typecheck against `Self`, but field access must be declared via
            // `@requires(...)` on the trait.
            ResolvedType::SelfType => {
                if let Some(info) = checker
                    .current_trait_properties
                    .as_ref()
                    .and_then(|properties| properties.get(field))
                    .cloned()
                {
                    if let Some(identity) = info.identity {
                        checker.type_info.record_resolved_identity(span, identity);
                    }
                    checker.type_info.record_computed_property_access(span, "Self", field);
                    info.return_type
                } else {
                    checker
                        .trait_required_field_type(field, span)
                        .unwrap_or(ResolvedType::Unknown)
                }
            }
            ResolvedType::Tuple(elements) => {
                if let Ok(idx) = field.parse::<usize>()
                    && idx < elements.len()
                {
                    return elements[idx].clone();
                }
                checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
                ResolvedType::Unknown
            }
            ResolvedType::Function(_, _) if field == "__name__" => {
                checker.retain_body_ir_callable_name_read(base, span);
                if let Some(owner) = checker.type_info.resolved_identity(base.span).cloned() {
                    checker.type_info.record_resolved_identity(
                        span,
                        Self::synthetic_member_identity(&owner, field, SemanticSourceTargetKind::Field),
                    );
                }
                ResolvedType::Str
            }
            ResolvedType::Named(type_name) => {
                if let Some(field_ty) = checker.resolve_nominal_field_type(type_name, None, field, span) {
                    return field_ty;
                }
                if let Some(property_ty) = checker.resolve_nominal_property_type(type_name, None, field, span) {
                    return property_ty;
                }
                checker.errors.push(errors::missing_field(type_name, field, span));
                ResolvedType::Unknown
            }
            ResolvedType::Generic(type_name, type_args) => {
                if let Some(field_ty) =
                    checker.resolve_nominal_field_type(type_name, Some(type_args.as_slice()), field, span)
                {
                    return field_ty;
                }
                if let Some(property_ty) =
                    checker.resolve_nominal_property_type(type_name, Some(type_args.as_slice()), field, span)
                {
                    return property_ty;
                }
                checker.errors.push(errors::missing_field(type_name, field, span));
                ResolvedType::Unknown
            }
            ResolvedType::TypeVar(name) => {
                if field == "__name__" {
                    checker.retain_body_ir_callable_name_read(base, span);
                    return ResolvedType::Str;
                }
                if let Some(property_ty) = checker.resolve_generic_placeholder_property(name, field, span) {
                    return property_ty;
                }
                checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
                ResolvedType::Unknown
            }
            _ => {
                checker.errors.push(errors::missing_field(&ty.to_string(), field, span));
                ResolvedType::Unknown
            }
        }
    }
}
