//! Preserve declaration-bound placeholders while projecting checked types into Body IR.

use super::{
    BodyBuilder, IncanType, SemanticSourceTargetKind, TypeCheckInfo, ast, bir, decorated_bindings,
    semantic_type_from_resolved,
};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type, type_param_subst_map};
use crate::symbols::ResolvedType;
use incan_lang::lang::derives::{self, DeriveId};
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
/// Only checker-minted canonical bindings authorize this projection. Distinct checked owners sharing a source name
/// retain separate identity-encoded carrier keys instead of collapsing their imported aliases.
pub(super) fn checked_semantic_type(ty: &crate::symbols::ResolvedType, facts: &TypeCheckInfo) -> IncanType {
    let ty = expand_rusttype_aliases(ty, facts);
    retain_checked_nominal_names(
        super::runtime_nominals::project_type(facts.semantic_type(&ty), facts),
        facts,
    )
}

/// Preserve the checked owned ABI of an imported native collection before storage projection erases its provenance.
pub(super) fn native_collection_parameter(ty: &ResolvedType, facts: &TypeCheckInfo) -> bool {
    let ty = expand_rusttype_aliases(ty, facts);
    super::runtime_nominals::native_collection_parameter(&semantic_type_from_resolved(&ty), facts)
}

/// Classify declaration binders before SDK and builtin storage projection so a same-spelled parameter keeps its own
/// category. Classify leaves exposed by structural Rust projection too, so a `Node<T>` display cannot close `T`
/// as a nominal before the declaration's own binders are applied.
pub(super) fn checked_parameter_type(
    ty: &crate::symbols::ResolvedType,
    facts: &TypeCheckInfo,
    parameters: &[String],
) -> IncanType {
    let ty = expand_rusttype_aliases(ty, facts);
    retain_checked_nominal_names(
        retain_parameter_type(
            super::runtime_nominals::project_type(
                facts.qualify_semantic_type(retain_parameter_type(semantic_type_from_resolved(&ty), parameters)),
                facts,
            ),
            parameters,
        ),
        facts,
    )
}

/// Substitute a transparent native alias before nominal source names enter Body IR. These declarations add no
/// wrapper layout or constructor; the metadata-selected Rust owner supplies their complete representation.
fn expand_rusttype_aliases(ty: &crate::symbols::ResolvedType, facts: &TypeCheckInfo) -> crate::symbols::ResolvedType {
    map_resolved_type_leaves(ty, &|leaf| {
        let (name, arguments) = match leaf {
            ResolvedType::Named(name) => (name, &[][..]),
            ResolvedType::Generic(name, arguments) => (name, arguments.as_slice()),
            _ => return None,
        };
        let (parameters, template) = facts.rust.type_aliases.get(name)?;
        Some(substitute_resolved_type(
            template,
            &type_param_subst_map(parameters, arguments),
        ))
    })
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

/// Collect same-spelled source types from checked provider declarations, including compiled function-only imports.
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
                        .callable_declaration_frames()
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
        IncanType::NativeArray { element, length } => IncanType::NativeArray {
            element: Box::new(refuse_runtime_conflicts(*element, conflicts)),
            length,
        },
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
        IncanType::Named(name) => IncanType::Named(name),
        IncanType::Generic { base, args } => {
            let base = facts
                .declarations
                .body_ir_native_type_identities
                .get(&base)
                .map_or(base.clone(), |owner| owner.declaration_name.clone());
            // A provider-qualified Incan base is a checked nominal, not a Rust item whose crate is named `pub`.
            let base = if crate::typechecker::split_canonical_public_library_type_name(&base).is_some() {
                match facts.qualify_semantic_type(IncanType::Named(base)) {
                    IncanType::Named(name) => name,
                    refused => return refused,
                }
            } else {
                base
            };
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
        IncanType::NativeArray { element, length } => IncanType::NativeArray {
            element: Box::new(retain_checked_nominal_names(*element, facts)),
            length,
        },
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

/// Copy checked carrier bindings and preserve unambiguous short names selected by standard storage projection.
/// A projected name carries its original canonical owner; conflicting owners never grant a short binding.
/// Native paths from structural storage add their declaration identities without replacing a source binding.
pub(super) fn retained_type_identities(
    facts: &TypeCheckInfo,
) -> std::collections::BTreeMap<String, super::CanonicalSymbolId> {
    let mut identities = facts.declarations.named_type_identities.clone();
    // Standard carrier projection uses the declaration's short native name. Preserve the checked binding of that
    // projected name too, including signature-only imports; two different owners never authorize a short alias.
    let mut projected = std::collections::BTreeMap::<String, Option<super::CanonicalSymbolId>>::new();
    for identity in identities.values() {
        let name = &identity.declaration_name;
        let candidate = projected.entry(name.clone()).or_insert_with(|| Some(identity.clone()));
        if candidate.as_ref() != Some(identity) {
            *candidate = None;
        }
    }
    for (name, identity) in projected {
        if let Some(identity) = identity
            && matches!(&identity.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library.starts_with("incan_stdlib_"))
        {
            identities.entry(name).or_insert(identity);
        }
    }
    for storage in facts.rust.storage_types.values() {
        if let IncanType::Named(name) = storage
            && let Some(identity) = bir::rust_item_identity(name)
        {
            identities.entry(name.clone()).or_insert(identity);
        }
    }
    for call in facts.rust.checked_calls.values().chain(
        facts
            .rust
            .iteration_hooks
            .values()
            .flat_map(|hooks| [&hooks.iter, &hooks.next]),
    ) {
        for call in call.receiver_adjustments.iter().chain(std::iter::once(call)) {
            if let Some(storage) = &call.storage {
                identities.extend(storage.identities.clone());
            }
        }
    }
    identities
}

/// Read the checker-retained owner of a source binding or carrier key without decoding or rebuilding a name.
pub(super) fn retained_nominal_identity<'facts>(
    name: &str,
    facts: &'facts TypeCheckInfo,
) -> Option<&'facts super::CanonicalSymbolId> {
    facts.declarations.named_type_identities.get(name)
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
        IncanType::NativeArray { element, length } => IncanType::NativeArray {
            element: Box::new(retain_parameter_type(*element, parameters)),
            length,
        },
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

    /// Retain active declaration binders and the exact checked project carrier before normalized aliases lose their
    /// owner.
    pub(super) fn checked_type(&self, ty: &crate::symbols::ResolvedType) -> IncanType {
        let projected = checked_context_type(ty, self.type_info, &self.type_parameters, self.runtime_type_conflicts);
        let mut projected = if self.shared_method_chain {
            decorated_bindings::planned_type(projected, self.type_info)
        } else {
            projected
        };
        if self.retain_nominal_carriers {
            super::nominal_carriers::retain_checked_type(&mut projected, ty, self.type_info);
        }
        projected
    }

    /// Retain an owned value read from a field at a return or by-value argument boundary.
    ///
    /// A field projection cannot move from its owner, which is often borrowed. A value position clones an open
    /// parameter field or a checked Clone nominal field, including an enum field. Formatting and mutable argument
    /// boundaries retain their distinct borrow facts.
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

    /// Whether a by-value field read clones its checker-selected nominal owner, whose retained derives include Clone.
    ///
    /// Only a path of field projections qualifies; an element or payload read keeps its own representation path.
    fn clones_nominal_field_read(&self, place: &bir::Place, ty: &IncanType) -> bool {
        let name = match ty {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => return false,
        };
        if !place
            .projection
            .iter()
            .all(|element| matches!(element, bir::PlaceElem::Field { .. }))
        {
            return false;
        }
        let Some(identity) = retained_nominal_identity(name, self.type_info) else {
            return false;
        };
        self.local_nominal_declarations.values().any(|declaration| {
            declaration.canonical == *identity
                && matches!(
                    declaration.canonical.kind,
                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                )
                && declaration
                    .derives
                    .iter()
                    .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
        }) || self.local_enum_declarations.get(identity).is_some_and(|declaration| {
            declaration
                .derives
                .iter()
                .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
        })
    }
}
