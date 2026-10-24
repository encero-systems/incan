//! Retain nominal member contracts from checked declarations and executable receiver values.

use std::collections::HashMap;

use crate::ast::{CallArg, Expr, Span, Spanned};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::symbols::{MethodInfo, ResolvedType, TypeInfo};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::body_ir::nominal_carrier_key;

use super::TypeChecker;

impl TypeChecker {
    /// Retain only the declaration already selected by ordinary member checking, specializing its own owner binders.
    pub(super) fn body_ir_nominal_method_type(
        &mut self,
        receiver: &Spanned<Expr>,
        method: &str,
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let actual = self.body_ir_checked_value_type(receiver.span)?.clone();
        let (owner, arguments) = nominal_parts(&actual)?;
        let selected = self.type_info.resolved_identity(span)?;
        let (parameters, member) = if let Some(info) = self.lookup_semantic_type_info(owner) {
            let (parameters, methods) = match info {
                TypeInfo::Newtype(info) => (&info.type_params, &info.methods),
                TypeInfo::Model(info) => (&info.type_params, &info.methods),
                TypeInfo::Class(info) => (&info.type_params, &info.methods),
                TypeInfo::Enum(info) => (&info.type_params, &info.methods),
                _ => return None,
            };
            (parameters.clone(), methods.get(method)?.clone())
        } else {
            let info = self.lookup_semantic_trait_info(owner)?;
            (info.type_params.clone(), info.methods.get(method)?.clone())
        };
        if member.identity.as_ref() != Some(selected) {
            return None;
        }
        let bindings = self.body_ir_member_bindings(&parameters, arguments, &member, args)?;
        for (name, bounds) in &member.type_param_bound_details {
            let ty = bindings.get(name)?;
            if bounds
                .iter()
                .any(|bound| !self.type_satisfies_explicit_bound_info(ty, bound, &bindings))
            {
                return None;
            }
        }
        let closed_receiver = if parameters.is_empty() {
            actual
        } else {
            ResolvedType::Generic(
                owner.to_owned(),
                parameters
                    .iter()
                    .filter_map(|name| bindings.get(name).cloned())
                    .collect(),
            )
        };
        let (_, returned) = self.method_types_substituting_call_site_self(&member, &closed_receiver);
        let returned = substitute_resolved_type(&returned, &bindings);
        if !Self::is_settled_type_argument(&returned) {
            return None;
        }
        let names = if member.receiver.is_none() {
            parameters.iter().chain(&member.type_params).collect::<Vec<_>>()
        } else {
            member.type_params.iter().collect()
        };
        let ordered = names
            .into_iter()
            .map(|name| bindings.get(name).cloned())
            .collect::<Option<Vec<_>>>()?;
        self.type_info
            .calls
            .body_ir_call_type_arguments
            .insert((span.start, span.end), ordered);
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert((span.start, span.end), returned.clone());
        Some(returned)
    }

    /// Bind owner arguments and checked actual parameters in a private map; no legacy inference record is revised.
    fn body_ir_member_bindings(
        &self,
        parameters: &[String],
        arguments: &[ResolvedType],
        member: &MethodInfo,
        args: &[CallArg],
    ) -> Option<HashMap<String, ResolvedType>> {
        if member.type_params.iter().any(|name| parameters.contains(name))
            || args
                .iter()
                .any(|arg| matches!(arg, CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_)))
        {
            return None;
        }
        let mut bindings = type_param_subst_map(parameters, arguments);
        bindings.retain(|_, ty| Self::is_settled_type_argument(ty));
        for (value, parameter) in Self::arguments_with_parameters(&member.params, args) {
            let parameter = parameter?;
            let actual = self.body_ir_checked_value_type(value.span)?;
            self.infer_type_param_bindings(&parameter.ty, actual, &mut bindings);
        }
        self.infer_type_param_bindings_from_source_bounds(&member.type_param_bound_details, &mut bindings);
        if parameters
            .iter()
            .chain(&member.type_params)
            .any(|name| bindings.get(name).is_none_or(|ty| !Self::is_settled_type_argument(ty)))
        {
            return None;
        }
        Some(bindings)
    }

    /// Read a checked nominal field under the receiver's exact declaration and closed owner arguments.
    pub(super) fn body_ir_nominal_field_type(&self, receiver: &Spanned<Expr>, field: &str) -> Option<ResolvedType> {
        let (owner, arguments) = nominal_parts(self.body_ir_checked_value_type(receiver.span)?)?;
        let qualified = self.symbols.module_type_identity(owner).and_then(nominal_carrier_key);
        let info = qualified
            .as_deref()
            .and_then(|key| self.lookup_semantic_type_info(key))
            .or_else(|| self.lookup_semantic_type_info(owner))?;
        let (parameters, declared) = match info {
            TypeInfo::Newtype(info) if field == "0" => (&info.type_params, &info.underlying),
            TypeInfo::Model(info) => (
                &info.type_params,
                &self.resolve_field_info(&info.fields, field, true, false)?.1.ty,
            ),
            TypeInfo::Class(info) => (
                &info.type_params,
                &self.resolve_field_info(&info.fields, field, false, true)?.1.ty,
            ),
            _ => return None,
        };
        let returned = substitute_resolved_type(declared, &type_param_subst_map(parameters, arguments));
        Self::is_settled_type_argument(&returned).then_some(returned)
    }

    /// Carry a list or dictionary element, or constant tuple element, from its already checked collection value.
    pub(super) fn body_ir_indexed_value_type(
        &self,
        receiver: &Spanned<Expr>,
        index: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        let actual = self.body_ir_checked_value_type(receiver.span)?;
        if let Some(element) = Self::body_ir_list_element_type(actual) {
            return Some(element.clone());
        }
        if let ResolvedType::Generic(name, parts) = actual
            && collections::from_str(name) == Some(CollectionTypeId::Dict)
            && let [_, value] = parts.as_slice()
        {
            return Some(value.clone());
        }
        let ResolvedType::Tuple(elements) = actual else {
            return None;
        };
        let mut index = Self::constant_tuple_index(index)?;
        if index < 0 {
            index = index.checked_add(elements.len().try_into().ok()?)?;
        }
        elements.get(usize::try_from(index).ok()?).cloned()
    }
}

/// Separate an already checked nominal's declaration spelling and ordered owner arguments.
fn nominal_parts(ty: &ResolvedType) -> Option<(&str, &[ResolvedType])> {
    match ty {
        ResolvedType::Named(owner) => Some((owner, &[])),
        ResolvedType::Generic(owner, arguments) => Some((owner, arguments)),
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => nominal_parts(inner),
        _ => None,
    }
}
