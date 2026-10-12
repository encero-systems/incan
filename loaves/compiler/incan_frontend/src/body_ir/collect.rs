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
                ty: parameter_types::checked_semantic_type(type_info.type_alias_target(&item.name)?, type_info),
                named_type_identities: type_info.declarations.named_type_identities.clone(),
            })
        })
        .collect()
}

/// Retain every static of this module: a primitive or primitive-collection literal, whose initialization has no
/// user-visible effects, keeps its literal initializer; any other checked initializer is lowered as a computed body
/// evaluated where legacy initializes the static (see [`bir::StaticInitializer::Computed`]).
pub(super) fn collect_statics(
    program: &ast::Program,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Vec<bir::StaticDeclaration> {
    let type_info = lowering_facts.type_info;
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Static(storage) = &declaration.node else {
                return None;
            };
            let ty = semantic_type_from_resolved(type_info.expr_type(storage.value.span)?);
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&(declaration.span.start, declaration.span.end))?
                .clone();
            let initial = static_initializer(&storage.value, &ty, type_info).unwrap_or_else(|| {
                bir::StaticInitializer::Computed(Box::new(lower_static_initializer_body(
                    storage,
                    declaration.span,
                    &canonical,
                    &ty,
                    lowering_facts,
                )))
            });
            Some(bir::StaticDeclaration { canonical, ty, initial })
        })
        .collect()
}

/// Recognize the empty `Set()` constructor only from the checker's canonical constructor fact; every other
/// initializer must be a literal.
fn static_initializer(
    value: &ast::Spanned<ast::Expr>,
    ty: &IncanType,
    type_info: &TypeCheckInfo,
) -> Option<bir::StaticInitializer> {
    let empty_call = match &value.node {
        ast::Expr::Call(_, type_args, args) => type_args.is_empty() && args.is_empty(),
        ast::Expr::Constructor(_, args) => args.is_empty(),
        _ => false,
    };
    if empty_call && type_info.resolved_collection_constructor(value.span) == Some(CollectionTypeId::Set) {
        let [element] = static_collection_arguments(ty, CollectionTypeId::Set)? else {
            return None;
        };
        return hashed_static_carrier(element).then(|| bir::StaticInitializer::Set(Vec::new()));
    }
    static_literal_initializer(&value.node, ty)
}

/// Preserve literal values only in their exact checked primitive carrier or primitive collection element carriers.
fn static_literal_initializer(value: &ast::Expr, ty: &IncanType) -> Option<bir::StaticInitializer> {
    match value {
        ast::Expr::Literal(literal) if primitive_static_carrier(ty) => Some(bir::StaticInitializer::Literal(
            primitives::lower_checked_literal(literal, ty),
        )),
        ast::Expr::List(entries) => {
            let [element] = static_collection_arguments(ty, CollectionTypeId::List)? else {
                return None;
            };
            if !primitive_static_carrier(element) {
                return None;
            }
            let values = entries
                .iter()
                .map(|entry| match entry {
                    ast::ListEntry::Element(item) => static_literal_constant(&item.node, element),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::StaticInitializer::List(values))
        }
        ast::Expr::Dict(entries) => {
            let [key, value] = static_collection_arguments(ty, CollectionTypeId::Dict)? else {
                return None;
            };
            if !hashed_static_carrier(key) || !primitive_static_carrier(value) {
                return None;
            }
            let pairs = entries
                .iter()
                .map(|entry| match entry {
                    ast::DictEntry::Pair(entry_key, entry_value) => Some((
                        static_literal_constant(&entry_key.node, key)?,
                        static_literal_constant(&entry_value.node, value)?,
                    )),
                    ast::DictEntry::Spread(_) => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(bir::StaticInitializer::Dict(pairs))
        }
        ast::Expr::Set(members) => {
            let [element] = static_collection_arguments(ty, CollectionTypeId::Set)? else {
                return None;
            };
            if !hashed_static_carrier(element) {
                return None;
            }
            let values = members
                .iter()
                .map(|member| static_literal_constant(&member.node, element))
                .collect::<Option<Vec<_>>>()?;
            Some(bir::StaticInitializer::Set(values))
        }
        _ => None,
    }
}

/// Lower one primitive literal element; a nested collection or computed element has no effect-free initializer.
fn static_literal_constant(value: &ast::Expr, ty: &IncanType) -> Option<bir::Constant> {
    match static_literal_initializer(value, ty)? {
        bir::StaticInitializer::Literal(constant) => Some(constant),
        _ => None,
    }
}

/// The type arguments of a checked builtin collection carrier of the expected kind.
fn static_collection_arguments(ty: &IncanType, expected: CollectionTypeId) -> Option<&[IncanType]> {
    match ty {
        IncanType::Generic { base, args } if collections::from_str(base) == Some(expected) => Some(args.as_slice()),
        _ => None,
    }
}

/// A primitive carrier a static literal may hold.
fn primitive_static_carrier(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool | IncanPrimitiveType::Str
        )
    )
}

/// A primitive hash key or set member: floating point lacks Eq and Hash.
fn hashed_static_carrier(ty: &IncanType) -> bool {
    primitive_static_carrier(ty) && !matches!(ty, IncanType::Primitive(IncanPrimitiveType::Float))
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
                                .map(|ty| parameter_types::checked_semantic_type(ty, type_info))
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

/// Collect the source expressions a synthesized partial needs to retain target defaults in Body IR: a function's
/// parameter defaults, and a model's or class's field defaults for a partial over its constructor, in declaration
/// order.
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
            ast::Declaration::Model(model) => Some((model.name.clone(), field_default_sources(&model.fields))),
            ast::Declaration::Class(class) => Some((class.name.clone(), field_default_sources(&class.fields))),
            _ => None,
        })
        .collect()
}

/// Retain each declared field's default expression in declaration order, as a constructor's parameter defaults.
fn field_default_sources(fields: &[ast::Spanned<ast::FieldDecl>]) -> Vec<FunctionDefaultSource> {
    fields
        .iter()
        .map(|field| FunctionDefaultSource {
            param_span: field.span,
            default: field.node.default.clone(),
        })
        .collect()
}
/// Retain a nongeneric source callable's exact return carrier before its private aliases leave the providing module.
/// Generic callable results keep the consumer's checked instantiation facts; unresolved producer results never
/// replace a consumer's type. The canonical key identifies the selected declaration, not a written import spelling.
pub(super) fn collect_published_callable_returns(
    context: &[bir::BodyIrModule],
) -> HashMap<CanonicalSymbolId, IncanType> {
    context
        .iter()
        .flat_map(|module| &module.bodies)
        .filter(|body| body.type_parameters.is_empty())
        .filter(|body| {
            !matches!(
                body.return_type,
                IncanType::Unknown | IncanType::Infer | IncanType::SelfType | IncanType::TypeVar(_)
            )
        })
        .filter_map(|body| {
            let identity = body.canonical.as_ref()?;
            (identity.kind == SemanticSourceTargetKind::Function).then(|| (identity.clone(), body.return_type.clone()))
        })
        .collect()
}

/// Collect the exact source spans eligible for same-module direct named-call dispatch.
pub(super) fn collect_local_function_declarations(program: &ast::Program) -> LocalFunctionDeclarations {
    let mut declarations = LocalFunctionDeclarations::new();
    for declaration in &program.declarations {
        let name = match &declaration.node {
            ast::Declaration::Function(function) => &function.name,
            ast::Declaration::Partial(partial) => &partial.name,
            _ => continue,
        };
        declarations.entry(name.clone()).or_default().push(declaration.span);
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
                field_aliases: fields.iter().map(|field| field.node.metadata.alias.clone()).collect(),
                public: visibility == ast::Visibility::Public,
                has_field_defaults: fields.iter().any(|field| field.node.default.is_some()),
                field_default_body: None,
                method_partial_defaults: Vec::new(),
                derives: if class_layout.is_some() {
                    // The class's own derive selection, plus the serde derives its trait adoptions imply.
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
                    derives
                } else {
                    type_info.declarations.body_ir_model_derives.get(name)?.clone()
                },
                field_types: fields
                    .iter()
                    .map(|field| {
                        if let Some(layout) = class_layout {
                            layout
                                .fields
                                .iter()
                                .find(|checked| checked.name == field.node.name)
                                .map(|checked| parameter_types::checked_semantic_type(&checked.ty, type_info))
                                .unwrap_or(IncanType::Unknown)
                        } else {
                            type_info
                                .declarations
                                .model_field_types
                                .get(&(field.span.start, field.span.end))
                                .map(|ty| parameter_types::checked_semantic_type(ty, type_info))
                                .unwrap_or(IncanType::Unknown)
                        }
                    })
                    .map(|ty| parameter_types::retain_parameter_type(ty, &type_parameters))
                    .collect(),
                named_type_identities: type_info.declarations.named_type_identities.clone(),
                type_parameter_count: type_parameters.len(),
                type_parameters,
            })
        })
        .collect()
}

/// Retain a raw newtype's checked carrier as one canonical tuple slot, with declaration-owned placeholders that only
/// checked concrete uses may specialize. Validation hooks and constraints remain outside raw construction.
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
    let underlying = parameter_types::retain_parameter_type(
        parameter_types::checked_semantic_type(&facts.underlying, type_info),
        &facts.type_params,
    );
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
    for selection in &facts.explicit_derives {
        if !derives.contains(selection) {
            derives.push(selection.clone());
        }
    }
    Some(bir::NominalDeclaration {
        direct_declaration_id: CompilerNodeId::declaration_span(module_identity, span.start, span.end),
        field_identities: vec![canonical.clone()],
        canonical,
        name: newtype.name.clone(),
        fields: vec!["0".to_owned()],
        field_types: vec![underlying],
        field_public: vec![true],
        field_aliases: vec![None],
        public: newtype.visibility == ast::Visibility::Public,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        derives,
        named_type_identities: type_info.declarations.named_type_identities.clone(),
        type_parameters: facts.type_params.clone(),
        type_parameter_count: facts.type_params.len(),
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
/// slot trait declares, taking those subtraits by name: the order legacy trait expansion fills a slot in. A targeted
/// method answers only the checker-recorded slot of its target, never another trait's same-named slot.
fn slot_implementation(
    type_info: &TypeCheckInfo,
    adopted: &[AdoptedTrait<'_>],
    slot_trait: &AdoptedTrait<'_>,
    slot: &ast::Spanned<ast::MethodDecl>,
    methods: &[ast::Spanned<ast::MethodDecl>],
) -> Option<(CanonicalSymbolId, Vec<bir::TraitTypeArgument>)> {
    let identity = type_info
        .traits
        .method_identities
        .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))?;
    let candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == slot.node.name && method_fills_trait_slot(type_info, &candidate.node, identity)
        })
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

/// Match an explicit method target through its checked slot identity; unqualified methods can fill any compatible
/// slot. Explicit target arguments still refuse because no checked target instantiation is retained for that clause.
fn method_fills_trait_slot(type_info: &TypeCheckInfo, method: &ast::MethodDecl, slot: &CanonicalSymbolId) -> bool {
    let Some(target) = &method.trait_target else {
        return true;
    };
    target.node.type_args.is_empty()
        && type_info
            .traits
            .method_identities
            .get(&(target.node.name.clone(), method.name.clone()))
            == Some(slot)
}

/// Retain trait slots and their checked concrete implementation identities for each admitted adopter.
///
/// Successful typechecking already proves adoption and method compatibility. This registry covers the adopter's local
/// traits at their checked instantiations and every local supertrait those reach; each slot keeps the body that fills
/// it and the type arguments that body's trait was adopted at. Imported traits without type parameters keep their own
/// slots under the checked provider identities (see [`imported_trait_implementations`]). Unrecorded instantiations and
/// overloaded implementations never gain a guessed target. A refused checked newtype constructor cannot contribute an
/// implementation without its owner layout, nor can an enum whose layout was not retained.
pub(super) fn collect_local_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominal_declarations: &[bir::NominalDeclaration],
    enum_declarations: &[bir::EnumDeclaration],
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for declaration in &program.declarations {
        let (adoptions, methods) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => {
                (&model.traits, &model.methods)
            }
            ast::Declaration::Class(class) if is_direct_replacement_checked_class(class, type_info) => {
                (&class.traits, &class.methods)
            }
            ast::Declaration::Newtype(newtype) if is_direct_replacement_plain_newtype(newtype) => {
                (&newtype.traits, &newtype.methods)
            }
            ast::Declaration::Enum(value) if is_direct_native_enum(value) => (&value.traits, &value.methods),
            _ => continue,
        };
        let Some(owner) = type_info
            .declarations
            .declaration_identities
            .get(&(declaration.span.start, declaration.span.end))
        else {
            continue;
        };
        let retained = nominal_declarations
            .iter()
            .any(|declaration| &declaration.canonical == owner)
            || enum_declarations
                .iter()
                .any(|declaration| &declaration.canonical == owner);
        if !retained {
            continue;
        }
        implementations.extend(imported_trait_implementations(
            program, type_info, adoptions, methods, owner,
        ));
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

/// Retain the slots of one adopter's imported traits, under checked provider identities.
///
/// Each slot keeps the body that fills it: the adopter's own method, or the imported trait's default. A project or
/// package trait's default lives in the module that declares the trait. A standard-library or compiled SDK protocol
/// brings its own slots and those of its standard supertraits ([`adopted_slots`]): the adopter's own method fills one
/// with or without type parameters, and the protocol's default fills one only when the protocol has none. That
/// default's body comes from the SDK's published representation, or a call to it refuses where it is lowered. Imported
/// supertraits of other traits and other modules' traits with type parameters get no record, so a call that needs one
/// of those keeps refusing. The records are sorted by slot identity, so the module's Body IR does not depend on hash
/// order.
fn imported_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    adoptions: &[ast::Spanned<ast::TraitBound>],
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    let mut implementations: Vec<bir::TraitImplementation> = Vec::new();
    for adoption in adoptions {
        let name = &adoption.node.name;
        if local_trait(program, name).is_some() {
            continue;
        }
        // Another module's trait with type parameters would need its defaults instantiated, which Body IR does not do
        // here; a protocol's own methods need no trait type arguments, and its defaults are taken only without them.
        let generic =
            !adoption.node.type_args.is_empty() || !type_info.traits.type_params.get(name).is_some_and(Vec::is_empty);
        for (trait_key, slot_name, method) in adopted_slots(type_info, name) {
            if implementations.iter().any(|existing| &existing.method == method) {
                continue;
            }
            let protocol = type_info.traits.is_standard_slot(method);
            if (generic && !protocol) || (!protocol && !is_executable_source_trait_slot(method)) {
                continue;
            }
            // A protocol whose behavior a Rust derive provides keeps the slots it leaves on that derive.
            let defaults = !generic && !(protocol && derive_backed_protocol(type_info, trait_key, method));
            if let Some(implementation) = imported_slot_implementation(type_info, method, slot_name, methods, defaults)
            {
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    type_arguments: Vec::new(),
                });
            }
        }
    }
    implementations.sort_by(|left, right| left.method.cmp(&right.method));
    implementations
}

/// Whether the standard protocol a slot belongs to declares its behavior through a Rust derive (`@rust.derive`), as
/// `std.serde.json.Serialize` does.
///
/// Such a protocol's defaults are what the derive provides, not bodies an adopter takes, exactly as legacy derives
/// them. The trait is named by the module its slot's identity names and the last segment of its key, the
/// module-qualified key its derive paths are recorded under.
fn derive_backed_protocol(type_info: &TypeCheckInfo, name: &str, slot: &CanonicalSymbolId) -> bool {
    let module_path = match &slot.origin {
        incan_semantics_core::SymbolOrigin::Package { module_path, .. } => module_path.as_slice(),
        incan_semantics_core::SymbolOrigin::Module(path) if path.first().map(String::as_str) == Some("std") => {
            &path[1..]
        }
        _ => return false,
    };
    let trait_name = name.rsplit('.').next().unwrap_or(name);
    let key = std::iter::once("std")
        .chain(module_path.iter().map(String::as_str))
        .chain(std::iter::once(trait_name))
        .collect::<Vec<_>>()
        .join(".");
    type_info
        .derivations
        .trait_rust_derive_paths
        .get(&key)
        .is_some_and(|paths| !paths.is_empty())
}

/// The slots one adopted trait brings, each with the trait key it is recorded under, its spelling and its identity.
///
/// A trait brings its own slots. A standard-library protocol also brings the slots of its standard supertraits without
/// type arguments, which the program need not import: those are recorded under the provider's module-qualified trait
/// key ([`standard_supertrait_keys`]), with the provider identities the SDK publishes their bodies under.
fn adopted_slots<'facts>(
    type_info: &'facts TypeCheckInfo,
    name: &str,
) -> Vec<(&'facts str, &'facts str, &'facts CanonicalSymbolId)> {
    let mut keys = vec![name.to_owned()];
    if type_info.traits.is_standard_trait(name) {
        keys.extend(standard_supertrait_keys(type_info, name));
    }
    let mut slots = type_info
        .traits
        .method_identities
        .iter()
        .filter(|((trait_key, _), _)| keys.contains(trait_key))
        .map(|((trait_key, slot_name), method)| (trait_key.as_str(), slot_name.as_str(), method))
        .collect::<Vec<_>>();
    slots.sort_by(|left, right| left.2.cmp(right.2));
    slots
}

/// The provider keys of a standard trait's standard supertraits without type arguments, transitively.
///
/// Each key is the supertrait's stdlib module path followed by its name, as `std.derives.comparison.Eq`: the key the
/// checker records a provider-declared trait's slots under. A supertrait the stdlib registry does not place in a
/// module has no such key and contributes no slots.
fn standard_supertrait_keys(type_info: &TypeCheckInfo, name: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut pending = vec![name.to_owned()];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        for (supertrait, arguments) in type_info.traits.direct_supertraits.get(&current).into_iter().flatten() {
            if !arguments.is_empty() {
                continue;
            }
            let Some(segments) = incan_lang::lang::stdlib::trait_method_module_segments(supertrait) else {
                continue;
            };
            keys.push(format!("{}.{supertrait}", segments.join(".")));
            pending.push(supertrait.clone());
        }
    }
    keys
}

/// Select the body that fills one imported slot: the adopter's only method of that name, else the slot's default when
/// `defaults` admits it.
fn imported_slot_implementation(
    type_info: &TypeCheckInfo,
    method: &CanonicalSymbolId,
    slot_name: &str,
    methods: &[ast::Spanned<ast::MethodDecl>],
    defaults: bool,
) -> Option<CanonicalSymbolId> {
    let candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == slot_name && method_fills_trait_slot(type_info, &candidate.node, method)
        })
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [candidate] => type_info
            .declarations
            .method_bindings_by_span
            .get(&(candidate.span.start, candidate.span.end))
            .and_then(|binding| binding.identity.clone()),
        [] if defaults && type_info.traits.default_method_identities.contains(method) => Some(method.clone()),
        _ => None,
    }
}

/// Whether a trait slot can be filled by a body of the project or of a package dependency.
///
/// The standard library's and Rust's protocols describe derive and runtime contracts, not bodies the project's modules
/// supply. Ordinary project and package identities remain eligible; an import alias never changes this boundary.
fn is_executable_source_trait_slot(identity: &CanonicalSymbolId) -> bool {
    match &identity.origin {
        incan_semantics_core::SymbolOrigin::Module(path) => !incan_lang::lang::stdlib::is_any_stdlib_path(path),
        incan_semantics_core::SymbolOrigin::Package { .. } => true,
        incan_semantics_core::SymbolOrigin::Builtin | incan_semantics_core::SymbolOrigin::RustCrate(_) => false,
    }
}
