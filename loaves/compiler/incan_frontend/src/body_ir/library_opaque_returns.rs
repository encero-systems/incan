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
    let mut cache = facts.body_ir_stdlib_cache();
    facts
        .references
        .resolved_identities
        .values()
        .filter_map(|identity| factory(identity, &mut cache).map(|factory| factory.constructor))
        .collect()
}

/// Require the selected public factory body when its declared result is its own trait interface.
/// Such SDK methods use an opaque native return ABI; the retained concrete layout alone cannot authorize an executable
/// call or a return-type coercion.
pub(super) fn body_requirements(facts: &TypeCheckInfo) -> BTreeSet<CanonicalSymbolId> {
    let mut cache = facts.body_ir_stdlib_cache();
    facts
        .references
        .resolved_identities
        .values()
        .filter_map(|identity| {
            let factory = factory(identity, &mut cache)?;
            let interface = match &factory.declared_result {
                IncanType::Named(name) => name,
                IncanType::Generic { base, .. } => base,
                _ => return None,
            };
            (interface == &factory.trait_name)
                .then(|| super::library_nominals::sdk_identity(identity))
                .flatten()
        })
        .collect()
}

/// Collect verified SDK adapter layouts without mutating the checker's legacy-visible metadata.
pub(super) fn layouts(facts: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut selected = TypeCheckInfo {
        body_ir_source_cache: facts.body_ir_source_cache.clone(),
        ..TypeCheckInfo::default()
    };
    for identity in identities(facts) {
        selected
            .declarations
            .named_type_identities
            .insert(identity.render_compact(), identity);
    }
    super::library_nominals::collect(&selected)
}

/// Retain whole temporary results of exact SDK defaults as concrete carriers or templates over declared caller binders.
///
/// Each body keeps ordinary SDK identities and authenticated default constructors whose whole nominal carrier is
/// already present in its checked locals. An opaque trait parameter cannot acquire a producer from a default's layout.
pub(super) fn retain(module: &mut bir::BodyIrModule, facts: &TypeCheckInfo) {
    let mut cache = facts.body_ir_stdlib_cache();
    let ordinary_types = super::library_nominals::type_identities(facts, &mut cache);
    let sdk_types = super::library_type_identities(facts);
    for body in &mut module.bodies {
        for identity in &sdk_types {
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
            ) {
                if !ordinary_types.contains(identity)
                    && !body
                        .locals
                        .iter()
                        .any(|local| whole_nominal_name(&local.ty) == Some(identity.declaration_name.as_str()))
                {
                    continue;
                }
                body.named_type_identities
                    .entry(identity.declaration_name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        let binders = body.type_parameters.iter().cloned().collect::<BTreeSet<_>>();
        let mut carriers = BTreeMap::new();
        let mut writes = BTreeMap::new();
        let mut iteration_borrows = BTreeSet::new();
        count_local_writes(&body.block.stmts, &mut writes, &mut iteration_borrows);
        retain_block(
            &mut body.block.stmts,
            &mut body.locals,
            &mut body.named_type_identities,
            facts,
            &mut cache,
            &binders,
            &writes,
            &iteration_borrows,
            &mut carriers,
        );
    }
}

/// Read the root name of a checked named or applied type; identity authentication and whole-place checks remain with
/// callers.
fn whole_nominal_name(ty: &IncanType) -> Option<&str> {
    match ty {
        IncanType::Named(name) => Some(name),
        IncanType::Generic { base, .. } => Some(base),
        _ => None,
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
        let self_results = info
            .methods
            .values()
            .filter(|slot| {
                slot.receiver.is_some()
                    && !slot.is_async
                    && slot.params.is_empty()
                    && slot.type_params.is_empty()
                    && (semantic_type_from_resolved(&slot.return_type) == IncanType::SelfType
                        || transparent_interface_self_result(&declaration, &owner, &info.type_params, slot))
            })
            .filter_map(|slot| slot.identity.as_ref().and_then(super::library_nominals::sdk_identity))
            .collect();
        return Some(Factory {
            trait_name: owner,
            trait_parameters: info.type_params,
            method_parameters: method.type_params.clone(),
            constructor,
            arguments: arguments.clone(),
            declared_result: semantic_type_from_resolved(&method.return_type),
            self_results,
        });
    }
    None
}

/// Prove an opaque interface-returning default keeps its receiver's concrete carrier.
/// Only the exact declaring slot and a sole `return self` body qualify; another adapter, effects or a differently
/// instantiated interface cannot confer this witness. The default call still executes normally.
fn transparent_interface_self_result(
    declaration: &ast::TraitDecl,
    owner: &str,
    parameters: &[String],
    slot: &crate::symbols::MethodInfo,
) -> bool {
    let IncanType::Generic { base, args } = semantic_type_from_resolved(&slot.return_type) else {
        return false;
    };
    if base != owner || args.len() != parameters.len() || !args.iter().zip(parameters).all(|(argument, name)| {
        matches!(argument, IncanType::Named(actual) | IncanType::TypeVar(actual) if actual == name)
    }) {
        return false;
    }
    let Some(identity) = &slot.identity else { return false };
    let Some(method) = declaration.methods.iter().find(|method| {
        method.span.start == identity.declaration_span.start && method.span.end == identity.declaration_span.end
    }) else {
        return false;
    };
    let Some(body) = &method.node.body else { return false };
    let mut statements = body.iter().filter(|statement| {
        !matches!(&statement.node, ast::Statement::Expr(value) if matches!(&value.node, ast::Expr::Literal(ast::Literal::String(_))))
    });
    matches!(statements.next().map(|statement| &statement.node), Some(ast::Statement::Return(Some(value))) if matches!(&value.node, ast::Expr::SelfExpr))
        && statements.next().is_none()
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
    let ResolvedMethodDispatch::Trait { type_args, .. } = &call.dispatch;
    // The exact selected slot already proves its declaring trait and source span. A module alias may change the
    // dispatch's diagnostic spelling without changing that identity or its checked arguments.
    if facts.references.resolved_identities.get(&(span.start, span.end)) != target.canonical.as_ref() {
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
    let name = whole_nominal_name(&ty)?;
    let direct = identities.get(name);
    let encoded = identities
        .values()
        .find(|identity| bir::nominal_carrier_key(identity).as_deref() == Some(name));
    let owner = match encoded {
        Some(owner) if direct.is_none_or(|binding| binding == owner) => owner,
        Some(_) => return None,
        None => direct?,
    };
    matches!(
        owner.kind,
        SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
    )
    .then_some(ty)
}

/// Keep a proven constructor carrier through its SDK trait's Self-returning slot without granting missing dispatch.
/// Checked source and package identities normalize to the same SDK scope; the physical slot and span must still match.
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
            slot.declaration_name == target.name
                && target
                    .canonical
                    .as_ref()
                    .and_then(super::library_nominals::sdk_identity)
                    .is_some_and(|actual| Some(actual) == super::library_nominals::sdk_identity(slot))
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

/// Count writes to each local across the whole body before projecting a trait-facing binding onto a fixed carrier.
/// Multiple writes, including branch writes and projected writes, keep the declared interface opaque. Builtin polling
/// also identifies the exact temporary whose retained borrow carries an authenticated concrete iterator.
fn count_local_writes(
    statements: &[bir::Statement],
    writes: &mut BTreeMap<usize, usize>,
    iteration_borrows: &mut BTreeSet<usize>,
) {
    for statement in statements {
        let destination = match &statement.kind {
            bir::StatementKind::Assign { place, .. } => Some(place),
            bir::StatementKind::Call { destination, .. }
            | bir::StatementKind::Await { destination, .. }
            | bir::StatementKind::Race { destination, .. } => destination.as_ref(),
            bir::StatementKind::TryPropagate { destination, .. } | bir::StatementKind::IterNext { destination, .. } => {
                Some(destination)
            }
            _ => None,
        };
        if let Some(index) = destination
            .and_then(bir::Place::local_id)
            .and_then(|local| usize::try_from(local.0).ok())
        {
            *writes.entry(index).or_default() += 1;
        }
        match &statement.kind {
            bir::StatementKind::IterNext {
                iterator: bir::Operand::Place(read),
                protocol: bir::IterProtocol::Builtin,
                ..
            } if read.place.projection.is_empty() => {
                if let Some(index) = read.place.local_id().map(|local| local.index()) {
                    iteration_borrows.insert(index);
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                count_local_writes(&then_block.stmts, writes, iteration_borrows);
                if let Some(block) = else_block {
                    count_local_writes(&block.stmts, writes, iteration_borrows);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                count_local_writes(&body.stmts, writes, iteration_borrows);
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    *writes.entry(arm.binding.index()).or_default() += 1;
                    count_local_writes(&arm.body.stmts, writes, iteration_borrows);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    count_local_writes(&arm.guard_stmts, writes, iteration_borrows);
                    count_local_writes(&arm.body_stmts, writes, iteration_borrows);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => count_local_writes(&body.stmts, writes, iteration_borrows),
            _ => {}
        }
    }
}

/// Preserve an authenticated constructor through one whole owned assignment, never through a projection or type-erased
/// replacement. A builtin polling temporary can keep the same concrete carrier through its checked shared borrow;
/// this does not grant an owned value to a user binding or an arbitrary borrowed alias.
fn retain_alias(
    destination: &bir::Place,
    rvalue: &bir::Rvalue,
    locals: &mut [bir::LocalDecl],
    writes: &BTreeMap<usize, usize>,
    iteration_borrows: &BTreeSet<usize>,
    carriers: &mut BTreeMap<usize, Carrier>,
) {
    let Some(output) = destination.local_id().and_then(|local| usize::try_from(local.0).ok()) else {
        return;
    };
    carriers.remove(&output);
    if !destination.projection.is_empty() || writes.get(&output) != Some(&1) {
        return;
    }
    let bir::Rvalue::Use(bir::Operand::Place(read)) = rvalue else {
        return;
    };
    let iteration_borrow = read.fact == bir::OwnershipFact::Borrow
        && iteration_borrows.contains(&output)
        && locals
            .get(output)
            .is_some_and(|local| local.origin == bir::LocalOrigin::Temporary);
    if !read.place.projection.is_empty()
        || (!iteration_borrow && !matches!(read.fact, bir::OwnershipFact::Move | bir::OwnershipFact::Clone))
    {
        return;
    }
    let Some(source) = read.place.local_id().and_then(|local| usize::try_from(local.0).ok()) else {
        return;
    };
    let Some(carrier) = carriers.get(&source).cloned() else {
        return;
    };
    if !locals.get(source).is_some_and(|local| local.ty == carrier.concrete) {
        return;
    }
    let Some(local) = locals.get_mut(output) else {
        return;
    };
    if !matches!(
        local.origin,
        bir::LocalOrigin::Temporary | bir::LocalOrigin::UserBinding
    ) || (local.ty != carrier.declared && local.ty != carrier.concrete)
    {
        return;
    }
    local.ty = carrier.concrete.clone();
    carriers.insert(output, carrier);
}

/// Project temporary ABI carriers in evaluation order while keeping missing default bodies and targets unchanged.
/// Match guards and arms, race bodies, and unsafe regions use the enclosing body's local arena; visiting their
/// statements preserves the same authenticated constructor facts as an ordinary nested branch.
#[expect(
    clippy::too_many_arguments,
    reason = "The recursive pass borrows independent checked authority tables and mutable carrier outputs without cloning them."
)]
fn retain_block(
    statements: &mut [bir::Statement],
    locals: &mut [bir::LocalDecl],
    identities: &mut BTreeMap<String, CanonicalSymbolId>,
    facts: &TypeCheckInfo,
    cache: &mut StdlibAstCache,
    binders: &BTreeSet<String>,
    writes: &BTreeMap<usize, usize>,
    iteration_borrows: &BTreeSet<usize>,
    carriers: &mut BTreeMap<usize, Carrier>,
) {
    for statement in statements {
        match &mut statement.kind {
            bir::StatementKind::Assign { place, rvalue } => {
                if let bir::Rvalue::Match { arms, .. } = rvalue {
                    for arm in arms {
                        retain_block(
                            &mut arm.guard_stmts,
                            locals,
                            identities,
                            facts,
                            cache,
                            binders,
                            writes,
                            iteration_borrows,
                            carriers,
                        );
                        retain_block(
                            &mut arm.body_stmts,
                            locals,
                            identities,
                            facts,
                            cache,
                            binders,
                            writes,
                            iteration_borrows,
                            carriers,
                        );
                    }
                }
                retain_alias(place, rvalue, locals, writes, iteration_borrows, carriers);
            }
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
                retain_block(
                    &mut then_block.stmts,
                    locals,
                    identities,
                    facts,
                    cache,
                    binders,
                    writes,
                    iteration_borrows,
                    carriers,
                );
                if let Some(else_block) = else_block {
                    retain_block(
                        &mut else_block.stmts,
                        locals,
                        identities,
                        facts,
                        cache,
                        binders,
                        writes,
                        iteration_borrows,
                        carriers,
                    );
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => retain_block(
                &mut body.stmts,
                locals,
                identities,
                facts,
                cache,
                binders,
                writes,
                iteration_borrows,
                carriers,
            ),
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    retain_block(
                        &mut arm.body.stmts,
                        locals,
                        identities,
                        facts,
                        cache,
                        binders,
                        writes,
                        iteration_borrows,
                        carriers,
                    );
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod transparent_default_tests {
    use super::{StdlibAstCache, transparent_interface_self_result};

    /// The real SDK Iterator identity default is recognized from its exact declaration, independently of a caller.
    #[test]
    fn real_iterator_identity_default_keeps_its_carrier() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_owned(), "derives".to_owned(), "collection".to_owned()];
        let (_, info) = cache
            .list_traits(&path)
            .into_iter()
            .find(|(name, _)| name == "Iterator")
            .ok_or("missing SDK Iterator")?;
        let slot = info.methods.get("iter").ok_or("missing SDK iter")?;
        let declaration = cache
            .lookup_trait_decl(&path, "Iterator")
            .ok_or("missing Iterator source")?;
        assert!(
            transparent_interface_self_result(&declaration, "Iterator", &info.type_params, slot),
            "slot={slot:?}; parameters={:?}",
            info.type_params
        );
        let index = declaration
            .methods
            .iter()
            .position(|method| method.node.name == "iter")
            .ok_or("missing iter body")?;
        let mut foreign = slot.clone();
        foreign
            .identity
            .as_mut()
            .ok_or("missing identity")?
            .declaration_span
            .start += 1;
        assert!(!transparent_interface_self_result(
            &declaration,
            "Iterator",
            &info.type_params,
            &foreign
        ));
        let mut wrong_type = slot.clone();
        wrong_type.return_type =
            crate::symbols::ResolvedType::Generic("Iterator".into(), vec![crate::symbols::ResolvedType::Str]);
        assert!(!transparent_interface_self_result(
            &declaration,
            "Iterator",
            &info.type_params,
            &wrong_type
        ));
        let mut effects = declaration.clone();
        let returned = effects.methods[index]
            .node
            .body
            .as_ref()
            .and_then(|body| body.last())
            .ok_or("missing return")?
            .clone();
        effects.methods[index]
            .node
            .body
            .as_mut()
            .ok_or("missing body")?
            .push(returned);
        assert!(!transparent_interface_self_result(
            &effects,
            "Iterator",
            &info.type_params,
            slot
        ));
        let mut other_carrier = declaration.clone();
        let replacement = declaration
            .methods
            .iter()
            .find(|method| method.node.name == "take")
            .ok_or("missing take")?
            .node
            .body
            .clone();
        other_carrier.methods[index].node.body = replacement;
        assert!(!transparent_interface_self_result(
            &other_carrier,
            "Iterator",
            &info.type_params,
            slot
        ));
        Ok(())
    }
}
