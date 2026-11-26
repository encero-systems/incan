//! Retain the source checker's Copy decisions independently of the ownership event graph.

use std::collections::HashMap;

use incan_lang::lang::derives::DeriveId;
use incan_semantics_core::CanonicalSymbolId;

use crate::ast::Span;
use crate::symbols::ResolvedType;
use crate::typechecker::derive_requirements::DeriveSupport;
use crate::typechecker::{TypeCheckInfo, TypeChecker};

impl TypeChecker {
    /// Shared references copy their address and exclusive references transfer access. Other values use the one
    /// checked derive relation, including nominal, generic and native evidence; an unknown result is not Copy.
    pub(super) fn input_copy_value(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Ref(_) => true,
            ResolvedType::RefMut(_) => false,
            ResolvedType::Tuple(items) => items.iter().all(|item| self.input_copy_value(item)),
            _ => matches!(self.derive_support(ty, DeriveId::Copy), DeriveSupport::Supported),
        }
    }

    /// Capture decisions while this exact body's lexical type environment is still active. Expression values,
    /// instantiated input slots and annotated local payloads are retained, so later validation never rebuilds
    /// nominal derive resolution from display names or substitutes native carrier copyability.
    pub(super) fn retain_input_copy_values(&mut self, owner: &CanonicalSymbolId, span: Span) {
        let expressions = self
            .type_info
            .expressions
            .expr_types
            .iter()
            .chain(self.type_info.expressions.body_ir_expr_types.iter())
            .filter(|(expression, _)| {
                expression.expansion == span.expansion && span.start <= expression.start && expression.end <= span.end
            })
            .map(|(_, ty)| ty);
        let parameters = self
            .type_info
            .protocols
            .explicit_input_calls
            .values()
            .filter(|call| call.owner.as_ref() == Some(owner))
            .flat_map(|call| call.parameters.iter().map(|parameter| &parameter.ty));
        let locals = self
            .type_info
            .protocols
            .explicit_local_modes
            .values()
            .filter(|mode| &mode.owner == owner)
            .map(|mode| &mode.value_type);
        let decisions: HashMap<_, _> = expressions
            .chain(parameters)
            .chain(locals)
            .map(|ty| (ty.clone(), self.input_copy_value(ty)))
            .collect();
        self.type_info
            .protocols
            .explicit_input_copy
            .insert(owner.clone(), decisions);
    }
}

impl TypeCheckInfo {
    /// Require the exact body-owned checked decision. A missing legacy record or another type's Copy fact does
    /// not authorize either a copy or a move; record restoration must preserve this independent authority.
    pub(in crate::typechecker) fn checked_input_copy_value(
        &self,
        owner: &CanonicalSymbolId,
        ty: &ResolvedType,
    ) -> Result<bool, &'static str> {
        self.protocols
            .explicit_input_copy
            .get(owner)
            .and_then(|decisions| decisions.get(ty))
            .copied()
            .ok_or("input value has no body-owned checked Copy decision")
    }
}
