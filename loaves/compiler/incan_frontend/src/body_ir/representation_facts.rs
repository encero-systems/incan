//! Body IR-only representation facts derived from checked calls and terminating type-test branches.
//!
//! These refinements never update checker state or the legacy lowering bridge. A callable's checked canonical hook
//! proves an environment-capable parameter; a returning type-test branch removes its exact checked member from the
//! remaining path. Assignments and unproven effects invalidate residual knowledge.

use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, SymbolOrigin};
use std::collections::HashMap;

/// Retain checked callable parameter storage and residual type-test facts on one direct-route body.
pub(super) fn retain(body: &mut bir::Body) {
    let mut held = Vec::new();
    collect_callable_parameters(&body.block, &body.locals, &body.params, &mut held);
    if let Some(facts) = &mut body.callable_representation {
        for local in held {
            if !facts.closure_holding_parameters.contains(&local) {
                facts.closure_holding_parameters.push(local);
            }
        }
    }
    retain_residual_types(&mut body.block.stmts);
}

/// Select generic parameter carriers only from a checker-resolved standard callable hook, including nested branches.
fn collect_callable_parameters(
    block: &bir::Block,
    locals: &[bir::LocalDecl],
    params: &[bir::CallableParam],
    held: &mut Vec<bir::LocalId>,
) {
    for statement in &block.stmts {
        match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if is_callable_hook(target) => {
                let Some(bir::Operand::Place(read)) = args.first().and_then(bir::ArgumentElement::as_one) else {
                    continue;
                };
                let Some(local) = read.place.local_id().and_then(|id| locals.get(id.index())) else {
                    continue;
                };
                if !matches!(local.ty, IncanType::TypeVar(_)) {
                    continue;
                }
                for param in params {
                    if param.ty == local.ty && !held.contains(&param.local) {
                        held.push(param.local);
                    }
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_callable_parameters(then_block, locals, params, held);
                if let Some(block) = else_block {
                    collect_callable_parameters(block, locals, params, held);
                }
            }
            bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
                collect_callable_parameters(body, locals, params, held);
            }
            _ => {}
        }
    }
}

/// Recognize the canonical standard callable method; same-spelled application methods carry no representation grant.
fn is_callable_hook(target: &bir::MethodTarget) -> bool {
    let Some(identity) = &target.canonical else {
        return false;
    };
    if identity.kind != SemanticSourceTargetKind::Method || identity.declaration_name != "__call__" {
        return false;
    }
    match &identity.origin {
        SymbolOrigin::Module(path) => path.iter().map(String::as_str).eq(["std", "traits", "callable"]),
        SymbolOrigin::Package { library, module_path } => {
            library == "incan_stdlib_core" && module_path.iter().map(String::as_str).eq(["traits", "callable"])
        }
        _ => false,
    }
}

/// Refine only a straight-line path after a returning checked type test; effects and writes erase prior refinements.
fn retain_residual_types(statements: &mut [bir::Statement]) {
    let mut residuals: HashMap<bir::LocalId, IncanType> = HashMap::new();
    let mut tests: HashMap<bir::LocalId, (bir::LocalId, IncanType, IncanType)> = HashMap::new();
    for statement in statements {
        match &mut statement.kind {
            bir::StatementKind::Assign { place, rvalue } => {
                if !matches!(rvalue, bir::Rvalue::IsInstance { .. }) {
                    residuals.clear();
                    tests.clear();
                    continue;
                }
                if let Some(local) = place.local_id() {
                    residuals.remove(&local);
                    tests.retain(|condition, (source, _, _)| *condition != local && *source != local);
                }
                if let bir::Rvalue::IsInstance {
                    value: bir::Operand::Place(read),
                    value_ty,
                    target,
                } = rvalue
                {
                    if !read.place.projection.is_empty() || !place.projection.is_empty() {
                        continue;
                    }
                    if let (Some(source), Some(output)) = (read.place.local_id(), place.local_id()) {
                        if let Some(residual) = residuals.get(&source) {
                            *value_ty = residual.clone();
                        }
                        tests.insert(output, (source, value_ty.clone(), target.ty.clone()));
                    }
                }
            }
            bir::StatementKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let returning = statements_return(&then_block.stmts);
                retain_residual_types(&mut then_block.stmts);
                if let Some(block) = else_block {
                    retain_residual_types(&mut block.stmts);
                }
                let selected = whole_local(cond).and_then(|local| tests.get(&local));
                if returning && else_block.is_none() {
                    if let Some((source, before, target)) = selected
                        && let Some(after) = exclude_member(before, target)
                    {
                        residuals.insert(*source, after);
                    }
                } else {
                    residuals.clear();
                    tests.clear();
                }
            }
            bir::StatementKind::Drop { .. } | bir::StatementKind::Expr { .. } | bir::StatementKind::Return { .. } => {}
            _ => {
                residuals.clear();
                tests.clear();
            }
        }
    }
}

/// Read only an unprojected local condition; globals and fields cannot inherit another local's path fact.
fn whole_local(operand: &bir::Operand) -> Option<bir::LocalId> {
    match operand {
        bir::Operand::Place(read) if read.place.projection.is_empty() => read.place.local_id(),
        _ => None,
    }
}

/// Remove one exact checked union member, retaining an ordinary member type when only one alternative remains.
fn exclude_member(before: &IncanType, target: &IncanType) -> Option<IncanType> {
    let IncanType::Generic { base, args } = before else {
        return None;
    };
    if base != "Union" || !args.contains(target) {
        return None;
    }
    let remaining: Vec<_> = args.iter().filter(|member| *member != target).cloned().collect();
    match remaining.as_slice() {
        [member] => Some(member.clone()),
        [] => None,
        _ => Some(IncanType::Generic {
            base: base.clone(),
            args: remaining,
        }),
    }
}

/// Prove that normal execution returns from this frame before reaching the end of the statements.
///
/// This is shared by residual type refinement and implicit-tail lowering: an exhaustive returning match must not
/// acquire an unreachable return of its unit placeholder. Only an unguarded wildcard or binding proves match
/// coverage here; other pattern families retain their existing conservative behavior. Loops and separate callable
/// frames do not prove a return, and non-returning transfers stop the proof.
pub(super) fn statements_return(statements: &[bir::Statement]) -> bool {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Return { .. } => return true,
            bir::StatementKind::If {
                then_block,
                else_block: Some(other),
                ..
            } if statements_return(&then_block.stmts) && statements_return(&other.stmts) => return true,
            bir::StatementKind::UnsafeRegion { body, .. } if statements_return(&body.stmts) => return true,
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } if arms.iter().any(|arm| {
                arm.guard.is_none() && matches!(arm.pattern, bir::Pattern::Wildcard | bir::Pattern::Var(_))
            }) && arms.iter().all(|arm| statements_return(&arm.body_stmts)) =>
            {
                return true;
            }
            bir::StatementKind::Break { .. }
            | bir::StatementKind::Continue
            | bir::StatementKind::Unsupported { .. } => return false,
            _ => {}
        }
    }
    false
}
