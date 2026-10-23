//! Infer native method arguments from unique retained trait references; rustc still validates every predicate.

use std::collections::HashMap;

use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, RustTraitBound};

use super::{TypeChecker, first_open_type_param};
use crate::symbols::ResolvedType;

impl TypeChecker {
    /// Close missing callable binders from a bound whose subject is already known, repeating only while new bindings
    /// appear. Conditional implementations supply inference candidates rather than proof that their predicates
    /// hold.
    pub(super) fn infer_rust_predicate_bindings(
        &self,
        signature: &RustFunctionSig,
        parameters: &[String],
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        for _ in 0..parameters.len() {
            let before = bindings.len();
            for predicate in &signature.bounds {
                if let Some(inferred) = self.rust_predicate_arguments(predicate, parameters, bindings) {
                    for (name, value) in inferred {
                        bindings.entry(name).or_insert(value);
                    }
                }
            }
            if bindings.len() == before {
                return;
            }
        }
    }

    /// Infer a trait reference only when all matching self templates agree on its callable binders.
    /// Existing argument or destination constraints exclude contradictory candidates; ambiguous references infer
    /// nothing.
    fn rust_predicate_arguments(
        &self,
        predicate: &RustTraitBound,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
    ) -> Option<HashMap<String, ResolvedType>> {
        let expected = predicate
            .type_arguments
            .iter()
            .map(|argument| {
                self.instantiate_rust_signature_type(
                    &self.native_rust_signature_type(argument, ""),
                    parameters,
                    bindings,
                )
            })
            .collect::<Vec<_>>();
        if !expected
            .iter()
            .any(|argument| first_open_type_param(argument, parameters).is_some())
        {
            return None;
        }
        let subject = self.instantiate_rust_signature_type(
            &self.native_rust_signature_type(&predicate.self_type, ""),
            parameters,
            bindings,
        );
        if first_open_type_param(&subject, parameters).is_some()
            || matches!(subject, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
        {
            return None;
        }
        let metadata = self.rust_item_metadata_for_path(&predicate.path)?;
        let RustItemKind::Trait(info) = &metadata.kind else {
            return None;
        };
        let mut selected: Option<HashMap<String, ResolvedType>> = None;
        for implementation in &info.implementations {
            let Some(inferred) = self.rust_impl_predicate_arguments(implementation, &subject, &expected, parameters)
            else {
                continue;
            };
            if inferred.iter().any(|(name, value)| {
                bindings
                    .get(name)
                    .is_some_and(|known| !self.rust_trait_carriers_match(known, value))
            }) {
                continue;
            }
            if let Some(previous) = &selected
                && (previous.len() != inferred.len()
                    || previous.iter().any(|(name, value)| {
                        inferred
                            .get(name)
                            .is_none_or(|other| !self.rust_trait_carriers_match(value, other))
                    }))
            {
                return None;
            }
            selected = Some(inferred);
        }
        selected
    }

    /// Match an implementation's self type before unifying its closed trait arguments with the callable predicate.
    /// Unresolved implementation binders and mismatched fixed arguments grant no inference.
    fn rust_impl_predicate_arguments(
        &self,
        implementation: &RustImplementedTrait,
        subject: &ResolvedType,
        expected: &[ResolvedType],
        parameters: &[String],
    ) -> Option<HashMap<String, ResolvedType>> {
        if implementation.type_arguments.len() != expected.len() {
            return None;
        }
        let bound = self.rust_impl_self_bindings(implementation, subject)?;
        let mut actuals = Vec::new();
        for argument in &implementation.type_arguments {
            let actual = self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(argument, ""),
                &implementation.type_params,
                &bound,
            );
            if first_open_type_param(&actual, &implementation.type_params).is_some()
                || matches!(actual, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
            {
                return None;
            }
            actuals.push(actual);
        }
        let mut inferred = HashMap::new();
        for (expected, actual) in expected.iter().zip(&actuals) {
            self.infer_type_param_bindings(expected, actual, &mut inferred);
        }
        if expected.iter().zip(&actuals).any(|(expected, actual)| {
            let closed = self.instantiate_rust_signature_type(expected, parameters, &inferred);
            !self.rust_trait_carriers_match(&closed, actual)
        }) {
            return None;
        }
        Some(inferred)
    }
}
