//! Record projection fixtures for checked declarations and local bodies.

use super::*;

/// Test-sink entry: check a fixture, select one owner, return only its canonical primitives and drop the checker.
/// This intentionally measures no production reuse; a future record producer calls `project` at its own boundary.
pub(crate) fn fixture_projection(source: &str, name: &str, declaration: bool) -> Result<Fields, NodeBindingError> {
    let (program, checker, owner) = fixture(source, name)?;
    project(&checker, &program, owner, declaration)
}

/// Test baseline: independently check the current revision, retain its coordinate bindings, reconstruct the supplied
/// fields and compare every native field. The cold checker supplies expected values only, never reconstructed ones.
pub(crate) fn verify_fixture(source: &str, name: &str, fields: Fields) -> Result<Vec<String>, NodeBindingError> {
    let (program, checker, owner) = fixture(source, name)?;
    let declaration = fields
        .first()
        .is_some_and(|header| header.get(3).is_some_and(|role| role == "declaration"));
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
    let NativeOwner::Settled(restored) = restore(&positions, &program, owner, &fields)? else {
        return Err(NodeBindingError::MissingNode);
    };
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

/// Capture the actual pre-settlement statement output, not a relabeled final body. Unsupported pending capabilities
/// refuse explicitly until the codec represents their decisions and references.
pub(crate) fn fixture_local_projection(source: &str, name: &str) -> Result<Fields, NodeBindingError> {
    let (program, checker, owner) = local_fixture(source, name)?;
    let local = checker
        .body_checks
        .iter()
        .map(|input| input.local())
        .find(|local| local.owner == owner && local.kind == LocalOwnerKind::Body)
        .ok_or(NodeBindingError::MissingOwner)?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let mut fields = local_fields(&positions, local)?;
    fields.push(vec!["status".into(), String::new(), "pending".into(), "true".into()]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            "local".into(),
            "incan.checker-free-functions/9".into(),
        ],
    );
    Ok(fields)
}

/// Rebuild a pending local owner and compare every native field to an independent pre-settlement cold baseline.
pub(crate) fn verify_local_fixture(source: &str, name: &str, fields: Fields) -> Result<(), NodeBindingError> {
    let (program, checker, owner) = local_fixture(source, name)?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let expected = checker
        .body_checks
        .iter()
        .map(|input| input.local())
        .find(|local| local.owner == owner && local.kind == LocalOwnerKind::Body)
        .cloned()
        .ok_or(NodeBindingError::MissingOwner)?;
    drop(checker);
    let NativeOwner::Local(local) = restore(&positions, &program, owner, &fields)? else {
        return Err(NodeBindingError::MissingNode);
    };
    compare_native(&expected, &local)
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
