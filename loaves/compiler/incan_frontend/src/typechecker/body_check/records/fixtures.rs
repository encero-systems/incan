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
    verify_checked_fixture(source, &mut fields, program, checker, owner)
}

/// Test baseline for an owner that reads one admitted Loaf dependency; the dependency artifact and module identity
/// are installed before the independent cold check, then the ordinary native comparison proves the decoded fields.
#[cfg(test)]
pub(crate) fn verify_fixture_with_loaves(
    source: &str,
    name: &str,
    mut fields: Fields,
    loaves: std::sync::Arc<crate::loaf_index::LoafIndex>,
    module_path: Vec<String>,
) -> Result<Vec<String>, NodeBindingError> {
    let (program, checker, owner) = fixture_with_context(source, name, Some((loaves, module_path)))?;
    verify_checked_fixture(source, &mut fields, program, checker, owner)
}

/// Reconstruct one independently checked owner and compare every native field with its cold checked value.
fn verify_checked_fixture(
    source: &str,
    fields: &mut Fields,
    program: Program,
    checker: TypeChecker,
    owner: Span,
) -> Result<Vec<String>, NodeBindingError> {
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
    compare_checked_body(&expected, &restored.view)?;
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

/// Compare reusable semantic values after removing invocation-local traversal offsets from both independent views.
fn compare_checked_body(expected: &CheckedBodyView, restored: &CheckedBodyView) -> Result<(), NodeBindingError> {
    let mut expected = expected.clone();
    let mut restored = restored.clone();
    Arc::make_mut(&mut expected.local).order = super::super::BodyDiagnosticOrder::default();
    Arc::make_mut(&mut restored.local).order = super::super::BodyDiagnosticOrder::default();
    compare_native(&expected, &restored)
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
    fixture_with_context(source, name, None)
}

/// Independently check one fixture after installing only the explicit package and module context it requires.
fn fixture_with_context(
    source: &str,
    name: &str,
    context: Option<(std::sync::Arc<crate::loaf_index::LoafIndex>, Vec<String>)>,
) -> Result<(Program, TypeChecker, Span), NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let owner = program.declarations.iter().find(|declaration| matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == name)).map(|declaration| declaration.span).ok_or(NodeBindingError::MissingOwner)?;
    let mut checker = TypeChecker::new();
    if let Some((loaves, module_path)) = context {
        checker.set_current_module_path(Some(module_path));
        checker.set_loaves(loaves);
    }
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
    environment: Vec<(String, String, Fields)>,
) -> Result<Fields, NodeBindingError> {
    let (checker, owner) = projected_local_checker(source, name, declaration, environment)?;
    checker
        .produced_local_record(owner)
        .cloned()
        .ok_or_else(|| NodeBindingError::InvalidField("projected local record is absent".into()))
}

/// Build the private checker used by the local-record producer so tests can inspect its publication boundary.
fn projected_local_checker(
    source: &str,
    name: &str,
    declaration: Fields,
    environment: Vec<(String, String, Fields)>,
) -> Result<(TypeChecker, Span), NodeBindingError> {
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
    for (index, (environment_name, identity, fields)) in environment.into_iter().enumerate() {
        install_environment_declaration(&mut checker, source.len(), index, &environment_name, &identity, &fields)?;
    }
    checker.record_type_param_declaration_spans(&program);
    checker.visibility_point = super::super::VisibilityPoint::Collected;
    checker.prepare_program_body_facts();
    if decoded_owner(&checker, &program, &declaration)? != owner {
        return Err(NodeBindingError::BaselineDifference("local declaration input".into()));
    }
    checker.retain_declaration_record(owner, declaration);
    let crate::ast::Declaration::Function(function) = &selected.node else {
        return Err(NodeBindingError::MissingOwner);
    };
    checker.check_reused_function(&program, function, owner);
    Ok((checker, owner))
}

/// Prove a selected declaration remains available when the current revision evaluates its local body record.
#[cfg(test)]
pub(crate) fn selected_declaration_survives_local_projection(source: &str, name: &str) -> Result<(), NodeBindingError> {
    let declaration = fixture_record(source, name, true)?;
    let expected = declaration.clone();
    let (checker, owner) = projected_local_checker(source, name, declaration, Vec::new())?;
    if checker.available_declaration_record(owner) == Some(&expected) {
        return Ok(());
    }
    Err(NodeBindingError::InvalidField(
        "selected declaration record was not retained for publication".into(),
    ))
}

/// Install one authenticated sibling signature without requiring its source declaration in the projected program.
fn install_environment_declaration(
    checker: &mut TypeChecker,
    source_len: usize,
    index: usize,
    name: &str,
    expected_identity: &str,
    fields: &Fields,
) -> Result<(), NodeBindingError> {
    use crate::symbols::{FunctionInfo, Symbol, SymbolKind};

    let header = fields
        .first()
        .filter(|row| row.len() == 5 && row[0] == "header" && row[3] == "declaration")
        .ok_or(NodeBindingError::MissingOwner)?;
    if header[1] != expected_identity {
        return Err(NodeBindingError::WrongOwner);
    }
    let signature = fields
        .iter()
        .find(|row| row.len() >= 4 && row[0] == "signature" && row[2] == "declaration")
        .ok_or_else(|| NodeBindingError::InvalidField("environment declaration signature is absent".into()))?;
    if !fields.iter().any(|row| row == &["status", "", "refused", "false"]) {
        return Err(NodeBindingError::InvalidField(
            "environment declaration is refused".into(),
        ));
    }
    let mut values = signature[3..].iter();
    let is_async = next(&mut values)?
        .parse::<bool>()
        .map_err(|_| NodeBindingError::MissingNode)?;
    let params = read_parameters(&mut values)?;
    let return_type = read_type(&mut values)?;
    if values.next().is_some() {
        return Err(NodeBindingError::InvalidField(
            "environment declaration signature has trailing fields".into(),
        ));
    }
    let start = source_len.saturating_add(index.saturating_mul(2)).saturating_add(1);
    let span = Span::new(start, start.saturating_add(1));
    let symbol_id = checker.symbols.define(Symbol {
        name: name.to_owned(),
        kind: SymbolKind::Function(FunctionInfo {
            params: params.clone(),
            return_type: return_type.clone(),
            is_async,
            type_params: Vec::new(),
            type_param_bounds: std::collections::HashMap::new(),
            type_param_bound_details: std::collections::HashMap::new(),
            emitted_name: None,
        }),
        span,
        scope: 0,
    });
    let canonical = checker
        .symbols
        .identity_of(symbol_id)
        .cloned()
        .ok_or(NodeBindingError::MissingOwner)?;
    let binding = super::super::super::type_info::FunctionBindingInfo {
        is_async,
        params,
        return_type,
        identity: Some(canonical.clone()),
    };
    let stable = checker
        .stable_function_identity(&binding)
        .filter(|identity| identity.render_compact() == expected_identity)
        .ok_or(NodeBindingError::WrongOwner)?;
    checker
        .type_info
        .declarations
        .function_bindings
        .insert(name.to_owned(), binding.clone());
    checker
        .type_info
        .declarations
        .function_bindings_by_span
        .insert(span, binding);
    checker
        .type_info
        .declarations
        .source_identities
        .insert(canonical, stable);
    Ok(())
}

/// Settle one decoded local value in a private checker and return only that owner's final record. Source collection
/// supplies current coordinates and declaration identities; no body check, ambient checker or sibling local value is
/// available to this producer.
pub(crate) fn produce_final_record(
    source: &str,
    name: &str,
    local: Fields,
    environment: Vec<(String, String, Fields)>,
) -> Result<Fields, NodeBindingError> {
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
    for (index, (environment_name, identity, fields)) in environment.into_iter().enumerate() {
        install_environment_declaration(&mut checker, source.len(), index, &environment_name, &identity, &fields)?;
    }
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

/// The local profile proves every unsupported semantic collection empty. Traversal offsets are invocation bindings
/// that settlement reconstructs from owner-relative events, so they never enter a reusable value.
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
