//! Function default sources, published callable returns and defaults, and local function declarations.

use super::*;

/// Collect the source expressions a synthesized partial needs to retain target defaults in Body IR: a function's
/// parameter defaults, a model's or class's field defaults, or a plain newtype's required value parameter, in
/// declaration order.
pub(in crate::body_ir) fn collect_function_default_sources(
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
pub(super) fn field_default_sources(fields: &[ast::Spanned<ast::FieldDecl>]) -> Vec<FunctionDefaultSource> {
    fields
        .iter()
        .map(|field| FunctionDefaultSource {
            param_span: field.span,
            default: field.node.default.clone(),
        })
        .collect()
}

/// Retain public, nongeneric newtype results from governed or compiled method declarations. Missing layouts
/// and conflicting copies remain explicit denials; no type is selected from a method or field spelling.
pub(in crate::body_ir) fn collect_published_newtype_returns(
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
pub(super) fn declared_newtype_return_owner(body: &bir::Body) -> Option<&CanonicalSymbolId> {
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
pub(in crate::body_ir) fn collect_published_callable_returns(
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
pub(in crate::body_ir) fn collect_published_callable_defaults(
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
pub(in crate::body_ir) fn collect_local_function_declarations(program: &ast::Program) -> LocalFunctionDeclarations {
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
pub(in crate::body_ir) fn collect_local_nongeneric_function_identities(
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
