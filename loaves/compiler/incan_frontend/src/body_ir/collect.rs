//! One-pass collection of the module-local facts lowering retains: defaults, declarations, and canonical member
//! layouts.

use super::*;

/// Retain physical immutable instance methods with their checked owning declaration, once per source module.
///
/// An unresolved caller-open method has no mutability, decorator, or generic-header contract. Those shapes stay
/// refused; only the ordinary checked source body and signature may complete a newly concrete receiver.
pub(super) fn collect_local_receiver_methods(
    program: &ast::Program,
    facts: &TypeCheckInfo,
) -> Vec<LocalReceiverMethod> {
    let mut methods = Vec::new();
    for declaration in &program.declarations {
        let source = match &declaration.node {
            ast::Declaration::Model(model) => &model.methods,
            ast::Declaration::Class(class) => &class.methods,
            _ => continue,
        };
        let Some(owner) = facts
            .declarations
            .declaration_identities
            .get(&(declaration.span.start, declaration.span.end))
        else {
            continue;
        };
        for method in source {
            if method.node.receiver != Some(ast::Receiver::Immutable)
                || method.node.body.is_none()
                || !method.node.type_params.is_empty()
                || !method.node.decorators.is_empty()
                || method.node.is_async()
            {
                continue;
            }
            methods.push(LocalReceiverMethod {
                owner: owner.clone(),
                name: method.node.name.clone(),
                span: method.span,
            });
        }
    }
    methods
}

/// Retain checker-evaluated constants, checked static literal aggregates and literal integer arithmetic once.
/// Computed aggregates and symbolic arithmetic stay absent; additional values belong only to Body IR and never
/// update legacy's evaluation table.
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
            let value = match type_info.const_value(&item.name) {
                Some(crate::typechecker::ConstValue::Int(value)) => bir::Constant::Int(*value),
                Some(crate::typechecker::ConstValue::Float(value)) => bir::Constant::Float(value.to_string()),
                Some(crate::typechecker::ConstValue::Bool(value)) => bir::Constant::Bool(*value),
                Some(crate::typechecker::ConstValue::FrozenStr(value)) => bir::Constant::Str(value.clone()),
                None if type_info.expr_type(item.value.span) == Some(&ResolvedType::Int) => {
                    bir::Constant::Int(literal_integer_constant(&item.value.node)?)
                }
                _ => frozen_literal_constant(
                    &item.value.node,
                    &semantic_type_from_resolved(type_info.expr_type(item.value.span)?),
                )?,
            };
            Some(bir::ConstantDeclaration { canonical, value })
        })
        .collect()
}

/// Preserve only effect-free checked literal trees. Storage types come from checking; no call or spread is evaluated
/// again by Body IR or inferred from a literal's spelling.
fn frozen_literal_constant(value: &ast::Expr, ty: &IncanType) -> Option<bir::Constant> {
    if let ast::Expr::Literal(literal) = value {
        return Some(primitives::lower_checked_literal(literal, ty));
    }
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    let elements = match (collections::from_str(base)?, value, args.as_slice()) {
        (CollectionTypeId::FrozenList, ast::Expr::List(entries), [element]) => entries
            .iter()
            .map(|entry| match entry {
                ast::ListEntry::Element(value) => frozen_literal_constant(&value.node, element),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        (CollectionTypeId::FrozenSet, ast::Expr::Set(entries), [element]) => entries
            .iter()
            .map(|value| frozen_literal_constant(&value.node, element))
            .collect::<Option<Vec<_>>>()?,
        (CollectionTypeId::FrozenDict, ast::Expr::Dict(entries), [key, element]) => {
            let mut values = Vec::new();
            for entry in entries {
                let ast::DictEntry::Pair(k, v) = entry else {
                    return None;
                };
                values.push(frozen_literal_constant(&k.node, key)?);
                values.push(frozen_literal_constant(&v.node, element)?);
            }
            values
        }
        _ => return None,
    };
    Some(bir::Constant::FrozenAggregate(ty.clone(), elements))
}

/// Evaluate only literal signed integer addition, subtraction, multiplication and negation. Each intermediate must fit
/// i64, matching legacy's const arithmetic; names, calls, sized literals and every other operation stay unretained.
fn literal_integer_constant(value: &ast::Expr) -> Option<i64> {
    match value {
        ast::Expr::Literal(ast::Literal::Int(value)) if value.suffix.is_none() && value.fits_i64() => Some(value.value),
        ast::Expr::Unary(ast::UnaryOp::Neg, inner) => literal_integer_constant(&inner.node)?.checked_neg(),
        ast::Expr::Binary(left, op, right) => {
            let left = literal_integer_constant(&left.node)?;
            let right = literal_integer_constant(&right.node)?;
            match op {
                ast::BinaryOp::Add => left.checked_add(right),
                ast::BinaryOp::Sub => left.checked_sub(right),
                ast::BinaryOp::Mul => left.checked_mul(right),
                _ => None,
            }
        }
        _ => None,
    }
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
                                .map(|ty| parameter_types::checked_parameter_type(ty, type_info, &parameter_names))
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
                native_type_arguments: Vec::new(),
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
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
            })
        })
        .collect()
}

/// Collect the source expressions a synthesized partial needs to retain target defaults in Body IR: a function's
/// parameter defaults, a model's or class's field defaults, or a plain newtype's required value parameter, in
/// declaration order.
pub(super) fn collect_function_default_sources(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
) -> FunctionDefaultSources {
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
            ast::Declaration::Newtype(newtype)
                if newtype.type_params.is_empty()
                    && is_direct_replacement_checked_newtype(newtype, type_info)
                    && super::partials::plain_newtype_partial(&newtype.name, type_info) =>
            {
                Some((
                    newtype.name.clone(),
                    vec![FunctionDefaultSource {
                        param_span: newtype.underlying.span,
                        default: None,
                    }],
                ))
            }
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
///
/// The returned map retains declaration authority rather than resolving consumer spellings.
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
            let retained = bir::NominalDeclaration {
                native_item: String::new(),
                class_inheritance: None,
                native_type_arguments: Vec::new(),
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
                field_types: fields
                    .iter()
                    .map(|field| {
                        if let Some(layout) = class_layout {
                            layout
                                .fields
                                .iter()
                                .find(|checked| checked.name == field.node.name)
                                .map(|checked| {
                                    parameter_types::checked_parameter_type(&checked.ty, type_info, &type_parameters)
                                })
                                .unwrap_or(IncanType::Unknown)
                        } else {
                            type_info
                                .declarations
                                .model_field_types
                                .get(&(field.span.start, field.span.end))
                                .map(|ty| parameter_types::checked_parameter_type(ty, type_info, &type_parameters))
                                .unwrap_or(IncanType::Unknown)
                        }
                    })
                    .collect(),
                named_type_identities: type_info.declarations.named_type_identities.clone(),
                type_parameter_count: type_parameters.len(),
                type_parameters,
            };
            retain_inherited_layout(retained, type_info)
        })
        .collect()
}

/// Retain the checked derive selection and adoption effects without changing legacy normalization tables.
fn nominal_derives(name: &str, class: bool, type_info: &TypeCheckInfo) -> Option<Vec<String>> {
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

/// Replace only an inherited class's physical fields with the checked parent-first layout and ancestor identities.
fn retain_inherited_layout(
    mut retained: bir::NominalDeclaration,
    type_info: &TypeCheckInfo,
) -> Option<bir::NominalDeclaration> {
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

/// Retain a newtype's checked carrier as one canonical tuple slot, with declaration-owned placeholders that only
/// checked concrete uses may specialize. Integer primitive constraints retain a raw carrier layout while construction
/// sites retain their validation; the layout alone does not authorize unchecked construction. Nongeneric source hooks
/// retain canonical checked ingress. Generic hooks, noninteger constraints, and validated serde reconstruction remain
/// refused.
fn collect_plain_newtype(
    span: ast::Span,
    newtype: &ast::NewtypeDecl,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> Option<bir::NominalDeclaration> {
    if !is_direct_replacement_checked_newtype(newtype, type_info) {
        return None;
    }
    let facts = type_info.declarations.newtype_construction.get(&newtype.name)?;
    if facts.checked_constructor.is_some()
        && (!facts.type_params.is_empty() || facts.checked_constructor_identity.is_none())
    {
        return None;
    }
    let canonical = type_info
        .declarations
        .declaration_identities
        .get(&(span.start, span.end))?
        .clone();
    let underlying = parameter_types::checked_parameter_type(&facts.underlying, type_info, &facts.type_params);
    if !facts.constraints.is_empty() && underlying != IncanType::Primitive(IncanPrimitiveType::Int) {
        return None;
    }
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
    for selection in newtype_derives::checked_newtype_derives(newtype, type_info)? {
        if !derives.contains(&selection) {
            derives.push(selection);
        }
    }
    Some(bir::NominalDeclaration {
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span(module_identity, span.start, span.end),
        field_identities: vec![canonical.clone()],
        canonical,
        name: newtype.name.clone(),
        fields: vec!["0".to_owned()],
        field_types: vec![underlying],
        field_public: vec![true],
        field_aliases: vec![None],
        reflection_fields: None,
        public: newtype.visibility == ast::Visibility::Public,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        derives,
        named_type_identities: type_info.declarations.named_type_identities.clone(),
        type_parameters: facts.type_params.clone(),
        type_parameter_count: facts.type_params.len(),
    })
}

/// Retain the checked string method, preferring an explicit declaration to the Error message fallback selected for
/// the type. Derive-only Display keeps its existing Debug rendering and has no source method identity here.
fn checked_display_method(declaration: &ast::Declaration, type_info: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    let (name, methods) = match declaration {
        ast::Declaration::Model(model) => (&model.name, &model.methods),
        ast::Declaration::Class(class) => (&class.name, &class.methods),
        _ => return None,
    };
    if let Some(method) = methods.iter().find(|method| method.node.name == "__str__") {
        return type_info
            .declarations
            .method_bindings_by_span
            .get(&(method.span.start, method.span.end))?
            .identity
            .clone();
    }
    type_info.error_message_display_type(name)?.identity.clone()
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
/// Bare clauses consume the separate checked Body IR witness; written clauses keep their existing explicit snapshot.
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
                .body_ir_adoption_type_args
                .get(&(adoption.span.start, adoption.span.end))
                .or_else(|| {
                    type_info
                        .traits
                        .adoption_type_args
                        .get(&(adoption.span.start, adoption.span.end))
                }) {
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
    owner: &CanonicalSymbolId,
) -> Option<(CanonicalSymbolId, Vec<bir::TraitTypeArgument>)> {
    let identity = type_info
        .traits
        .method_identities
        .get(&(slot_trait.declaration.name.clone(), slot.node.name.clone()))?;
    let selected_name = checked_slot_method_name(type_info, owner, &slot.node.name);
    let candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == selected_name && method_fills_trait_slot(type_info, &candidate.node, identity)
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

/// Retain checked source derive slots, selecting a unique local override or the retained default body identity.
/// Compiled standard slots remain SDK-owned and never masquerade as source trait implementations.
fn derived_trait_implementations(
    type_info: &TypeCheckInfo,
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    type_info
        .declarations
        .body_ir_derived_methods
        .get(&owner.declaration_name)
        .into_iter()
        .flatten()
        .filter_map(|method| {
            if type_info.traits.is_standard_slot(&method.method) || !is_executable_source_trait_slot(&method.method) {
                return None;
            }
            let mut candidates = methods
                .iter()
                .filter(|candidate| candidate.node.name == method.method.declaration_name);
            let implementation = match (candidates.next(), candidates.next()) {
                (Some(candidate), None) => type_info
                    .declarations
                    .method_bindings_by_span
                    .get(&(candidate.span.start, candidate.span.end))
                    .and_then(|binding| binding.identity.clone()),
                (None, None) if method.has_default => Some(method.method.clone()),
                _ => None,
            }?;
            Some(bir::TraitImplementation {
                owner: owner.clone(),
                method: method.method.clone(),
                implementation,
                type_arguments: method
                    .type_arguments
                    .iter()
                    .map(|(parameter, argument)| bir::TraitTypeArgument {
                        parameter: parameter.clone(),
                        argument: semantic_type_from_resolved(argument),
                    })
                    .collect(),
            })
        })
        .collect()
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
        let (adoptions, methods, properties): (
            &[ast::Spanned<ast::TraitBound>],
            &[ast::Spanned<ast::MethodDecl>],
            &[ast::Spanned<ast::PropertyDecl>],
        ) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => {
                (&model.traits, &model.methods, &model.properties)
            }
            ast::Declaration::Class(class) if is_direct_replacement_checked_class(class, type_info) => {
                (&class.traits, &class.methods, &class.properties)
            }
            ast::Declaration::Newtype(newtype) if is_direct_replacement_checked_newtype(newtype, type_info) => {
                (&newtype.traits, &newtype.methods, &[])
            }
            ast::Declaration::Enum(value) if is_direct_native_enum(value) => (&value.traits, &value.methods, &[]),
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
        implementations.extend(derived_trait_implementations(type_info, methods, owner));
        let adopted = adopted_local_traits(program, type_info, adoptions);
        implementations.extend(property_implementations(type_info, &adopted, properties, owner));
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
                    slot_implementation(type_info, &adopted, slot_trait, slot, methods, owner)
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

/// Retain a concrete getter filling a checked local trait property requirement, including local supertraits.
/// Property defaults are not admitted; ambiguous or absent source getters never acquire a guessed implementation.
fn property_implementations(
    facts: &TypeCheckInfo,
    adopted: &[AdoptedTrait<'_>],
    properties: &[ast::Spanned<ast::PropertyDecl>],
    owner: &CanonicalSymbolId,
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for trait_info in adopted {
        for slot in &trait_info.declaration.properties {
            if slot.node.body.is_some() {
                continue;
            }
            let Some(method) = facts
                .declarations
                .body_ir_property_bindings
                .get(&(slot.span.start, slot.span.end))
                .and_then(|binding| binding.callable.identity.as_ref())
            else {
                continue;
            };
            let mut candidates = properties
                .iter()
                .filter(|property| property.node.name == slot.node.name && property.node.body.is_some());
            let Some(property) = candidates.next() else {
                continue;
            };
            if candidates.next().is_some() {
                continue;
            }
            let Some(implementation) = facts
                .declarations
                .body_ir_property_bindings
                .get(&(property.span.start, property.span.end))
                .and_then(|binding| binding.callable.identity.as_ref())
            else {
                continue;
            };
            if implementations
                .iter()
                .any(|existing: &bir::TraitImplementation| &existing.method == method)
            {
                continue;
            }
            implementations.push(bir::TraitImplementation {
                owner: owner.clone(),
                method: method.clone(),
                implementation: implementation.clone(),
                type_arguments: Vec::new(),
            });
        }
    }
    implementations
}

/// Retain the slots of one adopter's imported traits, under checked provider identities.
///
/// Each slot keeps the body that fills it: the adopter's own method, or the imported trait's default. A project or
/// package trait's default lives in the module that declares the trait. A standard-library or compiled SDK protocol
/// brings its own slots and those of its standard supertraits ([`adopted_slots`]): the adopter's own method fills one
/// with or without type parameters. Defaults retain exact checked adoption arguments and resolve only through the
/// declaring package's published executable representation; a missing body still refuses at its call. Generic
/// supertrait instantiations without retained arguments remain excluded. Records are sorted by canonical slot identity.
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
        for (trait_key, slot_name, method) in adopted_slots(type_info, name) {
            if implementations.iter().any(|existing| &existing.method == method) {
                continue;
            }
            let protocol = type_info.traits.is_standard_slot(method);
            if !protocol && !is_executable_source_trait_slot(method) {
                continue;
            }
            let Some(type_arguments) = imported_slot_arguments(type_info, adoption, trait_key) else {
                continue;
            };
            // A protocol whose behavior a Rust derive provides keeps the slots it leaves on that derive.
            let defaults = !(protocol && derive_backed_protocol(type_info, trait_key, method));
            if let Some(implementation) =
                imported_slot_implementation(type_info, method, slot_name, methods, defaults, owner)
            {
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    type_arguments,
                });
            }
        }
    }
    implementations.sort_by(|left, right| left.method.cmp(&right.method));
    implementations
}

/// Close a direct imported trait only from checked adoption arguments and its recorded declaration binders.
/// Inherited standard slots retain an empty frame only along the checked argument-free edges selected by
/// [`standard_supertrait_keys`]; their provider-qualified keys need not have direct-adoption binder records.
/// Generic inherited slots lack a retained instantiation here and remain excluded.
fn imported_slot_arguments(
    type_info: &TypeCheckInfo,
    adoption: &ast::Spanned<ast::TraitBound>,
    trait_key: &str,
) -> Option<Vec<bir::TraitTypeArgument>> {
    if trait_key != adoption.node.name {
        return standard_supertrait_keys(type_info, &adoption.node.name)
            .iter()
            .any(|key| key == trait_key)
            .then(Vec::new);
    }
    let parameters = type_info.traits.type_params.get(trait_key)?;
    if parameters.is_empty() {
        return adoption.node.type_args.is_empty().then(Vec::new);
    }
    let key = (adoption.span.start, adoption.span.end);
    let arguments = type_info
        .traits
        .body_ir_adoption_type_args
        .get(&key)
        .or_else(|| type_info.traits.adoption_type_args.get(&key))?;
    if parameters.len() != arguments.len() {
        return None;
    }
    Some(
        parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| bir::TraitTypeArgument {
                parameter: parameter.clone(),
                argument: semantic_type_from_resolved(argument),
            })
            .collect(),
    )
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
    owner: &CanonicalSymbolId,
) -> Option<CanonicalSymbolId> {
    let selected_name = checked_slot_method_name(type_info, owner, slot_name);
    let candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == selected_name && method_fills_trait_slot(type_info, &candidate.node, method)
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

/// Follow the checker's same-owner alias projection before selecting the physical method body for a trait slot.
///
/// Only the recorded alias map participates; source spellings do not introduce a new relationship. The finite walk
/// also keeps a corrupted cyclic map from hanging the bridge, although ordinary checked input excludes cycles.
pub(super) fn checked_slot_method_name<'facts>(
    type_info: &'facts TypeCheckInfo,
    owner: &CanonicalSymbolId,
    slot_name: &'facts str,
) -> &'facts str {
    let Some(aliases) = type_info
        .declarations
        .type_method_rebindings
        .get(&owner.declaration_name)
    else {
        return slot_name;
    };
    let mut name = slot_name;
    for _ in 0..=aliases.len() {
        let Some(target) = aliases.get(name) else {
            return name;
        };
        name = target;
    }
    slot_name
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
