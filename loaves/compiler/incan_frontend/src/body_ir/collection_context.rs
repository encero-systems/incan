//! Retain empty collection storage from checked builtin writes without changing legacy inference.

use incan_lang::lang::surface::{dict_methods, set_methods};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{IncanPrimitiveType, IncanType, SymbolOrigin, body_ir as bir};

use super::BodyBuilder;

/// Checked writes, whole-value storage links and compiler-owned empty construction in one frame.
#[derive(Default)]
struct ContextFacts {
    seeds: Vec<usize>,
    edges: Vec<(usize, usize)>,
    writes: Vec<(usize, IncanType)>,
}

impl BodyBuilder<'_, '_> {
    /// Close only a consistently constrained empty collection and its whole-value aliases in this frame.
    ///
    /// Canonical builtin write identities and checked operand types supply the constraints. Separate lexical locals
    /// remain separate even when their names match. Missing, conflicting or undeclared leaves stay unresolved;
    /// nested callable frames retain their own facts rather than lending local IDs to this frame.
    pub(super) fn retain_empty_collection_context(&mut self, statements: &[bir::Statement]) {
        let mut facts = ContextFacts::default();
        collect_context(statements, &self.locals, &mut facts);
        let mut visited = vec![false; self.locals.len()];
        for seed in &facts.seeds {
            if visited.get(*seed).copied().unwrap_or(true) {
                continue;
            }
            let group = linked_locals(*seed, &facts.edges);
            for local in &group {
                if let Some(visited) = visited.get_mut(*local) {
                    *visited = true;
                }
            }
            let Some(ty) = group_type(&group, &self.locals, &facts.writes) else {
                continue;
            };
            if !closed_type(&ty, &self.type_parameters) {
                continue;
            }
            for local in group {
                if let Some(declaration) = self.locals.get_mut(local) {
                    declaration.ty = ty.clone();
                }
            }
        }
    }
}

/// Visit source control flow in one frame; a closure or generator owns a different local table.
fn collect_context(statements: &[bir::Statement], locals: &[bir::LocalDecl], facts: &mut ContextFacts) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Assign { place, rvalue } => {
                collect_assignment(place, rvalue, locals, facts);
                if let bir::Rvalue::Match { arms, .. } = rvalue {
                    for arm in arms {
                        collect_context(&arm.guard_stmts, locals, facts);
                        collect_context(&arm.body_stmts, locals, facts);
                    }
                }
            }
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } => {
                if let Some(write) = checked_write(target, args, locals) {
                    facts.writes.push(write);
                }
            }
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                collect_context(&then_block.stmts, locals, facts);
                if let Some(block) = else_block {
                    collect_context(&block.stmts, locals, facts);
                }
            }
            bir::StatementKind::Loop { body } => collect_context(&body.stmts, locals, facts),
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms {
                    collect_context(&arm.body.stmts, locals, facts);
                }
            }
            bir::StatementKind::Call { .. }
            | bir::StatementKind::Drop { .. }
            | bir::StatementKind::Break { .. }
            | bir::StatementKind::Continue
            | bir::StatementKind::Return { .. }
            | bir::StatementKind::Await { .. }
            | bir::StatementKind::Yield { .. }
            | bir::StatementKind::Assert { .. }
            | bir::StatementKind::Expr { .. }
            | bir::StatementKind::TryPropagate { .. }
            | bir::StatementKind::IterNext { .. }
            | bir::StatementKind::Unsupported { .. } => {}
        }
    }
}

/// Record empty aggregate provenance and whole-value links, without merging element or field storage.
fn collect_assignment(place: &bir::Place, value: &bir::Rvalue, locals: &[bir::LocalDecl], facts: &mut ContextFacts) {
    let Some(destination) = whole_local(place) else {
        return;
    };
    let Some(declaration) = locals.get(destination) else {
        return;
    };
    let Some((kind, _)) = collection_shape(&declaration.ty) else {
        return;
    };
    let empty = match value {
        bir::Rvalue::Aggregate(bir::AggregateKind::Set, values) => kind == CollectionTypeId::Set && values.is_empty(),
        bir::Rvalue::Dict(values) => kind == CollectionTypeId::Dict && values.is_empty(),
        _ => false,
    };
    if empty {
        facts.seeds.push(destination);
    }
    if let bir::Rvalue::Use(bir::Operand::Place(read)) = value
        && let Some(source) = whole_local(&read.place)
        && locals
            .get(source)
            .and_then(|local| collection_shape(&local.ty))
            .is_some_and(|(source_kind, _)| source_kind == kind)
    {
        facts.edges.push((destination, source));
    }
}

/// Retain argument storage only for exact checker-selected collection mutation contracts.
fn checked_write(
    target: &bir::MethodTarget,
    arguments: &[bir::ArgumentElement],
    locals: &[bir::LocalDecl],
) -> Option<(usize, IncanType)> {
    let identity = target.canonical.as_ref()?;
    if identity.origin != SymbolOrigin::Builtin {
        return None;
    }
    let (owner, member) = identity.declaration_name.split_once('.')?;
    let bir::ArgumentElement::One(bir::Operand::Place(receiver)) = arguments.first()? else {
        return None;
    };
    let receiver = whole_local(&receiver.place)?;
    let (kind, members) = collection_shape(&locals.get(receiver)?.ty)?;
    let expected = collections::from_str(owner)?;
    if kind != expected {
        return None;
    }
    let slots = match kind {
        CollectionTypeId::Set if set_methods::from_str(member) == Some(set_methods::SetMethodId::Add) => 1,
        CollectionTypeId::Dict if dict_methods::from_str(member) == Some(dict_methods::DictMethodId::Insert) => 2,
        _ => return None,
    };
    if members.len() != slots || arguments.len() != slots + 1 {
        return None;
    }
    let types: Option<Vec<_>> = arguments[1..]
        .iter()
        .map(|argument| {
            let bir::ArgumentElement::One(operand) = argument else {
                return None;
            };
            operand_type(operand, locals)
        })
        .collect();
    Some((
        receiver,
        IncanType::Generic {
            base: owner.to_string(),
            args: types?,
        },
    ))
}

/// Read a checked whole-local type or an unambiguous literal category; payload-free None gives no element type.
fn operand_type(operand: &bir::Operand, locals: &[bir::LocalDecl]) -> Option<IncanType> {
    match operand {
        bir::Operand::Place(read) => locals.get(whole_local(&read.place)?).map(|local| local.ty.clone()),
        bir::Operand::Constant(value) => {
            let primitive = match value {
                bir::Constant::Int(_) => IncanPrimitiveType::Int,
                bir::Constant::Float(_) => IncanPrimitiveType::Float,
                bir::Constant::Bool(_) => IncanPrimitiveType::Bool,
                bir::Constant::Str(_) => IncanPrimitiveType::Str,
                bir::Constant::Bytes(_) => IncanPrimitiveType::Bytes,
                bir::Constant::Unit => IncanPrimitiveType::Unit,
                bir::Constant::TypedNumeric(_) | bir::Constant::FrozenStr(_) | bir::Constant::None => return None,
            };
            Some(IncanType::Primitive(primitive))
        }
    }
}

/// Restrict this storage relation to complete local values, excluding global and projected places.
fn whole_local(place: &bir::Place) -> Option<usize> {
    if !place.projection.is_empty() {
        return None;
    }
    place.local_id().map(|local| local.index())
}

/// Recognize only compiler collection shapes whose arity is part of the existing collection registry.
fn collection_shape(ty: &IncanType) -> Option<(CollectionTypeId, &[IncanType])> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    let kind = collections::from_str(base)?;
    match (kind, args.len()) {
        (CollectionTypeId::Set, 1) | (CollectionTypeId::Dict, 2) => Some((kind, args)),
        _ => None,
    }
}

/// Find all aliases connected to one admitted empty construction without consulting source names.
fn linked_locals(seed: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut group = vec![seed];
    let mut next = 0;
    while next < group.len() {
        let local = group[next];
        for &(left, right) in edges {
            let other = if left == local {
                Some(right)
            } else if right == local {
                Some(left)
            } else {
                None
            };
            if let Some(other) = other
                && !group.contains(&other)
            {
                group.push(other);
            }
        }
        next += 1;
    }
    group
}

/// Combine independently checked storage and write constraints; any contradiction keeps the entire group open.
fn group_type(group: &[usize], locals: &[bir::LocalDecl], writes: &[(usize, IncanType)]) -> Option<IncanType> {
    let mut ty = locals.get(*group.first()?)?.ty.clone();
    for local in group {
        ty = merge_types(&ty, &locals.get(*local)?.ty)?;
    }
    for (local, written) in writes {
        if group.contains(local) {
            ty = merge_types(&ty, written)?;
        }
    }
    Some(ty)
}

/// Fill a missing checked leaf only from an equal, consistent structural constraint; never widen or choose a union.
fn merge_types(left: &IncanType, right: &IncanType) -> Option<IncanType> {
    if left == right {
        return Some(left.clone());
    }
    if matches!(left, IncanType::Unknown | IncanType::Infer) {
        return Some(right.clone());
    }
    if matches!(right, IncanType::Unknown | IncanType::Infer) {
        return Some(left.clone());
    }
    match (left, right) {
        (
            IncanType::Generic {
                base: left_base,
                args: left_args,
            },
            IncanType::Generic {
                base: right_base,
                args: right_args,
            },
        ) if left_base == right_base && left_args.len() == right_args.len() => {
            let args: Option<Vec<_>> = left_args
                .iter()
                .zip(right_args)
                .map(|(left, right)| merge_types(left, right))
                .collect();
            Some(IncanType::Generic {
                base: left_base.clone(),
                args: args?,
            })
        }
        _ => None,
    }
}

/// A retained frame type may contain only known leaves and this declaration's own type parameters.
fn closed_type(ty: &IncanType, parameters: &[String]) -> bool {
    match ty {
        IncanType::Unknown | IncanType::Infer => false,
        IncanType::TypeVar(name) => parameters.contains(name),
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(|ty| closed_type(ty, parameters)),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => {
            closed_type(inner, parameters)
        }
        IncanType::Function { params, return_type } => {
            params.iter().all(|parameter| closed_type(&parameter.ty, parameters))
                && closed_type(return_type, parameters)
        }
        _ => true,
    }
}
