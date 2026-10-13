//! Retain concrete SDK adapter carriers from exact checked default constructors, without granting execution.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    CanonicalSymbolId, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir, semantic_type_from_resolved,
};
use crate::symbols::TypeInfo;
use crate::typechecker::{ResolvedMethodDispatch, stdlib_loader::StdlibAstCache};
use incan_semantics_core::SymbolOrigin;

/// One exact default whose whole return is an explicitly instantiated SDK model constructor.
struct Factory {
    trait_name: String,
    trait_parameters: Vec<String>,
    method_parameters: Vec<String>,
    constructor: CanonicalSymbolId,
    arguments: Vec<ast::Spanned<ast::Type>>,
    declared_result: IncanType,
    self_results: Vec<CanonicalSymbolId>,
}

/// One proven SDK constructor result, retaining its interface and exact Self-returning SDK slots.
#[derive(Clone)]
struct Carrier {
    declared: IncanType,
    concrete: IncanType,
    self_results: Vec<CanonicalSymbolId>,
}

/// Retain constructor identities needed by the checked SDK defaults called from this module.
pub(super) fn identities(facts: &TypeCheckInfo) -> BTreeSet<CanonicalSymbolId> {
    let mut cache = StdlibAstCache::new();
    facts
        .references
        .resolved_identities
        .values()
        .filter_map(|identity| factory(identity, &mut cache).map(|factory| factory.constructor))
        .collect()
}

/// Collect verified SDK adapter layouts without mutating the checker's legacy-visible metadata.
pub(super) fn layouts(facts: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut selected = TypeCheckInfo::default();
    for identity in identities(facts) {
        selected
            .declarations
            .named_type_identities
            .insert(identity.render_compact(), identity);
    }
    super::library_nominals::collect(&selected)
}

/// Retain whole temporary results of exact SDK defaults as concrete carriers or templates over declared caller binders.
pub(super) fn retain(module: &mut bir::BodyIrModule, facts: &TypeCheckInfo) {
    let mut cache = StdlibAstCache::new();
    let sdk_types = super::library_nominals::type_identities(facts, &mut cache);
    for body in &mut module.bodies {
        for identity in &sdk_types {
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            ) {
                body.named_type_identities
                    .entry(identity.declaration_name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        let binders = body.type_parameters.iter().cloned().collect::<BTreeSet<_>>();
        let mut carriers = BTreeMap::new();
        retain_block(
            &mut body.block,
            &mut body.locals,
            &mut body.named_type_identities,
            facts,
            &mut cache,
            &binders,
            &mut carriers,
        );
    }
}

/// Prove a default's concrete constructor from its declaring trait, exact method span, and explicit source result.
fn factory(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<Factory> {
    if identity.kind != SemanticSourceTargetKind::Method {
        return None;
    }
    let published = super::library_nominals::sdk_identity(identity)?;
    let SymbolOrigin::Package { module_path, .. } = &published.origin else {
        return None;
    };
    let path = std::iter::once("std".to_owned())
        .chain(module_path.iter().cloned())
        .collect::<Vec<_>>();
    for (owner, info) in cache.list_traits(&path) {
        let Some(method) = info.methods.get(&identity.declaration_name) else {
            continue;
        };
        let Some(mut declared) = method.identity.clone() else {
            continue;
        };
        if declared.origin != SymbolOrigin::Module(path.clone()) {
            continue;
        }
        declared.origin = published.origin.clone();
        if declared != published || !method.has_body || method.is_async {
            continue;
        }
        let declaration = cache.lookup_trait_decl(&path, &owner)?;
        let source = declaration.methods.iter().find(|method| {
            method.span.start == declared.declaration_span.start && method.span.end == declared.declaration_span.end
        })?;
        let mut statements = source.node.body.as_ref()?.iter().filter(|statement| {
            !matches!(&statement.node, ast::Statement::Expr(value) if matches!(&value.node, ast::Expr::Literal(ast::Literal::String(_))))
        });
        let ast::Statement::Return(Some(value)) = &statements.next()?.node else {
            return None;
        };
        if statements.next().is_some() {
            return None;
        }
        let ast::Expr::Call(callee, arguments, _) = &value.node else {
            return None;
        };
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        let mut constructor = cache.lookup_identity(&path, name)?;
        let TypeInfo::Model(model) = cache.lookup_type(&path, name)? else {
            return None;
        };
        if constructor.kind != SemanticSourceTargetKind::Model || model.type_params.len() != arguments.len() {
            return None;
        }
        constructor.origin = published.origin.clone();
        cache.callable_source_identity(&constructor)?;
        return Some(Factory {
            trait_name: owner,
            trait_parameters: info.type_params,
            method_parameters: method.type_params.clone(),
            constructor,
            arguments: arguments.clone(),
            declared_result: semantic_type_from_resolved(&method.return_type),
            self_results: info
                .methods
                .values()
                .filter(|slot| {
                    slot.receiver.is_some()
                        && !slot.is_async
                        && slot.params.is_empty()
                        && slot.type_params.is_empty()
                        && semantic_type_from_resolved(&slot.return_type) == IncanType::SelfType
                })
                .filter_map(|slot| slot.identity.clone())
                .collect(),
        });
    }
    None
}

/// Apply checked trait and method arguments after authenticating the exact call-site identity and owning trait.
/// Generic body checking retains call references even when its ambient trait-identity map has no entry.
fn instance(
    factory: &Factory,
    target: &bir::MethodTarget,
    span: ast::Span,
    receiver: IncanType,
    declared_result: &IncanType,
    facts: &TypeCheckInfo,
    binders: &BTreeSet<String>,
) -> Option<IncanType> {
    let call = facts.resolved_method_call(span)?;
    let ResolvedMethodDispatch::Trait {
        trait_name, type_args, ..
    } = &call.dispatch;
    if trait_name != &factory.trait_name
        || facts.references.resolved_identities.get(&(span.start, span.end)) != target.canonical.as_ref()
    {
        return None;
    }
    if factory.trait_parameters.len() != type_args.len() || factory.method_parameters.len() != target.type_args.len() {
        return None;
    }
    let mut bindings = factory
        .trait_parameters
        .iter()
        .cloned()
        .zip(type_args.iter().map(semantic_type_from_resolved))
        .collect::<BTreeMap<_, _>>();
    bindings.extend(
        factory
            .method_parameters
            .iter()
            .cloned()
            .zip(target.type_args.iter().cloned()),
    );
    bindings.insert("Self".to_owned(), receiver);
    if &substitute(&factory.declared_result, &bindings)? != declared_result {
        return None;
    }
    let args = factory
        .arguments
        .iter()
        .map(|argument| match &argument.node {
            ast::Type::Simple(name) => bindings.get(name).cloned(),
            ast::Type::SelfType => bindings.get("Self").cloned(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    args.iter().all(|arg| closed(arg, binders)).then(|| IncanType::Generic {
        base: factory.constructor.declaration_name.clone(),
        args,
    })
}

/// Instantiate the checked interface signature before claiming its concrete default constructor.
fn substitute(ty: &IncanType, bindings: &BTreeMap<String, IncanType>) -> Option<IncanType> {
    match ty {
        IncanType::TypeVar(name) => bindings.get(name).cloned(),
        IncanType::Named(name) if bindings.contains_key(name) => bindings.get(name).cloned(),
        IncanType::SelfType => bindings.get("Self").cloned(),
        IncanType::Generic { base, args } => Some(IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .map(|arg| substitute(arg, bindings))
                .collect::<Option<Vec<_>>>()?,
        }),
        _ => Some(ty.clone()),
    }
}

/// Allow only concrete types or the caller's declared binders in a verified constructor template.
///
/// Unknown types and undeclared placeholders grant no carrier. A declared binder preserves its identity until the
/// normal checked function specialization closes it; this does not turn a trait parameter into a nominal receiver.
fn closed(ty: &IncanType, binders: &BTreeSet<String>) -> bool {
    match ty {
        IncanType::Unknown | IncanType::SelfType => false,
        IncanType::TypeVar(name) => binders.contains(name),
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(|arg| closed(arg, binders)),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => closed(inner, binders),
        IncanType::Function { params, return_type } => {
            params.iter().all(|parameter| closed(&parameter.ty, binders)) && closed(return_type, binders)
        }
        _ => true,
    }
}

/// Read a proven whole nominal receiver; a trait parameter or projected field does not supply an opaque producer.
fn receiver_type(
    args: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> Option<IncanType> {
    let bir::ArgumentElement::One(bir::Operand::Place(read)) = args.first()? else {
        return None;
    };
    if !read.place.projection.is_empty() {
        return None;
    }
    let local = usize::try_from(read.place.local_id()?.0).ok()?;
    let ty = locals.get(local)?.ty.clone();
    let name = match &ty {
        IncanType::Named(name) => name,
        IncanType::Generic { base, .. } => base,
        _ => return None,
    };
    let owner = identities.get(name)?;
    matches!(
        owner.kind,
        SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
    )
    .then_some(ty)
}

/// Keep a proven constructor carrier through its SDK trait's Self-returning slot without granting missing dispatch.
fn retain_self_result(
    destination: &bir::Place,
    target: &bir::MethodTarget,
    args: &[bir::ArgumentElement],
    locals: &mut [bir::LocalDecl],
    carriers: &mut BTreeMap<usize, Carrier>,
) -> bool {
    if !target.type_args.is_empty() || args.len() != 1 || !destination.projection.is_empty() {
        return false;
    }
    let Some(bir::ArgumentElement::One(bir::Operand::Place(read))) = args.first() else {
        return false;
    };
    if !read.place.projection.is_empty() {
        return false;
    }
    let Some(source) = read.place.local_id().and_then(|local| usize::try_from(local.0).ok()) else {
        return false;
    };
    let Some(carrier) = carriers.get(&source).cloned() else {
        return false;
    };
    if !locals.get(source).is_some_and(|local| local.ty == carrier.concrete)
        || !carrier.self_results.iter().any(|slot| {
            slot.declaration_name == target.name && target.canonical.as_ref().is_none_or(|identity| identity == slot)
        })
    {
        return false;
    }
    let Some(output) = destination.local_id().and_then(|local| usize::try_from(local.0).ok()) else {
        return false;
    };
    let Some(local) = locals.get_mut(output) else {
        return false;
    };
    if local.origin != bir::LocalOrigin::Temporary || local.ty != carrier.declared {
        return false;
    }
    local.ty = carrier.concrete.clone();
    carriers.insert(output, carrier);
    true
}

/// Project temporary ABI carriers in evaluation order while keeping missing default bodies and targets unchanged.
fn retain_block(
    block: &mut bir::Block,
    locals: &mut [bir::LocalDecl],
    identities: &mut BTreeMap<String, CanonicalSymbolId>,
    facts: &TypeCheckInfo,
    cache: &mut StdlibAstCache,
    binders: &BTreeSet<String>,
    carriers: &mut BTreeMap<usize, Carrier>,
) {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            bir::StatementKind::Call {
                destination: Some(destination),
                callee: bir::Callee::Method(target),
                args,
                ..
            } => {
                if !destination.projection.is_empty() {
                    continue;
                }
                if retain_self_result(destination, target, args, locals, carriers) {
                    continue;
                }
                let Some(identity) = &target.canonical else {
                    continue;
                };
                let Some(factory) = factory(identity, cache) else {
                    continue;
                };
                let Some(receiver) = receiver_type(args, locals, identities) else {
                    continue;
                };
                let Some(local_index) = destination.local_id().and_then(|local| usize::try_from(local.0).ok()) else {
                    continue;
                };
                let Some(declared_result) = locals.get(local_index).map(|local| local.ty.clone()) else {
                    continue;
                };
                let Some(ty) = instance(
                    &factory,
                    target,
                    ast::Span::new(statement.span.start, statement.span.end),
                    receiver,
                    &declared_result,
                    facts,
                    binders,
                ) else {
                    continue;
                };
                let Some(local) = destination
                    .local_id()
                    .and_then(|local| usize::try_from(local.0).ok())
                    .and_then(|local| locals.get_mut(local))
                else {
                    continue;
                };
                if local.origin != bir::LocalOrigin::Temporary
                    || !matches!(&local.ty, IncanType::Generic { base, .. } if base == &factory.trait_name)
                {
                    continue;
                }
                if identities
                    .get(&factory.constructor.declaration_name)
                    .is_some_and(|owner| owner != &factory.constructor)
                {
                    continue;
                }
                identities.insert(factory.constructor.declaration_name.clone(), factory.constructor);
                carriers.insert(
                    local_index,
                    Carrier {
                        declared: declared_result,
                        concrete: ty.clone(),
                        self_results: factory.self_results,
                    },
                );
                local.ty = ty;
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_block(then_block, locals, identities, facts, cache, binders, carriers);
                if let Some(else_block) = else_block {
                    retain_block(else_block, locals, identities, facts, cache, binders, carriers);
                }
            }
            bir::StatementKind::Loop { body } => {
                retain_block(body, locals, identities, facts, cache, binders, carriers)
            }
            _ => {}
        }
    }
}
