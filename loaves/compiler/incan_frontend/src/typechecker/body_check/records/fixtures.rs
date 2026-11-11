//! Record fixtures for checked declarations and body-owned producers.

use super::*;

/// Test-sink entry: check a fixture, select one owner, return only its canonical primitives and drop the checker.
/// This measures the production declaration record without selecting it for reuse.
pub(crate) fn fixture_record(source: &str, name: &str, declaration: bool) -> Result<Fields, NodeBindingError> {
    let (_, checker, owner) = fixture(source, name)?;
    if declaration {
        return checker
            .produced_declaration_records
            .get(&owner)
            .cloned()
            .ok_or_else(|| NodeBindingError::InvalidField("fixture declaration record is absent".into()));
    }
    match checker.produced_final_record(owner) {
        Some(fields) => Ok(fields.clone()),
        None => Err(checker
            .final_record_refusal(owner)
            .unwrap_or_else(|| NodeBindingError::InvalidField("fixture final record is absent".into()))),
    }
}

/// Test baseline: independently check the current revision, retain its coordinate bindings, reconstruct the supplied
/// fields and compare every native field. The cold checker supplies expected values only, never reconstructed ones.
pub(crate) fn verify_fixture(source: &str, name: &str, mut fields: Fields) -> Result<Vec<String>, NodeBindingError> {
    let (program, checker, owner) = fixture(source, name)?;
    let header = fields.first_mut().ok_or(NodeBindingError::MissingOwner)?;
    if header.is_empty() || header[0] != "header" {
        return Err(NodeBindingError::MissingOwner);
    }
    let declaration = if header.len() == 5 {
        match header[3].as_str() {
            "declaration" => true,
            "final" => false,
            _ => return Err(NodeBindingError::Unsupported("record stage")),
        }
    } else if header.len() == 4 {
        header.insert(3, "final".into());
        false
    } else {
        return Err(NodeBindingError::MissingOwner);
    };
    let expected = if declaration {
        checker.checked_declaration(owner)
    } else {
        checker.checked_body(owner)
    }
    .cloned()
    .ok_or(NodeBindingError::MissingOwner)?;
    let positions = Positions::new(&checker, &program, owner, declaration)?;
    let expected_binding = checker.declaration_view().function(owner).cloned();
    let expected_types = final_type_values(&checker, &expected);
    let expected_ownership = expected_binding
        .as_ref()
        .and_then(|binding| binding.identity.as_ref())
        .and_then(|identity| checker.type_info.declarations.ownership_contracts.get(identity))
        .cloned();
    drop(checker);
    let restored = restore(&positions, &program, owner, &fields)?;
    compare_native(&expected, &restored.view)?;
    if restored.ownership != expected_ownership {
        return Err(NodeBindingError::BaselineDifference("ownership contract".into()));
    }
    if let (Some(contract), Some(binding)) = (&restored.ownership, &expected_binding) {
        contract
            .validate(binding.params.len(), false)
            .map_err(|message| NodeBindingError::InvalidField(message.into()))?;
    }
    if declaration && restored.binding != expected_binding {
        return Err(NodeBindingError::BaselineDifference("declaration signature".into()));
    }
    if declaration && restored.defaults != default_writers(&program, owner)? {
        return Err(NodeBindingError::BaselineDifference("selected default writers".into()));
    }
    if expected_types != restored.final_types {
        return Err(NodeBindingError::BaselineDifference(
            "finalized own type projections".into(),
        ));
    }
    let mut current = DiagnosticPositions::new(&program, positions.nodes.owner().origin.clone());
    restored
        .view
        .diagnostics
        .iter()
        .chain(&restored.view.warnings)
        .map(|diagnostic| {
            let error = current.render(diagnostic).map_err(|_| NodeBindingError::MissingNode)?;
            Ok(crate::diagnostics::format_error("case.incn", source, &error))
        })
        .collect()
}

/// Default writer membership comes from exact source default nodes; it never supplies a checked type or value.
pub(super) fn default_writers(
    program: &Program,
    owner: Span,
) -> Result<BTreeMap<Span, (Span, String)>, NodeBindingError> {
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| declaration.span == owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err(NodeBindingError::Unsupported("default owner"));
    };
    Ok(function
        .params
        .iter()
        .filter_map(|parameter| {
            parameter
                .node
                .default
                .as_ref()
                .map(|default| (default.span, (owner, parameter.node.name.clone())))
        })
        .collect())
}

/// Independent full checking belongs only to the fixture baseline. Canonical production conversion never invokes it.
pub(super) fn fixture(source: &str, name: &str) -> Result<(Program, TypeChecker, Span), NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let owner = program.declarations.iter().find(|declaration| matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == name)).map(|declaration| declaration.span).ok_or(NodeBindingError::MissingOwner)?;
    let mut checker = TypeChecker::new();
    let _ = checker.check_program(&program);
    Ok((program, checker, owner))
}

/// Evaluate one local owner from decoded declaration/environment values in a private checker. Declaration collection
/// supplies only current source coordinates and headers; no whole-program check, settled body or ambient checker is
/// available to the producer.
pub(crate) fn produce_local_record(
    source: &str,
    name: &str,
    declaration: Fields,
    environment: Vec<Fields>,
) -> Result<Fields, NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let selected = program
        .declarations
        .iter()
        .find(|item| matches!(&item.node, crate::ast::Declaration::Function(function) if function.name == name))
        .ok_or(NodeBindingError::MissingOwner)?;
    let owner = selected.span;
    let mut checker = TypeChecker::new();
    checker.reset_program_check(&program, false);
    checker.collect_program_check_declarations(&program);
    checker.record_program_header_facts();
    checker.record_source_declaration_identities(&program);
    checker.record_type_param_declaration_spans(&program);
    checker.visibility_point = super::super::VisibilityPoint::Collected;
    checker.prepare_program_body_facts();
    for fields in environment {
        let environment_owner = decoded_owner(&checker, &program, &fields)?;
        if environment_owner == owner {
            return Err(NodeBindingError::BaselineDifference(
                "local declaration appears in environment".into(),
            ));
        }
        checker.retain_declaration_record(environment_owner, fields);
    }
    if decoded_owner(&checker, &program, &declaration)? != owner {
        return Err(NodeBindingError::BaselineDifference("local declaration input".into()));
    }
    checker.retain_declaration_record(owner, declaration);
    checker.check_environment_record_declaration(&program, selected);
    checker
        .produced_local_record(owner)
        .cloned()
        .ok_or(NodeBindingError::MissingOwner)
}

/// Settle one decoded local value in a private checker and return only that owner's final record. Source collection
/// supplies current coordinates and declaration identities; no body check, ambient checker or sibling local value is
/// available to this producer.
pub(crate) fn produce_final_record(source: &str, name: &str, local: Fields) -> Result<Fields, NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let owner = program
        .declarations
        .iter()
        .find(|item| matches!(&item.node, crate::ast::Declaration::Function(function) if function.name == name))
        .map(|item| item.span)
        .ok_or(NodeBindingError::MissingOwner)?;
    let mut checker = TypeChecker::new();
    checker.reset_program_check(&program, false);
    checker.collect_program_check_declarations(&program);
    checker.record_program_header_facts();
    checker.record_source_declaration_identities(&program);
    checker.record_type_param_declaration_spans(&program);
    checker.visibility_point = super::super::VisibilityPoint::Collected;
    checker.prepare_program_body_facts();
    let positions = Positions::for_decoded_local(&checker, &program, owner, &local)?;
    let restored = restore_pending(&positions, owner, &local)?;
    checker.body_checks.push(BodyCheckInput::Local(restored));
    checker.settle_body_checks(&program);
    let view = checker.checked_body(owner).ok_or(NodeBindingError::MissingOwner)?;
    encode_final(&checker, &program, owner, view, &positions)
}

/// Resolve one decoded declaration record to the current revision's source handle without reading checked payloads.
fn decoded_owner(checker: &TypeChecker, program: &Program, fields: &Fields) -> Result<Span, NodeBindingError> {
    let identity = fields
        .first()
        .filter(|header| header.len() == 5 && header[0] == "header" && header[3] == "declaration")
        .map(|header| header[1].as_str())
        .ok_or(NodeBindingError::MissingOwner)?;
    for declaration in &program.declarations {
        let Some(binding) = checker.declaration_view().function(declaration.span) else {
            continue;
        };
        let Some(canonical) = binding.identity.as_ref() else {
            continue;
        };
        if checker
            .declaration_view()
            .source_identity(canonical)
            .is_some_and(|stable| stable.render_compact() == identity)
        {
            return Ok(declaration.span);
        }
    }
    Err(NodeBindingError::MissingOwner)
}

/// The initial local profile proves every unsupported collection empty and refuses traversal-indexed decisions.
pub(super) fn validate_local(local: &BodyLocal) -> Result<(), NodeBindingError> {
    let BodyLocal {
        owner: _,
        kind: _,
        order: _,
        nested: _,
        expressions: _,
        references: _,
        calls: _,
        protocols: _,
        rust: _,
        traits: _,
        c_abi: _,
        mutation: _,
        dictionary: _,
        iterator_sum_items: _,
        diagnostic_slots: _,
        warning_slots: _,
    } = local;
    unsupported::expression_artifacts(&local.expressions)?;
    unsupported::reference_artifacts(&local.references)?;
    unsupported::call_artifacts(&local.calls)?;
    unsupported::protocol_artifacts(&local.protocols)?;
    unsupported::rust_interop_artifacts(&local.rust)?;
    unsupported::trait_artifacts(&local.traits)?;
    unsupported::cabi_interop_artifacts(&local.c_abi)?;
    if !local.iterator_sum_items.is_empty()
        || !local.dictionary.is_empty()
        || !local.diagnostic_slots.is_empty()
        || !local.warning_slots.is_empty()
        || local.order.mutation != 0
        || local.order.dictionary != 0
    {
        return Err(NodeBindingError::Unsupported("pending local decisions"));
    }
    Ok(())
}

/// One conversion path owns all local field primitives, independently of whether settlement has completed.
pub(super) fn local_fields(positions: &Positions, local: &BodyLocal) -> Result<Fields, NodeBindingError> {
    validate_local(local)?;
    let mut fields = Vec::new();
    project_expressions(positions, local, &mut fields)?;
    project_references(positions, local, &mut fields)?;
    project_calls(positions, local, &mut fields)?;
    inputs::project(positions, local, &mut fields)?;
    for canonical in local.nested.values() {
        fields.push(vec![
            "binding".into(),
            positions.nodes.anchor(span_of(canonical))?.path,
            "declaration".into(),
            positions.identity(canonical)?,
        ]);
    }
    mutation::project(positions, &local.mutation, &mut fields)?;
    Ok(fields)
}
