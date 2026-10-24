//! Infer native method arguments from unique retained trait references; rustc still validates every predicate.

use std::collections::HashMap;

use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, RustTraitBound};
use incan_lang::lang::types::numerics::NumericTypeId;

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
                self.infer_rust_associated_bindings(predicate, parameters, bindings);
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

    /// Close native output binders from an already bound subject's retained associated equalities. Source generic
    /// subjects use their active checked bounds; a caller's binder may have the same name as the callee's binder.
    /// Metadata supplies the associated value, and rustc still verifies the specialized native predicate.
    fn infer_rust_associated_bindings(
        &self,
        predicate: &RustTraitBound,
        parameters: &[String],
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        let Some(subject) = bindings.get(&predicate.self_type).cloned() else {
            return;
        };
        for associated in &predicate.associated_type_bindings {
            let Some(actual) = self.rust_associated_type(&subject, &predicate.path, &associated.name) else {
                continue;
            };
            let expected =
                self.rust_inference_type(&self.native_rust_signature_type(&associated.value_path, ""), parameters);
            self.infer_type_param_bindings(&expected, &actual, bindings);
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

    /// Rebind a callable binder that an owned source text or bytes value closed to the borrowed native carrier
    /// (`&str`, `&[u8]`), when its trait bound has a native implementation for the borrow and none for the owned
    /// carrier (`P: Pattern` accepts `&str`, not `String`; `impl Buf` accepts `&[u8]`, not `Vec<u8>`). A binder whose
    /// bound the owned carrier already satisfies keeps it.
    pub(super) fn prefer_borrowed_bindings(
        &self,
        signature: &RustFunctionSig,
        parameters: &[String],
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        for predicate in &signature.bounds {
            let name = predicate.self_type.trim();
            if !parameters.iter().any(|parameter| parameter == name) {
                continue;
            }
            // Borrow coercion belongs to a written value argument. A binder supplied only by a callback's output
            // owns that output; an auto-trait blanket impl for a borrow cannot change the callback's result ABI.
            if !signature
                .params
                .iter()
                .any(|parameter| parameter.type_display.trim() == name)
            {
                continue;
            }
            let Some((owned, borrowed)) = bindings.get(name).and_then(Self::borrowed_native_carrier) else {
                continue;
            };
            let Some(metadata) = self.rust_item_metadata_for_path(&predicate.path) else {
                continue;
            };
            let RustItemKind::Trait(info) = &metadata.kind else {
                continue;
            };
            let accepts = |owner: &ResolvedType| {
                info.implementations.iter().any(|implementation| {
                    self.rust_impl_may_apply(implementation, owner)
                        && self.rust_impl_self_bindings(implementation, owner).is_some()
                })
            };
            if !accepts(&owned) && accepts(&borrowed) {
                bindings.insert(name.to_owned(), borrowed);
            }
        }
    }

    /// The owned native carrier a source text or bytes binding stands for, with the borrowed carrier a bound may
    /// instead require.
    fn borrowed_native_carrier(binding: &ResolvedType) -> Option<(ResolvedType, ResolvedType)> {
        let byte = ResolvedType::Numeric(NumericTypeId::U8);
        match binding {
            ResolvedType::Str => Some((
                ResolvedType::RustPath("std::string::String".into()),
                ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into()))),
            )),
            ResolvedType::Generic(base, arguments) if base == "List" && arguments.as_slice() == [byte.clone()] => {
                Some((
                    ResolvedType::RustPath("std::vec::Vec<u8>".into()),
                    ResolvedType::Ref(Box::new(ResolvedType::Generic("[]".into(), vec![byte]))),
                ))
            }
            _ => None,
        }
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
