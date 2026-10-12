//! Checked primitive predicates and their fail-fast newtype construction boundary.

use super::args::fixed_elements;
use super::{BodyBuilder, HirSourceSpan, IncanPrimitiveType, IncanType, bir};
use crate::ast::TypeConstraintKey;
use crate::symbols::NewtypePrimitiveConstraint;

impl BodyBuilder<'_, '_> {
    /// Evaluate the checker's ordered predicates over one saved input and retain legacy's aggregate constraint label.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_newtype_constraints(
        &mut self,
        name: &str,
        constraints: &[NewtypePrimitiveConstraint],
        value: &bir::Operand,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let mut condition = bir::Operand::Constant(bir::Constant::Bool(true));
        let mut descriptions = Vec::with_capacity(constraints.len());
        for constraint in constraints {
            let (operation, key) = match constraint.key {
                TypeConstraintKey::Gt => (bir::BinOp::Gt, "gt"),
                TypeConstraintKey::Ge => (bir::BinOp::Ge, "ge"),
                TypeConstraintKey::Lt => (bir::BinOp::Lt, "lt"),
                TypeConstraintKey::Le => (bir::BinOp::Le, "le"),
            };
            let limit = bir::Constant::Int(constraint.value);
            let comparison = self.push_assign_temp(
                bir::Rvalue::BinaryOp(operation, value.clone(), bir::Operand::Constant(limit)),
                IncanType::Primitive(IncanPrimitiveType::Bool),
                scope,
                span,
                out,
            );
            condition = self.push_assign_temp(
                bir::Rvalue::BinaryOp(bir::BinOp::And, condition, comparison),
                IncanType::Primitive(IncanPrimitiveType::Bool),
                scope,
                span,
                out,
            );
            descriptions.push(format!("{key}={}", constraint.repr));
        }
        self.push_call_temp(
            bir::Callee::Helper(bir::HelperOp::NewtypeConstraintValidate),
            fixed_elements(vec![
                condition,
                bir::Operand::Constant(bir::Constant::Str(name.to_owned())),
                bir::Operand::Constant(bir::Constant::Str(descriptions.join(", "))),
            ]),
            IncanType::Primitive(IncanPrimitiveType::Unit),
            scope,
            span,
            true,
            out,
        );
    }

    /// Save a constrained input once and validate only the retained integer profile before returning its copy read.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn constrained_newtype_input(
        &mut self,
        name: &str,
        value: bir::Operand,
        constraints: &[NewtypePrimitiveConstraint],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let underlying = IncanType::Primitive(IncanPrimitiveType::Int);
        let saved = self.push_assign_temp(bir::Rvalue::Use(value), underlying, scope, span, out);
        self.validate_newtype_constraints(name, constraints, &saved, scope, span, out);
        saved
    }

    /// Validate explicit integer-newtype constructions outside the owning implementation; other nominal calls retain
    /// their original operands. Canonical owner equality, not a matching spelling, authorizes the raw bypass.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn constrain_nominal_arguments(
        &mut self,
        name: &str,
        canonical: Option<&super::CanonicalSymbolId>,
        operands: Vec<bir::Operand>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::Operand> {
        let Some(identity) = canonical.filter(|identity| identity.kind == super::SemanticSourceTargetKind::Newtype)
        else {
            return operands;
        };
        if self.newtype_implementation.as_ref() == Some(identity) {
            return operands;
        }
        let type_info = self.type_info;
        let Some(facts) = type_info.declarations.newtype_construction.get(name) else {
            return operands;
        };
        if facts.constraints.is_empty() || facts.checked_constructor.is_some() || operands.len() != 1 {
            return operands;
        }
        if super::parameter_types::checked_semantic_type(&facts.underlying, type_info)
            != IncanType::Primitive(IncanPrimitiveType::Int)
        {
            return operands;
        }
        operands
            .into_iter()
            .map(|value| self.constrained_newtype_input(name, value, &facts.constraints, scope, span, out))
            .collect()
    }

    /// Record only the checked source-local newtype owner; another module's same-named declaration grants no bypass.
    pub(super) fn retain_newtype_implementation(&mut self, owner: &str) {
        self.newtype_implementation = self
            .type_info
            .declarations
            .declaration_identities
            .values()
            .find(|identity| {
                identity.kind == super::SemanticSourceTargetKind::Newtype
                    && identity.declaration_name == owner
                    && incan_semantics_core::canonical_module_identity(identity).as_deref()
                        == Some(self.module_identity)
            })
            .cloned();
    }
}
