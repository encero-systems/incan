//! Preserve declaration-bound placeholders while projecting checked types into Body IR.

use super::{BodyBuilder, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir, semantic_type_from_resolved};

/// Project checked nominal spellings onto their declaring carrier names, without resolving source syntax again.
///
/// Imported checker types can use package-qualified names while the retained declaration uses its source name.
/// Only checker-minted canonical bindings authorize this projection. Project assembly still refuses equal carrier
/// names from distinct owners, so this does not choose between conflicting declarations.
pub(super) fn checked_semantic_type(ty: &crate::symbols::ResolvedType, facts: &TypeCheckInfo) -> IncanType {
    retain_checked_nominal_names(semantic_type_from_resolved(ty), facts)
}

/// Normalize nested nominal spellings and structural union order before they cross the Body IR boundary.
/// Imported metadata may list an equivalent union in another order; retaining one order keeps nested return and
/// parameter positions on the same layout without changing the legacy checker's representation.
fn retain_checked_nominal_names(ty: IncanType, facts: &TypeCheckInfo) -> IncanType {
    match ty {
        IncanType::Named(name) => IncanType::Named(
            facts
                .declarations
                .named_type_identities
                .get(&name)
                .map_or(name.clone(), |identity| identity.declaration_name.clone()),
        ),
        IncanType::Generic { base, args } => {
            let mut args: Vec<_> = args
                .into_iter()
                .map(|ty| retain_checked_nominal_names(ty, facts))
                .collect();
            if base == "Union" {
                args.sort_by_cached_key(ToString::to_string);
            }
            IncanType::Generic { base, args }
        }
        IncanType::Tuple(elements) => IncanType::Tuple(
            elements
                .into_iter()
                .map(|ty| retain_checked_nominal_names(ty, facts))
                .collect(),
        ),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(retain_checked_nominal_names(*inner, facts))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(retain_checked_nominal_names(*inner, facts))),
        IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(retain_checked_nominal_names(*inner, facts))),
        IncanType::Function {
            mut params,
            return_type,
        } => {
            for parameter in &mut params {
                parameter.ty = retain_checked_nominal_names(parameter.ty.clone(), facts);
            }
            IncanType::Function {
                params,
                return_type: Box::new(retain_checked_nominal_names(*return_type, facts)),
            }
        }
        ty => ty,
    }
}

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
        IncanType::Function { params, return_type } => IncanType::Function {
            params: params
                .into_iter()
                .map(|mut parameter| {
                    parameter.ty = retain_parameter_type(parameter.ty, parameters);
                    parameter
                })
                .collect(),
            return_type: Box::new(retain_parameter_type(*return_type, parameters)),
        },
        ty => ty,
    }
}

impl BodyBuilder<'_, '_> {
    /// Convert a checked type while retaining the active declaration's explicit placeholder bindings.
    pub(super) fn checked_type(&self, ty: &crate::symbols::ResolvedType) -> IncanType {
        retain_parameter_type(checked_semantic_type(ty, self.type_info), &self.type_parameters)
    }

    /// Retain an owned value read from a field at a return or by-value argument boundary.
    ///
    /// A field projection cannot move from its owner, which is often borrowed. The legacy ownership policy clones it in
    /// value position: an open parameter field, whose concrete Copy instances can later implement that clone as a
    /// copy, and a model or class field reached through field projections alone (#241), whose checked derives include
    /// Clone. Formatting and mutable argument boundaries retain their distinct borrow facts rather than being converted
    /// by this helper.
    pub(super) fn owned_parameter_operand(
        &self,
        expr: &ast::Spanned<ast::Expr>,
        operand: bir::Operand,
    ) -> bir::Operand {
        let ty = self.resolve_ty(expr.span);
        match operand {
            bir::Operand::Place(mut read)
                if read.fact == bir::OwnershipFact::Borrow
                    && !read.place.projection.is_empty()
                    && (matches!(ty, IncanType::TypeVar(_)) || self.clones_nominal_field_read(&read.place, &ty)) =>
            {
                read.fact = bir::OwnershipFact::Clone;
                bir::Operand::Place(read)
            }
            operand => operand,
        }
    }

    /// Whether a by-value read of `place` clones a retained model or class value whose checked derives include Clone.
    ///
    /// Only a path of field projections qualifies; an element or payload read keeps its own representation path.
    fn clones_nominal_field_read(&self, place: &bir::Place, ty: &IncanType) -> bool {
        let name = match ty {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => return false,
        };
        place
            .projection
            .iter()
            .all(|element| matches!(element, bir::PlaceElem::Field { .. }))
            && self.local_nominal_declarations.values().any(|declaration| {
                declaration.name == *name
                    && matches!(
                        declaration.canonical.kind,
                        SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                    )
                    && declaration.derives.iter().any(|derive| derive == "Clone")
            })
    }
}
