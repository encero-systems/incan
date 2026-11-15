//! Native restoration independently authenticates the retained invalid-graph fault.

use std::collections::BTreeMap;

use super::super::{BodyLocal, Positions, fixture};
use super::{InputFlow, Reader, restore};

/// A graph with an invalid entry restores only as its exact recomputed refusal, never as successful analysis.
#[test]
fn invalid_graph_restore_requires_the_exact_analysis_fault() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def together(left: Borrowed[int], right: Borrowed[int]) -> bool:\n    return left == right\n";
    let (program, checker, owner) = fixture(source, "together")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let fields = checker
        .produced_final_record(owner)
        .cloned()
        .ok_or("missing final record")?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing projected input-flow row")?;
    assert_eq!(checked.get(4).map(String::as_str), Some("checked"));

    let mut reader = Reader {
        positions: &positions,
        values: checked[5..].iter(),
    };
    let graph = reader.graph(3)?;
    reader.finish()?;
    let invalid_entry = graph.nodes.len();
    let mut invalid = checked.clone();
    invalid[4] = "invalid".into();
    invalid[6] = invalid_entry.to_string();
    invalid.push("missing entry node".into());
    let mut local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut local, &invalid)?;
    assert!(matches!(
        local.protocols.explicit_input_flow.get(&graph.owner),
        Some(InputFlow::Invalid { graph, fault })
            if graph.entry == invalid_entry && fault.0 == "missing entry node"
    ));

    let mut mismatched = invalid;
    *mismatched.last_mut().ok_or("missing retained fault")? = "unknown loan".into();
    let mut mismatched_local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    assert!(restore(&positions, &mut mismatched_local, &mismatched).is_err());
    assert!(mismatched_local.protocols.explicit_input_flow.is_empty());

    let mut relabeled = checked;
    relabeled[4] = "invalid".into();
    relabeled.push("missing entry node".into());
    let mut relabeled_local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    assert!(restore(&positions, &mut relabeled_local, &relabeled).is_err());
    assert!(relabeled_local.protocols.explicit_input_flow.is_empty());
    Ok(())
}

/// The record retains initializer-to-event authority; changing its index cannot become a checked graph.
#[test]
fn owned_initializer_restore_requires_the_exact_source_event() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(value: Owned[int]) -> int:\n    retained: Owned[int] = value\n    return retained\n";
    let (program, checker, owner) = fixture(source, "keep")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let fields = checker
        .produced_final_record(owner)
        .cloned()
        .ok_or("missing final record")?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing owned initializer graph")?;
    assert_eq!(checked.get(4).map(String::as_str), Some("checked"));
    let mut reader = Reader {
        positions: &positions,
        values: checked[5..].iter(),
    };
    let graph = reader.graph(3)?;
    reader.finish()?;
    let initializer = graph.owned_locals.values().next().ok_or("missing owned initializer")?;
    assert!(initializer.source.is_some());
    assert!(initializer.access_node.is_some());
    let mut restored = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut restored, &checked)?;
    assert!(
        matches!(restored.protocols.explicit_input_flow.get(&graph.owner), Some(InputFlow::Checked { graph, .. }) if graph.owned_locals.len() == 1)
    );

    for missing_source in [false, true] {
        let mut altered = graph.clone();
        let node_count = altered.nodes.len();
        let initializer = altered
            .owned_locals
            .values_mut()
            .next()
            .ok_or("missing altered initializer")?;
        if missing_source {
            initializer.source = None;
        } else {
            initializer.access_node = Some(node_count);
        }
        let mut row = checked[..5].to_vec();
        super::input_flow::project(&positions, &altered, &mut row)?;
        let mut refused = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut refused, &row).is_err());
        assert!(refused.protocols.explicit_input_flow.is_empty());
    }
    Ok(())
}

/// Operation-specific invalid facts retain their exact refusal; neither a successful label nor another fault can
/// authorize missing completion or consumption nodes after restoration.
#[test]
fn operation_dependency_restore_requires_its_exact_analysis_fault() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def observe(value: Borrowed[str]) -> int:\n    let view: Borrowed[str] = value\n    return len(view)\n";
    let (program, checker, owner) = fixture(source, "observe")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let fields = checker
        .produced_final_record(owner)
        .cloned()
        .ok_or("missing final record")?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing observed operation graph")?;
    let mut reader = Reader {
        positions: &positions,
        values: checked[5..].iter(),
    };
    let graph = reader.graph(3)?;
    reader.finish()?;
    assert_eq!(graph.operations.len(), 1);
    for completion in [true, false] {
        let mut altered = graph.clone();
        let missing_node = altered.nodes.len();
        let operation = altered.operations.values_mut().next().ok_or("missing operation")?;
        if completion {
            operation.completion_node = missing_node;
        } else {
            operation.operands.first_mut().ok_or("missing operand")?.final_access = missing_node;
        }
        let fault = crate::typechecker::ownership_contracts::flow::analyze(&altered)
            .err()
            .ok_or("invalid operation unexpectedly analyzed")?;
        let mut row = checked[..5].to_vec();
        super::input_flow::project(&positions, &altered, &mut row)?;
        let mut refused = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut refused, &row).is_err());
        assert!(refused.protocols.explicit_input_flow.is_empty());

        row[4] = "invalid".into();
        row.push(fault.0.into());
        let mut retained = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        restore(&positions, &mut retained, &row)?;
        assert!(matches!(retained.protocols.explicit_input_flow.get(&graph.owner),
            Some(InputFlow::Invalid { graph: restored, fault: actual })
                if actual == &fault && restored.operations.len() == altered.operations.len()));

        *row.last_mut().ok_or("missing operation fault")? = "missing entry node".into();
        let mut mismatched = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut mismatched, &row).is_err());
        assert!(mismatched.protocols.explicit_input_flow.is_empty());
    }
    Ok(())
}

/// Invocation-specific out-of-range links restore only as their exact structural refusal, never as successful
/// input authority or a differently labeled fault.
#[test]
fn invocation_evaluation_restore_requires_its_exact_analysis_fault() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def read(value: Borrowed[int]) -> int:\n    return value\ndef forward(value: Borrowed[int]) -> int:\n    return read(value)\n";
    let (program, checker, owner) = fixture(source, "forward")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let view = checker.checked_body(owner).ok_or("missing invocation body")?;
    let mut fields = Vec::new();
    super::project(&positions, &view.local, &mut fields)?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing invocation graph")?;
    let mut reader = Reader {
        positions: &positions,
        values: checked[5..].iter(),
    };
    let graph = reader.graph(3)?;
    reader.finish()?;
    assert_eq!(graph.invocations.len(), 1);
    let mut positive = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut positive, &checked)?;
    let mut positive_fields = Vec::new();
    super::project(&positions, &positive, &mut positive_fields)?;
    assert_eq!(positive_fields, vec![checked.clone()]);

    for field in 0..5 {
        let mut altered = graph.clone();
        let missing_node = altered.nodes.len();
        let invocation = altered.invocations.values_mut().next().ok_or("missing invocation")?;
        let argument = invocation.arguments.first_mut().ok_or("missing invocation argument")?;
        match field {
            0 => argument.evaluation.start_node = missing_node,
            1 => argument.evaluation.end_node = missing_node,
            2 => argument.evaluation.source_node = Some(missing_node),
            3 => invocation.completion_node = missing_node,
            _ => *invocation.final_accesses.first_mut().ok_or("missing final loan use")? = missing_node,
        }
        let fault = crate::typechecker::ownership_contracts::flow::analyze(&altered)
            .err()
            .ok_or("invalid invocation unexpectedly analyzed")?;
        assert_eq!(fault.0, "invocation event link is outside callable");
        let mut row = checked[..5].to_vec();
        super::input_flow::project(&positions, &altered, &mut row)?;
        let mut refused = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut refused, &row).is_err());
        assert!(refused.protocols.explicit_input_flow.is_empty());
        row[4] = "invalid".into();
        row.push(fault.0.into());
        let mut retained = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        restore(&positions, &mut retained, &row)?;
        let mut restored_fields = Vec::new();
        super::project(&positions, &retained, &mut restored_fields)?;
        assert_eq!(restored_fields, vec![row.clone()]);
        assert!(matches!(retained.protocols.explicit_input_flow.get(&graph.owner),
            Some(InputFlow::Invalid { graph: restored, fault: actual })
                if actual == &fault && restored.invocations.len() == altered.invocations.len()));
        *row.last_mut().ok_or("missing invocation fault")? = "missing entry node".into();
        let mut mismatched = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut mismatched, &row).is_err());
        assert!(mismatched.protocols.explicit_input_flow.is_empty());
    }
    Ok(())
}

/// Checked Copy choices survive independently of graph access tags; empty and duplicate owner maps remain distinct.
#[test]
fn checked_copy_restore_retains_exact_type_decisions() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(value: Owned[int]) -> int:\n    return value\n";
    let (program, checker, owner) = fixture(source, "keep")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let view = checker.checked_body(owner).ok_or("missing Copy owner body")?;
    let mut fields = Vec::new();
    super::project(&positions, &view.local, &mut fields)?;
    let row = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-copy"))
        .ok_or("missing checked Copy decisions")?;
    let mut restored = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut restored, &row)?;
    let mut roundtrip = Vec::new();
    super::project(&positions, &restored, &mut roundtrip)?;
    assert_eq!(roundtrip, vec![row.clone()]);
    assert!(restore(&positions, &mut restored, &row).is_err());
    let mut after_duplicate = Vec::new();
    super::project(&positions, &restored, &mut after_duplicate)?;
    assert_eq!(after_duplicate, roundtrip);

    let prefix = row[..5].to_vec();
    for entries in [
        vec!["2", "str", "false", "int", "true"],
        vec!["2", "int", "true", "int", "false"],
        vec!["1", "int", "maybe"],
        vec!["1", "callable", "00", "int", "false"],
        vec!["2", "int", "true"],
    ] {
        let mut malformed = prefix.clone();
        malformed.extend(entries.into_iter().map(str::to_owned));
        let mut refused = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut refused, &malformed).is_err());
        assert!(refused.protocols.explicit_input_copy.is_empty());
    }
    let mut callable = prefix.clone();
    callable.extend(["1", "callable", "0", "int", "false"].into_iter().map(str::to_owned));
    let mut retained_callable = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut retained_callable, &callable)?;
    let mut callable_fields = Vec::new();
    super::project(&positions, &retained_callable, &mut callable_fields)?;
    assert_eq!(callable_fields, vec![callable]);
    let mut empty = prefix;
    empty.push("0".into());
    let mut retained_empty = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut retained_empty, &empty)?;
    assert_eq!(retained_empty.protocols.explicit_input_copy.len(), 1);
    let mut empty_fields = Vec::new();
    super::project(&positions, &retained_empty, &mut empty_fields)?;
    assert_eq!(empty_fields, vec![empty]);
    Ok(())
}

/// A conflict is an explicit None requirement, not an absent parameter; duplicate or nonparameter keys refuse.
#[test]
fn parameter_requirements_preserve_conflicts_and_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def add(value: int) -> int:\n    return value\ndef use(value: Borrowed[int]) -> int:\n    return add(value)\n";
    let (program, checker, owner) = fixture(source, "use")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let fields = checker
        .produced_final_record(owner)
        .cloned()
        .ok_or("missing final record")?;
    let row = fields
        .iter()
        .find(|row| row.get(2).is_some_and(|selector| selector == "parameter-requirement"))
        .ok_or("missing parameter requirement")?;
    assert_eq!(row.get(4).map(String::as_str), Some("some"));
    for conflict in [false, true] {
        let row = if conflict {
            let mut row = row[..4].to_vec();
            row.push("none".into());
            row
        } else {
            row.clone()
        };
        let mut local = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        super::super::restore_local(&positions, &mut local, &mut BTreeMap::new(), &row)?;
        let requirement = local
            .expressions
            .body_ir_parameter_requirements
            .values()
            .next()
            .ok_or("missing restored requirement")?;
        assert_eq!(requirement.is_none(), conflict);
        let mut restored = Vec::new();
        super::super::project_expressions(&positions, &local, &mut restored)?;
        assert_eq!(restored, vec![row.clone()]);
        assert!(super::super::restore_local(&positions, &mut local, &mut BTreeMap::new(), &row).is_err());
    }
    let mut wrong = row.clone();
    let body = checker.checked_body(owner).ok_or("missing checked owner")?;
    let identity = body
        .local
        .protocols
        .explicit_input_flow
        .keys()
        .next()
        .ok_or("missing callable")?;
    wrong[1] = positions.nodes.anchor(super::super::span_of(identity))?.path;
    wrong[3] = positions.identity(identity)?;
    let mut local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    assert!(super::super::restore_local(&positions, &mut local, &mut BTreeMap::new(), &wrong).is_err());
    assert!(local.expressions.body_ir_parameter_requirements.is_empty());
    Ok(())
}

/// Record types retain nominal versus generic identity, while the current owner profile still refuses generics.
#[test]
fn result_requirement_types_roundtrip_without_widening_record_owners() -> Result<(), Box<dyn std::error::Error>> {
    use crate::symbols::ResolvedType;
    use crate::typechecker::canonical_nodes::NodeBindingError;
    for info in incan_lang::lang::types::numerics::NUMERIC_TYPES {
        let expected = ResolvedType::Numeric(info.id);
        let mut fields = Vec::new();
        super::super::types::type_fields(&expected, &mut fields)?;
        let mut reader = fields.iter();
        assert_eq!(super::super::types::read_type(&mut reader)?, expected);
        assert_eq!(reader.next(), None);
    }
    for leaf in [ResolvedType::Named("E".into()), ResolvedType::TypeVar("E".into())] {
        let expected = ResolvedType::Generic(
            "Result".into(),
            vec![ResolvedType::Unit, ResolvedType::Generic("Option".into(), vec![leaf])],
        );
        let mut fields = Vec::new();
        super::super::types::type_fields(&expected, &mut fields)?;
        let mut reader = fields.iter();
        assert_eq!(super::super::types::read_type(&mut reader)?, expected);
        assert_eq!(reader.next(), None);
        fields.pop();
        assert!(super::super::types::read_type(&mut fields.iter()).is_err());
    }
    let source = "trait Failure[E]:\n    def fail(self, marker: Borrowed[int]) -> Result[None, E]:\n        pass\ndef forward[E, F with Failure[E]](failure: Borrowed[F], marker: Borrowed[int]) -> Result[None, E]:\n    return failure.fail(marker)\n";
    let (program, checker, owner) = fixture(source, "forward")?;
    assert!(matches!(
        Positions::new(&checker, &program, owner, false),
        Err(NodeBindingError::Unsupported("modified, generic, or generated owner"))
    ));
    Ok(())
}
