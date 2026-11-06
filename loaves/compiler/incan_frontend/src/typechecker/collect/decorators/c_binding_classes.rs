//! C binding class validation and member collection.

use super::*;

impl TypeChecker {
    /// Validate the source-owned descriptor attached to a checked C binding class.
    ///
    /// The import-activated `binding` vocabulary lowers to this ordinary decorator/class form, so the parser transports
    /// only vocabulary data while this language-layer check owns all explicit, inspectable ABI meaning.
    pub fn validate_c_binding_class(&mut self, class: &ClassDecl) {
        // A class must contribute one descriptor at most. Rejecting the duplicate before parsing its fields avoids
        // constructing a partial descriptor whose source authority would depend on decorator ordering.
        let bindings = class
            .decorators
            .iter()
            .filter(|decorator| self.decorator_id(&decorator.node) == Some(DecoratorId::CBinding))
            .collect::<Vec<_>>();
        if bindings.len() > 1 {
            self.errors.push(CompileError::type_error(
                "a class may declare only one @c.binding descriptor".to_string(),
                bindings[1].span,
            ));
            return;
        }
        let Some(decorator) = bindings.first() else {
            return;
        };
        let mut valid = true;
        if !self.c_interop_decorator_is_imported(&decorator.node) {
            self.errors.push(CompileError::type_error(
                "@c.binding requires `from std.interop import c` (or an alias of c)".to_string(),
                decorator.span,
            ));
            valid = false;
        }
        if class.extends.as_deref() != Some(c_abi::BINDING_DECLARATION_BASE) {
            self.errors.push(CompileError::type_error(
                "@c.binding classes must extend BindingDeclaration".to_string(),
                decorator.span,
            ));
            valid = false;
        }

        // The decorator is the declaration's outer envelope: header and link facts live here, while its vocabulary
        // members describe the named ABI surface below. Keep these channels separate so neither can imply the other.
        let mut header = None;
        let mut link = None;
        for argument in &decorator.node.args {
            let DecoratorArg::Named(name, DecoratorArgValue::Expr(value)) = argument else {
                self.errors.push(CompileError::type_error(
                    "@c.binding accepts only named `header` and `link` arguments".to_string(),
                    decorator.span,
                ));
                valid = false;
                continue;
            };
            let slot = match c_abi::binding_argument_from_str(name) {
                Some(BindingArgumentId::Header) => &mut header,
                Some(BindingArgumentId::Link) => &mut link,
                None => {
                    self.errors.push(CompileError::type_error(
                        format!("@c.binding does not accept argument `{name}`"),
                        value.span,
                    ));
                    valid = false;
                    continue;
                }
            };
            if slot.replace(value).is_some() {
                self.errors.push(CompileError::type_error(
                    format!("@c.binding repeats `{name}`"),
                    value.span,
                ));
                valid = false;
            }
        }

        let header = match header.and_then(|header| match &header.node {
            Expr::Literal(Literal::String(value)) if !value.is_empty() => Some(value.clone()),
            _ => None,
        }) {
            Some(header) => header,
            None => {
                self.errors.push(CompileError::type_error(
                    "@c.binding requires a named non-empty string-literal `header` argument".to_string(),
                    decorator.span,
                ));
                valid = false;
                String::new()
            }
        };
        let link = link.and_then(|value| self.c_system_library_name(value));
        let (system_library, link_capability) = match link {
            Some(link) => link,
            None => {
                self.errors.push(CompileError::type_error(
                    "@c.binding requires `link = c.system_library(\"name\")` or `c.framework(\"name\")`".to_string(),
                    decorator.span,
                ));
                valid = false;
                (String::new(), LinkCapabilityId::SystemLibrary)
            }
        };

        // Do not register an incomplete descriptor. Later access checking and lowering intentionally consume only this
        // complete checked product, never the raw class body or a best-effort subset of its declarations.
        let Some(members) = self.validate_c_binding_members(class, decorator.span) else {
            return;
        };
        if valid {
            self.type_info.c_abi.bindings.insert(
                class.name.clone(),
                CBindingDescriptor {
                    span: decorator.span,
                    class_name: class.name.clone(),
                    header,
                    system_library,
                    link_capability,
                    resources: members.resources,
                    symbols: members.symbols,
                    enums: members.enums,
                    structs: members.structs,
                },
            );
        }
    }

    /// Validate declarative C members retained by the ordinary lowered class.
    pub(super) fn validate_c_binding_members(
        &mut self,
        class: &ClassDecl,
        fallback_span: Span,
    ) -> Option<CBindingMembers> {
        let mut valid = true;
        let mut resources = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut raw_symbols = Vec::<RawCBindingSymbol>::new();

        // Collect declared type names first so a signature can name a structure or resource declared later.
        let struct_names = Self::c_binding_declared_names(class, BindingMemberId::Struct);
        let resource_names = Self::c_binding_declared_names(class, BindingMemberId::Resource);

        for member in &class.declarative_members {
            let span = Self::c_member_span(member, fallback_span);
            let Some(kind) = c_abi::binding_member_from_str(&member.keyword) else {
                self.errors.push(CompileError::type_error(
                    format!("@c.binding does not accept `{}` declarations", member.keyword),
                    span,
                ));
                valid = false;
                continue;
            };
            let Some(name) = member.head.name.as_ref().filter(|name| !name.is_empty()) else {
                self.errors.push(CompileError::type_error(
                    format!("C {} declarations require a name", member.keyword),
                    span,
                ));
                valid = false;
                continue;
            };
            if !member.decorators.is_empty() || !member.head.header_args.is_empty() {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C {} declarations do not accept decorators or header arguments",
                        member.keyword
                    ),
                    span,
                ));
                valid = false;
                continue;
            }
            // Every accepted member becomes data in the descriptor. In particular, `symbol` owns no executable body:
            // it maps one Incan-facing signature to one explicit native spelling for later target verification.
            match kind {
                BindingMemberId::Resource => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C resource declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some((native, release)) = Self::c_resource_fields(member, span, &mut self.errors) else {
                        valid = false;
                        continue;
                    };
                    resources.push(CBindingResource {
                        span,
                        name: name.clone(),
                        native,
                        release,
                    });
                }
                BindingMemberId::Symbol => {
                    let (symbol, symbol_valid) =
                        self.validate_raw_c_binding_symbol(member, name, span, &resource_names, &struct_names);
                    valid &= symbol_valid;
                    if let Some(symbol) = symbol {
                        raw_symbols.push(symbol);
                    }
                }
                BindingMemberId::Enum => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C enum declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some(enumeration) = Self::c_enum(member, name, span, &mut self.errors) else {
                        valid = false;
                        continue;
                    };
                    enums.push(enumeration);
                }
                BindingMemberId::Struct => {
                    if !member.head.parameters.is_empty() || member.head.return_type.is_some() {
                        self.errors.push(CompileError::type_error(
                            "C struct declarations may not have a signature".to_string(),
                            span,
                        ));
                        valid = false;
                        continue;
                    }
                    let Some(structure) =
                        Self::c_struct(member, name, span, &resource_names, &struct_names, &mut self.errors)
                    else {
                        valid = false;
                        continue;
                    };
                    structs.push(structure);
                }
            }
        }

        if !self.c_binding_member_names_unique(&raw_symbols, &enums, &resources, &structs, fallback_span) {
            valid = false;
        }
        self.finish_c_binding_members(raw_symbols, resources, enums, structs, valid)
    }

    /// Collect one member kind's declared names without validating or reordering its declarations. Forward C
    /// signature references use this inventory while the member pass retains ownership of diagnostics.
    pub(super) fn c_binding_declared_names(class: &ClassDecl, kind: BindingMemberId) -> HashSet<String> {
        class
            .declarative_members
            .iter()
            .filter(|member| c_abi::binding_member_from_str(&member.keyword) == Some(kind))
            .filter_map(|member| member.head.name.clone())
            .collect()
    }

    /// Check one raw C symbol without suppressing independent parameter errors or its later data validation. The
    /// validity flag is separate from the retained symbol because invalid parameters still leave outcome/buffer
    /// checks to the descriptor's final pass, preserving error accumulation.
    pub(super) fn validate_raw_c_binding_symbol(
        &mut self,
        member: &incan_vocab::VocabDeclaration,
        name: &str,
        span: Span,
        resource_names: &HashSet<String>,
        struct_names: &HashSet<String>,
    ) -> (Option<RawCBindingSymbol>, bool) {
        let Some(return_type) = member.head.return_type.as_ref() else {
            self.errors.push(CompileError::type_error(
                "C symbols require an explicit return type".to_string(),
                span,
            ));
            return (None, false);
        };
        let Some(return_type) = Self::c_binding_type(&return_type.source, resource_names, struct_names, false) else {
            self.errors.push(CompileError::type_error(
                format!("C symbol `{name}` uses an unsupported return type"),
                span,
            ));
            return (None, false);
        };
        let mut valid = true;
        let mut parameters = Vec::new();
        for parameter in &member.head.parameters {
            let Some(parameter_type) = parameter.param_type.as_ref() else {
                self.errors.push(CompileError::type_error(
                    format!("C symbol `{name}` parameters require explicit types"),
                    span,
                ));
                valid = false;
                continue;
            };
            let Some(parameter_type) = Self::c_binding_type(&parameter_type.source, resource_names, struct_names, true)
            else {
                self.errors.push(CompileError::type_error(
                    format!("C symbol `{name}` uses an unsupported parameter type"),
                    span,
                ));
                valid = false;
                continue;
            };
            if parameter.default_value.is_some() || parameter.name.is_empty() {
                self.errors.push(CompileError::type_error(
                    format!("C symbol `{name}` does not accept default or unnamed parameters"),
                    span,
                ));
                valid = false;
                continue;
            }
            parameters.push(CBindingParameter {
                name: parameter.name.clone(),
                ty: parameter_type,
            });
        }
        let Some((native, bounds, outcomes)) = Self::c_symbol_data(member, span, &mut self.errors) else {
            return (None, false);
        };
        (
            Some(RawCBindingSymbol {
                span,
                name: name.to_string(),
                native,
                parameters,
                return_type,
                bounds,
                outcomes,
            }),
            valid,
        )
    }

    /// Require unique names within each C member kind after every raw declaration has been checked. Preserve the
    /// existing kind order and single aggregate diagnostic rather than suppressing later signature validation.
    pub(super) fn c_binding_member_names_unique(
        &mut self,
        raw_symbols: &[RawCBindingSymbol],
        enums: &[CBindingEnum],
        resources: &[CBindingResource],
        structs: &[CBindingStruct],
        span: Span,
    ) -> bool {
        if raw_symbols
            .iter()
            .map(|symbol| &symbol.name)
            .collect::<HashSet<_>>()
            .len()
            != raw_symbols.len()
            || enums
                .iter()
                .map(|enumeration| &enumeration.name)
                .collect::<HashSet<_>>()
                .len()
                != enums.len()
            || resources
                .iter()
                .map(|resource| &resource.name)
                .collect::<HashSet<_>>()
                .len()
                != resources.len()
            || structs
                .iter()
                .map(|structure| &structure.name)
                .collect::<HashSet<_>>()
                .len()
                != structs.len()
        {
            self.errors.push(CompileError::type_error(
                "C binding members must have unique names within their declaration kind".to_string(),
                span,
            ));
            return false;
        }
        true
    }

    /// Validate raw symbol outcomes and buffers before resource release signatures, then publish the complete
    /// descriptor only when every collection and cross-member check succeeded. Invalid raw declarations still
    /// contribute their existing later diagnostics rather than short-circuiting the whole binding.
    pub(super) fn finish_c_binding_members(
        &mut self,
        raw_symbols: Vec<RawCBindingSymbol>,
        resources: Vec<CBindingResource>,
        enums: Vec<CBindingEnum>,
        structs: Vec<CBindingStruct>,
        mut valid: bool,
    ) -> Option<CBindingMembers> {
        let mut symbols = Vec::with_capacity(raw_symbols.len());
        for raw_symbol in raw_symbols {
            let Some(outcomes) = Self::c_symbol_outcomes(
                raw_symbol.outcomes,
                &raw_symbol.name,
                &raw_symbol.return_type,
                &raw_symbol.parameters,
                &enums,
                raw_symbol.span,
                &mut self.errors,
            ) else {
                valid = false;
                continue;
            };
            let Some(buffers) = Self::c_symbol_buffer_contracts(
                raw_symbol.bounds,
                &raw_symbol.name,
                &raw_symbol.parameters,
                raw_symbol.span,
                &mut self.errors,
            ) else {
                valid = false;
                continue;
            };
            symbols.push(CBindingSymbol {
                name: raw_symbol.name,
                native: raw_symbol.native,
                parameters: raw_symbol.parameters,
                return_type: raw_symbol.return_type,
                buffers,
                outcomes,
            });
        }

        for resource in &resources {
            let release = symbols.iter().find(|symbol| symbol.name == resource.release);
            if !release.is_some_and(|symbol| {
                matches!(
                    symbol.parameters.as_slice(),
                    [CBindingParameter {
                        ty: CBindingType::Resource {
                            access: CResourceAccess::Owned,
                            resource: parameter_resource,
                        },
                        ..
                    }] if parameter_resource == &resource.name
                )
            }) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C resource `{}` release symbol `{}` must accept exactly c.Owned[{}]",
                        resource.name, resource.release, resource.name
                    ),
                    resource.span,
                ));
                valid = false;
            }
        }

        valid.then_some(CBindingMembers {
            resources,
            symbols,
            enums,
            structs,
        })
    }

    /// Prefer a vocabulary member's source span, retaining the binding span for synthesized members.
    pub(super) fn c_member_span(member: &incan_vocab::VocabDeclaration, fallback: Span) -> Span {
        if member.span.end > member.span.start {
            Span::new(member.span.start, member.span.end).with_expansion(fallback.expansion)
        } else {
            fallback
        }
    }
}
