//! One-pass collection of the module-local facts lowering retains: defaults, declarations, and canonical member
//! layouts.

use super::*;
use crate::typechecker::ClassLayoutInfo;

/// Retain imported storage types from their checked owning declarations without resolving a nominal by spelling.
/// Conflicting copies of one canonical declaration cannot authorize either representation at a consumer global place.
pub(super) fn collect_published_storage_types(
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
pub(super) fn collect_constants(
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
fn checked_newtype_constant(
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
fn literal_integer_constant(value: &ast::Expr) -> Option<i64> {
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
pub(super) fn frozen_literal_constant(value: &ast::Expr, ty: &IncanType) -> Option<bir::Constant> {
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
pub(super) fn collect_computed_constants(
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
/// The source-owned return of one checked nongeneric method, indexed without resolving a consumer spelling.
#[derive(Clone, PartialEq)]
pub(super) struct PublishedNewtypeReturn {
    /// The providing frame's checked return carrier, with its own canonical type binding.
    pub(super) ty: IncanType,
    /// Exact checked storage selected by that return binding; a consumer must retain this same declaration.
    pub(super) declaration: bir::NominalDeclaration,
}

/// Retain public, nongeneric newtype results from governed or compiled method declarations. Missing layouts
/// and conflicting copies remain explicit denials; no type is selected from a method or field spelling.
pub(super) fn collect_published_newtype_returns(
    context: &[bir::BodyIrModule],
) -> HashMap<CanonicalSymbolId, Option<PublishedNewtypeReturn>> {
    let mut returns = HashMap::new();
    let mut newtype_methods = HashSet::new();
    for module in context {
        for body in module.callable_declaration_frames() {
            let Some(identity) = body.canonical.as_ref().filter(|identity| {
                identity.kind == SemanticSourceTargetKind::Method
                    && identity.namespace == incan_semantics_core::SymbolNamespace::Member
            }) else {
                continue;
            };
            let owner = declared_newtype_return_owner(body);
            if owner.is_some() {
                newtype_methods.insert(identity.clone());
            }
            let declaration = owner.and_then(|owner| {
                module.nominal_declarations.iter().find(|declaration| {
                    declaration.canonical == *owner
                        && declaration.public
                        && declaration.type_parameters.is_empty()
                        && declaration.native_item.is_empty()
                        && declaration.fields == ["0"]
                        && declaration.field_identities == [owner.clone()]
                        && declaration.field_public == [true]
                        && module.is_well_formed_nominal_declaration(declaration)
                })
            });
            let candidate = declaration.map(|declaration| PublishedNewtypeReturn {
                ty: body.return_type.clone(),
                declaration: declaration.clone(),
            });
            match returns.entry(identity.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if entry.get() != &candidate {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    returns.retain(|identity, _| newtype_methods.contains(identity));
    returns
}

/// Read only a closed, synchronous source method's own checked newtype return binding. Other copies still contribute
/// a denial to conflict detection, so losing this binding cannot make context order decide whether storage is valid.
fn declared_newtype_return_owner(body: &bir::Body) -> Option<&CanonicalSymbolId> {
    if body.is_async || !body.type_parameters.is_empty() {
        return None;
    }
    let IncanType::Named(name) = &body.return_type else {
        return None;
    };
    body.named_type_identities
        .get(name)
        .filter(|owner| owner.kind == SemanticSourceTargetKind::Newtype)
}

/// Retain a nongeneric function's or checker-selected validation hook's exact declaration return carrier before its
/// private aliases leave the provider. Ordinary method results keep their consumer's checked specialization.
/// Generic callable results keep the consumer's checked instantiation facts; unresolved producer results never
/// replace a consumer's type. The canonical key identifies the selected declaration, not a written import spelling.
pub(super) fn collect_published_callable_returns(
    context: &[bir::BodyIrModule],
    facts: &TypeCheckInfo,
) -> HashMap<CanonicalSymbolId, IncanType> {
    let hooks = facts
        .expressions
        .validated_newtype_coercions
        .values()
        .flat_map(|coercion| &coercion.steps)
        .filter_map(|step| step.ctor_identity.as_ref())
        .collect::<HashSet<_>>();
    context
        .iter()
        .flat_map(|module| module.callable_declaration_frames())
        .filter(|body| body.type_parameters.is_empty())
        .filter(|body| {
            !matches!(
                body.return_type,
                IncanType::Unknown | IncanType::Infer | IncanType::SelfType | IncanType::TypeVar(_)
            )
        })
        .filter_map(|body| {
            let identity = body.canonical.as_ref()?;
            (identity.kind == SemanticSourceTargetKind::Function || hooks.contains(identity))
                .then(|| (identity.clone(), body.return_type.clone()))
        })
        .collect()
}

/// Retain each published callable's parameter defaults, in parameter order, from its declaration record among the
/// linked Loaves' declarations, so a call that omits an argument passes the declared default.
pub(super) fn collect_published_callable_defaults(
    context: &[bir::BodyIrModule],
) -> HashMap<CanonicalSymbolId, Vec<bir::CallableParamDefault>> {
    context
        .iter()
        .flat_map(|module| module.callable_declaration_frames())
        .filter_map(|body| {
            let identity = body.canonical.as_ref()?;
            Some((
                identity.clone(),
                body.params.iter().map(|param| param.default.clone()).collect(),
            ))
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

/// Retain exact physical function identities whose checked declaration needs no type-argument instantiation.
/// Intrinsic references can omit expression snapshots, but generic or imported declarations cannot use this fallback.
pub(super) fn collect_local_nongeneric_function_identities(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
) -> HashSet<CanonicalSymbolId> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Function(function) = &declaration.node else {
                return None;
            };
            if !function.type_params.is_empty() {
                return None;
            }
            type_info
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
                .and_then(|binding| binding.identity.clone())
        })
        .collect()
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

/// Retain source field order while resolving types from the checked class layout or span-keyed model facts.
/// A missing field fact keeps the existing unknown carrier rather than inferring a layout from source spelling.
fn nominal_field_types(
    fields: &[ast::Spanned<ast::FieldDecl>],
    class_layout: Option<&ClassLayoutInfo>,
    type_parameters: &[String],
    type_info: &TypeCheckInfo,
) -> Vec<IncanType> {
    fields
        .iter()
        .map(|field| {
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

/// Retain class reflection and the checked parent-first field layout with ancestor identities.
fn retain_inherited_layout(
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
fn retain_class_reflection(retained: &mut bir::NominalDeclaration, facts: &TypeCheckInfo) {
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
fn checked_trait_adoptions(owner: &str, parameters: &[String], facts: &TypeCheckInfo) -> Vec<bir::TraitAdoption> {
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
fn collect_field_metadata(
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
                .get(&declaration.span)
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
/// A generic trait adopted without recorded arguments contributes nothing. Distinct checked instantiations retain
/// separate dispatch records; their slot arguments distinguish the same declaration's overload selections.
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
                .get(&adoption.span)
                .or_else(|| type_info.traits.adoption_type_args.get(&adoption.span))
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
    let mut candidates = methods
        .iter()
        .filter(|candidate| {
            candidate.node.name == selected_name && method_fills_trait_slot(type_info, &candidate.node, identity)
        })
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        candidates.retain(|candidate| instantiated_slot_signature_matches(type_info, slot_trait, slot, candidate));
    }
    match candidates.as_slice() {
        [candidate] => type_info
            .declarations
            .method_bindings_by_span
            .get(&candidate.span)
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

/// Select among checked overloads using the slot's instantiated signature contract. The checker already proved
/// adoption compatibility; Body IR consumes its resolved signatures instead of resolving annotations again.
fn instantiated_slot_signature_matches(
    facts: &TypeCheckInfo,
    adopted: &AdoptedTrait<'_>,
    slot: &ast::Spanned<ast::MethodDecl>,
    candidate: &ast::Spanned<ast::MethodDecl>,
) -> bool {
    let bindings = &facts.declarations.method_bindings_by_span;
    let Some(expected) = bindings.get(&slot.span) else {
        return false;
    };
    let Some(actual) = bindings.get(&candidate.span) else {
        return false;
    };
    let parameters = adopted
        .declaration
        .type_params
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect::<Vec<_>>();
    let substitution = crate::resolved_type_subst::type_param_subst_map(&parameters, &adopted.arguments);
    expected.params.len() == actual.params.len()
        && instantiate_declared_slot_type(&expected.return_type, &substitution) == actual.return_type
        && expected.params.iter().zip(&actual.params).all(|(expected, actual)| {
            expected.kind == actual.kind && instantiate_declared_slot_type(&expected.ty, &substitution) == actual.ty
        })
}

/// Substitute only the declaring trait's binders, including legacy Named placeholders retained by checked abstract
/// signatures. A same-spelled nominal outside that declaration's parameter map never becomes a binder.
fn instantiate_declared_slot_type(ty: &ResolvedType, bindings: &HashMap<String, ResolvedType>) -> ResolvedType {
    crate::resolved_type_subst::map_resolved_type_leaves(ty, &|leaf| match leaf {
        ResolvedType::Named(name) | ResolvedType::TypeVar(name) => bindings.get(name).cloned(),
        _ => None,
    })
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
                    .get(&candidate.span)
                    .and_then(|binding| binding.identity.clone()),
                (None, None) if method.has_default => Some(method.method.clone()),
                _ => None,
            }?;
            Some(bir::TraitImplementation {
                owner: owner.clone(),
                method: method.method.clone(),
                implementation,
                slot_arguments: method
                    .type_arguments
                    .iter()
                    .map(|(_, argument)| semantic_type_from_resolved(argument))
                    .collect(),
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

/// Snapshot checked local trait adoption arguments independently from the body that fills each method slot.
/// Own methods carry no default-body arguments, so their slot records cannot substitute for this checked adoption.
pub(super) fn collect_trait_argument_witnesses(
    program: &ast::Program,
    facts: &TypeCheckInfo,
    nominals: &[bir::NominalDeclaration],
) -> std::collections::BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>> {
    let mut witnesses = std::collections::BTreeMap::new();
    for declaration in &program.declarations {
        let adoptions = match &declaration.node {
            ast::Declaration::Model(model) => &model.traits,
            ast::Declaration::Class(class) => &class.traits,
            ast::Declaration::Newtype(value) => &value.traits,
            _ => continue,
        };
        let Some(identity) = facts.declarations.declaration_identities.get(&declaration.span) else {
            continue;
        };
        let Some(owner) = nominals.iter().find(|owner| &owner.canonical == identity) else {
            continue;
        };
        for adopted in adopted_local_traits(program, facts, adoptions) {
            let Some(trait_owner) = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.node {
                    ast::Declaration::Trait(value) if std::ptr::eq(value, adopted.declaration) => {
                        facts.declarations.declaration_identities.get(&declaration.span)
                    }
                    _ => None,
                })
            else {
                continue;
            };
            let arguments = adopted
                .arguments
                .iter()
                .map(|argument| parameter_types::checked_parameter_type(argument, facts, &owner.type_parameters))
                .collect();
            witnesses.insert((identity.clone(), trait_owner.clone()), arguments);
        }
    }
    witnesses
}

/// Retain trait slots and their checked concrete implementation identities for each admitted adopter.
///
/// Successful typechecking already proves adoption and method compatibility. This registry covers the adopter's local
/// traits at their checked instantiations and every local supertrait those reach; each slot keeps the body that fills
/// it and the type arguments that body's trait was adopted at. Imported traits without type parameters keep their own
/// slots under the checked provider identities (see [`imported_trait_implementations`]). A class's Rust trait slots
/// keep the methods checking bound to them; the native compiler, not this registry, proves that implementation.
/// Unrecorded instantiations and overloaded implementations never gain a guessed target. A refused checked newtype
/// constructor cannot contribute an implementation without its owner layout, nor can an enum whose layout was not
/// retained.
pub(super) fn collect_local_trait_implementations(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    nominal_declarations: &[bir::NominalDeclaration],
    enum_declarations: &[bir::EnumDeclaration],
) -> Vec<bir::TraitImplementation> {
    let mut implementations = Vec::new();
    for declaration in &program.declarations {
        let (adoptions, methods, properties) = match &declaration.node {
            ast::Declaration::Model(model) if is_direct_replacement_checked_model(model, type_info) => (
                model.traits.as_slice(),
                model.methods.as_slice(),
                model.properties.as_slice(),
            ),
            ast::Declaration::Class(class) if is_direct_replacement_checked_class(class, type_info) => (
                class.traits.as_slice(),
                class.methods.as_slice(),
                class.properties.as_slice(),
            ),
            ast::Declaration::Newtype(newtype) if is_direct_replacement_checked_newtype(newtype, type_info) => {
                (newtype.traits.as_slice(), newtype.methods.as_slice(), &[][..])
            }
            ast::Declaration::Enum(value) if is_direct_native_enum(value) => {
                (value.traits.as_slice(), value.methods.as_slice(), &[][..])
            }
            _ => continue,
        };
        let Some(owner) = type_info.declarations.declaration_identities.get(&declaration.span) else {
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
        implementations.extend(
            type_info
                .declarations
                .native_trait_implementations
                .iter()
                .filter(|implementation| &implementation.owner == owner)
                .cloned(),
        );
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
                if implementations.iter().any(|existing: &bir::TraitImplementation| {
                    &existing.owner == owner
                        && &existing.method == method
                        && existing.slot_arguments
                            == slot_trait
                                .arguments
                                .iter()
                                .map(semantic_type_from_resolved)
                                .collect::<Vec<_>>()
                }) {
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
                    slot_arguments: slot_trait.arguments.iter().map(semantic_type_from_resolved).collect(),
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
                .get(&slot.span)
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
                .get(&property.span)
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
                slot_arguments: trait_info.arguments.iter().map(semantic_type_from_resolved).collect(),
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
        let source_comparison = adopted_slots(type_info, name)
            .into_iter()
            .any(|(_, slot_name, method)| {
                matches!(slot_name, "__eq__" | "__lt__")
                    && super::source_comparison::is_standard_slot(method, slot_name)
                    && imported_slot_implementation(type_info, method, slot_name, methods, false, owner).is_some()
            });
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
            // Source comparison bridges retain the defaults they must forward; derive-only protocols leave those slots
            // on the native derive.
            let defaults = source_comparison || !(protocol && derive_backed_protocol(type_info, trait_key, method));
            if let Some(implementation) =
                imported_slot_implementation(type_info, method, slot_name, methods, defaults, owner)
            {
                implementations.push(bir::TraitImplementation {
                    owner: owner.clone(),
                    method: method.clone(),
                    implementation,
                    slot_arguments: type_arguments.iter().map(|binding| binding.argument.clone()).collect(),
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
    let key = adoption.span;
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

/// The retained keys of a standard trait's standard supertraits without type arguments, transitively.
///
/// Dependency-only slots use their provider-qualified key, while an explicitly imported supertrait can retain only its
/// visible binding. Both forms must carry the checked standard slot identities from the registry's module. A source
/// trait with the same spelling grants no inherited standard slots.
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
            let Some(key) = retained_standard_trait_key(type_info, supertrait, &segments) else {
                continue;
            };
            keys.push(key);
            pending.push(supertrait.clone());
        }
    }
    keys
}

/// Resolve an already checked standard edge to its actual slot key, requiring the registered declaring module and an
/// active standard binding.
fn retained_standard_trait_key(type_info: &TypeCheckInfo, name: &str, segments: &[String]) -> Option<String> {
    if segments.first().map(String::as_str) != Some("std") {
        return None;
    }
    for key in [format!("{}.{name}", segments.join(".")), name.to_owned()] {
        if !type_info.traits.is_standard_trait(&key) {
            continue;
        }
        let matches_module = type_info
            .traits
            .method_identities
            .iter()
            .filter(|((owner, _), _)| owner == &key)
            .all(|(_, identity)| match &identity.origin {
                incan_semantics_core::SymbolOrigin::Module(path) => path.as_slice() == segments,
                incan_semantics_core::SymbolOrigin::Package { module_path, .. } => {
                    module_path.as_slice() == &segments[1..]
                }
                _ => false,
            });
        if matches_module {
            return Some(key);
        }
    }
    None
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
            .get(&candidate.span)
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
