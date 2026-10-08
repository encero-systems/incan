//! Checked admission for canonical global list enumeration and Zip construction.
//!
//! The owning Body local table supplies element and result types, including for empty lists and deferred frames.
//! Canonical Enumerate and Zip destinations retain exact polling item contracts through bare aliases and explicit
//! closure/generator captures. Optional defaults and presets do not establish provenance for overrideable parameters.
//! A nominal iterator spelling alone never grants execution: the non-list polling exception requires Zip provenance.

use std::collections::{BTreeMap, BTreeSet};

use incan_core::lang::traits::{self, TraitId};

use super::{
    ArgumentElement, Body, CallableParam, CallableParamDefault, CallableTarget, Callee, HirSourceSpan, IncanType,
    IterProtocol, LocalId, Operand, Place, PlaceElem, ReplacementExecutionError, Rvalue, Statement, StatementKind,
    bare_local, collections, declared_local_type, explicit_builtin, fixed_operands, is_direct_structural_type,
    is_int_type, unsupported,
};
use incan_core::lang::{builtins::BuiltinFnId, types::collections::CollectionTypeId};

/// Validate canonical list calls and polling throughout the owning local-id space, retaining checked Zip provenance.
pub(super) fn validate_body(body: &Body) -> Result<BTreeSet<LocalId>, ReplacementExecutionError> {
    let mut iterator_locals = BTreeMap::new();
    visit_body(body, &mut |statement| {
        if let StatementKind::Call {
            destination,
            callee: Callee::Function(CallableTarget::Named(target)),
            args,
            ..
        } = &statement.kind
            && let Some(builtin @ (BuiltinFnId::Enumerate | BuiltinFnId::Zip)) = explicit_builtin(target)
        {
            let destination = validate_call(body, builtin, destination.as_ref(), args, statement.span)?;
            iterator_locals.insert(destination, builtin);
        }
        Ok(())
    })?;

    loop {
        let mut changed = false;
        visit_body(body, &mut |statement| {
            if let StatementKind::Assign { place, rvalue } = &statement.kind {
                match rvalue {
                    Rvalue::Use(source) if place.projection.is_empty() => {
                        changed |= retain_alias(body, &mut iterator_locals, place.local, source, statement.span)?;
                    }
                    Rvalue::Closure {
                        captured_operands,
                        body: closure,
                        ..
                    } => {
                        if captured_operands.len() != closure.capture_locals.len() {
                            return Err(unsupported("callable capture metadata mismatch", statement.span));
                        }
                        for (source, destination) in captured_operands.iter().zip(&closure.capture_locals) {
                            changed |= retain_alias(body, &mut iterator_locals, *destination, source, statement.span)?;
                        }
                    }
                    Rvalue::Generator {
                        source,
                        captured_operands,
                        body: generator,
                    } => {
                        if captured_operands.len() != generator.capture_locals.len() {
                            return Err(unsupported("generator capture metadata mismatch", statement.span));
                        }
                        changed |= retain_alias(
                            body,
                            &mut iterator_locals,
                            generator.source_local,
                            source,
                            statement.span,
                        )?;
                        for (source, destination) in captured_operands.iter().zip(&generator.capture_locals) {
                            changed |= retain_alias(body, &mut iterator_locals, *destination, source, statement.span)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        })?;
        if !changed {
            break;
        }
    }

    visit_body(body, &mut |statement| {
        if let StatementKind::IterNext {
            destination,
            iterator: Operand::Place(iterator),
            protocol: IterProtocol::Builtin,
        } = &statement.kind
            && iterator.place.projection.is_empty()
            && let Some(builtin) = iterator_locals.get(&iterator.place.local)
        {
            let iterator_type = declared_local_type(body, iterator.place.local, statement.span)?;
            let destination_type = declared_local_type(body, bare_local(destination, statement.span)?, statement.span)?;
            let item_type = match builtin {
                BuiltinFnId::Enumerate => list_element_type(iterator_type),
                BuiltinFnId::Zip => zip_item_type(iterator_type),
                _ => None,
            };
            if item_type != Some(destination_type) {
                return Err(unsupported(
                    "enumerate/Zip polling destination disagrees with its checked pair type",
                    statement.span,
                ));
            }
        }
        Ok(())
    })?;
    Ok(iterator_locals
        .into_iter()
        .filter_map(|(local, builtin)| (builtin == BuiltinFnId::Zip).then_some(local))
        .collect())
}

/// Follow a compiler-recorded value transfer without deriving iterator identity from its nominal type.
fn retain_alias(
    body: &Body,
    iterator_locals: &mut BTreeMap<LocalId, BuiltinFnId>,
    destination: LocalId,
    source: &Operand,
    span: HirSourceSpan,
) -> Result<bool, ReplacementExecutionError> {
    let Operand::Place(source) = source else {
        return Ok(false);
    };
    if !source.place.projection.is_empty() {
        return Ok(false);
    }
    let Some(builtin) = iterator_locals.get(&source.place.local).copied() else {
        return Ok(false);
    };
    let source_type = declared_local_type(body, source.place.local, span)?;
    let destination_type = declared_local_type(body, destination, span)?;
    if source_type != destination_type {
        return Err(unsupported(
            "enumerate/Zip alias changes its checked iterator type",
            span,
        ));
    }
    Ok(iterator_locals.insert(destination, builtin) != Some(builtin))
}

/// Require fixed arity, checked structural-list inputs, and the exact compiler-selected result shape.
fn validate_call(
    body: &Body,
    builtin: BuiltinFnId,
    destination: Option<&Place>,
    args: &[ArgumentElement],
    span: HirSourceSpan,
) -> Result<LocalId, ReplacementExecutionError> {
    let args = fixed_operands(args).ok_or_else(|| unsupported("enumerate/Zip with a spread argument", span))?;
    let destination = bare_local(
        destination.ok_or_else(|| unsupported("discarded enumerate/Zip result", span))?,
        span,
    )?;
    let destination_type = declared_local_type(body, destination, span)?;
    let valid = match (builtin, args.as_slice()) {
        (BuiltinFnId::Enumerate, [source]) => {
            let element = list_operand_element(body, source, span)?;
            matches!(list_element_type(destination_type), Some(IncanType::Tuple(pair))
                if matches!(pair.as_slice(), [index, item] if is_int_type(index) && item == element))
        }
        (BuiltinFnId::Zip, [left, right]) => {
            let left = list_operand_element(body, left, span)?;
            let right = list_operand_element(body, right, span)?;
            matches!(zip_item_type(destination_type), Some(IncanType::Tuple(pair))
                if matches!(pair.as_slice(), [left_item, right_item] if left_item == left && right_item == right))
        }
        _ => return Err(unsupported("enumerate/Zip call arity", span)),
    };
    if !valid {
        return Err(unsupported(
            "enumerate/Zip result disagrees with its checked list element types",
            span,
        ));
    }
    Ok(destination)
}

/// Resolve the supported structural place projection without inferring types from runtime list contents.
fn list_operand_element<'a>(
    body: &'a Body,
    operand: &Operand,
    span: HirSourceSpan,
) -> Result<&'a IncanType, ReplacementExecutionError> {
    let Operand::Place(operand) = operand else {
        return Err(unsupported(
            "enumerate/Zip requires a checked structural list operand",
            span,
        ));
    };
    let local_type = declared_local_type(body, operand.place.local, span)?;
    let operand_type = match operand.place.projection.as_slice() {
        [] => Some(local_type),
        [PlaceElem::Index(_)] => list_element_type(local_type),
        [PlaceElem::Field(field)] => match local_type {
            IncanType::Tuple(elements) => field.parse::<usize>().ok().and_then(|index| elements.get(index)),
            _ => None,
        },
        _ => None,
    };
    operand_type
        .and_then(list_element_type)
        .filter(|element| is_direct_structural_type(element))
        .ok_or_else(|| unsupported("enumerate/Zip requires a checked structural list operand", span))
}

/// Extract exactly one list element type through the canonical collection registry.
fn list_element_type(ty: &IncanType) -> Option<&IncanType> {
    match ty {
        IncanType::Generic { base, args } if collections::from_str(base) == Some(CollectionTypeId::List) => {
            match args.as_slice() {
                [element] => Some(element),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Check a canonical Zip result's iterator vocabulary and recursively structural pair payload.
fn zip_item_type(ty: &IncanType) -> Option<&IncanType> {
    match ty {
        IncanType::Generic { base, args } if base == traits::as_str(TraitId::Iterator) => match args.as_slice() {
            [item @ IncanType::Tuple(pair)] if pair.len() == 2 && pair.iter().all(is_direct_structural_type) => {
                Some(item)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Visit ordinary statements and source defaults in the same compiler-owned Body local-id space.
fn visit_body(
    body: &Body,
    visit: &mut impl FnMut(&Statement) -> Result<(), ReplacementExecutionError>,
) -> Result<(), ReplacementExecutionError> {
    visit_defaults(&body.params, visit)?;
    visit_statements(&body.block.stmts, visit)
}

/// Include default computations so they cannot bypass checked empty-list admission.
fn visit_defaults(
    params: &[CallableParam],
    visit: &mut impl FnMut(&Statement) -> Result<(), ReplacementExecutionError>,
) -> Result<(), ReplacementExecutionError> {
    for param in params {
        if let CallableParamDefault::Source(computation) = &param.default {
            visit_statements(&computation.stmts, visit)?;
        }
    }
    Ok(())
}

/// Traverse normalized control flow and deferred frames without interpreting or executing them.
fn visit_statements(
    statements: &[Statement],
    visit: &mut impl FnMut(&Statement) -> Result<(), ReplacementExecutionError>,
) -> Result<(), ReplacementExecutionError> {
    for statement in statements {
        visit(statement)?;
        match &statement.kind {
            StatementKind::If {
                then_block, else_block, ..
            } => {
                visit_statements(&then_block.stmts, visit)?;
                if let Some(else_block) = else_block {
                    visit_statements(&else_block.stmts, visit)?;
                }
            }
            StatementKind::Loop { body } => visit_statements(&body.stmts, visit)?,
            StatementKind::Race { arms, .. } => {
                for arm in arms {
                    visit_statements(&arm.body.stmts, visit)?;
                }
            }
            StatementKind::Assign { rvalue, .. } => match rvalue {
                Rvalue::Closure { params, body, .. } => {
                    visit_defaults(params, visit)?;
                    visit_statements(&body.stmts, visit)?;
                }
                Rvalue::Generator { body, .. } => visit_statements(&body.stmts, visit)?,
                Rvalue::Match { arms, .. } => {
                    for arm in arms {
                        visit_statements(&arm.guard_stmts, visit)?;
                        visit_statements(&arm.body_stmts, visit)?;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}
