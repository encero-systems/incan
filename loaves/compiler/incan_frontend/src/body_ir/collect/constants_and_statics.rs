//! Published storage types, receiver methods, constants, type aliases, statics and static initializers.

use super::*;

/// Retain imported storage types from their checked owning declarations without resolving a nominal by spelling.
/// Conflicting copies of one canonical declaration cannot authorize either representation at a consumer global place.
pub(in crate::body_ir) fn collect_published_storage_types(
    context: &[bir::BodyIrModule],
) -> HashMap<CanonicalSymbolId, Option<IncanType>> {
    let mut types = HashMap::new();
    for declaration in context.iter().flat_map(|module| &module.static_declarations) {
        match types.entry(declaration.canonical.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(declaration.ty.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != Some(&declaration.ty) {
                    entry.insert(None);
                }
            }
        }
    }
    types
}

/// Retain physical immutable instance methods with their checked owning declaration, once per source module.
///
/// An unresolved caller-open method has no mutability, decorator, or generic-header contract. Those shapes stay
/// refused; only the ordinary checked source body and signature may complete a newly concrete receiver.
pub(in crate::body_ir) fn collect_local_receiver_methods(
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
        let Some(owner) = facts.declarations.declaration_identities.get(&declaration.span) else {
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

/// Retain checker-evaluated constants, checked literal integer arithmetic and static literal aggregates once. Computed
/// or symbolic values remain absent. These
/// Body IR facts never update legacy evaluation tables.
pub(in crate::body_ir) fn collect_constants(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominals: &[bir::NominalDeclaration],
) -> Vec<bir::ConstantDeclaration> {
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
                .get(&declaration.span)?
                .clone();
            let value = match type_info.const_value(&item.name) {
                Some(crate::typechecker::ConstValue::Int(value)) => bir::Constant::Int(*value),
                Some(crate::typechecker::ConstValue::Float(value)) => bir::Constant::Float(value.to_string()),
                Some(crate::typechecker::ConstValue::Bool(value)) => bir::Constant::Bool(*value),
                Some(crate::typechecker::ConstValue::FrozenStr(value)) => bir::Constant::Str(value.clone()),
                None if type_info.expr_type(item.value.span) == Some(&ResolvedType::Int) => {
                    bir::Constant::Int(literal_integer_constant(&item.value.node)?)
                }
                _ => checked_newtype_constant(&item.value, type_info, nominals).or_else(|| {
                    frozen_literal_constant(
                        &item.value.node,
                        &semantic_type_from_resolved(type_info.expr_type(item.value.span)?),
                    )
                })?,
            };
            Some(bir::ConstantDeclaration { canonical, value })
        })
        .collect()
}

/// Bind the const checker's exact numeric value to its already retained source newtype layout.
/// The checker accepted one positional literal; only that proven one-slot binding is represented. Generic owners,
/// runtime ingress hooks, constraints, imported layouts and missing physical declarations remain unretained.
pub(super) fn checked_newtype_constant(
    expression: &ast::Spanned<ast::Expr>,
    type_info: &TypeCheckInfo,
    nominals: &[bir::NominalDeclaration],
) -> Option<bir::Constant> {
    let fact = type_info.consts.body_ir_newtype_values.get(&expression.span)?;
    let mut owners = nominals.iter().filter(|nominal| nominal.canonical == fact.owner);
    let owner = owners.next()?;
    if owners.next().is_some() {
        return None;
    }
    let ingress = type_info.declarations.newtype_construction.get(&owner.name)?;
    if ingress.checked_constructor.is_some() || !ingress.constraints.is_empty() {
        return None;
    }
    let value = bir::NumericNewtypeConstant {
        constructor: bir::ConstructorTarget {
            name: owner.name.clone(),
            canonical: Some(owner.canonical.clone()),
            direct_declaration_id: Some(owner.direct_declaration_id.clone()),
            canonical_field_layout: Some(owner.fields.clone()),
            binding: bir::ArgumentBinding::resolved_positional(1),
        },
        value: fact.value.clone(),
    };
    value
        .matches_layout(owner)
        .then(|| bir::Constant::NumericNewtype(Box::new(value)))
}

/// Evaluate only literal signed integer addition, subtraction, multiplication and negation. Each intermediate must fit
/// i64; names, calls, sized literals and every other operation stay unretained. A unary-negative literal is admitted
/// from its full magnitude before narrowing, so the minimum does not require an unrepresentable positive intermediate.
pub(super) fn literal_integer_constant(value: &ast::Expr) -> Option<i64> {
    match value {
        ast::Expr::Literal(ast::Literal::Int(value)) if value.suffix.is_none() => {
            i64::try_from(value.signed_value(false)?).ok()
        }
        ast::Expr::Unary(ast::UnaryOp::Neg, inner) => {
            if let ast::Expr::Literal(ast::Literal::Int(value)) = &inner.node
                && value.suffix.is_none()
            {
                return i64::try_from(value.signed_value(true)?).ok();
            }
            literal_integer_constant(&inner.node)?.checked_neg()
        }
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

/// Preserve only effect-free checked literal trees. Storage types come from checking; no call or spread is evaluated
/// again by Body IR or inferred from a literal's spelling.
pub(in crate::body_ir) fn frozen_literal_constant(value: &ast::Expr, ty: &IncanType) -> Option<bir::Constant> {
    if let ast::Expr::Literal(literal) = value {
        return Some(primitives::lower_checked_literal(literal, ty));
    }
    if let ast::Expr::Unary(ast::UnaryOp::Neg, inner) = value
        && let ast::Expr::Literal(literal) = &inner.node
    {
        return primitives::lower_checked_negative_literal(literal, ty);
    }
    if let (ast::Expr::Tuple(values), IncanType::Tuple(types)) = (value, ty) {
        if values.len() != types.len() || values.len() == 1 {
            return None;
        }
        let elements = values
            .iter()
            .zip(types)
            .map(|(value, ty)| frozen_literal_constant(&value.node, ty))
            .collect::<Option<Vec<_>>>()?;
        return Some(bir::Constant::FrozenAggregate(ty.clone(), elements));
    }
    let (collection, args) = ty.collection()?;
    let elements = match (collection, value, args) {
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

/// Retain alias-expanded nongeneric type declarations and their checker-proven nominal references.
pub(in crate::body_ir) fn collect_type_aliases(
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
                .get(&declaration.span)?
                .clone();
            Some(bir::TypeAliasDeclaration {
                canonical,
                ty: parameter_types::checked_semantic_type(type_info.type_alias_target(&item.name)?, type_info),
                named_type_identities: parameter_types::retained_type_identities(type_info),
            })
        })
        .collect()
}

/// Retain every static of this module: a primitive or primitive-collection literal, whose initialization has no
/// user-visible effects, keeps its literal initializer; any other checked initializer is lowered as a computed body
/// evaluated where legacy initializes the static (see [`bir::StaticInitializer::Computed`]).
pub(in crate::body_ir) fn collect_statics(
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
            let ty = registry_descriptions::initialization_type(storage, type_info)
                .or_else(|| registry_effects::entry_type(storage, declaration.span, type_info))
                .or_else(|| type_info.expr_type(storage.value.span).map(semantic_type_from_resolved))?;
            let canonical = type_info
                .declarations
                .declaration_identities
                .get(&declaration.span)?
                .clone();
            let initial = static_initializer(&storage.value, &ty, type_info).unwrap_or_else(|| {
                bir::StaticInitializer::Computed(Box::new(lower_static_initializer_body(
                    program,
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

/// Retain every `const` the checker did not evaluate to a retained constant (a list of model values, for one) as
/// read-only computed storage beside the module's statics: reads of it are its global place.
pub(in crate::body_ir) fn collect_computed_constants(
    program: &ast::Program,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
    retained: &[bir::ConstantDeclaration],
) -> Vec<bir::StaticDeclaration> {
    let type_info = lowering_facts.type_info;
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
                .get(&declaration.span)?
                .clone();
            if retained.iter().any(|constant| constant.canonical == canonical) {
                return None;
            }
            let ty = semantic_type_from_resolved(type_info.expr_type(item.value.span)?);
            let body = lower_constant_initializer_body(item, declaration.span, &canonical, &ty, lowering_facts);
            Some(bir::StaticDeclaration {
                canonical,
                ty,
                initial: bir::StaticInitializer::Computed(Box::new(body)),
            })
        })
        .collect()
}

/// Recognize the empty `Set()` constructor only from the checker's canonical constructor fact; every other
/// initializer must be a literal.
pub(super) fn static_initializer(
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
        let [element] = ty.collection_arguments(CollectionTypeId::Set)? else {
            return None;
        };
        return element
            .hashed_static_literal_carrier()
            .then(|| bir::StaticInitializer::Set(Vec::new()));
    }
    static_literal_initializer(&value.node, ty)
}

/// Preserve literal values only in their exact checked primitive carrier or primitive collection element carriers.
pub(super) fn static_literal_initializer(value: &ast::Expr, ty: &IncanType) -> Option<bir::StaticInitializer> {
    match value {
        ast::Expr::Literal(literal) if ty.static_literal_carrier() => Some(bir::StaticInitializer::Literal(
            primitives::lower_checked_literal(literal, ty),
        )),
        ast::Expr::List(entries) => {
            let [element] = ty.collection_arguments(CollectionTypeId::List)? else {
                return None;
            };
            if !element.static_literal_carrier() {
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
            let [key, value] = ty.collection_arguments(CollectionTypeId::Dict)? else {
                return None;
            };
            if !key.hashed_static_literal_carrier() || !value.static_literal_carrier() {
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
            let [element] = ty.collection_arguments(CollectionTypeId::Set)? else {
                return None;
            };
            if !element.hashed_static_literal_carrier() {
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
pub(super) fn static_literal_constant(value: &ast::Expr, ty: &IncanType) -> Option<bir::Constant> {
    match static_literal_initializer(value, ty)? {
        bir::StaticInitializer::Literal(constant) => Some(constant),
        _ => None,
    }
}
