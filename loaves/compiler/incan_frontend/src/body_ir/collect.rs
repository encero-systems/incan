//! One-pass collection of the module-local facts lowering retains: defaults, declarations, and canonical member
//! layouts.

use super::*;

/// Retain only scalar literal statics, whose lazy initialization has no user-visible evaluation effects.
pub(super) fn collect_scalar_statics(program: &ast::Program, type_info: &TypeCheckInfo) -> Vec<bir::StaticDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Static(storage) = &declaration.node else {
                return None;
            };
            let ast::Expr::Literal(literal) = &storage.value.node else {
                return None;
            };
            let ty = semantic_type_from_resolved(type_info.expr_type(storage.value.span)?);
            if !matches!(
                ty,
                IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool)
            ) {
                return None;
            }
            let initial = primitives::lower_checked_literal(literal, &ty);
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            Some(bir::StaticDeclaration { canonical, ty, initial })
        })
        .collect()
}

/// Retain canonical normal-enum layouts from checked annotation and derive facts, never syntax-based type guesses.
pub(super) fn collect_local_enum_declarations(
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
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            let variants = value
                .variants
                .iter()
                .map(|variant| {
                    let canonical = type_info
                        .declarations
                        .member_declaration_identities
                        .get(&(variant.span.start, variant.span.end))?
                        .clone();
                    let fields = variant
                        .node
                        .fields
                        .iter()
                        .map(|field| {
                            type_info
                                .declarations
                                .enum_payload_types
                                .get(&(field.span.start, field.span.end))
                                .map(semantic_type_from_resolved)
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some(bir::EnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_span(
                            module_identity,
                            variant.span.start,
                            variant.span.end,
                        ),
                        canonical,
                        name: variant.node.name.clone(),
                        fields,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::EnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                canonical,
                name: value.name.clone(),
                public: value.visibility == ast::Visibility::Public,
                variants,
                derives: type_info.declarations.enum_derives.get(&value.name)?.clone(),
            })
        })
        .collect()
}

/// Collect the source expressions a synthesized local partial needs to retain target defaults in Body IR.
pub(super) fn collect_function_default_sources(program: &ast::Program) -> FunctionDefaultSources {
    program
        .declarations
        .iter()
        .filter_map(|decl| match &decl.node {
            ast::Declaration::Function(function) => Some((
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| FunctionDefaultSource {
                        param_span: param.span,
                        default: param.node.default.clone(),
                    })
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}
/// Collect the exact source spans eligible for same-module direct named-call dispatch.
pub(super) fn collect_local_function_declarations(program: &ast::Program) -> LocalFunctionDeclarations {
    let mut declarations = LocalFunctionDeclarations::new();
    for declaration in &program.declarations {
        if let ast::Declaration::Function(function) = &declaration.node {
            declarations
                .entry(function.name.clone())
                .or_default()
                .push(declaration.span);
        }
    }
    declarations
}
/// Retain directly executable model, class, and plain newtype declarations in source order.
///
/// Constructor argument binding already comes from the typechecker; this adds only the source-local declaration
/// identity and canonical raw field order the direct runtime otherwise could not establish without reopening AST or
/// typechecker state. This deliberately does not retain a general nominal registry.
pub(super) fn collect_local_nominal_declarations(
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
            let (name, fields, visibility, type_parameter_count, class_layout) = match &declaration.node {
                ast::Declaration::Model(model) if is_direct_replacement_plain_model(model) => (
                    &model.name,
                    &model.fields,
                    model.visibility,
                    model.type_params.len(),
                    None,
                ),
                ast::Declaration::Class(class) if is_direct_replacement_class(class) => (
                    &class.name,
                    &class.fields,
                    class.visibility,
                    class.type_params.len(),
                    Some(type_info.declarations.class_layouts.get(&class.name)?),
                ),
                _ => return None,
            };
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            let field_identities = fields
                .iter()
                .map(|field| {
                    type_info
                        .declarations
                        .member_declaration_identities
                        .get(&(field.span.start, field.span.end))
                        .cloned()
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::NominalDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                canonical,
                name: name.clone(),
                fields: fields.iter().map(|field| field.node.name.clone()).collect(),
                field_identities,
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
                public: visibility == ast::Visibility::Public,
                has_field_defaults: fields.iter().any(|field| field.node.default.is_some()),
                derives: incan_lang::lang::derives::plain_model_derives()
                    .map(str::to_owned)
                    .to_vec(),
                field_types: fields
                    .iter()
                    .map(|field| {
                        if let Some(layout) = class_layout {
                            layout
                                .fields
                                .iter()
                                .find(|checked| checked.name == field.node.name)
                                .map(|checked| semantic_type_from_resolved(&checked.ty))
                                .unwrap_or(IncanType::Unknown)
                        } else {
                            type_info
                                .declarations
                                .model_field_types
                                .get(&(field.span.start, field.span.end))
                                .map(semantic_type_from_resolved)
                                .unwrap_or(IncanType::Unknown)
                        }
                    })
                    .collect(),
                named_type_identities: type_info.declarations.named_type_identities.clone(),
                type_parameter_count,
            })
        })
        .collect()
}

/// Retain a plain newtype's checked carrier as one canonical tuple slot; its owner identity authorizes that slot.
fn collect_plain_newtype(
    span: ast::Span,
    newtype: &ast::NewtypeDecl,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Option<bir::NominalDeclaration> {
    if !is_direct_replacement_plain_newtype(newtype) {
        return None;
    }
    let facts = type_info.declarations.newtype_construction.get(&newtype.name)?;
    if facts.checked_constructor.is_some() || !facts.constraints.is_empty() {
        return None;
    }
    let canonical = type_info
        .declarations
        .declaration_identities
        .get(&(span.start, span.end))?
        .clone();
    let underlying = semantic_type_from_resolved(&facts.underlying);
    let mut derives = facts.automatic_derives.clone();
    if matches!(
        underlying,
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool)
    ) {
        if !derives.iter().any(|derive| derive == "Clone") {
            derives.push("Clone".to_owned());
        }
        derives.push("Copy".to_owned());
    }
    Some(bir::NominalDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(module_identity, span.start, span.end),
        field_identities: vec![canonical.clone()],
        canonical,
        name: newtype.name.clone(),
        fields: vec!["0".to_owned()],
        field_types: vec![underlying],
        field_public: vec![true],
        public: newtype.visibility == ast::Visibility::Public,
        has_field_defaults: false,
        derives,
        named_type_identities: type_info.declarations.named_type_identities.clone(),
        type_parameter_count: 0,
    })
}
/// Retain exact source-local fieldless normal-enum declaration and unit-member facts in source order.
///
/// Only this registry reaches the direct runtime. It deliberately has no payload layouts, aliases, match facts, or
/// source-symbol lookup facility, so its existence cannot widen into general enum execution by spelling alone.
pub(super) fn collect_local_fieldless_enum_declarations(
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
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            let variants = enum_decl
                .variants
                .iter()
                .map(|variant| {
                    let canonical = type_info
                        .declarations
                        .member_declaration_identities
                        .get(&(variant.span.start, variant.span.end))?
                        .clone();
                    Some(bir::FieldlessEnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_span(
                            module_identity,
                            variant.span.start,
                            variant.span.end,
                        ),
                        canonical,
                        name: variant.node.name.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::FieldlessEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
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
pub(super) fn collect_local_value_enum_declarations(
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
                .get(&(declaration.span.start, declaration.span.end))?
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
                        .get(&(variant.span.start, variant.span.end))?
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
                        direct_declaration_id: CompilerNodeId::declaration_span(
                            module_identity,
                            variant.span.start,
                            variant.span.end,
                        ),
                        canonical,
                        name: variant.node.name.clone(),
                        raw_value,
                    })
                })
                .collect::<Vec<_>>();
            (variants.len() == enum_decl.variants.len()).then(|| bir::ValueEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                canonical,
                name: enum_decl.name.clone(),
                backing,
                variants,
            })
        })
        .collect()
}

/// Retain physical source trait owners so consumers can validate their default-method bodies.
pub(super) fn collect_local_trait_declarations(
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
                .get(&(declaration.span.start, declaration.span.end))
                .cloned()
        })
        .collect()
}

/// Retain non-generic local trait slots and their checked concrete implementation identities.
///
/// Successful typechecking already proves adoption and method compatibility. This registry retains only local,
/// unambiguous method declarations with admitted owner layouts; imported, generic, and overloaded implementations never
/// gain a guessed target. A refused checked newtype constructor cannot contribute an implementation without its owner
/// layout.
pub(super) fn collect_local_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominal_declarations: &[bir::NominalDeclaration],
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for declaration in &program.declarations {
        let (adoptions, methods) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_plain_model(model) => {
                (&model.traits, &model.methods)
            }
            ast::Declaration::Class(class) if is_direct_replacement_class(class) => (&class.traits, &class.methods),
            ast::Declaration::Newtype(newtype) if is_direct_replacement_plain_newtype(newtype) => {
                (&newtype.traits, &newtype.methods)
            }
            _ => continue,
        };
        let Some(owner) = type_info
            .declarations
            .declaration_identities
            .get(&(declaration.span.start, declaration.span.end))
        else {
            continue;
        };
        if !nominal_declarations
            .iter()
            .any(|declaration| &declaration.canonical == owner)
        {
            continue;
        }
        for adoption in adoptions {
            if !adoption.node.type_args.is_empty() {
                continue;
            }
            let Some(trait_decl) = program.declarations.iter().find_map(|item| match &item.node {
                ast::Declaration::Trait(trait_decl)
                    if trait_decl.name == adoption.node.name && trait_decl.type_params.is_empty() =>
                {
                    Some(trait_decl)
                }
                _ => None,
            }) else {
                continue;
            };
            for slot in &trait_decl.methods {
                let Some(method) = type_info
                    .traits
                    .method_identities
                    .get(&(trait_decl.name.clone(), slot.node.name.clone()))
                else {
                    continue;
                };
                let candidates = methods
                    .iter()
                    .filter(|candidate| candidate.node.name == slot.node.name)
                    .collect::<Vec<_>>();
                let implementation = match candidates.as_slice() {
                    [candidate] => type_info
                        .declarations
                        .method_bindings_by_span
                        .get(&(candidate.span.start, candidate.span.end))
                        .and_then(|binding| binding.identity.as_ref()),
                    [] if slot.node.body.is_some() => Some(method),
                    _ => None,
                };
                if let Some(implementation) = implementation {
                    implementations.push(bir::TraitImplementation {
                        owner: owner.clone(),
                        method: method.clone(),
                        implementation: implementation.clone(),
                    });
                }
            }
        }
    }
    implementations
}
