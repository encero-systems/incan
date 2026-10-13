//! Preserve declaration-bound placeholders while projecting checked types into Body IR.

use super::{
    BodyBuilder, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir, decorated_bindings,
    semantic_type_from_resolved,
};
use incan_semantics_core::IncanCallableParamKind;

/// Preserve the already checked source role independently from its element or packed carrier type.
pub(super) fn source_parameter_kind(kind: ast::ParamKind) -> IncanCallableParamKind {
    match kind {
        ast::ParamKind::Normal => IncanCallableParamKind::Normal,
        ast::ParamKind::RestPositional => IncanCallableParamKind::RestPositional,
        ast::ParamKind::RestKeyword => IncanCallableParamKind::RestKeyword,
    }
}

/// Project checked nominal spellings onto their declaring carrier names, without resolving source syntax again.
///
/// Imported checker types can use package-qualified names while the retained declaration uses its source name.
/// Only checker-minted canonical bindings authorize this projection. Project assembly still refuses equal carrier
/// names from distinct owners, so this does not choose between conflicting declarations.
pub(super) fn checked_semantic_type(ty: &crate::symbols::ResolvedType, facts: &TypeCheckInfo) -> IncanType {
    retain_checked_nominal_names(
        super::runtime_nominals::project_type(semantic_type_from_resolved(ty), facts),
        facts,
    )
}

/// Classify declaration binders before SDK and builtin storage projection so a same-spelled parameter keeps its own
/// category.
pub(super) fn checked_parameter_type(
    ty: &crate::symbols::ResolvedType,
    facts: &TypeCheckInfo,
    parameters: &[String],
) -> IncanType {
    retain_checked_nominal_names(
        super::runtime_nominals::project_type(
            retain_parameter_type(semantic_type_from_resolved(ty), parameters),
            facts,
        ),
        facts,
    )
}

/// Retain signature storage only where both the current module and its checked provider context agree on provenance.
pub(super) fn checked_context_type(
    ty: &crate::symbols::ResolvedType,
    facts: &TypeCheckInfo,
    parameters: &[String],
    conflicts: &std::collections::BTreeSet<String>,
) -> IncanType {
    refuse_runtime_conflicts(checked_parameter_type(ty, facts, parameters), conflicts)
}

/// Collect same-spelled source types from the already checked provider modules, including function-only imports.
pub(super) fn runtime_name_conflicts(context: &[bir::BodyIrModule]) -> std::collections::BTreeSet<String> {
    context
        .iter()
        .flat_map(|module| {
            module
                .nominal_declarations
                .iter()
                .map(|declaration| &declaration.canonical)
                .chain(
                    module
                        .enum_declarations
                        .iter()
                        .map(|declaration| &declaration.canonical),
                )
                .chain(
                    module
                        .fieldless_enum_declarations
                        .iter()
                        .map(|declaration| &declaration.canonical),
                )
                .chain(
                    module
                        .value_enum_declarations
                        .iter()
                        .map(|declaration| &declaration.canonical),
                )
                .chain(
                    module
                        .type_alias_declarations
                        .iter()
                        .map(|declaration| &declaration.canonical),
                )
                .chain(module.trait_declarations.iter())
                .chain(
                    module
                        .bodies
                        .iter()
                        .flat_map(|body| body.named_type_identities.values()),
                )
                .chain(
                    module
                        .nominal_declarations
                        .iter()
                        .flat_map(|declaration| declaration.named_type_identities.values()),
                )
                .chain(
                    module
                        .type_alias_declarations
                        .iter()
                        .flat_map(|declaration| declaration.named_type_identities.values()),
                )
        })
        .filter(|identity| {
            identity.declaration_name == incan_semantics_core::RuntimeBuiltinType::ValidationError.binding_name()
                && incan_semantics_core::RuntimeBuiltinType::from_identity(identity).is_none()
        })
        .map(|identity| identity.declaration_name.clone())
        .collect()
}

/// Refuse an unresolved runtime-name collision in every nested storage position; source nominal aliases stay source
/// types.
fn refuse_runtime_conflicts(ty: IncanType, conflicts: &std::collections::BTreeSet<String>) -> IncanType {
    match ty {
        IncanType::NativeBuiltin(ref builtin) if conflicts.contains(builtin.binding_name()) => {
            IncanType::Unsupported(format!("ambiguous builtin {} type provenance", builtin.binding_name()))
        }
        IncanType::Generic { base, args } => IncanType::Generic {
            base,
            args: args
                .into_iter()
                .map(|argument| refuse_runtime_conflicts(argument, conflicts))
                .collect(),
        },
        IncanType::Tuple(items) => IncanType::Tuple(
            items
                .into_iter()
                .map(|item| refuse_runtime_conflicts(item, conflicts))
                .collect(),
        ),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(refuse_runtime_conflicts(*inner, conflicts))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(refuse_runtime_conflicts(*inner, conflicts))),
        IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(refuse_runtime_conflicts(*inner, conflicts))),
        IncanType::Function {
            mut params,
            return_type,
        } => {
            for parameter in &mut params {
                parameter.ty = refuse_runtime_conflicts(parameter.ty.clone(), conflicts);
            }
            IncanType::Function {
                params,
                return_type: Box::new(refuse_runtime_conflicts(*return_type, conflicts)),
            }
        }
        ty => ty,
    }
}

/// Normalize nested nominal spellings, checked runtime storage and structural union order at the Body IR boundary.
/// Imported metadata may list an equivalent union in another order; retaining one order keeps nested return and
/// parameter positions on the same layout without changing the legacy checker's representation.
fn retain_checked_nominal_names(ty: IncanType, facts: &TypeCheckInfo) -> IncanType {
    match ty {
        IncanType::Named(ref name)
            if facts
                .declarations
                .body_ir_native_type_identities
                .get(name)
                .is_some_and(|identity| {
                    identity.declaration_name == *name
                        && incan_semantics_core::RuntimeBuiltinType::from_identity(identity).is_some()
                }) =>
        {
            if ambiguous_runtime_name(name, facts) {
                IncanType::Unsupported(format!("ambiguous builtin {name} type provenance"))
            } else {
                IncanType::NativeBuiltin(incan_semantics_core::RuntimeBuiltinType::ValidationError)
            }
        }
        IncanType::Named(name) => IncanType::Named(
            facts
                .declarations
                .named_type_identities
                .get(&name)
                .or_else(|| facts.declarations.body_ir_native_type_identities.get(&name))
                .map_or(name.clone(), |identity| identity.declaration_name.clone()),
        ),
        IncanType::Generic { base, args } => {
            let base = facts
                .declarations
                .body_ir_native_type_identities
                .get(&base)
                .map_or(base.clone(), |owner| owner.declaration_name.clone());
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

/// Refuse bare provider leaves that share a builtin's name until exact annotation and signature provenance survives
/// projection.
fn ambiguous_runtime_name(name: &str, facts: &TypeCheckInfo) -> bool {
    facts
        .declarations
        .named_type_identities
        .values()
        .chain(
            facts
                .declarations
                .qualified_type_references
                .values()
                .map(|reference| &reference.identity),
        )
        .any(|identity| {
            identity.declaration_name == name
                && incan_semantics_core::RuntimeBuiltinType::from_identity(identity).is_none()
        })
}

/// Distinguish declared binders from nominal types using the active declaration's retained parameter set.
///
/// Legacy checking can encode an in-scope binder as `Named`; Body IR must retain the placeholder category so
/// executable instance selection substitutes it. This projection does not infer parameters from values or add
/// binders absent from the checked declaration, and leaves the legacy checker's representation unchanged.
pub(super) fn retain_parameter_type(ty: IncanType, parameters: &[String]) -> IncanType {
    match ty {
        IncanType::Named(name) if parameters.contains(&name) => IncanType::TypeVar(name),
        IncanType::RustInteropPath(name) if parameters.contains(&name) => IncanType::TypeVar(name),
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
    /// Consume the checker's exact list/dictionary type for the declared rest binding, without wrapping its element
    /// annotation again. Missing local facts stay Unknown and are refused before execution.
    pub(super) fn checked_rest_parameter_type(&self, parameter: &ast::Spanned<ast::Param>) -> IncanType {
        self.type_info
            .resolved_write_type(parameter.span, &parameter.node.name)
            .map(|ty| self.checked_type(ty))
            .unwrap_or(IncanType::Unknown)
    }

    /// Convert a checked type while retaining the active declaration's explicit placeholder bindings.
    pub(super) fn checked_type(&self, ty: &crate::symbols::ResolvedType) -> IncanType {
        let ty = checked_context_type(ty, self.type_info, &self.type_parameters, self.runtime_type_conflicts);
        if self.shared_method_chain {
            decorated_bindings::planned_type(ty, self.type_info)
        } else {
            ty
        }
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
