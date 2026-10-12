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

/// Refine only whole temporary results of exact SDK defaults whose checked arguments close their constructor.
pub(super) fn retain(module: &mut bir::BodyIrModule, facts: &TypeCheckInfo) {
    let mut cache = StdlibAstCache::new();
    for body in &mut module.bodies {
        retain_block(
            &mut body.block,
            &mut body.locals,
            &mut body.named_type_identities,
            facts,
            &mut cache,
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
        });
    }
    None
}

/// Apply checked trait and method arguments to the source constructor's declared placeholders.
fn instance(
    factory: &Factory,
    target: &bir::MethodTarget,
    span: ast::Span,
    receiver: IncanType,
    declared_result: &IncanType,
    facts: &TypeCheckInfo,
) -> Option<IncanType> {
    let call = facts.resolved_method_call(span)?;
    let ResolvedMethodDispatch::Trait {
        trait_name, type_args, ..
    } = &call.dispatch;
    if facts
        .traits
        .method_identities
        .get(&(trait_name.clone(), target.name.clone()))
        != target.canonical.as_ref()
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
    args.iter().all(closed).then(|| IncanType::Generic {
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

/// Keep unresolved binders and unknown types out of concrete adapter claims.
fn closed(ty: &IncanType) -> bool {
    match ty {
        IncanType::Unknown | IncanType::SelfType | IncanType::TypeVar(_) => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(closed),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => closed(inner),
        IncanType::Function { params, return_type } => {
            params.iter().all(|parameter| closed(&parameter.ty)) && closed(return_type)
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

/// Project temporary ABI carriers in evaluation order while keeping missing default bodies and targets unchanged.
fn retain_block(
    block: &mut bir::Block,
    locals: &mut [bir::LocalDecl],
    identities: &mut BTreeMap<String, CanonicalSymbolId>,
    facts: &TypeCheckInfo,
    cache: &mut StdlibAstCache,
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
                local.ty = ty;
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                retain_block(then_block, locals, identities, facts, cache);
                if let Some(else_block) = else_block {
                    retain_block(else_block, locals, identities, facts, cache);
                }
            }
            bir::StatementKind::Loop { body } => retain_block(body, locals, identities, facts, cache),
            _ => {}
        }
    }
}
