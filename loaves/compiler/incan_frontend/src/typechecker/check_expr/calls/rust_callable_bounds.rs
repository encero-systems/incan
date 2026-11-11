//! Contextual closure expression types and environment admission from rustc's retained callable predicates.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Spanned};
use crate::diagnostics::errors;
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::{CallableParam, ResolvedType};
use incan_lang::interop::{RustFunctionSig, RustItemKind, rust_auto_trait, rust_callable_trait};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::body_ir::NativeClosureStorage;

impl TypeChecker {
    /// Retain callback slots against the callee's complete binder order, including method-owner parameters.
    pub(in crate::typechecker::check_expr) fn retain_native_callback_parameters(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        native_parameters: &[String],
    ) {
        let slots = signature
            .params
            .iter()
            .enumerate()
            .filter_map(|(slot, parameter)| {
                let index = native_parameters
                    .iter()
                    .position(|name| name == &parameter.type_display)?;
                self.rust_bound_callable_type(signature, &parameter.type_display)?;
                Some((slot, index))
            })
            .collect();
        self.type_info
            .rust
            .native_callable_parameters
            .insert(path.trim_start_matches("rust::").to_owned(), slots);
    }

    /// Recover a callable's inputs and result from its declared predicate, rather than inferring them from F's name.
    /// The declaration's original binders remain intact until checked argument substitution supplies their values.
    pub(in crate::typechecker::check_expr) fn rust_bound_callable_type(
        &self,
        signature: &RustFunctionSig,
        parameter: &str,
    ) -> Option<ResolvedType> {
        let bound = signature.bounds.iter().find(|bound| {
            bound.self_type == parameter
                && rust_callable_trait(&bound.path).is_some()
                && bound
                    .associated_type_bindings
                    .iter()
                    .any(|binding| binding.name == "Output")
        })?;
        let inputs = self.native_rust_signature_type(bound.type_arguments.first()?, "");
        let inputs = match inputs {
            ResolvedType::Tuple(inputs) => inputs,
            ResolvedType::Unit => Vec::new(),
            _ => return None,
        };
        let output = &bound
            .associated_type_bindings
            .iter()
            .find(|binding| binding.name == "Output")?
            .value_path;
        Some(ResolvedType::Function(
            inputs.into_iter().map(CallableParam::positional).collect(),
            Box::new(self.native_rust_signature_type(output, "")),
        ))
    }

    /// Instantiate a retained callback predicate with checked argument bindings before contextual checking or
    /// boundary validation. Unbound callee parameters become contextual holes only after substitution, so a source
    /// nominal nested in a native input never makes a round trip through its display spelling.
    fn instantiated_rust_bound_callable_type(
        &self,
        signature: &RustFunctionSig,
        parameter: &str,
        bindings: &HashMap<String, ResolvedType>,
        contextual: bool,
    ) -> Option<ResolvedType> {
        let declared = self.rust_bound_callable_type(signature, parameter)?;
        let instantiated = self.instantiate_rust_signature_type(&declared, &signature.type_params, bindings);
        if !contextual {
            return Some(instantiated);
        }
        Some(map_resolved_type_leaves(&instantiated, &|leaf| match leaf {
            ResolvedType::TypeVar(name) if signature.type_params.contains(name) && !bindings.contains_key(name) => {
                Some(ResolvedType::Unknown)
            }
            _ => None,
        }))
    }

    /// Recheck deferred closures after other arguments constrain native callback binders, retaining proven capture
    /// storage. The structural substitution precedes closure checking; display-specialized signatures are not its
    /// source of parameter identities.
    pub(in crate::typechecker::check_expr) fn contextualize_rust_bound_closures(
        &mut self,
        signature: &RustFunctionSig,
        owner_path: &str,
        args: &[CallArg],
        types: &[ResolvedType],
        expected_return: Option<&ResolvedType>,
    ) -> Vec<ResolvedType> {
        let needs_context = args
            .iter()
            .any(|argument| matches!(Self::call_arg_expr(argument).node, Expr::Closure(..)))
            && signature
                .bounds
                .iter()
                .any(|bound| rust_callable_trait(&bound.path).is_some());
        let bindings = if needs_context {
            let inference_parameters = Self::rust_signature_inference_parameters(signature);
            self.infer_rust_signature_bindings(
                signature,
                owner_path,
                &inference_parameters,
                args,
                types,
                expected_return,
            )
        } else {
            HashMap::new()
        };
        let parameters = if Self::rust_signature_has_receiver(signature) {
            &signature.params[1..]
        } else {
            &signature.params
        };
        args.iter()
            .zip(types)
            .enumerate()
            .map(|(index, (argument, ty))| {
                let expression = Self::call_arg_expr(argument);
                let parameter = match argument {
                    CallArg::Positional(_) => parameters.get(index),
                    CallArg::Named(name, _) => parameters
                        .iter()
                        .find(|parameter| parameter.name.as_ref() == Some(&name.node)),
                    _ => None,
                };
                let Some(parameter) = parameter else {
                    return ty.clone();
                };
                let checked = if matches!(expression.node, Expr::Closure(_, _))
                    && let Some(expected) =
                        self.instantiated_rust_bound_callable_type(signature, &parameter.type_display, &bindings, true)
                {
                    self.check_expr_with_expected(expression, Some(&expected))
                } else {
                    ty.clone()
                };
                self.check_rust_callback_storage(signature, &parameter.type_display, expression);
                checked
            })
            .collect()
    }

    /// Validate a source callable directly against its instantiated native predicate. Return whether this argument
    /// has a retained callable bound, including a diagnosed mismatch, so callers never validate it a second time
    /// against a rendered function-pointer display. A borrowed scalar result has no admitted lifetime contract and
    /// is refused here rather than reaching native plan validation. Other arguments keep ordinary boundary validation.
    pub(super) fn validate_rust_bound_callable_argument(
        &mut self,
        signature: &RustFunctionSig,
        parameter: &str,
        bindings: &HashMap<String, ResolvedType>,
        expression: &Spanned<Expr>,
        actual: &ResolvedType,
    ) -> bool {
        if !matches!(actual, ResolvedType::Function(..)) {
            return false;
        }
        let Some(expected) = self.instantiated_rust_bound_callable_type(signature, parameter, bindings, false) else {
            return false;
        };
        let actual_storage = self.rust_inference_type(actual, &[]);
        let expected_storage = self.rust_inference_type(&expected, &[]);
        if let ResolvedType::Function(_, result) = &actual_storage
            && matches!(result.as_ref(), ResolvedType::Ref(inner) | ResolvedType::RefMut(inner)
                if matches!(inner.as_ref(), ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool))
        {
            self.errors.push(crate::diagnostics::CompileError::type_error(
                "Rust callback cannot return a borrowed scalar without a retained lifetime contract".into(),
                expression.span,
            ));
            return true;
        }
        if !self.types_compatible(&actual_storage, &expected_storage) {
            self.errors.push(errors::type_mismatch(
                &expected.to_string(),
                &actual.to_string(),
                expression.span,
            ));
        }
        true
    }

    /// Retain owned environments only after each requested auto trait is proved for every captured value.
    /// Unbounded native binders still have no callable storage contract; missing evidence names the capture.
    fn check_rust_callback_storage(
        &mut self,
        signature: &RustFunctionSig,
        parameter: &str,
        expression: &Spanned<Expr>,
    ) {
        if signature.type_params.iter().any(|declared| declared == parameter)
            && self.rust_bound_callable_type(signature, parameter).is_none()
        {
            self.refuse_capturing_callable(
                expression,
                "passed to a Rust generic parameter without retained callable bounds",
            );
            return;
        }
        let Some(origin) = self.native_closure_origin(expression) else {
            if let Some(bound) = signature
                .bounds
                .iter()
                .find(|bound| bound.self_type == parameter && rust_auto_trait(&bound.path).is_some())
            {
                self.refuse_capturing_callable(
                    expression,
                    &format!(
                        "passed to a Rust callback requiring `{}` without retained capture types",
                        bound.path,
                    ),
                );
            }
            return;
        };
        let Some(captures) = self.capturing_callables.captures.get(&origin).cloned() else {
            return;
        };
        let mut storage = NativeClosureStorage {
            move_capture: true,
            ..Default::default()
        };
        for bound in signature.bounds.iter().filter(|bound| bound.self_type == parameter) {
            let marker = match rust_auto_trait(&bound.path) {
                Some("Send") => {
                    storage.send = true;
                    "Send"
                }
                Some("Sync") => {
                    storage.sync = true;
                    "Sync"
                }
                _ => continue,
            };
            for capture in &captures {
                if !self.capture_has_native_auto_trait(&capture.ty, marker, 0) {
                    self.errors
                        .push(crate::diagnostics::errors::rust_callback_capture_missing_auto_trait(
                            &capture.name,
                            &capture.ty.to_string(),
                            marker,
                            expression.span,
                        ));
                    return;
                }
            }
        }
        let retained = self.type_info.rust.closure_storage.entry(origin).or_default();
        retained.move_capture |= storage.move_capture;
        retained.send |= storage.send;
        retained.sync |= storage.sync;
    }

    /// Find the originating closure behind parentheses and immutable local aliases, preserving lexical identity.
    pub(in crate::typechecker) fn native_closure_origin(&self, expression: &Spanned<Expr>) -> Option<crate::ast::Span> {
        match &expression.node {
            Expr::Paren(inner) => self.native_closure_origin(inner),
            Expr::Closure(..) => Some(expression.span),
            Expr::Ident(name) => {
                let symbol = self.lookup_symbol(name)?;
                self.capturing_callables.origins.get(&symbol.span).copied()
            }
            _ => None,
        }
    }

    /// Prove a captured native carrier from exact retained impl templates, recursively satisfying their conditions.
    /// Reference captures cannot authorize escaping borrowed storage, and incomplete evidence stays fail-closed.
    fn capture_has_native_auto_trait(&self, ty: &ResolvedType, marker: &str, depth: usize) -> bool {
        if depth > 24 || matches!(ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) {
            return false;
        }
        if self.has_forwarded_auto_trait_evidence(ty, marker) {
            return true;
        }
        if let ResolvedType::Generic(name, arguments) = ty
            && matches!(
                collections::from_str(name),
                Some(
                    CollectionTypeId::List
                        | CollectionTypeId::Dict
                        | CollectionTypeId::Set
                        | CollectionTypeId::Option
                        | CollectionTypeId::Result
                        | CollectionTypeId::FrozenList
                        | CollectionTypeId::FrozenDict
                        | CollectionTypeId::FrozenSet
                )
            )
        {
            return arguments
                .iter()
                .all(|argument| self.capture_has_native_auto_trait(argument, marker, depth + 1));
        }
        if let ResolvedType::Tuple(arguments) = ty {
            return arguments
                .iter()
                .all(|argument| self.capture_has_native_auto_trait(argument, marker, depth + 1));
        }
        let native = match ty {
            ResolvedType::Int => ResolvedType::RustPath("i64".into()),
            ResolvedType::Float => ResolvedType::RustPath("f64".into()),
            ResolvedType::Bool => ResolvedType::RustPath("bool".into()),
            ResolvedType::Str => ResolvedType::RustPath("std::string::String".into()),
            ResolvedType::Numeric(kind) => {
                ResolvedType::RustPath(incan_lang::lang::types::numerics::rust_name(*kind).into())
            }
            _ => self.rusttype_native_target(ty).unwrap_or_else(|| ty.clone()),
        };
        if matches!(marker, "Send" | "Sync")
            && let Some((base, definition, arguments, _)) = self.rust_identity_for_type(&native)
        {
            let owner = definition.as_deref().unwrap_or(&base);
            if arguments.len() == 1 {
                if matches!(owner, "std::sync::Arc" | "alloc::sync::Arc") {
                    return self.capture_has_native_auto_trait(&arguments[0], "Send", depth + 1)
                        && self.capture_has_native_auto_trait(&arguments[0], "Sync", depth + 1);
                }
                if matches!(
                    owner,
                    "std::vec::Vec" | "alloc::vec::Vec" | "std::option::Option" | "core::option::Option"
                ) {
                    return self.capture_has_native_auto_trait(&arguments[0], marker, depth + 1);
                }
            }
            if arguments.len() == 2
                && matches!(
                    owner,
                    "std::collections::BTreeMap"
                        | "alloc::collections::BTreeMap"
                        | "alloc::collections::btree::map::BTreeMap"
                )
            {
                return arguments
                    .iter()
                    .all(|argument| self.capture_has_native_auto_trait(argument, marker, depth + 1));
            }
        }
        let Some(path) = self.rust_canonical_path_for_receiver_type(&native) else {
            return false;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(&path) else {
            return false;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return false;
        };
        // Primitive lookup uses its Rust cache key, while impl matching keeps the checker's canonical scalar.
        let native = if matches!(
            ty,
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Numeric(_)
        ) {
            ty.clone()
        } else {
            native
        };
        info.implemented_traits.iter().any(|implementation| {
            if rust_auto_trait(&implementation.path) != Some(marker) {
                return false;
            }
            let Some(bindings) = self.rust_impl_self_bindings(implementation, &native) else {
                return false;
            };
            implementation.bounds.iter().all(|bound| {
                if matches!(bound.path.as_str(), "std::marker::Sized" | "core::marker::Sized") {
                    return true;
                }
                let Some(required) = rust_auto_trait(&bound.path) else {
                    return false;
                };
                let subject = self.native_rust_signature_type(&bound.self_type, "");
                let subject = crate::resolved_type_subst::substitute_resolved_type(&subject, &bindings);
                self.capture_has_native_auto_trait(&subject, required, depth + 1)
            })
        })
    }

    /// Reuse an admitted transparent rusttype's native proof after a Rust owner has projected the alias to storage.
    /// The declaration check already proved the backing Rust carrier implements the marker; wrappers such as Arc
    /// retain that carrier rather than its source spelling when their conditional auto-trait bounds are solved.
    fn has_forwarded_auto_trait_evidence(&self, ty: &ResolvedType, marker: &str) -> bool {
        self.type_info
            .rust
            .rusttype_forwarded_trait_adoptions
            .iter()
            .filter(|(_, trait_name)| {
                trait_name == marker
                    || trait_name
                        .rsplit(['.', ':'])
                        .find(|segment| !segment.is_empty())
                        .is_some_and(|name| name == marker)
            })
            .any(|(name, _)| {
                let source = ResolvedType::Named(name.clone());
                source == *ty
                    || self
                        .rusttype_native_target(&source)
                        .is_some_and(|target| self.rust_trait_carriers_match(&target, ty))
            })
    }
}
