//! Native restoration independently authenticates the retained invalid-graph fault.

use super::super::{BodyLocal, Positions, fixture, project};
use super::{InputFlow, Reader, restore};

/// A graph with an invalid entry restores only as its exact recomputed refusal, never as successful analysis.
#[test]
fn invalid_graph_restore_requires_the_exact_analysis_fault() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def together(left: Borrowed[int], right: Borrowed[int]) -> bool:\n    return left == right\n";
    let (program, checker, owner) = fixture(source, "together")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let fields = project(&checker, &program, owner, false)?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing projected input-flow row")?;
    assert_eq!(checked.get(4).map(String::as_str), Some("checked"));

    let mut reader = Reader {
        positions: &positions,
        values: checked[5..].iter(),
    };
    let graph = reader.graph()?;
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
