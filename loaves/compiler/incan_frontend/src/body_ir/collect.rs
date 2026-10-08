//! One-pass collection of the module-local facts lowering retains: defaults, declarations, and canonical member
//! layouts.

use super::*;

/// Retain checker-evaluated constants once; unsupported aggregate and symbolic constants remain absent.
pub(super) fn collect_constants(program: &ast::Program, type_info: &TypeCheckInfo) -> Vec<bir::ConstantDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Const(item) = &declaration.node else {
                return None;
            };
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            let value = match type_info.const_value(&item.name)? {
                crate::typechecker::ConstValue::Int(value) => bir::Constant::Int(*value),
                crate::typechecker::ConstValue::Float(value) => bir::Constant::Float(value.to_string()),
                crate::typechecker::ConstValue::Bool(value) => bir::Constant::Bool(*value),
                crate::typechecker::ConstValue::FrozenStr(value) => bir::Constant::Str(value.clone()),
                _ => return None,
            };
            Some(bir::ConstantDeclaration { canonical, value })
        })
        .collect()
}

/// Retain alias-expanded nongeneric type declarations and their checker-proven nominal references.
pub(super) fn collect_type_aliases(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
) -> Vec<bir::TypeAliasDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::TypeAlias(item) = &declaration.node else {
                return None;
            };
            if !item.type_params.is_empty() {
                return None;
            }
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            Some(bir::TypeAliasDeclaration {
                canonical,
                ty: semantic_type_from_resolved(type_info.type_alias_target(&item.name)?),
                named_type_identities: type_info.declarations.named_type_identities.clone(),
            })
        })
        .collect()
}

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
                ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => (
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
                field_default_body: None,
                derives: if class_layout.is_some() {
                    incan_lang::lang::derives::plain_model_derives()
                        .map(str::to_owned)
                        .to_vec()
                } else {
                    type_info.declarations.model_derives.get(name)?.clone()
                },
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
        field_default_body: None,
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

/// One source-local trait an admitted adopter implements, at the instantiation the checker recorded for it.
struct AdoptedTrait<'program> {
    declaration: &'program ast::TraitDecl,
    /// Checked type arguments, one per trait type parameter.
    arguments: Vec<ResolvedType>,
    /// Supertraits this direct adoption reaches, by name; `None` for a trait reached only as a supertrait.
    direct_ancestors: Option<Vec<String>>,
}

impl AdoptedTrait<'_> {
    /// Bind the trait's type parameters to this instantiation's arguments for its default bodies.
    fn type_arguments(&self) -> Vec<bir::TraitTypeArgument> {
        self.declaration
            .type_params
            .iter()
            .zip(&self.arguments)
            .map(|(parameter, argument)| bir::TraitTypeArgument {
                parameter: parameter.name.clone(),
                argument: semantic_type_from_resolved(argument),
            })
            .collect()
    }
}

/// Find the trait this module declares under `name`.
fn local_trait<'program>(program: &'program ast::Program, name: &str) -> Option<&'program ast::TraitDecl> {
    program.declarations.iter().find_map(|item| match &item.node {
        ast::Declaration::Trait(trait_decl) if trait_decl.name == name => Some(trait_decl),
        _ => None,
    })
}

/// Expand one adopted local trait into its local supertraits, instantiating each supertrait clause the checker
/// resolved with the adopting instantiation's arguments.
fn local_supertraits<'program>(
    program: &'program ast::Program,
    type_info: &TypeCheckInfo,
    root: &'program ast::TraitDecl,
    arguments: &[ResolvedType],
) -> Vec<(&'program ast::TraitDecl, Vec<ResolvedType>)> {
    let mut reached = Vec::new();
    let mut seen = HashSet::new();
    let mut work = vec![(root, arguments.to_vec())];
    while let Some((declaration, arguments)) = work.pop() {
        let names = declaration
            .type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        let substitution = crate::resolved_type_subst::type_param_subst_map(&names, &arguments);
        for (name, clause) in type_info
            .traits
            .direct_supertraits
            .get(&declaration.name)
            .into_iter()
            .flatten()
        {
            let Some(supertrait) = local_trait(program, name) else {
                continue;
            };
            let instantiated = clause
                .iter()
                .map(|argument| crate::resolved_type_subst::substitute_resolved_type(argument, &substitution))
                .collect::<Vec<_>>();
            if seen.insert((name.clone(), format!("{instantiated:?}"))) {
                reached.push((supertrait, instantiated.clone()));
                work.push((supertrait, instantiated));
            }
        }
    }
    reached
}

/// Collect the local traits one adopter implements: each direct adoption at its checked arguments, then the local
/// supertraits those reach.
///
/// A generic trait adopted without recorded arguments contributes nothing. A trait reached at two different
/// instantiations is dropped entirely, because its slot identities cannot tell the implementations apart.
fn adopted_local_traits<'program>(
    program: &'program ast::Program,
    type_info: &TypeCheckInfo,
    adoptions: &[ast::Spanned<ast::TraitBound>],
) -> Vec<AdoptedTrait<'program>> {
    let mut adopted = Vec::new();
    for adoption in adoptions {
        let Some(declaration) = local_trait(program, &adoption.node.name) else {
            continue;
        };
        let arguments = if declaration.type_params.is_empty() {
            Vec::new()
        } else {
            match type_info
                .traits
                .adoption_type_args
                .get(&(adoption.span.start, adoption.span.end))
            {
                Some(arguments) if arguments.len() == declaration.type_params.len() => arguments.clone(),
                _ => continue,
            }
        };
        let supertraits = local_supertraits(program, type_info, declaration, &arguments);
        adopted.push(AdoptedTrait {
            declaration,
            arguments,
            direct_ancestors: Some(
                supertraits
                    .iter()
                    .map(|(trait_decl, _)| trait_decl.name.clone())
                    .collect(),
            ),
        });
        adopted.extend(supertraits.into_iter().map(|(declaration, arguments)| AdoptedTrait {
            declaration,
            arguments,
            direct_ancestors: None,
        }));
    }
    let conflicting = adopted
        .iter()
        .filter(|candidate| {
            adopted.iter().any(|other| {
                other.declaration.name == candidate.declaration.name && other.arguments != candidate.arguments
            })
        })
        .map(|candidate| candidate.declaration.name.clone())
        .collect::<HashSet<_>>();
    adopted.retain(|candidate| !conflicting.contains(&candidate.declaration.name));
    adopted
}

/// Select the body that fills one trait slot for an adopter, with the type arguments its default needs.
///
/// The adopter's own method wins, then the slot trait's default, then the default a directly adopted subtrait of the
/// slot trait declares, taking those subtraits by name: the order legacy trait expansion fills a slot in.
fn slot_implementation(
    type_info: &TypeCheckInfo,
    adopted: &[AdoptedTrait<'_>],
    slot_trait: &AdoptedTrait<'_>,
    slot: &ast::Spanned<ast::MethodDecl>,
    methods: &[ast::Spanned<ast::MethodDecl>],
) -> Option<(CanonicalSymbolId, Vec<bir::TraitTypeArgument>)> {
    let candidates = methods
        .iter()
        .filter(|candidate| candidate.node.name == slot.node.name)
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [candidate] => type_info
            .declarations
            .method_bindings_by_span
            .get(&(candidate.span.start, candidate.span.end))
            .and_then(|binding| binding.identity.clone())
            .map(|identity| (identity, Vec::new())),
        [] if slot.node.body.is_some() => type_info
            .traits
            .method_identities
            .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))
            .map(|identity| (identity.clone(), slot_trait.type_arguments())),
        [] => {
            let mut subtraits = adopted
                .iter()
                .filter(|candidate| {
                    candidate
                        .direct_ancestors
                        .as_ref()
                        .is_some_and(|ancestors| ancestors.contains(&slot_trait.declaration.name))
                })
                .collect::<Vec<_>>();
            subtraits.sort_by(|left, right| left.declaration.name.cmp(&right.declaration.name));
            subtraits.into_iter().find_map(|subtrait| {
                subtrait
                    .declaration
                    .methods
                    .iter()
                    .any(|method| method.node.name == slot.node.name && method.node.body.is_some())
                    .then(|| {
                        type_info
                            .traits
                            .method_identities
                            .get(&(subtrait.declaration.name.clone(), slot.node.name.clone()))
                    })
                    .flatten()
                    .map(|identity| (identity.clone(), subtrait.type_arguments()))
            })
        }
        _ => None,
    }
}

/// Retain local trait slots and their checked concrete implementation identities for each admitted adopter.
///
/// Successful typechecking already proves adoption and method compatibility. This registry covers the adopter's local
/// traits at their checked instantiations and every local supertrait those reach; each slot keeps the body that fills
/// it and the type arguments that body's trait was adopted at. Imported traits, unrecorded instantiations, and
/// overloaded implementations never gain a guessed target. A refused checked newtype constructor cannot contribute an
/// implementation without its owner layout.
pub(super) fn collect_local_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominal_declarations: &[bir::NominalDeclaration],
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for declaration in &program.declarations {
        let (adoptions, methods) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => {
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
        let adopted = adopted_local_traits(program, type_info, adoptions);
        for slot_trait in &adopted {
            for slot in &slot_trait.declaration.methods {
                let Some(method) = type_info
                    .traits
                    .method_identities
                    .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))
                else {
                    continue;
                };
                if implementations
                    .iter()
                    .any(|existing: &bir::TraitImplementation| &existing.owner == owner && &existing.method == method)
                {
                    continue;
                }
                let Some((implementation, type_arguments)) =
                    slot_implementation(type_info, &adopted, slot_trait, slot, methods)
                else {
                    continue;
                };
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    type_arguments,
                });
            }
        }
    }
    implementations
}
