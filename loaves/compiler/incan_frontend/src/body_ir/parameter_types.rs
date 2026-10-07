//! Preserve declaration-bound placeholders while projecting checked types into Body IR.

use super::{BodyBuilder, IncanType, ast, bir, semantic_type_from_resolved};

/// Distinguish declared binders from nominal types using the active declaration's retained parameter set.
///
/// Legacy checking can encode an in-scope binder as `Named`; Body IR must retain the placeholder category so
/// executable instance selection substitutes it. This projection does not infer parameters from values or add
/// binders absent from the checked declaration, and leaves the legacy checker's representation unchanged.
pub(super) fn retain_parameter_type(ty: IncanType, parameters: &[String]) -> IncanType {
    match ty {
        IncanType::Named(name) if parameters.contains(&name) => IncanType::TypeVar(name),
        IncanType::Generic { base, args } => IncanType::Generic {
            base,
            args: args
                .into_iter()
                .map(|ty| retain_parameter_type(ty, parameters))
                .collect(),
        },
        IncanType::Tuple(elements) => IncanType::Tuple(
            elements
                .into_iter()
                .map(|ty| retain_parameter_type(ty, parameters))
                .collect(),
        ),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(retain_parameter_type(*inner, parameters))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(retain_parameter_type(*inner, parameters))),
        IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(retain_parameter_type(*inner, parameters))),
        ty => ty,
    }
}

impl BodyBuilder<'_, '_> {
    /// Convert a checked type while retaining the active declaration's explicit placeholder bindings.
    pub(super) fn checked_type(&self, ty: &crate::symbols::ResolvedType) -> IncanType {
        retain_parameter_type(semantic_type_from_resolved(ty), &self.type_parameters)
    }

    /// Retain an owned value read from an open parameter field at a return or by-value argument boundary.
    ///
    /// Generic field projections cannot move from their borrowed owner. The legacy ownership policy clones them
    /// in value position; concrete Copy instances can later implement that clone as a copy. Formatting and mutable
    /// argument boundaries retain their distinct borrow facts rather than being converted by this helper.
    pub(super) fn owned_parameter_operand(
        &self,
        expr: &ast::Spanned<ast::Expr>,
        operand: bir::Operand,
    ) -> bir::Operand {
        if !matches!(self.resolve_ty(expr.span), IncanType::TypeVar(_)) {
            return operand;
        }
        match operand {
            bir::Operand::Place(mut read)
                if read.fact == bir::OwnershipFact::Borrow && !read.place.projection.is_empty() =>
            {
                read.fact = bir::OwnershipFact::Clone;
                bir::Operand::Place(read)
            }
            operand => operand,
        }
    }
}
