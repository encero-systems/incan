//! Projection of checked body facts into record fields and their validation.

use super::*;

/// Emit one declaration record at the declaration producer's settlement boundary. Unsupported nonempty native
/// fields refuse this profile; no later whole-check observer can manufacture a declaration record.
pub(crate) fn produce_declaration(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
) -> Result<Fields, NodeBindingError> {
    let positions = Positions::new(checker, program, owner, true)?;
    let view = checker
        .checked_declaration(owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    validate(view)?;
    let mut fields = local_fields(&positions, &view.local)?;
    ownership::project(checker, owner, &positions, &mut fields)?;
    project_signature(checker, program, owner, &positions, &mut fields)?;
    project_diagnostics(checker, program, &positions, view, &mut fields)?;
    project_final_types(checker, view, &positions, &mut fields)?;
    fields.push(vec![
        "status".into(),
        String::new(),
        "refused".into(),
        view.is_refused().to_string(),
    ]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            "declaration".into(),
            "incan.checker-free-functions/11".into(),
        ],
    );
    Ok(fields)
}

/// Emit one final-family value from the exact settled view supplied by its owner operation. The Incan schema assigns
/// the family tag; native output contains only owner/root/profile metadata and canonical field primitives.
pub(crate) fn produce_final(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    view: &CheckedBodyView,
) -> Result<Fields, NodeBindingError> {
    let positions = Positions::for_local(checker, program, owner, &view.local)
        .map_err(|error| final_boundary_error("positions", error))?;
    encode_final(checker, program, owner, view, &positions)
}

/// Encode one settled statement owner with activation-owned identity bindings. The caller must have produced the
/// supplied view by settling exactly this owner's decoded local input.
pub(super) fn encode_final(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    view: &CheckedBodyView,
    positions: &Positions,
) -> Result<Fields, NodeBindingError> {
    if view.owner() != owner {
        return Err(NodeBindingError::WrongOwner);
    }
    validate(view).map_err(|error| final_boundary_error("validation", error))?;
    let mut fields =
        local_fields(positions, &view.local).map_err(|error| final_boundary_error("local fields", error))?;
    ownership::project(checker, owner, positions, &mut fields)
        .map_err(|error| final_boundary_error("ownership", error))?;
    project_diagnostics(checker, program, positions, view, &mut fields)
        .map_err(|error| final_boundary_error("diagnostics", error))?;
    project_final_types(checker, view, positions, &mut fields).map_err(|error| final_boundary_error("types", error))?;
    fields.push(vec![
        "status".into(),
        String::new(),
        "refused".into(),
        view.is_refused().to_string(),
    ]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            "incan.checker-free-functions/11".into(),
        ],
    );
    Ok(fields)
}

/// Retain the failed final-family boundary when an authenticated owner lacks one required identity binding.
fn final_boundary_error(boundary: &str, error: NodeBindingError) -> NodeBindingError {
    if error == NodeBindingError::MissingOwner {
        return NodeBindingError::InvalidField(format!("final {boundary} is missing an owner binding"));
    }
    error
}

/// References keep their stable target and a structural source role; parameter writes stay declaration-owned.
pub(super) fn project_references(
    positions: &Positions,
    local: &BodyLocal,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, canonical) in &local.references.resolved_identities {
        fields.push(vec![
            "reference".into(),
            positions.nodes.anchor(*span)?.path,
            "read".into(),
            positions.identity(canonical)?,
        ]);
    }
    for ((span, name), canonical) in &local.references.resolved_write_identities {
        fields.push(vec![
            "write".into(),
            positions.nodes.anchor(*span)?.path,
            name.clone(),
            positions.identity(canonical)?,
        ]);
    }
    for ((span, name), ty) in &local.references.resolved_write_types {
        let mut row = vec!["parameter".into(), positions.nodes.anchor(*span)?.path, name.clone()];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    Ok(())
}

/// Calls retain the selected builtin and checked parameter/type arguments, not a re-resolved source name.
pub(super) fn project_calls(
    positions: &Positions,
    local: &BodyLocal,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, builtin) in &local.calls.resolved_builtin_calls {
        fields.push(vec![
            "builtin".into(),
            positions.nodes.anchor(*span)?.path,
            "call".into(),
            incan_lang::lang::builtins::as_str(*builtin).into(),
        ]);
    }
    for (span, builtin) in &local.calls.resolved_builtin_method_calls {
        fields.push(vec![
            "builtin".into(),
            positions.nodes.anchor(*span)?.path,
            "method".into(),
            builtin.owner().into(),
            builtin.member().into(),
        ]);
    }
    for (selector, values) in [
        ("call", &local.calls.call_site_callable_params),
        ("body-call", &local.calls.body_ir_call_parameter_types),
    ] {
        for (span, parameters) in values {
            let mut row = vec!["signature".into(), positions.nodes.anchor(*span)?.path, selector.into()];
            parameter_fields(parameters, &mut row)?;
            fields.push(row);
        }
    }
    for (selector, values) in [
        ("monomorphs", &local.calls.call_site_monomorph_type_args),
        ("type-arguments", &local.calls.body_ir_call_type_arguments),
    ] {
        for (span, arguments) in values {
            let mut row = vec![
                "signature".into(),
                positions.nodes.anchor(*span)?.path,
                selector.into(),
                arguments.len().to_string(),
            ];
            for ty in arguments {
                type_fields(ty, &mut row)?;
            }
            fields.push(row);
        }
    }
    if !local.calls.body_ir_call_result_types.is_empty() || !local.calls.body_ir_native_argument_types.is_empty() {
        return Err(NodeBindingError::Unsupported("native call refinements"));
    }
    Ok(())
}

/// Every unrepresented family is explicitly refused before a canonical value can escape.
pub(super) fn validate(view: &CheckedBodyView) -> Result<(), NodeBindingError> {
    if !view.effect_reads.is_empty()
        || !view.mutation.is_empty()
        || !view.dictionary.is_empty()
        || !view.open.is_empty()
        || !view.local.iterator_sum_items.is_empty()
        || !view.local.dictionary.is_empty()
        || !view.local.diagnostic_slots.is_empty()
        || !view.local.warning_slots.is_empty()
        || view.local.order.mutation != 0
        || view.local.order.dictionary != 0
    {
        return Err(NodeBindingError::Unsupported("deferred effect or diagnostic ordering"));
    }
    Ok(())
}

/// Extract the exact authored declaration range from its current activation handle.
pub(super) fn span_of(canonical: &CanonicalSymbolId) -> Span {
    Span {
        start: canonical.declaration_span.start,
        end: canonical.declaration_span.end,
        expansion: canonical.declaration_span.expansion,
    }
}

/// Declaration-owned source signatures and exact default-writer references are independent from statement outputs.
pub(super) fn project_signature(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    positions: &Positions,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let binding = checker
        .declaration_view()
        .function(owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    let mut row = vec![
        "signature".into(),
        positions.nodes.anchor(owner)?.path,
        "declaration".into(),
        binding.is_async.to_string(),
    ];
    parameter_fields(&binding.params, &mut row)?;
    type_fields(&binding.return_type, &mut row)?;
    fields.push(row);
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| declaration.span == owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    if let crate::ast::Declaration::Function(function) = &declaration.node {
        for parameter in &function.params {
            if let Some(default) = &parameter.node.default {
                let selected = checker
                    .checked_declaration(owner)
                    .ok_or(NodeBindingError::MissingOwner)?;
                if !selected.expressions().expr_types.contains_key(&default.span) {
                    return Err(NodeBindingError::Unsupported("default writer"));
                }
                fields.push(vec![
                    "default".into(),
                    positions.nodes.anchor(default.span)?.path,
                    parameter.node.name.clone(),
                    positions.nodes.owner().render_compact(),
                ]);
            }
        }
    }
    Ok(())
}

/// Select the same final expression/binding projections as G3's native publication adapter, without its broad copy.
pub(super) fn final_type_values(checker: &TypeChecker, view: &CheckedBodyView) -> FinalTypes {
    let expressions = view
        .expressions()
        .expr_types
        .keys()
        .filter_map(|span| {
            checker
                .type_info
                .expressions
                .body_ir_expr_types
                .get(span)
                .map(|ty| (*span, ty.clone()))
        })
        .collect();
    let bindings = view
        .references()
        .resolved_write_identities
        .values()
        .chain(view.references().resolved_identities.values())
        .filter_map(|canonical| {
            checker
                .type_info
                .expressions
                .body_ir_binding_types
                .get(canonical)
                .map(|ty| (canonical.clone(), ty.clone()))
        })
        .collect();
    FinalTypes { expressions, bindings }
}

/// Publish the selected finalized empty-list/value types and an explicitly witnessed empty native-call capability.
/// A nonempty native-call field already fails the exhaustive local inventory until its typed codec is supported.
pub(super) fn project_final_types(
    checker: &TypeChecker,
    view: &CheckedBodyView,
    positions: &Positions,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let values = final_type_values(checker, view);
    if values.expressions.is_empty() && values.bindings.is_empty() {
        fields.push(vec!["final-type".into(), String::new(), "empty".into(), "true".into()]);
    }
    for (span, ty) in &values.expressions {
        let mut row = vec![
            "final-type".into(),
            positions.nodes.anchor(*span)?.path,
            "expression".into(),
        ];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    for (canonical, ty) in &values.bindings {
        let mut row = vec!["final-type".into(), positions.identity(canonical)?, "binding".into()];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    fields.push(vec!["native-call".into(), String::new(), "empty".into(), "true".into()]);
    Ok(())
}
