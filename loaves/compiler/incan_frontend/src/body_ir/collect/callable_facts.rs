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
    context: ContextModules<'_>,
) -> HashMap<CanonicalSymbolId, Option<PublishedNewtypeReturn>> {
    let mut returns = HashMap::new();
    let mut newtype_methods = HashSet::new();
    let layouts = published_newtype_layouts(context);
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
            let declaration = owner.and_then(|owner| layouts.get(owner).copied().flatten());
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

/// Index exact newtype owners across the retained modules: a method may return another module's declaration.
/// Validate each layout in its own module and keep missing, malformed or conflicting ownership as a denial.
fn published_newtype_layouts(
    context: ContextModules<'_>,
) -> HashMap<CanonicalSymbolId, Option<&bir::NominalDeclaration>> {
    let mut layouts = HashMap::new();
    for module in context {
        for declaration in &module.nominal_declarations {
            let owner = &declaration.canonical;
            if owner.kind != SemanticSourceTargetKind::Newtype {
                continue;
            }
            let candidate = (declaration.public
                && declaration.type_parameters.is_empty()
                && declaration.native_item.is_empty()
                && declaration.fields == ["0"]
                && declaration.field_identities == [owner.clone()]
                && declaration.field_public == [true]
                && module.is_well_formed_nominal_declaration(declaration))
            .then_some(declaration);
            layouts
                .entry(owner.clone())
                .and_modify(|retained| {
                    if *retained != candidate {
                        *retained = None;
                    }
                })
                .or_insert(candidate);
        }
    }
    layouts
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

/// Callable declaration facts borrowed from their admitted provider frames.
pub(in crate::body_ir) type PublishedCallables<'a> = HashMap<&'a CanonicalSymbolId, PublishedCallable<'a>>;

/// The callable's ordered parameters and its eligible closed return carrier.
#[derive(Default)]
pub(in crate::body_ir) struct PublishedCallable<'a> {
    pub(in crate::body_ir) parameters: &'a [bir::CallableParam],
    pub(in crate::body_ir) return_type: Option<&'a IncanType>,
}

/// Index both callable facts in one pass. A generic or unresolved return stays with the consumer's checked
/// specialization. The latest frame supplies its parameters, and the latest eligible frame supplies its return.
pub(in crate::body_ir) fn collect_published_callables<'a>(
    context: ContextModules<'a>,
    facts: &TypeCheckInfo,
) -> PublishedCallables<'a> {
    let _prepare =
        tracing::trace_span!(target: "incan::invocation", "incan_frontend::prepare_published_callables").entered();
    let hooks = facts
        .expressions
        .validated_newtype_coercions
        .values()
        .flat_map(|coercion| &coercion.steps)
        .filter_map(|step| step.ctor_identity.as_ref())
        .collect::<HashSet<_>>();
    let mut published = PublishedCallables::new();
    let mut frames = 0;
    let mut default_slots = 0;
    let mut return_types = 0;
    for body in context.iter().flat_map(|module| module.callable_declaration_frames()) {
        frames += 1;
        let Some(identity) = &body.canonical else { continue };
        let declaration = published.entry(identity).or_default();
        declaration.parameters = &body.params;
        default_slots += body.params.len();
        if body.type_parameters.is_empty()
            && !matches!(
                body.return_type,
                IncanType::Unknown | IncanType::Infer | IncanType::SelfType | IncanType::TypeVar(_)
            )
            && (identity.kind == SemanticSourceTargetKind::Function || hooks.contains(identity))
        {
            declaration.return_type = Some(&body.return_type);
            return_types += 1;
        }
    }
    tracing::trace!(target: "incan::invocation", frames, default_slots, return_types, "indexed published callables");
    published
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
