//! Infer native method arguments from unique retained trait references; rustc still validates every predicate.

use std::collections::HashMap;

use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, RustTraitBound};
use incan_lang::lang::types::numerics::NumericTypeId;

use super::{TypeChecker, first_open_type_param};
use crate::ast::{Expr, Span, Spanned};
use crate::diagnostics::CompileError;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;

impl TypeChecker {
    /// Reject a closed scalar's unsatisfied parameterized native bound before retaining an executable call.
    /// Callable and marker obligations keep their existing admission path. Concrete primitive carriers have no
    /// downstream inference left: retained impl templates must match both Self and the bound's type arguments.
    pub(super) fn validate_rust_scalar_predicates(
        &mut self,
        signature: &RustFunctionSig,
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        for predicate in &signature.bounds {
            if predicate.type_arguments.is_empty()
                || incan_lang::interop::rust_callable_trait(&predicate.path).is_some()
            {
                continue;
            }
            let subject = self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(&predicate.self_type, ""),
                &signature.type_params,
                bindings,
            );
            if !matches!(
                subject,
                ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool
            ) {
                continue;
            }
            let expected = predicate
                .type_arguments
                .iter()
                .map(|argument| {
                    let argument = self.instantiate_rust_signature_type(
                        &self.native_rust_signature_type(argument, ""),
                        &signature.type_params,
                        bindings,
                    );
                    let display = render_resolved_type_as_rust_arg(&argument);
                    if display.contains(" as ") {
                        self.rust_projected_type(&display, 0)
                    } else {
                        Some(argument)
                    }
                })
                .collect::<Option<Vec<_>>>();
            let Some(expected) = expected else { continue };
            if expected
                .iter()
                .any(|argument| Self::mentions_foreign_type_variable(argument, &[]))
            {
                continue;
            }
            let Some(metadata) = self.rust_item_metadata_for_path(&predicate.path) else {
                continue;
            };
            let RustItemKind::Trait(info) = &metadata.kind else {
                continue;
            };
            if info.implementations.iter().any(|implementation| {
                self.rust_impl_predicate_arguments(implementation, &subject, &expected, &[])
                    .is_some()
                    && self.rust_impl_may_apply(implementation, &subject)
            }) {
                continue;
            }
            let arguments = expected
                .iter()
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>()
                .join(", ");
            self.errors.push(CompileError::type_error(
                format!(
                    "Rust type `{}` does not satisfy bound `{}<{}>`",
                    render_resolved_type_as_rust_arg(&subject),
                    predicate.path,
                    arguments,
                ),
                span,
            ));
        }
    }

    /// Infer a literal native carrier's element destination from the selected parameter's declared trait arguments.
    /// Only unsuffixed range endpoints take this context. Matching impl self constructors must agree, and all impl
    /// binders must close from the bound itself; a default i64 endpoint is never evidence against that destination.
    pub(super) fn rust_range_bound_destination(
        &self,
        signature: &RustFunctionSig,
        parameter: &str,
        expression: &Spanned<Expr>,
        native: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_range_bound_destination(signature, parameter, inner, native);
        }
        let Expr::Range {
            start,
            end,
            inclusive: false,
        } = &expression.node
        else {
            return None;
        };
        if !super::super::ops::is_unsuffixed_integer_literal(start)
            || !super::super::ops::is_unsuffixed_integer_literal(end)
        {
            return None;
        }
        let native = self.rust_inference_type(native, &[]);
        let ResolvedType::Generic(base, _) = &native else {
            return None;
        };
        let mut selected = None;
        for bound in &signature.bounds {
            if bound.self_type != parameter || bound.type_arguments.is_empty() {
                continue;
            }
            let metadata = self.rust_item_metadata_for_path(&bound.path)?;
            let RustItemKind::Trait(info) = &metadata.kind else {
                continue;
            };
            for implementation in &info.implementations {
                let template = self.rust_inference_type(
                    &self.native_rust_signature_type(&implementation.self_type, ""),
                    &implementation.type_params,
                );
                if !matches!(&template, ResolvedType::Generic(owner, _) if owner == base)
                    || implementation.type_arguments.len() != bound.type_arguments.len()
                {
                    continue;
                }
                let mut inferred = HashMap::new();
                for (template, destination) in implementation.type_arguments.iter().zip(&bound.type_arguments) {
                    self.infer_type_param_bindings(
                        &self.rust_inference_type(
                            &self.native_rust_signature_type(template, ""),
                            &implementation.type_params,
                        ),
                        &self.native_rust_signature_type(destination, ""),
                        &mut inferred,
                    );
                }
                let target = self.instantiate_rust_signature_type(&template, &implementation.type_params, &inferred);
                if !matches!(&target, ResolvedType::Generic(_, arguments)
                    if matches!(arguments.as_slice(), [ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_)]))
                {
                    continue;
                }
                if first_open_type_param(&target, &implementation.type_params).is_some()
                    || first_open_type_param(&target, &signature.type_params).is_some()
                {
                    continue;
                }
                if selected
                    .as_ref()
                    .is_some_and(|previous| !self.rust_trait_carriers_match(previous, &target))
                {
                    return None;
                }
                selected = Some(target);
            }
        }
        selected
    }

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
    /// A source collection subject answers through its native owner: `I = list[str]` closes `I::Item` as String.
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
            let Some(actual) = self.rust_binding_associated_type(&subject, &predicate.path, &associated.name) else {
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

    /// Prefer a native borrowed carrier only when the retained bound admits that borrow and rejects the owner.
    /// Text and bytes keep their native views; a nominal value supplied to `&T` can bind T to a shared reference,
    /// retaining the resulting nested loan in the checked signature. Callback outputs never acquire input loans.
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
            let input = signature
                .params
                .iter()
                .map(|parameter| {
                    self.rust_inference_type(
                        &self.native_rust_signature_type(&parameter.type_display, ""),
                        parameters,
                    )
                })
                .find(|ty| match ty {
                    ResolvedType::TypeVar(parameter) => parameter == name,
                    ResolvedType::Ref(inner) => {
                        matches!(inner.as_ref(), ResolvedType::TypeVar(parameter) if parameter == name)
                    }
                    _ => false,
                });
            let Some(input) = input else {
                continue;
            };
            let Some((owned, borrowed)) = bindings
                .get(name)
                .and_then(|binding| Self::borrowed_native_carrier(binding, matches!(input, ResolvedType::Ref(_))))
            else {
                continue;
            };
            let Some(metadata) = self.rust_item_metadata_for_path(&predicate.path) else {
                continue;
            };
            let RustItemKind::Trait(info) = &metadata.kind else {
                continue;
            };
            let accepts =
                |owner: &ResolvedType| self.rust_predicate_accepts_carrier(predicate, &info.implementations, owner);
            if !accepts(&owned) && accepts(&borrowed) {
                bindings.insert(name.to_owned(), borrowed);
            }
        }
    }

    /// Consult the native owner's retained implementations as well as the trait's explicit templates. Auto traits
    /// such as Send can be proven on String without an explicit impl in the trait's list; losing that proof must
    /// not turn an owned value into a borrowed generic argument. Rustc still verifies the complete selected bounds.
    fn rust_predicate_accepts_carrier(
        &self,
        predicate: &RustTraitBound,
        trait_implementations: &[RustImplementedTrait],
        owner: &ResolvedType,
    ) -> bool {
        let metadata = self
            .rust_canonical_path_for_receiver_type(owner)
            .and_then(|path| self.rust_item_metadata_for_path(&path));
        let owner_implementations = match metadata.as_deref().map(|metadata| &metadata.kind) {
            Some(RustItemKind::Type(info)) => info.implemented_traits.as_slice(),
            _ => &[],
        };
        let identity = self.rust_trait_identity(&predicate.path);
        owner_implementations
            .iter()
            .chain(trait_implementations)
            .any(|implementation| {
                self.rust_trait_identity(&implementation.path) == identity
                    && self.rust_impl_may_apply(implementation, owner)
            })
    }

    /// Pair an owned input with its native view. Nominal inputs require an already borrowed parameter, so selecting
    /// a reference-only implementation never turns an owned generic result into a reference.
    fn borrowed_native_carrier(binding: &ResolvedType, borrowed_input: bool) -> Option<(ResolvedType, ResolvedType)> {
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
            ResolvedType::RustPath(_) | ResolvedType::Generic(_, _) if borrowed_input => {
                Some((binding.clone(), ResolvedType::Ref(Box::new(binding.clone()))))
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
