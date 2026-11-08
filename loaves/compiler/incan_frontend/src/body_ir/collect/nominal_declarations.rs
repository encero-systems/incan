//! Local enum, nominal, newtype, fieldless enum, value enum and trait declarations.

use super::*;

/// Retain canonical normal-enum layouts from checked annotation and derive facts, never syntax-based type guesses.
pub(in crate::body_ir) fn collect_local_enum_declarations(
    program: &ast::Program,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Vec<bir::EnumDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Enum(value) = &declaration.node else {
                return None;
            };
            if !is_direct_native_enum(value) {
                return None;
            }
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)?
                .clone();
            let parameter_names = value
                .type_params
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect::<Vec<_>>();
            let variants = value
                .variants
                .iter()
                .map(|variant| {
                    let canonical = type_info
                        .declarations
                        .member_declaration_identities
                        .get(&variant.span)?
                        .clone();
                    let fields = variant
                        .node
                        .fields
                        .iter()
                        .map(|field| {
                            type_info
                                .declarations
                                .enum_payload_types
                                .get(&field.span)
                                .map(|ty| parameter_types::checked_parameter_type(ty, type_info, &parameter_names))
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some(bir::EnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_at(module_identity, variant.span),
                        canonical,
                        name: variant.node.name.clone(),
                        fields,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::EnumDeclaration {
                native_item: String::new(),
                native_type_arguments: Vec::new(),
                direct_declaration_id: CompilerNodeId::declaration_at(module_identity, declaration.span),
                canonical,
                name: value.name.clone(),
                public: value.visibility == ast::Visibility::Public,
                type_parameters: value
                    .type_params
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect(),
                variants,
                derives: type_info.declarations.enum_derives.get(&value.name)?.clone(),
                named_type_identities: parameter_types::retained_type_identities(type_info),
            })
        })
        .collect()
}

/// Retain directly executable model, class, and plain newtype declarations in source order.
///
/// Constructor argument binding already comes from the typechecker; this adds only the source-local declaration
/// identity and canonical raw field order the direct runtime otherwise could not establish without reopening AST or
/// typechecker state. This deliberately does not retain a general nominal registry.
pub(in crate::body_ir) fn collect_local_nominal_declarations(
    program: &ast::Program,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Vec<bir::NominalDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            if let ast::Declaration::Newtype(newtype) = &declaration.node {
                return collect_plain_newtype(declaration.span, newtype, module_identity, type_info);
            }
            let (name, fields, visibility, type_parameters, class_layout) = match &declaration.node {
                ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => (
                    &model.name,
                    &model.fields,
                    model.visibility,
                    model
                        .type_params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect::<Vec<_>>(),
                    None,
                ),
                ast::Declaration::Class(class) if is_direct_replacement_checked_class(class, type_info) => (
                    &class.name,
                    &class.fields,
                    class.visibility,
                    type_info
                        .declarations
                        .class_layouts
                        .get(&class.name)?
                        .type_params
                        .clone(),
                    Some(type_info.declarations.class_layouts.get(&class.name)?),
                ),
                _ => return None,
            };
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)?
                .clone();
            let field_identities = fields
                .iter()
                .map(|field| {
                    type_info
                        .declarations
                        .member_declaration_identities
                        .get(&field.span)
                        .cloned()
                })
                .collect::<Option<Vec<_>>>()?;
            let retained = bir::NominalDeclaration {
                trait_adoptions: checked_trait_adoptions(name, &type_parameters, type_info),
                native_item: String::new(),
                class_inheritance: None,
                native_type_arguments: Vec::new(),
                direct_declaration_id: CompilerNodeId::declaration_at(module_identity, declaration.span),
                canonical,
                name: name.clone(),
                fields: fields.iter().map(|field| field.node.name.clone()).collect(),
                field_identities,
                owned_callable_fields: Vec::new(),
                field_public: fields
                    .iter()
                    .map(|field| {
                        if let Some(layout) = class_layout {
                            layout
                                .fields
                                .iter()
                                .find(|checked| checked.name == field.node.name)
                                .map(|checked| checked.visibility == ast::Visibility::Public)
                        } else {
                            type_info
                                .declarations
                                .model_field_visibilities
                                .get(name)
                                .and_then(|fields| fields.get(&field.node.name))
                                .map(|visibility| *visibility == ast::Visibility::Public)
                        }
                    })
                    .collect::<Option<Vec<_>>>()?,
                field_aliases: fields.iter().map(|field| field.node.metadata.alias.clone()).collect(),
                comparison_equality: None,
                reflection_metadata: if class_layout.is_none() {
                    collect_field_metadata(name, fields, type_info)
                } else {
                    None
                },
                reflection_fields: class_layout.is_none().then(|| {
                    fields
                        .iter()
                        .enumerate()
                        .filter_map(|(slot, field)| {
                            (!type_info
                                .declarations
                                .model_type_private_fields
                                .contains(&(name.clone(), field.node.name.clone())))
                            .then_some(slot)
                        })
                        .collect()
                }),
                public: visibility == ast::Visibility::Public,
                has_field_defaults: fields.iter().any(|field| field.node.default.is_some()),
                field_default_body: None,
                method_partial_defaults: Vec::new(),
                display_method: checked_display_method(&declaration.node, type_info),
                derives: nominal_derives(name, class_layout.is_some(), type_info)?,
                newtype_deserialize: bir::NewtypeDeserialize::Unselected,
                newtype_string_conversion: bir::NewtypeStringConversion::Unselected,
                field_types: nominal_field_types(fields, class_layout, &type_parameters, type_info),
                named_type_identities: parameter_types::retained_type_identities(type_info),
                type_parameter_count: type_parameters.len(),
                type_parameters,
            };
            retain_inherited_layout(retained, type_info)
        })
        .collect()
}

/// Retain source field order while resolving types from the checked class layout or span-keyed model facts. A field
/// written exactly as a declaration parameter keeps that template even when a checked constructor has already closed
/// one use; physical instance specialization owns its replacement. Missing facts retain an unknown carrier.
pub(super) fn nominal_field_types(
    fields: &[ast::Spanned<ast::FieldDecl>],
    class_layout: Option<&ClassLayoutInfo>,
    type_parameters: &[String],
    type_info: &TypeCheckInfo,
) -> Vec<IncanType> {
    fields
        .iter()
        .map(|field| {
            if let ast::Type::Simple(name) = &field.node.ty.node
                && type_parameters.contains(name)
            {
                return IncanType::TypeVar(name.clone());
            }
            if let Some(layout) = class_layout {
                layout
                    .fields
                    .iter()
                    .find(|checked| checked.name == field.node.name)
                    .map(|checked| parameter_types::checked_parameter_type(&checked.ty, type_info, type_parameters))
                    .unwrap_or(IncanType::Unknown)
            } else {
                type_info
                    .declarations
                    .model_field_types
                    .get(&field.span)
                    .map(|ty| parameter_types::checked_parameter_type(ty, type_info, type_parameters))
                    .unwrap_or(IncanType::Unknown)
            }
        })
        .collect()
}

/// Retain the checked derive selection and adoption effects without changing legacy normalization tables.
pub(super) fn nominal_derives(name: &str, class: bool, type_info: &TypeCheckInfo) -> Option<Vec<String>> {
    if !class {
        return type_info.declarations.body_ir_model_derives.get(name).cloned();
    }
    let mut derives = type_info.declarations.class_derives.get(name)?.clone();
    for implied in type_info
        .declarations
        .class_adoption_derives
        .get(name)
        .into_iter()
        .flatten()
    {
        if !derives.contains(implied) {
            derives.push(implied.clone());
        }
    }
    Some(derives)
}

/// Retain class reflection and the checked parent-first field layout with ancestor identities.
pub(super) fn retain_inherited_layout(
    mut retained: bir::NominalDeclaration,
    type_info: &TypeCheckInfo,
) -> Option<bir::NominalDeclaration> {
    retain_class_reflection(&mut retained, type_info);
    let Some(facts) = type_info.declarations.body_ir_class_inheritance.get(&retained.name) else {
        return Some(retained);
    };
    let layout = type_info.declarations.class_layouts.get(&retained.name)?;
    if facts.field_identities.len() != layout.fields.len()
        || !layout.missing_fields.is_empty()
        || !layout.unmaterializable_defaults.is_empty()
        || layout.fields.iter().any(|field| field.default.is_some())
    {
        return None;
    }
    retained.fields = layout.fields.iter().map(|field| field.name.clone()).collect();
    retained.field_identities = facts.field_identities.clone();
    retained.field_aliases = layout.fields.iter().map(|field| field.alias.clone()).collect();
    retained.field_types = layout
        .fields
        .iter()
        .map(|field| parameter_types::checked_semantic_type(&field.ty, type_info))
        .collect();
    retained.field_public = layout
        .fields
        .iter()
        .map(|field| field.visibility == ast::Visibility::Public)
        .collect();
    retained.class_inheritance = Some(facts.inheritance.clone());
    retained.has_field_defaults = false;
    Some(retained)
}

/// Retain a class's public field policy and presentation metadata from its checked parent-first layout.
/// These slots also govern typed overlays, so inherited and local fields share the same visibility decision.
pub(super) fn retain_class_reflection(retained: &mut bir::NominalDeclaration, facts: &TypeCheckInfo) {
    let Some(layout) = facts.declarations.class_layouts.get(&retained.name) else {
        return;
    };
    retained.reflection_fields = Some(
        layout
            .fields
            .iter()
            .enumerate()
            .filter_map(|(slot, field)| (field.visibility == ast::Visibility::Public).then_some(slot))
            .collect(),
    );
    retained.reflection_metadata = Some(
        layout
            .fields
            .iter()
            .filter(|field| field.visibility == ast::Visibility::Public)
            .map(|field| bir::FieldMetadata {
                name: field.name.clone(),
                record_identity: field.record_identity.clone(),
                record_canonical: field.record_canonical,
                renamed_from: field.renamed_from.clone(),
                alias: field.alias.clone(),
                description: field.description.clone(),
                type_name: field.surface_type_name.clone().unwrap_or_else(|| field.ty.to_string()),
                has_default: field.default.is_some(),
            })
            .collect(),
    );
}

/// Project checked adoptions after inference, retaining lexical binders before resolving runtime carrier names.
/// Final clause witnesses are Body IR-only; legacy's checked adoption arguments are never rewritten.
pub(super) fn checked_trait_adoptions(
    owner: &str,
    parameters: &[String],
    facts: &TypeCheckInfo,
) -> Vec<bir::TraitAdoption> {
    facts
        .declarations
        .body_ir_trait_adoptions
        .get(owner)
        .into_iter()
        .flatten()
        .map(|adoption| {
            let key = adoption.span;
            let arguments = facts
                .traits
                .body_ir_adoption_type_args
                .get(&key)
                .or_else(|| facts.traits.adoption_type_args.get(&key))
                .unwrap_or(&adoption.arguments);
            bir::TraitAdoption {
                trait_identity: adoption.trait_identity.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| parameter_types::checked_parameter_type(argument, facts, parameters))
                    .collect(),
                methods: adoption.methods.clone(),
                primitive_adapter: None,
                fields: adoption.fields.clone(),
            }
        })
        .collect()
}

/// Retain a newtype's checked carrier as one canonical tuple slot, with declaration-owned placeholders that only
/// checked concrete uses may specialize. Integer primitive constraints retain a raw carrier layout while construction
/// sites retain their validation; the layout alone does not authorize unchecked construction. Nongeneric source hooks
/// retain canonical checked ingress. String Deserialize hooks retain an explicit reconstruction selection, including
/// an unbounded identity-binder template that only exact checked str uses may close. Other generic hooks and
/// noninteger constraints remain refused.
pub(super) fn collect_plain_newtype(
    span: ast::Span,
    newtype: &ast::NewtypeDecl,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Option<bir::NominalDeclaration> {
    if !is_direct_replacement_checked_newtype(newtype, type_info) {
        return None;
    }
    let facts = type_info.declarations.newtype_construction.get(&newtype.name)?;
    let checked_deserialize = checked_json::checked_deserialize_selection(newtype, type_info);
    if facts.checked_constructor.is_some()
        && ((!facts.type_params.is_empty() && checked_deserialize.is_none())
            || facts.checked_constructor_identity.is_none())
    {
        return None;
    }
    let canonical = type_info.declarations.declaration_identities.get(&span)?.clone();
    let underlying = parameter_types::checked_parameter_type(&facts.underlying, type_info, &facts.type_params);
    if !facts.constraints.is_empty() && underlying != IncanType::Primitive(IncanPrimitiveType::Int) {
        return None;
    }
    let mut derives = facts.automatic_derives.clone();
    if matches!(
        underlying,
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool)
    ) {
        if !derives
            .iter()
            .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
        {
            derives.push(derives::as_str(DeriveId::Clone).to_owned());
        }
        derives.push(derives::as_str(DeriveId::Copy).to_owned());
    }
    for selection in type_info.declarations.body_ir_newtype_derives.get(&newtype.name)? {
        if !derives.contains(selection) {
            derives.push(selection.clone());
        }
    }
    let newtype_deserialize = match checked_deserialize {
        Some(hook) => bir::NewtypeDeserialize::Checked(hook),
        None if derives.iter().any(|derive| derive == "serde::Deserialize") => bir::NewtypeDeserialize::Ordinary,
        None => bir::NewtypeDeserialize::Unselected,
    };
    let newtype_string_conversion =
        super::newtype_string_conversion::automatic_candidate(newtype, &canonical, type_info);
    Some(bir::NominalDeclaration {
        trait_adoptions: checked_trait_adoptions(&newtype.name, &facts.type_params, type_info),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_at(module_identity, span),
        field_identities: vec![canonical.clone()],
        canonical,
        name: newtype.name.clone(),
        fields: vec!["0".to_owned()],
        field_types: vec![underlying],
        owned_callable_fields: Vec::new(),
        field_public: vec![true],
        field_aliases: vec![None],
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: newtype.visibility == ast::Visibility::Public,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize,
        newtype_string_conversion,
        derives,
        named_type_identities: parameter_types::retained_type_identities(type_info),
        type_parameters: facts.type_params.clone(),
        type_parameter_count: facts.type_params.len(),
    })
}

/// Retain the complete visible field records once, with the same checked visibility and presentation contract as
/// legacy. Missing checked types make the entire metadata surface unavailable instead of silently omitting a field.
pub(super) fn collect_field_metadata(
    name: &str,
    fields: &[ast::Spanned<ast::FieldDecl>],
    facts: &TypeCheckInfo,
) -> Option<Vec<bir::FieldMetadata>> {
    fields
        .iter()
        .filter(|field| {
            !facts
                .declarations
                .model_type_private_fields
                .contains(&(name.to_owned(), field.node.name.clone()))
        })
        .map(|field| {
            let ty = facts.declarations.model_field_types.get(&field.span)?;
            Some(bir::FieldMetadata {
                name: field.node.name.clone(),
                record_identity: crate::symbols::field_record_identity(&field.node),
                record_canonical: crate::symbols::field_record_canonical(&field.node),
                renamed_from: crate::symbols::field_renamed_from(&field.node),
                alias: field.node.metadata.alias.clone(),
                description: field.node.metadata.description.clone(),
                type_name: crate::symbols::field_surface_type_name(&field.node.ty.node, ty),
                has_default: field.node.default.is_some(),
            })
        })
        .collect()
}

/// Retain the checked string method, preferring an explicit declaration to the Error message fallback selected for
/// the type. Derive-only Display keeps its existing Debug rendering and has no source method identity here.
pub(super) fn checked_display_method(
    declaration: &ast::Declaration,
    type_info: &TypeCheckInfo,
) -> Option<CanonicalSymbolId> {
    let (name, methods) = match declaration {
        ast::Declaration::Model(model) => (&model.name, &model.methods),
        ast::Declaration::Class(class) => (&class.name, &class.methods),
        _ => return None,
    };
    if let Some(method) = methods.iter().find(|method| method.node.name == "__str__") {
        return type_info
            .declarations
            .method_bindings_by_span
            .get(&method.span)?
            .identity
            .clone();
    }
    type_info.error_message_display_type(name)?.identity.clone()
}

/// Retain exact source-local fieldless normal-enum declaration and unit-member facts in source order.
///
/// Only this registry reaches the direct runtime. It deliberately has no payload layouts, aliases, match facts, or
/// source-symbol lookup facility, so its existence cannot widen into general enum execution by spelling alone.
pub(in crate::body_ir) fn collect_local_fieldless_enum_declarations(
    program: &ast::Program,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Vec<bir::FieldlessEnumDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Enum(enum_decl) = &declaration.node else {
                return None;
            };
            if !is_direct_replacement_fieldless_enum(enum_decl) {
                return None;
            }
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)?
                .clone();
            let variants = enum_decl
                .variants
                .iter()
                .map(|variant| {
                    let canonical = type_info
                        .declarations
                        .member_declaration_identities
                        .get(&variant.span)?
                        .clone();
                    Some(bir::FieldlessEnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_at(module_identity, variant.span),
                        canonical,
                        name: variant.node.name.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::FieldlessEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_at(module_identity, declaration.span),
                canonical,
                name: enum_decl.name.clone(),
                variants,
            })
        })
        .collect()
}

/// Retain exact source-local RFC 032 value-enum declaration and canonical literal-member facts in source order.
///
/// A later direct executor receives only this Body-IR registry. It does not reopen AST/typechecker state to resolve
/// a `Name.Member` spelling, so lowering returns no record for imports, aliases, ordinary enums, or declarations
/// whose shape cannot truthfully support the generated scalar `.value()` surface.
pub(in crate::body_ir) fn collect_local_value_enum_declarations(
    program: &ast::Program,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Vec<bir::ValueEnumDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Enum(enum_decl) = &declaration.node else {
                return None;
            };
            if !is_direct_replacement_value_enum(enum_decl) {
                return None;
            }
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)?
                .clone();
            let backing = match enum_decl.value_type.as_ref().map(|value| value.node) {
                Some(ast::ValueEnumType::Int) => bir::ValueEnumBacking::Int,
                Some(ast::ValueEnumType::Str) => bir::ValueEnumBacking::Str,
                None => return None,
            };
            let variants = enum_decl
                .variants
                .iter()
                .filter_map(|variant| {
                    let canonical = type_info
                        .declarations
                        .member_declaration_identities
                        .get(&variant.span)?
                        .clone();
                    let raw_value = match variant.node.value.as_ref().map(|value| &value.node) {
                        Some(ast::ValueEnumLiteral::Int(value)) if matches!(backing, bir::ValueEnumBacking::Int) => {
                            bir::Constant::Int(value.value)
                        }
                        Some(ast::ValueEnumLiteral::Str(value)) if matches!(backing, bir::ValueEnumBacking::Str) => {
                            bir::Constant::Str(value.clone())
                        }
                        _ => return None,
                    };
                    Some(bir::ValueEnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_at(module_identity, variant.span),
                        canonical,
                        name: variant.node.name.clone(),
                        raw_value,
                    })
                })
                .collect::<Vec<_>>();
            (variants.len() == enum_decl.variants.len()).then(|| bir::ValueEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_at(module_identity, declaration.span),
                canonical,
                name: enum_decl.name.clone(),
                backing,
                variants,
            })
        })
        .collect()
}

/// Retain physical source trait owners so consumers can validate their default-method bodies.
pub(in crate::body_ir) fn collect_local_trait_declarations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
) -> Vec<CanonicalSymbolId> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            if !matches!(declaration.node, ast::Declaration::Trait(_)) {
                return None;
            }
            type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)
                .cloned()
        })
        .collect()
}
