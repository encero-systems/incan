//! Preserve generic reference binders before specialization erases their relationship in concrete frame types.

use std::collections::BTreeMap;

use incan_mir_plan_native::{GenericLoan, PlanType};
use incan_semantics_core::body_ir::Body;
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, IncanType};

use crate::type_parameters::source_nominal_owner;

/// 1. Retain parameter occurrences. 2. Retain result occurrences under the same source type identities.
pub fn generic_loans(body: &Body, arguments: &[PlanType]) -> Vec<GenericLoan> {
    let mut loans = Vec::new();
    let source_nominals = &*body.named_type_identities;
    let scope = LoanScope {
        names: &body.type_parameters,
        arguments,
        source_nominals,
    };
    // 1. A source type parameter keeps its identity at every structural occurrence in an input.
    for (slot, parameter) in body.params.iter().enumerate() {
        type_generic_loans(&parameter.ty, &scope, slot as i64, &[], &mut loans);
    }
    // 2. Return occurrences carry exactly the same identity, including loans hidden inside native values.
    type_generic_loans(&body.return_type, &scope, body.params.len() as i64, &[], &mut loans);
    loans
}

/// The type parameters, their concrete storage and the frame's source nominal scope.
struct LoanScope<'a> {
    names: &'a [String],
    arguments: &'a [PlanType],
    source_nominals: &'a BTreeMap<String, CanonicalSymbolId>,
}

/// Record type-parameter provenance through references, tuples and declaration-ordered generic arguments.
fn type_generic_loans(ty: &IncanType, scope: &LoanScope<'_>, slot: i64, path: &[i64], loans: &mut Vec<GenericLoan>) {
    match ty {
        IncanType::TypeVar(name) => {
            if let Some(index) = scope.names.iter().position(|candidate| candidate == name) {
                loans.push(GenericLoan {
                    slot,
                    path: path.to_vec(),
                    parameter: index as i64,
                    ty: scope.arguments[index].clone(),
                });
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            let mut nested = path.to_vec();
            nested.push(0);
            type_generic_loans(inner, scope, slot, &nested, loans);
        }
        IncanType::Tuple(elements) => element_generic_loans(elements, scope, slot, path, loans),
        IncanType::Generic { base, args } => {
            if source_nominal_owner(scope.source_nominals, base) {
                return;
            }
            element_generic_loans(args, scope, slot, path, loans);
        }
        _ => {}
    }
}

/// Keep distinct source type parameters distinct even when their concrete native types happen to be equal.
fn element_generic_loans(
    elements: &[IncanType],
    scope: &LoanScope<'_>,
    slot: i64,
    path: &[i64],
    loans: &mut Vec<GenericLoan>,
) {
    for (index, element) in elements.iter().enumerate() {
        let mut nested = path.to_vec();
        nested.push(index as i64);
        type_generic_loans(element, scope, slot, &nested, loans);
    }
}

/// Retain generic loans: 1. Visit original parameters. 2. Visit the result. 3. Tie a returned view to its source.
/// 4. Preserve source elision made ambiguous by specialization. 5. Omit unneeded annotations.
///
/// Each reference introduced by one type argument has a distinct binder, reused at every occurrence of that argument.
/// Source-written references remain elided; they never acquire a substituted argument's binder.
pub fn signature_lifetimes(body: &Body, arguments: &[IncanType]) -> Result<Vec<Vec<String>>, String> {
    let names = &body.type_parameters;
    // 1. Visit original parameters while the source type parameters still exist.
    let mut signature: Vec<Vec<String>> = body
        .params
        .iter()
        .map(|parameter| type_lifetimes(&parameter.ty, names, arguments, ""))
        .collect();
    // 2. Visit the original result under the same binder identities.
    signature.push(type_lifetimes(&body.return_type, names, arguments, ""));
    // 3. A returned view's reference shares its source parameter's outer borrow, and nothing else. A direct view's
    // reference is the result itself; a carried view's is the first reference of its carrying payload.
    if let Some(view) = &body.returned_view
        && view.access != IncanInputAccess::Owned
    {
        let slot = view.slot;
        signature[slot].insert(0, "view".to_string());
        let last = signature.len() - 1;
        if view.component.is_empty() {
            signature[last].insert(0, "view".to_string());
        } else {
            let position = component_reference(&body.return_type, &view.component, names, arguments);
            if position >= signature[last].len() {
                return Err("aggregate returned view has no component reference".into());
            }
            signature[last][position] = "view".to_string();
        }
        retain_unique_source_elision(&mut signature);
        return Ok(signature);
    }
    // 4. A generic source signature with one elided input loan ties an elided output to it. If substituting a
    // reference type adds another loan, retain that original relationship explicitly instead of asking rustc to
    // reconstruct it from the now-concrete signature.
    if has_generic_loans(&signature) {
        retain_unique_source_elision(&mut signature);
    }
    // 5. Omit annotations when specialization introduces no generic loans, including transformed generators.
    if has_generic_loans(&signature) {
        return Ok(signature);
    }
    Ok(Vec::new())
}

/// Name the one source-written input loan and every source-elided result loan after generic substitution adds
/// ambiguity.
fn retain_unique_source_elision(signature: &mut [Vec<String>]) {
    if signature.len() < 2 {
        return;
    }
    let Some((result, inputs)) = signature.split_last_mut() else {
        return;
    };
    let input_elisions = inputs.iter().flatten().filter(|name| name.is_empty()).count();
    if input_elisions != 1 || !result.iter().any(String::is_empty) {
        return;
    }
    for names in inputs.iter_mut().chain(std::iter::once(result)) {
        for name in names.iter_mut() {
            if name.is_empty() {
                *name = "elided".to_string();
            }
        }
    }
}

/// Return the preorder position of the reference stored at one checked aggregate component.
fn component_reference(result: &IncanType, component: &[usize], names: &[String], arguments: &[IncanType]) -> usize {
    let Some((&slot, rest)) = component.split_first() else {
        return 0;
    };
    match result {
        IncanType::Generic { args: elements, .. } | IncanType::Tuple(elements) => {
            references_before(elements, slot, names, arguments)
                + component_reference(&elements[slot], rest, names, arguments)
        }
        _ => 0,
    }
}

/// Count the references of the components before the one carrying a view.
fn references_before(elements: &[IncanType], payload: usize, names: &[String], arguments: &[IncanType]) -> usize {
    elements[..payload]
        .iter()
        .map(|element| type_lifetimes(element, names, arguments, "").len())
        .sum()
}

/// Decide whether specialization introduced any reference binder the concrete signature must preserve.
fn has_generic_loans(signature: &[Vec<String>]) -> bool {
    signature.iter().flatten().any(|loan| !loan.is_empty())
}

/// Collect references in structural preorder, naming only loans introduced by a retained generic argument.
fn type_lifetimes(ty: &IncanType, names: &[String], arguments: &[IncanType], binder: &str) -> Vec<String> {
    let mut loans = Vec::new();
    match ty {
        IncanType::TypeVar(name) => {
            if let Some(index) = names.iter().position(|candidate| candidate == name) {
                return type_lifetimes(&arguments[index], &[], &[], &format!("t{index}"));
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            loans.push(binder.to_string());
            loans.extend(type_lifetimes(inner, names, arguments, &child_binder(binder, 0)));
        }
        IncanType::Generic { args: elements, .. } | IncanType::Tuple(elements) => {
            for (index, element) in elements.iter().enumerate() {
                loans.extend(type_lifetimes(element, names, arguments, &child_binder(binder, index)));
            }
        }
        _ => {}
    }
    loans
}

/// Extend only a generic loan's identity; source references retain ordinary independent elision.
fn child_binder(binder: &str, index: usize) -> String {
    if binder.is_empty() {
        return String::new();
    }
    format!("{binder}_{index}")
}
