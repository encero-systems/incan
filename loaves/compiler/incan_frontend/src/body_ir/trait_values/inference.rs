//! Close caller arguments from checked nominal adoptions, without reopening legacy inference or source spellings.

use super::{CanonicalSymbolId, IncanType, TraitInputs, bir};
use std::collections::BTreeMap;

/// Fill every declared argument from exact trait annotation/adopter pairs; conflicting or incomplete witnesses grant
/// none. Published hidden frames without their original annotation still require their existing checked explicit
/// arguments.
pub(super) fn complete_arguments(
    inputs: &TraitInputs,
    carriers: &[IncanType],
    nominals: &[bir::NominalDeclaration],
    adoptions: &BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>>,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> Option<Vec<IncanType>> {
    if inputs.annotations.len() != carriers.len() {
        return None;
    }
    let mut bindings = BTreeMap::new();
    for ((annotation, trait_owner), carrier) in inputs.annotations.iter().zip(carriers) {
        let IncanType::Generic { args: expected, .. } = annotation else {
            continue;
        };
        let actual = adoption_arguments(carrier, trait_owner, nominals, adoptions, identities)?;
        if expected.len() != actual.len() {
            return None;
        }
        for (expected, actual) in expected.iter().zip(&actual) {
            if !bind(expected, actual, &inputs.parameters, &mut bindings) {
                return None;
            }
        }
    }
    inputs
        .parameters
        .iter()
        .map(|name| bindings.get(name).cloned())
        .collect()
}

/// Require one canonical adopter's exact checked trait adoption, then close the adopter's declared owner binders.
fn adoption_arguments(
    carrier: &IncanType,
    trait_owner: &CanonicalSymbolId,
    nominals: &[bir::NominalDeclaration],
    adoptions: &BTreeMap<(CanonicalSymbolId, CanonicalSymbolId), Vec<IncanType>>,
    identities: &BTreeMap<String, CanonicalSymbolId>,
) -> Option<Vec<IncanType>> {
    let (name, arguments) = match carrier {
        IncanType::Named(name) => (name, &[][..]),
        IncanType::Generic { base, args } => (base, args.as_slice()),
        _ => return None,
    };
    let identity = identities.get(name)?;
    let owner = nominals
        .iter()
        .find(|owner| &owner.canonical == identity && owner.name == *name)?;
    if owner.type_parameters.len() != arguments.len() {
        return None;
    }
    let bindings = owner
        .type_parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<BTreeMap<_, _>>();
    adoptions
        .get(&(identity.clone(), trait_owner.clone()))?
        .iter()
        .map(|argument| close(argument, &bindings))
        .collect()
}

/// Substitute only semantic placeholders belonging to the adopter's declared frame, preserving nominal names.
fn close(ty: &IncanType, bindings: &BTreeMap<String, IncanType>) -> Option<IncanType> {
    match ty {
        IncanType::TypeVar(name) => bindings.get(name).cloned(),
        IncanType::Generic { base, args } => Some(IncanType::Generic {
            base: base.clone(),
            args: args.iter().map(|ty| close(ty, bindings)).collect::<Option<_>>()?,
        }),
        IncanType::Tuple(args) => Some(IncanType::Tuple(
            args.iter().map(|ty| close(ty, bindings)).collect::<Option<_>>()?,
        )),
        _ => Some(ty.clone()),
    }
}

/// Bind a declared callee placeholder once, or verify the exact structural type the checked annotation already
/// requires.
fn bind(
    expected: &IncanType,
    actual: &IncanType,
    parameters: &[String],
    bindings: &mut BTreeMap<String, IncanType>,
) -> bool {
    if let IncanType::TypeVar(name) = expected {
        if !parameters.contains(name) || !concrete(actual) {
            return false;
        }
        if let Some(previous) = bindings.get(name) {
            return previous == actual;
        }
        bindings.insert(name.clone(), actual.clone());
        return true;
    }
    match (expected, actual) {
        (
            IncanType::Generic {
                base: left,
                args: expected,
            },
            IncanType::Generic {
                base: right,
                args: actual,
            },
        ) if left == right && expected.len() == actual.len() => expected
            .iter()
            .zip(actual)
            .all(|(expected, actual)| bind(expected, actual, parameters, bindings)),
        (IncanType::Tuple(expected), IncanType::Tuple(actual)) if expected.len() == actual.len() => expected
            .iter()
            .zip(actual)
            .all(|(expected, actual)| bind(expected, actual, parameters, bindings)),
        _ => expected == actual,
    }
}

/// Admit only fully represented arguments; unbound placeholders cannot be smuggled in through an adoption witness.
fn concrete(ty: &IncanType) -> bool {
    match ty {
        IncanType::TypeVar(_) | IncanType::Unknown | IncanType::SelfType | IncanType::Unsupported(_) => false,
        IncanType::Generic { args, .. } | IncanType::Tuple(args) => args.iter().all(concrete),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => concrete(inner),
        _ => true,
    }
}
