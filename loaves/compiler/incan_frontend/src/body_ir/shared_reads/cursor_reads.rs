//! Shared views of native owners: a field binding or traversal cursor that every use only reads.

use super::*;
use crate::body_ir::SemanticSourceTargetKind;
use incan_semantics_core::SymbolOrigin;

/// Only native owners with retained declaration identity participate; source models and open types keep their ABI.
fn native_owner(ty: &IncanType, identities: &std::collections::BTreeMap<String, CanonicalSymbolId>) -> bool {
    let (name, checked_path) = match ty {
        IncanType::Named(name) => (name, false),
        IncanType::Generic { base: name, .. } | IncanType::RustInteropPath(name) => (name, true),
        _ => return false,
    };
    let declaration_name = name.rsplit("::").next().unwrap_or(name);
    identities
        .get(name)
        .or_else(|| {
            identities
                .values()
                .find(|identity| identity.declaration_name == declaration_name)
        })
        .is_some_and(|identity| {
            identity.kind == SemanticSourceTargetKind::RustItem && matches!(identity.origin, SymbolOrigin::RustCrate(_))
        })
        || checked_path && bir::rust_item_identity(name).is_some()
}

/// Borrow a shared native field binding or traversal cursor when every use stays read-only. A field binding starts
/// from an explicit projected loan; a cursor's later assignments must already return shared views of its owner.
pub(super) fn retain_cursors(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    for index in 0..body.locals.len() {
        let local = body.locals[index].id;
        let owner = body.locals[index].ty.clone();
        if !matches!(body.locals[index].origin, bir::LocalOrigin::UserBinding)
            || !native_owner(&owner, &body.named_type_identities)
        {
            continue;
        }
        let mut writes = Vec::new();
        walk(&body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign { place, rvalue } = &statement.kind
                && place.local_id() == Some(local)
            {
                writes.push((place.clone(), rvalue.clone()));
            }
        });
        let Some((_, bir::Rvalue::Use(bir::Operand::Place(initial)))) = writes.first() else {
            continue;
        };
        let source = initial.place.clone();
        let reference = IncanType::Ref(Box::new(owner.clone()));
        let field_view = writes.len() == 1
            && initial.fact == bir::OwnershipFact::Borrow
            && !source.projection.is_empty()
            && source
                .projection
                .iter()
                .all(|step| matches!(step, bir::PlaceElem::Field { .. }))
            && source
                .local_id()
                .and_then(|root| body.locals.get(root.index()))
                .is_some_and(|root| !matches!(root.origin, bir::LocalOrigin::Temporary));
        if (!field_view && writes.len() < 2) || writes.iter().any(|(place, _)| !place.projection.is_empty())
            || writes.iter().skip(1).any(|(_, value)| !matches!(value,
                bir::Rvalue::Use(bir::Operand::Place(read)) if whole_type(&read.place, &body.locals) == Some(&reference)))
        { continue; }
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local,
            cursor: true,
            derived: false,
        };
        if !readonly(&body.block.stmts, &context) || source_changes(&body.block.stmts, &source, local) {
            continue;
        }
        body.locals[index].ty = reference;
        walk_mut(&mut body.block.stmts, &mut |statement| {
            if let bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } = &mut statement.kind
                && place.local_id() == Some(local)
            {
                read.fact = bir::OwnershipFact::Borrow;
                read.last_use = false;
            }
        });
    }
}

/// Refuse a view when the frame replaces or exclusively lends its original owner. Initializing the cursor itself
/// is exempt; the checker already admitted that original value read.
fn source_changes(statements: &[bir::Statement], source: &bir::Place, cursor: bir::LocalId) -> bool {
    let mut changed = false;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign { place, .. } if place.root == source.root && place.local_id() != Some(cursor) => {
            changed = true
        }
        bir::StatementKind::Call { args, .. } => {
            changed |= args.iter().filter_map(bir::ArgumentElement::as_one).any(|argument| matches!(argument,
                bir::Operand::Place(read) if read.place.root == source.root && read.fact == bir::OwnershipFact::MutBorrow));
        }
        _ => {}
    });
    changed
}
