//! Preserve the concrete carrier behind legacy's opaque trait results and supertrait returns.
//!
//! Only checked trait annotations, canonical callees, and a selected method's exact Self-result fact authorize
//! refinement. Different return carriers remain open and are refused; no boxing or vtable conversion is introduced.

use super::{CanonicalSymbolId, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir};
use std::collections::{BTreeMap, HashMap};

/// A retained callable's original source result and its current concrete representation witness.
#[derive(Clone)]
struct ReturnSignature {
    is_async: bool,
    original: IncanType,
    carrier: IncanType,
    parameters: Vec<String>,
    identities: BTreeMap<String, CanonicalSymbolId>,
    opaque: bool,
    hidden_inputs: Vec<(usize, usize)>,
}

/// Immutable declaration and checker facts for one refinement pass. Original local types constrain propagation;
/// retained signatures and owners may include authenticated dependency context.
struct ReturnContext<'facts> {
    nominals: &'facts [&'facts bir::NominalDeclaration],
    implementations: &'facts [&'facts bir::TraitImplementation],
    signatures: &'facts HashMap<CanonicalSymbolId, ReturnSignature>,
    facts: &'facts TypeCheckInfo,
    original: &'facts [IncanType],
}

/// Refine opaque frames and checked call results to one concrete witness, propagating through canonical calls.
/// The finite iteration bound follows the number of source bodies; unresolved cycles retain their named refusal.
/// Context records were checked at their source or authenticated when loaded. Their slot and implementation can
/// belong to different modules, so a module-local physical declaration check cannot validate that joined relation.
pub(super) fn retain_returns(
    module: &mut bir::BodyIrModule,
    program: &ast::Program,
    facts: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
) {
    let mut signatures = HashMap::new();
    for body in &mut module.bodies {
        retain_return_annotation(body, program, facts);
        if let Some(identity) = &body.canonical {
            signatures.insert(identity.clone(), signature(body));
        }
    }
    for body in context.iter().flat_map(|module| module.callable_declaration_frames()) {
        if let Some(identity) = &body.canonical {
            signatures.entry(identity.clone()).or_insert_with(|| signature(body));
        }
    }
    let original_locals = module
        .bodies
        .iter()
        .map(|body| body.locals.iter().map(|local| local.ty.clone()).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    // The checked slot can belong to another module or Loaf. Local-only implementation validation would discard
    // that selection before its published default can supply the return carrier; native admission validates the
    // complete project separately. No record or signature is reconstructed here.
    let implementations = module
        .trait_implementations
        .iter()
        .chain(context.iter().flat_map(|module| &module.trait_implementations))
        .collect::<Vec<_>>();
    let nominals = module
        .nominal_declarations
        .iter()
        .chain(context.iter().flat_map(|module| &module.nominal_declarations))
        .collect::<Vec<_>>();
    for _ in 0..=module.bodies.len() {
        let mut changed = false;
        for (body, original) in module.bodies.iter_mut().zip(&original_locals) {
            refine_block(
                &mut body.block.stmts,
                &mut body.locals,
                &mut body.named_type_identities,
                &ReturnContext {
                    nominals: &nominals,
                    implementations: &implementations,
                    signatures: &signatures,
                    facts,
                    original,
                },
            );
            let Some(identity) = &body.canonical else { continue };
            let Some(retained) = signatures.get_mut(identity) else {
                continue;
            };
            if !retained.opaque {
                continue;
            }
            if let Some(carrier) = return_witness(&body.block, &body.locals, &body.named_type_identities)
                && carrier != body.return_type
            {
                body.return_type = carrier.clone();
                retained.carrier = carrier;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Keep the original trait's exact annotation identity beside its executable witness, without changing checker facts.
fn retain_return_annotation(body: &mut bir::Body, program: &ast::Program, facts: &TypeCheckInfo) {
    let Some(annotation) = return_annotation_span(program, body) else {
        return;
    };
    let Some(identity) = facts
        .resolved_identity(annotation)
        .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
    else {
        return;
    };
    let key = body.span;
    let declared = facts
        .declarations
        .function_bindings_by_span
        .get(&key)
        .map(|binding| &binding.return_type)
        .or_else(|| {
            facts
                .declarations
                .method_bindings_by_span
                .get(&key)
                .and_then(|binding| {
                    (binding.identity.as_ref() == body.canonical.as_ref()).then_some(&binding.return_type)
                })
        });
    let Some(declared) = declared else {
        return;
    };
    let declared = super::parameter_types::checked_semantic_type(declared, facts);
    let Some(name) = type_base(&declared) else {
        return;
    };
    body.named_type_identities.insert(name.to_owned(), identity.clone());
}

/// Find the physical return annotation for one frame; nested method spans must match exactly within their owner.
fn return_annotation_span(program: &ast::Program, body: &bir::Body) -> Option<ast::Span> {
    program.declarations.iter().find_map(|declaration| {
        if let ast::Declaration::Function(function) = &declaration.node {
            return (declaration.span == body.span).then_some(function.return_type.span);
        }
        let methods = match &declaration.node {
            ast::Declaration::Model(owner) => &owner.methods,
            ast::Declaration::Class(owner) => &owner.methods,
            ast::Declaration::Trait(owner) => &owner.methods,
            ast::Declaration::Enum(owner) => &owner.methods,
            ast::Declaration::Newtype(owner) => &owner.methods,
            _ => return None,
        };
        methods
            .iter()
            .find_map(|method| (method.span == body.span).then_some(method.node.return_type.span))
    })
}

/// Snapshot the checked callable frame before refinement so only its declared result authorizes an opaque return.
fn signature(body: &bir::Body) -> ReturnSignature {
    ReturnSignature {
        is_async: body.is_async,
        original: body.return_type.clone(),
        carrier: body.return_type.clone(),
        parameters: body.type_parameters.clone(),
        identities: body.named_type_identities.clone(),
        opaque: trait_type(&body.return_type, &body.named_type_identities),
        hidden_inputs: body
            .params
            .iter()
            .enumerate()
            .filter_map(|(slot, parameter)| {
                let IncanType::TypeVar(name) = &parameter.ty else {
                    return None;
                };
                if !name.starts_with("__IncanTrait")
                    || !body
                        .named_type_identities
                        .get(name)
                        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
                {
                    return None;
                }
                Some((
                    slot,
                    body.type_parameters.iter().position(|parameter| parameter == name)?,
                ))
            })
            .collect(),
    }
}

/// Follow checked carriers through every same-frame block, leaving projected or unknown values open. Closure and
/// generator bodies own different local arenas and remain separate.
fn refine_block(
    statements: &mut [bir::Statement],
    locals: &mut [bir::LocalDecl],
    identities: &mut BTreeMap<String, CanonicalSymbolId>,
    context: &ReturnContext<'_>,
) {
    let ReturnContext {
        nominals,
        implementations,
        signatures,
        facts,
        original,
    } = context;
    for statement in statements {
        match &mut statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    refine_block(&mut arm.guard_stmts, locals, identities, context);
                    refine_block(&mut arm.body_stmts, locals, identities, context);
                }
            }
            bir::StatementKind::Assign { place, rvalue } if place.projection.is_empty() => {
                let Some(local) = place.local_id() else { continue };
                let retained_copy = match rvalue {
                    bir::Rvalue::Use(bir::Operand::Place(read)) if read.place.projection.is_empty() => {
                        read.place.local_id().is_some_and(|source| {
                            original[source.index()] == original[local.index()]
                                && locals[source.index()].ty != original[source.index()]
                        })
                    }
                    _ => false,
                };
                if !trait_type(&original[local.index()], identities) && !retained_copy {
                    continue;
                }
                let carrier = match rvalue {
                    bir::Rvalue::Use(operand) => operand_type(operand, locals),
                    bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _) => target
                        .canonical
                        .as_ref()
                        .and_then(|identity| {
                            nominals
                                .iter()
                                .find(|nominal| &nominal.canonical == identity && nominal.type_parameters.is_empty())
                        })
                        .map(|nominal| IncanType::Named(nominal.name.clone())),
                    _ => None,
                };
                if let Some(carrier) = carrier {
                    locals[local.index()].ty = carrier;
                }
            }
            bir::StatementKind::Call {
                destination: Some(place),
                callee,
                args,
                ..
            } if place.projection.is_empty() => {
                let Some(local) = place.local_id() else { continue };
                if let bir::Callee::Function(bir::CallableTarget::Named(target)) = callee {
                    refresh_hidden_arguments(target, args, locals, signatures);
                }
                let carrier = call_carrier(callee, args, statement.span, locals, signatures, facts);
                let may_refine = trait_type(&locals[local.index()].ty, identities)
                    || matches!(callee, bir::Callee::Method(target)
                        if locals[local.index()].origin == bir::LocalOrigin::Temporary
                            && target.canonical.as_ref().and_then(|id| signatures.get(id))
                                .is_some_and(|signature| signature.original == IncanType::SelfType))
                    || matches!(callee, bir::Callee::Function(bir::CallableTarget::Named(target))
                        if target.canonical.as_ref().and_then(|id| signatures.get(id))
                            .is_some_and(|signature| original[local.index()] == signature.original
                                || trait_type(&original[local.index()], &signature.identities)));
                if may_refine && let Some(carrier) = carrier {
                    locals[local.index()].ty = carrier;
                }
                if let bir::Callee::Method(target) = callee
                    && let Some(carrier) =
                        method_return_witness(target, args, locals, identities, nominals, implementations, signatures)
                    && locals[local.index()].origin == bir::LocalOrigin::Temporary
                {
                    locals[local.index()].ty = carrier;
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                refine_block(&mut then_block.stmts, locals, identities, context);
                if let Some(block) = else_block {
                    refine_block(&mut block.stmts, locals, identities, context);
                }
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    refine_block(&mut arm.body.stmts, locals, identities, context);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                refine_block(&mut body.stmts, locals, identities, context)
            }
            _ => {}
        }
    }
}

/// Instantiate a selected source method's executable return using its exact owner/adoption and checked arguments.
/// A published body already carries its refined return ABI, so the consumer must not require its former opaque
/// annotation to survive publication. Unknown receivers, conflicting slots and unresolved binders grant no witness.
fn method_return_witness(
    target: &bir::MethodTarget,
    args: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
    identities: &mut BTreeMap<String, CanonicalSymbolId>,
    nominals: &[&bir::NominalDeclaration],
    implementations: &[&bir::TraitImplementation],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
) -> Option<IncanType> {
    let selected = target.canonical.as_ref()?;
    if selected.kind != SemanticSourceTargetKind::Method {
        return None;
    }
    let bir::ArgumentElement::One(receiver) = args.first()? else {
        return None;
    };
    let receiver = operand_type(receiver, locals)?;
    let (name, owner_arguments) = match &receiver {
        IncanType::Named(name) => (name, &[][..]),
        IncanType::Generic { base, args } => (base, args.as_slice()),
        _ => return None,
    };
    let owner = retained_return_owner(name, identities, nominals)?;
    if owner.type_parameters.len() != owner_arguments.len() {
        return None;
    }
    let mut bindings = HashMap::new();
    for (parameter, argument) in owner.type_parameters.iter().zip(owner_arguments) {
        bindings.insert(parameter.clone(), argument.clone());
    }
    let mut adopted = implementations.iter().filter(|record| {
        record.owner == owner.canonical
            && &record.method == selected
            && target.trait_arguments.as_ref().is_none_or(|arguments| {
                record
                    .slot_arguments
                    .iter()
                    .map(|argument| instantiate_witness(argument, &bindings, &receiver))
                    .collect::<Option<Vec<_>>>()
                    .as_ref()
                    == Some(arguments)
            })
    });
    let signature = if let Some(record) = adopted.next() {
        if adopted.next().is_some() {
            return None;
        }
        let signature = signatures.get(&record.implementation)?;
        let trait_bindings = record
            .type_arguments
            .iter()
            .map(|argument| {
                Some((
                    argument.parameter.clone(),
                    instantiate_witness(&argument.argument, &bindings, &receiver)?,
                ))
            })
            .collect::<Option<HashMap<_, _>>>()?;
        let own_method = record.implementation.origin == owner.canonical.origin
            && owner.canonical.declaration_span.start <= record.implementation.declaration_span.start
            && record.implementation.declaration_span.end <= owner.canonical.declaration_span.end;
        if !own_method {
            bindings = trait_bindings;
        }
        signature
    } else {
        if selected.origin != owner.canonical.origin
            || selected.declaration_span.start < owner.canonical.declaration_span.start
            || selected.declaration_span.end > owner.canonical.declaration_span.end
        {
            return None;
        }
        signatures.get(selected)?
    };
    let method_parameters = signature
        .parameters
        .iter()
        .filter(|name| !bindings.contains_key(*name))
        .collect::<Vec<_>>();
    if method_parameters.len() != target.type_args.len() {
        return None;
    }
    for (parameter, argument) in method_parameters.into_iter().zip(&target.type_args) {
        bindings.insert(parameter.clone(), argument.clone());
    }
    let carrier = instantiate_witness(&signature.carrier, &bindings, &receiver)?;
    retain_witness_identities(&carrier, &signature.identities, identities)
        .then(|| async_call_carrier(signature.is_async, carrier))
}

/// A callable's logical return refines the future's output; it never replaces an async call's owned future storage.
fn async_call_carrier(is_async: bool, carrier: IncanType) -> IncanType {
    if is_async {
        IncanType::Generic {
            base: "Awaitable".into(),
            args: vec![carrier],
        }
    } else {
        carrier
    }
}

/// Carry only a selected executable return's named owners into the receiving frame, including generic heads.
/// Conflicting caller bindings grant no carrier; unrelated names from the producer's namespace are never imported.
fn retain_witness_identities(
    ty: &IncanType,
    source: &BTreeMap<String, CanonicalSymbolId>,
    destination: &mut BTreeMap<String, CanonicalSymbolId>,
) -> bool {
    let name = match ty {
        IncanType::Named(name) | IncanType::Generic { base: name, .. } => Some(name),
        _ => None,
    };
    if let Some((name, identity)) = name.and_then(|name| source.get(name).map(|identity| (name, identity))) {
        if destination.get(name).is_some_and(|previous| previous != identity) {
            return false;
        }
        destination.insert(name.clone(), identity.clone());
    }
    match ty {
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args
            .iter()
            .all(|argument| retain_witness_identities(argument, source, destination)),
        IncanType::Ref(inner) | IncanType::RefMut(inner) => retain_witness_identities(inner, source, destination),
        IncanType::Function { params, return_type } => {
            params
                .iter()
                .all(|parameter| retain_witness_identities(&parameter.ty, source, destination))
                && retain_witness_identities(return_type, source, destination)
        }
        _ => true,
    }
}

/// Resolve the exact checked nominal before canonical carrier bindings have been added to the body table.
/// An encoded carrier requires an existing checked owner binding; a conflicting direct binding grants no witness.
fn retained_return_owner<'module>(
    name: &str,
    identities: &BTreeMap<String, CanonicalSymbolId>,
    nominals: &[&'module bir::NominalDeclaration],
) -> Option<&'module bir::NominalDeclaration> {
    let direct = identities.get(name);
    nominals.iter().copied().find(|owner| {
        if direct == Some(&owner.canonical) {
            return true;
        }
        direct.is_none()
            && bir::nominal_carrier_key(&owner.canonical).as_deref() == Some(name)
            && identities.values().any(|identity| identity == &owner.canonical)
    })
}

/// Close a represented constructor template without guessing a binder or changing its canonical carrier base.
fn instantiate_witness(
    ty: &IncanType,
    bindings: &HashMap<String, IncanType>,
    receiver: &IncanType,
) -> Option<IncanType> {
    match ty {
        IncanType::TypeVar(name) => bindings.get(name).cloned(),
        IncanType::SelfType => Some(receiver.clone()),
        IncanType::Generic { base, args } => Some(IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .map(|ty| instantiate_witness(ty, bindings, receiver))
                .collect::<Option<_>>()?,
        }),
        IncanType::Tuple(elements) => Some(IncanType::Tuple(
            elements
                .iter()
                .map(|ty| instantiate_witness(ty, bindings, receiver))
                .collect::<Option<_>>()?,
        )),
        IncanType::Unknown | IncanType::Infer | IncanType::Unsupported(_) => None,
        ty => Some(ty.clone()),
    }
}

/// Refresh only hidden trait input bindings from their represented declaration slots after return refinement.
/// Explicit type arguments remain the checker's original selection; an unresolved or projected argument stays open.
fn refresh_hidden_arguments(
    target: &mut bir::NamedCallableTarget,
    args: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
) {
    let Some(signature) = target.canonical.as_ref().and_then(|identity| signatures.get(identity)) else {
        return;
    };
    if target.type_args.len() != signature.parameters.len() {
        return;
    }
    // Hidden carriers map by slot, so a declaration-order binding of a named call supplies them as well.
    let Some(arguments) = target.binding.slot_arguments() else {
        return;
    };
    for (slot, parameter) in &signature.hidden_inputs {
        let Some(position) = arguments.iter().position(|argument| argument.slot == *slot) else {
            continue;
        };
        let Some(bir::ArgumentElement::One(operand)) = args.get(position) else {
            continue;
        };
        if let Some(carrier) = operand_type(operand, locals) {
            target.type_args[*parameter] = carrier;
        }
    }
}

/// Select a call's representation from the exact body or the checker's exact Self-result declaration fact.
fn call_carrier(
    callee: &bir::Callee,
    args: &[bir::ArgumentElement],
    span: super::HirSourceSpan,
    locals: &[bir::LocalDecl],
    signatures: &HashMap<CanonicalSymbolId, ReturnSignature>,
    facts: &TypeCheckInfo,
) -> Option<IncanType> {
    match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => {
            let signature = signatures.get(target.canonical.as_ref()?)?;
            substitute(&signature.carrier, &signature.parameters, &target.type_args)
                .map(|carrier| async_call_carrier(signature.is_async, carrier))
        }
        bir::Callee::Method(target) => {
            let selected = target.canonical.as_ref()?;
            let declared_self = signatures
                .get(selected)
                .is_some_and(|signature| signature.original == IncanType::SelfType);
            if !declared_self && facts.calls.body_ir_self_result_methods.get(&span) != Some(selected) {
                return None;
            }
            let bir::ArgumentElement::One(receiver) = args.first()? else {
                return None;
            };
            operand_type(receiver, locals).map(|carrier| {
                async_call_carrier(
                    signatures.get(selected).is_some_and(|signature| signature.is_async),
                    carrier,
                )
            })
        }
        _ => None,
    }
}

/// Require every reachable retained return to agree on one non-trait carrier; mixed or unresolved results refuse.
fn return_witness(
    block: &bir::Block,
    locals: &[bir::LocalDecl],
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> Option<IncanType> {
    let mut returns = Vec::new();
    collect_returns(&block.stmts, &mut returns);
    let mut witness = None;
    for value in returns {
        let carrier = operand_type(value?, locals)?;
        if trait_type(&carrier, identities) || carrier == IncanType::Unknown {
            return None;
        }
        if witness.as_ref().is_some_and(|known| known != &carrier) {
            return None;
        }
        witness = Some(carrier);
    }
    witness
}

/// Collect value returns through same-frame control flow without treating loop breaks or closure/generator returns as
/// function returns.
fn collect_returns<'a>(statements: &'a [bir::Statement], out: &mut Vec<Option<&'a bir::Operand>>) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Return { value } => out.push(value.as_ref()),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_returns(&then_block.stmts, out);
                if let Some(block) = else_block {
                    collect_returns(&block.stmts, out);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                collect_returns(&body.stmts, out)
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    collect_returns(&arm.guard_stmts, out);
                    collect_returns(&arm.body_stmts, out);
                }
            }
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    collect_returns(&arm.body.stmts, out);
                }
            }
            _ => {}
        }
    }
}

/// Read a represented unprojected local carrier; field, index, and other projection contracts remain untouched.
fn operand_type(operand: &bir::Operand, locals: &[bir::LocalDecl]) -> Option<IncanType> {
    let bir::Operand::Place(read) = operand else {
        return None;
    };
    if !read.place.projection.is_empty() {
        return None;
    }
    locals.get(read.place.local_id()?.index()).map(|local| local.ty.clone())
}

/// Close only placeholders declared by the selected body, preserving nested generic and tuple carriers.
fn substitute(ty: &IncanType, parameters: &[String], arguments: &[IncanType]) -> Option<IncanType> {
    match ty {
        IncanType::TypeVar(name) => arguments
            .get(parameters.iter().position(|parameter| parameter == name)?)
            .cloned(),
        IncanType::Generic { base, args } => Some(IncanType::Generic {
            base: base.clone(),
            args: args
                .iter()
                .map(|ty| substitute(ty, parameters, arguments))
                .collect::<Option<_>>()?,
        }),
        IncanType::Tuple(elements) => Some(IncanType::Tuple(
            elements
                .iter()
                .map(|ty| substitute(ty, parameters, arguments))
                .collect::<Option<_>>()?,
        )),
        ty => Some(ty.clone()),
    }
}

/// Whether a named carrier position has a checked trait identity, rather than an ordinary nominal layout.
fn trait_type(ty: &IncanType, identities: &BTreeMap<String, CanonicalSymbolId>) -> bool {
    type_base(ty)
        .and_then(|name| identities.get(name))
        .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Trait)
}

/// The checked named or generic base whose binding supplies one canonical trait identity.
fn type_base(ty: &IncanType) -> Option<&str> {
    match ty {
        IncanType::Named(name) => Some(name),
        IncanType::Generic { base, .. } => Some(base),
        _ => None,
    }
}

#[cfg(test)]
mod carrier_identity_tests {
    use super::*;

    /// Canonical spelling alone cannot grant a witness, and a competing checked binding refuses it.
    #[test]
    fn return_owner_requires_checked_binding_without_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex("model Value:\n    item: int\n").map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let path = vec!["main".to_owned()];
        checker.set_current_module_path(Some(path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let providers = crate::body_ir::ProviderOperationCatalog::new();
        let module = crate::body_ir::build_body_ir_module(
            &program,
            &path,
            checker.type_info(),
            &crate::body_ir::BodyIrContext {
                modules: &[],
                provider_operations: &providers,
                registry_package: None,
                library_package: None,
            },
        );
        let owner = module.nominal_declarations.first().ok_or("missing source owner")?;
        let nominals = module.nominal_declarations.iter().collect::<Vec<_>>();
        let key = bir::nominal_carrier_key(&owner.canonical).ok_or("missing carrier key")?;
        let mut identities = BTreeMap::new();
        assert!(retained_return_owner(&key, &identities, &nominals).is_none());
        identities.insert(owner.name.clone(), owner.canonical.clone());
        assert_eq!(retained_return_owner(&key, &identities, &nominals), Some(owner));
        let mut foreign = owner.canonical.clone();
        foreign.origin = incan_semantics_core::SymbolOrigin::Module(vec!["foreign".to_owned()]);
        identities.insert(key.clone(), foreign);
        assert!(retained_return_owner(&key, &identities, &nominals).is_none());
        Ok(())
    }
}
