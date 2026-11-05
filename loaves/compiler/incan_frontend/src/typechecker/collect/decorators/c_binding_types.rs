//! C binding enums, structs and types, and C interop library names.

use super::*;

impl TypeChecker {
    /// Collect one C enum and enforce its shared scalar carrier.
    pub(super) fn c_enum(
        member: &incan_vocab::VocabDeclaration,
        name: &str,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<CBindingEnum> {
        let mut carrier = None;
        let mut variants = Vec::new();
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(incan_vocab::IncanStatement::TypedLet {
                name: variant,
                mutable: false,
                ty,
                value,
            }) = item
            else {
                errors.push(CompileError::type_error(
                    "C enum declarations require typed carrier assignments".to_string(),
                    span,
                ));
                return None;
            };
            let Some(this_carrier) = c_abi::scalar_type_from_str(&ty.source) else {
                errors.push(CompileError::type_error(
                    format!("C enum variant `{variant}` uses an unsupported carrier type"),
                    span,
                ));
                return None;
            };
            if carrier
                .replace(this_carrier)
                .is_some_and(|existing| existing != this_carrier)
            {
                errors.push(CompileError::type_error(
                    "C enum variants must use one shared carrier type".to_string(),
                    span,
                ));
                return None;
            }
            let Some(native) = Self::c_native_reference(value) else {
                errors.push(CompileError::type_error(
                    format!("C enum variant `{variant}` requires a native constant reference"),
                    span,
                ));
                return None;
            };
            variants.push(CBindingEnumVariant {
                name: variant.clone(),
                native,
            });
        }
        let Some(carrier) = carrier else {
            errors.push(CompileError::type_error(
                "C enum declarations require at least one variant".to_string(),
                span,
            ));
            return None;
        };
        Some(CBindingEnum {
            name: name.to_string(),
            carrier,
            variants,
        })
    }

    /// Collect one plain C structure and its declared native field layout.
    pub(super) fn c_struct(
        member: &incan_vocab::VocabDeclaration,
        name: &str,
        span: Span,
        resources: &HashSet<String>,
        structs: &HashSet<String>,
        errors: &mut Vec<CompileError>,
    ) -> Option<CBindingStruct> {
        let mut native = None;
        let mut fields = Vec::new();
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C struct declarations contain fields only".to_string(),
                    span,
                ));
                return None;
            };
            match statement {
                incan_vocab::IncanStatement::Let { name: field, value, .. }
                | incan_vocab::IncanStatement::Assign { target: field, value }
                    if c_abi::plain_struct_argument_from_str(field).is_some() =>
                {
                    let incan_vocab::IncanExpr::Str(value) = value else {
                        errors.push(CompileError::type_error(
                            "C struct `native` must be a non-empty string literal".to_string(),
                            span,
                        ));
                        return None;
                    };
                    if value.is_empty() || native.replace(value.clone()).is_some() {
                        errors.push(CompileError::type_error(
                            "C struct requires exactly one non-empty `native` field".to_string(),
                            span,
                        ));
                        return None;
                    }
                }
                incan_vocab::IncanStatement::TypedLet {
                    name: field,
                    mutable: false,
                    ty,
                    value: incan_vocab::IncanExpr::Name(value),
                } if field == value => {
                    let Some(ty) = Self::c_binding_type(&ty.source, resources, structs, false) else {
                        errors.push(CompileError::type_error(
                            format!("C struct field `{field}` uses an unsupported C type"),
                            span,
                        ));
                        return None;
                    };
                    fields.push(CBindingStructField {
                        name: field.clone(),
                        ty,
                    });
                }
                _ => {
                    errors.push(CompileError::type_error(
                        "C struct fields must use `field: c.Type = field`".to_string(),
                        span,
                    ));
                    return None;
                }
            }
        }
        let Some(native) = native else {
            errors.push(CompileError::type_error(
                "C struct declarations require `native`".to_string(),
                span,
            ));
            return None;
        };
        if fields.is_empty() {
            errors.push(CompileError::type_error(
                "C struct declarations require at least one field".to_string(),
                span,
            ));
            return None;
        }
        Some(CBindingStruct {
            name: name.to_string(),
            native,
            fields,
        })
    }

    /// Resolve a supported C spelling into the checked binding type model.
    pub fn c_binding_type(
        source: &str,
        resources: &HashSet<String>,
        structs: &HashSet<String>,
        accepts_output: bool,
    ) -> Option<CBindingType> {
        let source = source.trim();
        if c_abi::is_void_type_spelling(source) {
            return Some(CBindingType::Void);
        }
        if let Some(scalar) = c_abi::scalar_type_from_str(source) {
            return Some(CBindingType::Scalar(scalar));
        }
        if let Some(inner) = source.strip_prefix("Option[").and_then(|rest| rest.strip_suffix(']')) {
            let value = Self::c_binding_type(inner, resources, structs, false)?;
            return matches!(
                value,
                CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                }
            )
            .then(|| CBindingType::Nullable(Box::new(value)));
        }
        for (constructor, access) in [
            (ResourceTypeConstructorId::Owned, CResourceAccess::Owned),
            (ResourceTypeConstructorId::Borrowed, CResourceAccess::Borrowed),
            (ResourceTypeConstructorId::BorrowedMut, CResourceAccess::BorrowedMut),
        ] {
            if let Some(inner) = source
                .strip_prefix(c_abi::resource_type_constructor_as_str(constructor))
                .and_then(|rest| rest.strip_prefix('['))
                .and_then(|rest| rest.strip_suffix(']'))
            {
                return resources.contains(inner).then(|| CBindingType::Resource {
                    access,
                    resource: inner.to_string(),
                });
            }
        }
        if accepts_output {
            for (constructor, mode) in [
                (ResourceTypeConstructorId::Out, COutputMode::Out),
                (ResourceTypeConstructorId::InOut, COutputMode::InOut),
            ] {
                if let Some(inner) = source
                    .strip_prefix(c_abi::resource_type_constructor_as_str(constructor))
                    .and_then(|rest| rest.strip_prefix('['))
                    .and_then(|rest| rest.strip_suffix(']'))
                {
                    let value = Self::c_binding_type(inner, resources, structs, false)?;
                    if matches!(
                        (&mode, &value),
                        (
                            COutputMode::InOut,
                            CBindingType::Resource {
                                access: CResourceAccess::Owned,
                                ..
                            }
                        )
                    ) {
                        return None;
                    }
                    return Some(CBindingType::Output {
                        mode,
                        value: Box::new(value),
                    });
                }
            }
        }
        for (constructor, mutable) in [("c.ConstPtr", false), ("c.MutPtr", true)] {
            if let Some(inner) = source
                .strip_prefix(constructor)
                .and_then(|rest| rest.strip_prefix('['))
                .and_then(|rest| rest.strip_suffix(']'))
            {
                return Some(CBindingType::Pointer {
                    mutable,
                    pointee: Box::new(Self::c_binding_type(inner, resources, structs, false)?),
                });
            }
        }
        structs
            .contains(source)
            .then(|| CBindingType::Struct(source.to_string()))
    }

    /// Render a source name or qualified access as a native C constant reference.
    pub(super) fn c_native_reference(value: &incan_vocab::IncanExpr) -> Option<String> {
        match value {
            incan_vocab::IncanExpr::Name(name) if !name.is_empty() => Some(name.clone()),
            incan_vocab::IncanExpr::Field { object, field } if !field.is_empty() => {
                Some(format!("{}.{}", Self::c_native_reference(object)?, field))
            }
            incan_vocab::IncanExpr::RelationField { relation, field } if !relation.is_empty() && !field.is_empty() => {
                Some(format!("{relation}.{field}"))
            }
            _ => None,
        }
    }

    /// Return whether the decorator's leading alias resolves to the activated C vocabulary.
    pub(super) fn c_interop_decorator_is_imported(&self, decorator: &Decorator) -> bool {
        decorator.path.segments.first().is_some_and(|prefix| {
            self.import_binding_path(prefix)
                .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        })
    }

    /// Extract the logical library name and exact link shape from an imported C link declaration.
    pub(super) fn c_system_library_name(&self, value: &Spanned<Expr>) -> Option<(String, LinkCapabilityId)> {
        let (namespace, method, type_args, arguments) = match &value.node {
            Expr::MethodCall(namespace, method, type_args, arguments) => (
                namespace.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            Expr::Call(callee, type_args, arguments) => {
                let Expr::Field(namespace, method) = &callee.node else {
                    return None;
                };
                (
                    namespace.as_ref(),
                    method.as_str(),
                    type_args.as_slice(),
                    arguments.as_slice(),
                )
            }
            _ => return None,
        };
        if !type_args.is_empty() {
            return None;
        }
        let link_capability = c_abi::link_capability_from_str(method)?;
        let Expr::Ident(namespace_name) = &namespace.node else {
            return None;
        };
        if !self
            .import_binding_path(namespace_name)
            .is_some_and(|path| c_abi::is_interop_namespace_path(path.iter().map(String::as_str)))
        {
            return None;
        }
        match arguments {
            [CallArg::Positional(value)] => match &value.node {
                Expr::Literal(Literal::String(value)) if !value.is_empty() => Some((value.clone(), link_capability)),
                _ => None,
            },
            _ => None,
        }
    }
}
