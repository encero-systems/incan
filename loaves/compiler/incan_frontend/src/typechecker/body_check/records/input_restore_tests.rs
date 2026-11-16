//! Native restoration independently authenticates the retained invalid-graph fault.

use std::collections::BTreeMap;

use super::super::{BodyLocal, Positions, fixture};
use super::{InputFlow, Reader, restore};

/// A checker-authenticated expression nominal remains available to its canonical record even when no lexical symbol
/// owns it.
#[test]
fn checked_expression_nominal_admits_its_record_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def view() -> int:\n    return 1\n";
    let (program, checker, owner) = fixture(source, "view")?;
    let identity = incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
        origin: incan_semantics_core::SymbolOrigin::Package {
            library: "incan_platform".into(),
            module_path: vec!["fs".into()],
        },
        declaration_name: "Path".into(),
        kind: incan_semantics_core::SemanticSourceTargetKind::Rusttype,
        scope_discriminant: None,
        declaration_span: crate::ast::Span::new(0, 1).into(),
    };
    let mut local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    local
        .expressions
        .expression_type_identities
        .insert(owner, BTreeMap::from([(Vec::new(), identity.clone())]));
    let expected = incan_semantics_core::stable_identity::StableDeclarationId::from_canonical(&identity, None, None)
        .ok_or("missing stable package identity")?
        .render_compact();
    let positions = Positions::for_local(&checker, &program, owner, &local)?;
    assert_eq!(positions.type_identity(&identity)?, expected);
    Ok(())
}

/// Structural checked types keep their enum shape instead of falling back to a native Debug spelling.
#[test]
fn structural_checked_type_variants_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    use crate::symbols::ResolvedType;

    let values = vec![
        ResolvedType::Never,
        ResolvedType::FrozenStr,
        ResolvedType::FrozenBytes,
        ResolvedType::FrozenList(Box::new(ResolvedType::Int)),
        ResolvedType::FrozenDict(Box::new(ResolvedType::Str), Box::new(ResolvedType::Int)),
        ResolvedType::FrozenSet(Box::new(ResolvedType::Bool)),
        ResolvedType::TypeToken(Box::new(ResolvedType::Named("Item".into()))),
        ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str]),
        ResolvedType::SelfType,
        ResolvedType::Ref(Box::new(ResolvedType::Int)),
        ResolvedType::RefMut(Box::new(ResolvedType::Int)),
        ResolvedType::CallSiteInfer,
    ];
    for expected in values {
        let mut fields = Vec::new();
        super::super::type_fields(&expected, &mut fields)?;
        let mut encoded = fields.iter();
        assert_eq!(super::super::read_type(&mut encoded)?, expected);
        assert!(encoded.next().is_none());
    }
    Ok(())
}

/// A checked native result type survives the record boundary without falling back to its source-facing type.
#[test]
fn native_call_refinement_types_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def view(value: int) -> int:\n    return value\n";
    let (program, checker, owner) = fixture(source, "view")?;
    let mut local = checker
        .checked_body(owner)
        .ok_or("missing checked body")?
        .local
        .as_ref()
        .clone();
    let span = *local
        .expressions
        .expr_types
        .keys()
        .next()
        .ok_or("missing checked expression")?;
    let native = crate::symbols::ResolvedType::Int;
    local.calls.body_ir_call_result_types.insert(span, native.clone());
    local.calls.body_ir_native_argument_types.insert(span, native.clone());
    let positions = Positions::for_local(&checker, &program, owner, &local)?;
    let fields = super::super::local_fields(&positions, &local)?;
    let rows = fields.iter().filter(|row| row[0] == "native-call").collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    let mut restored = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    for row in rows {
        super::super::restore_local(&positions, &mut restored, &mut BTreeMap::new(), row)?;
    }
    assert_eq!(restored.calls.body_ir_call_result_types.get(&span), Some(&native));
    assert_eq!(restored.calls.body_ir_native_argument_types.get(&span), Some(&native));
    Ok(())
}

/// A local Rust-derived value retains its exact native carrier independently of its projected source type.
#[test]
fn native_value_types_roundtrip_by_local_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(value: int) -> int:\n    retained = value\n    return retained\n";
    let (program, checker, owner) = fixture(source, "keep")?;
    let mut local = checker
        .checked_body(owner)
        .ok_or("missing checked body")?
        .local
        .as_ref()
        .clone();
    let identity = local
        .references
        .resolved_write_identities
        .iter()
        .find(|((_, name), _)| name == "retained")
        .map(|(_, identity)| identity.clone())
        .ok_or("missing retained local identity")?;
    let native = crate::symbols::ResolvedType::RustPath("serde_json::Value".into());
    local.rust.value_native_types.insert(identity.clone(), native.clone());
    let positions = Positions::for_local(&checker, &program, owner, &local)?;
    let fields = super::super::local_fields(&positions, &local)?;
    let row = fields
        .iter()
        .find(|row| row[0] == "native-value" && row[2] == "type")
        .ok_or("missing native value type row")?;
    let mut restored = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    super::super::restore_local(&positions, &mut restored, &mut BTreeMap::new(), row)?;
    assert_eq!(restored.rust.value_native_types.get(&identity), Some(&native));
    Ok(())
}

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
    let graph = reader.graph(5)?;
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
    let graph = reader.graph(5)?;
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

/// Schema 5 retains an immutable parameter snapshot and every exact constructor materialization event. Removing the
/// authority changes the bytes; restoring and reprojecting preserves them and current checked facts still accept the
/// body. Schema 4 remains readable but has no snapshot authority.
#[test]
fn parameter_snapshot_authority_roundtrips_only_in_schema_five()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "model Field:\n    owner: str\n    root: str\ndef keep(value: str) -> str:\n    return value\ndef collect(seed: Borrowed[int], rows: list[list[str]], other: list[list[str]], owned: Owned[list[list[str]]]) -> list[Field]:\n    header = rows[0]\n    mut mutable_header = other[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=keep(header[1]), root=header[2]))\n    return fields\n";
    let (program, checker, owner_span) = fixture(source, "collect")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let view = checker.checked_body(owner_span).ok_or("missing snapshot body")?;
    let (canonical_owner, flow) = view
        .local
        .protocols
        .explicit_input_flow
        .iter()
        .next()
        .ok_or("missing snapshot flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("snapshot fixture is not checked: {flow:?}").into());
    };
    let (destination, snapshot) = graph
        .owned_snapshot_locals
        .iter()
        .next()
        .ok_or("missing live snapshot fact")?;
    assert_eq!(graph.owned_snapshot_locals.len(), 1);
    assert_eq!(snapshot.materializations.len(), 2);
    let initializer = graph.nodes.get(snapshot.access_node).ok_or("missing snapshot initializer node")?;
    assert!(matches!(
        &initializer.event,
        crate::typechecker::ownership_contracts::flow::Event::Access {
            place,
            access: crate::typechecker::ownership_contracts::flow::Access::Read,
            through: None,
        } if place == &snapshot.source
    ));
    for (span, materialization) in &snapshot.materializations {
        let node = graph
            .nodes
            .get(materialization.access_node)
            .ok_or("materialization access is outside graph")?;
        assert!(node.span.start >= span.start && node.span.end <= span.end);
        assert!(matches!(&node.event,
            crate::typechecker::ownership_contracts::flow::Event::Access {
                place,
                access: crate::typechecker::ownership_contracts::flow::Access::Read,
                through: None,
            } if matches!(&place.root,
                crate::typechecker::ownership_contracts::flow::Root::Binding(root)
                    if root == destination)
                && matches!(place.projection.as_slice(), [
                    crate::typechecker::ownership_contracts::flow::Projection::Element(index)
                ] if index.start >= span.start && index.end <= span.end)));
        assert_eq!(checker.type_info.expr_type(*span), Some(&materialization.value_type));
    }

    let mut fields = Vec::new();
    super::project(&positions, &view.local, &mut fields)?;
    let checked = fields
        .into_iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing projected snapshot graph")?;
    assert_eq!(checked.get(3).map(String::as_str), Some("5"));
    let mut without = graph.clone();
    without.owned_snapshot_locals.clear();
    let mut with_snapshot = checked[..5].to_vec();
    super::input_flow::project(&positions, graph, &mut with_snapshot)?;
    let mut without_snapshot = checked[..5].to_vec();
    super::input_flow::project(&positions, &without, &mut without_snapshot)?;
    assert_ne!(with_snapshot, without_snapshot, "snapshot authority has no persisted representation");

    let mut restored = BodyLocal {
        owner: owner_span,
        ..BodyLocal::default()
    };
    restore(&positions, &mut restored, &checked)?;
    let restored_flow = restored
        .protocols
        .explicit_input_flow
        .get(canonical_owner)
        .ok_or("missing restored snapshot flow")?;
    let InputFlow::Checked { graph: restored_graph, .. } = restored_flow else {
        return Err(format!("restored snapshot is not checked: {restored_flow:?}").into());
    };
    assert_eq!(restored_graph.owned_snapshot_locals.len(), 1);
    let restored_snapshot = restored_graph
        .owned_snapshot_locals
        .get(destination)
        .ok_or("restored graph lost snapshot identity")?;
    assert_eq!(restored_snapshot.initializer, snapshot.initializer);
    assert_eq!(restored_snapshot.base, snapshot.base);
    assert_eq!(restored_snapshot.source, snapshot.source);
    assert_eq!(restored_snapshot.access_node, snapshot.access_node);
    assert_eq!(restored_snapshot.materializations, snapshot.materializations);
    let mut roundtrip = Vec::new();
    super::project(&positions, &restored, &mut roundtrip)?;
    assert_eq!(roundtrip, vec![checked.clone()]);
    let mut facts = checker.type_info.clone();
    facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow.clone();
    assert!(facts.checked_input_body(canonical_owner)?.is_some());

    let v4_source = "enum Family:\n    Local\n    Final\nmodel Record:\n    label: str\ndef family_name(family: Family) -> str:\n    match family:\n        Family.Local => return \"local\"\n        Family.Final => return \"final\"\ndef label_len(label: Borrowed[str]) -> int:\n    return len(label)\ndef inspect(seed: Borrowed[int], family: Family, record: Borrowed[Record]) -> str:\n    if seed < 0:\n        return \"invalid\"\n    name = family_name(family)\n    if label_len(record.label) == 0:\n        return \"empty\"\n    return name\n";
    let (v4_program, v4_checker, v4_owner_span) = fixture(v4_source, "inspect")?;
    let v4_positions = Positions::new(&v4_checker, &v4_program, v4_owner_span, false)?;
    let (v4_owner, v4_flow) = v4_checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .ok_or("missing schema-4 invocation flow")?;
    let InputFlow::Checked { graph: checked_v4_graph, .. } = v4_flow else {
        return Err(format!("schema-4 invocation is not checked: {v4_flow:?}").into());
    };
    let mut v4_graph = checked_v4_graph.clone();
    let (shared_loan, shared_source, shared_evaluation) = {
        let shared_argument = v4_graph
            .invocations
            .values_mut()
            .flat_map(|invocation| &mut invocation.arguments)
            .find(|argument| argument.access == incan_semantics_core::IncanInputAccess::Shared)
            .ok_or("schema-4 fixture has no shared invocation argument")?;
        let retained = (
            shared_argument.loan,
            shared_argument.source.clone(),
            (
                shared_argument.evaluation.start_node,
                shared_argument.evaluation.end_node,
                shared_argument.evaluation.source_node,
            ),
        );
        shared_argument.access = incan_semantics_core::IncanInputAccess::InferredShared;
        retained
    };
    let v4_arguments = v4_graph
        .invocations
        .values()
        .flat_map(|invocation| &invocation.arguments)
        .collect::<Vec<_>>();
    assert!(!v4_arguments.is_empty(), "missing schema-4 invocation argument");
    assert!(v4_arguments.iter().any(|argument| {
        argument.access == incan_semantics_core::IncanInputAccess::InferredShared
            && argument.loan == shared_loan
            && argument.source.as_ref() == shared_source.as_ref()
            && (
                argument.evaluation.start_node,
                argument.evaluation.end_node,
                argument.evaluation.source_node,
            ) == shared_evaluation
    }));
    assert!(v4_arguments.iter().any(|argument| argument.closed_enum_parameter_observer));
    assert!(crate::typechecker::ownership_contracts::flow::analyze(&v4_graph).is_ok());
    let mut v4_row = projected_schema_five_snapshot_row(&v4_positions, &v4_graph)?;
    let v5_snapshot = snapshot_wire_section(&v4_row)?;
    assert!(v5_snapshot.snapshots.is_empty());
    assert_eq!(v5_snapshot.end, v5_snapshot.count + 1);
    assert_eq!(v4_row[v5_snapshot.count], "0");
    v4_row.drain(v5_snapshot.count..v5_snapshot.end);
    v4_row[3] = "4".into();
    let mut v4_restored = BodyLocal {
        owner: v4_owner_span,
        ..BodyLocal::default()
    };
    restore(&v4_positions, &mut v4_restored, &v4_row)?;
    let InputFlow::Checked { graph: restored_v4_graph, .. } = v4_restored
        .protocols
        .explicit_input_flow
        .get(v4_owner)
        .ok_or("missing restored schema-4 flow")?
    else {
        return Err("restored schema-4 invocation is not checked".into());
    };
    let restored_arguments = restored_v4_graph
        .invocations
        .values()
        .flat_map(|invocation| &invocation.arguments)
        .collect::<Vec<_>>();
    assert!(!restored_arguments.is_empty(), "restored schema-4 invocation lost its argument");
    assert!(restored_arguments.iter().any(|argument| {
        argument.access == incan_semantics_core::IncanInputAccess::InferredShared
            && argument.loan == shared_loan
            && argument.source.as_ref() == shared_source.as_ref()
            && (
                argument.evaluation.start_node,
                argument.evaluation.end_node,
                argument.evaluation.source_node,
            ) == shared_evaluation
    }));
    assert!(restored_arguments.iter().any(|argument| argument.closed_enum_parameter_observer));
    assert!(restored_v4_graph.owned_snapshot_locals.is_empty());
    Ok(())
}

fn projected_schema_five_snapshot_row(
    positions: &Positions,
    graph: &crate::typechecker::ownership_contracts::flow::Graph,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut row = vec![
        "input-flow".into(),
        positions.nodes.anchor(super::super::span_of(&graph.owner))?.path,
        "graph".into(),
        "5".into(),
        "checked".into(),
    ];
    super::input_flow::project(positions, graph, &mut row)?;
    Ok(row)
}

#[derive(Debug)]
struct SnapshotWireMaterialization {
    span: usize,
    type_start: usize,
    type_end: usize,
    access_node: usize,
}

#[derive(Debug)]
struct SnapshotWire {
    destination: usize,
    initializer: usize,
    base: usize,
    source_root: usize,
    source_index: usize,
    initializer_node: usize,
    materialization_count: usize,
    materializations: Vec<SnapshotWireMaterialization>,
}

#[derive(Debug)]
struct PermissionWire {
    start: usize,
    end: usize,
    root: Option<usize>,
    exclusive: usize,
    writable: usize,
    transferable: usize,
}

#[derive(Debug)]
struct LoanMapWireEntry {
    start: usize,
    end: usize,
    identity: usize,
}

#[derive(Debug)]
struct LoanMapsWire {
    local_count: usize,
    locals: Vec<LoanMapWireEntry>,
    dependent_count: usize,
    dependents: Vec<LoanMapWireEntry>,
}

#[derive(Debug)]
struct InvocationWireEntry {
    span: usize,
    callee: usize,
}

#[derive(Debug)]
struct SnapshotWireSection {
    permission_count: usize,
    count: usize,
    end: usize,
    permissions: Vec<PermissionWire>,
    snapshots: Vec<SnapshotWire>,
}

struct WireCursor<'a> {
    values: &'a [String],
    offset: usize,
}

impl WireCursor<'_> {
    fn token(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        let index = self.offset;
        self.values.get(index).ok_or("truncated input-flow row")?;
        self.offset += 1;
        Ok(index)
    }

    fn count(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        let index = self.token()?;
        Ok(self.values[index].parse::<usize>()?)
    }

    fn root(&mut self) -> Result<Option<usize>, Box<dyn std::error::Error>> {
        let kind = self.token()?;
        match self.values[kind].as_str() {
            "binding" => Ok(Some(self.token()?)),
            "temporary" => {
                self.token()?;
                self.token()?;
                self.token()?;
                Ok(None)
            }
            _ => Err("invalid input-flow root".into()),
        }
    }

    fn place(&mut self) -> Result<(Option<usize>, Vec<usize>), Box<dyn std::error::Error>> {
        let root = self.root()?;
        let count = self.count()?;
        let mut elements = Vec::new();
        for _ in 0..count {
            let kind = self.token()?;
            let value = self.token()?;
            if self.values[kind] == "element" {
                elements.push(value);
            }
        }
        Ok((root, elements))
    }

    fn optional_index(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let tag = self.token()?;
        match self.values[tag].as_str() {
            "none" => Ok(()),
            "some" => self.token().map(|_| ()),
            _ => Err("invalid optional index".into()),
        }
    }

    fn optional_place(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let tag = self.token()?;
        match self.values[tag].as_str() {
            "none" => Ok(()),
            "some" => self.place().map(|_| ()),
            _ => Err("invalid optional place".into()),
        }
    }

    fn checked_type(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut values = self.values[self.offset..].iter();
        super::super::read_type(&mut values)?;
        self.offset = self.values.len() - values.len();
        Ok(())
    }

    fn input_evaluation(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.token()?;
        self.token()?;
        self.optional_index()
    }
}

/// Locate the persisted local-loan and dependent-local maps without rebuilding either from current source facts.
fn loan_maps_wire(values: &[String]) -> Result<LoanMapsWire, Box<dyn std::error::Error>> {
    let mut reader = WireCursor { values, offset: 5 };
    reader.token()?;
    reader.token()?;
    for _ in 0..reader.count()? {
        reader.root()?;
        reader.token()?;
        reader.token()?;
        reader.token()?;
    }
    for _ in 0..reader.count()? {
        reader.place()?;
        reader.token()?;
        reader.token()?;
        reader.optional_index()?;
        reader.token()?;
    }
    let local_count = reader.token()?;
    let mut locals = Vec::new();
    for _ in 0..values[local_count].parse::<usize>()? {
        let start = reader.offset;
        let identity = reader.token()?;
        reader.token()?;
        locals.push(LoanMapWireEntry {
            start,
            end: reader.offset,
            identity,
        });
    }
    let dependent_count = reader.token()?;
    let mut dependents = Vec::new();
    for _ in 0..values[dependent_count].parse::<usize>()? {
        let start = reader.offset;
        let identity = reader.token()?;
        for _ in 0..reader.count()? {
            reader.token()?;
        }
        dependents.push(LoanMapWireEntry {
            start,
            end: reader.offset,
            identity,
        });
    }
    Ok(LoanMapsWire {
        local_count,
        locals,
        dependent_count,
        dependents,
    })
}

/// Locate the invocation keys and canonical callees in a schema-5 row while consuming every preceding section.
fn invocation_wire(values: &[String]) -> Result<Vec<InvocationWireEntry>, Box<dyn std::error::Error>> {
    let snapshots = snapshot_wire_section(values)?;
    let mut reader = WireCursor {
        values,
        offset: snapshots.end,
    };
    for _ in 0..reader.count()? {
        reader.token()?;
        reader.token()?;
        for _ in 0..reader.count()? {
            reader.token()?;
            reader.place()?;
            reader.token()?;
            reader.token()?;
        }
    }
    let mut invocations = Vec::new();
    for _ in 0..reader.count()? {
        let span = reader.token()?;
        let callee = reader.token()?;
        let requirement = reader.token()?;
        match values[requirement].as_str() {
            "none" => {}
            "some" => reader.checked_type()?,
            _ => return Err("invalid invocation result requirement".into()),
        }
        reader.optional_index()?;
        reader.token()?;
        reader.optional_place()?;
        for _ in 0..reader.count()? {
            reader.token()?;
        }
        for _ in 0..reader.count()? {
            reader.token()?;
            reader.token()?;
            reader.token()?;
            reader.token()?;
            reader.optional_index()?;
            reader.optional_place()?;
            reader.token()?;
            reader.input_evaluation()?;
        }
        let receiver = reader.token()?;
        match values[receiver].as_str() {
            "none" => {}
            "some" => reader.input_evaluation()?,
            _ => return Err("invalid receiver evaluation".into()),
        }
        reader.token()?;
        for _ in 0..reader.count()? {
            reader.token()?;
        }
        invocations.push(InvocationWireEntry { span, callee });
    }
    Ok(invocations)
}

/// Locate the schema-5 snapshot section by decoding every preceding count-prefixed graph section. This helper is
/// deliberately schema-aware: schema-4 controls remove this exact section rather than relabeling a latest row.
fn snapshot_wire_section(values: &[String]) -> Result<SnapshotWireSection, Box<dyn std::error::Error>> {
    let mut reader = WireCursor { values, offset: 5 };
    reader.token()?;
    reader.token()?;
    let permission_count = reader.token()?;
    let mut permissions = Vec::new();
    for _ in 0..values[permission_count].parse::<usize>()? {
        let start = reader.offset;
        let root = reader.root()?;
        let exclusive = reader.token()?;
        let writable = reader.token()?;
        let transferable = reader.token()?;
        permissions.push(PermissionWire {
            start,
            end: reader.offset,
            root,
            exclusive,
            writable,
            transferable,
        });
    }
    for _ in 0..reader.count()? {
        reader.place()?;
        reader.token()?;
        reader.token()?;
        reader.optional_index()?;
        reader.token()?;
    }
    for _ in 0..reader.count()? {
        reader.token()?;
        reader.token()?;
    }
    for _ in 0..reader.count()? {
        reader.token()?;
        for _ in 0..reader.count()? {
            reader.token()?;
        }
    }
    for _ in 0..reader.count()? {
        reader.token()?;
    }
    for _ in 0..reader.count()? {
        reader.token()?;
        reader.token()?;
        reader.optional_place()?;
        reader.optional_index()?;
    }

    let count = reader.token()?;
    let snapshot_count = values[count].parse::<usize>()?;
    let mut snapshots = Vec::new();
    for _ in 0..snapshot_count {
        let destination = reader.token()?;
        let initializer = reader.token()?;
        let base = reader.token()?;
        let (source_root, source_elements) = reader.place()?;
        let source_root = source_root.ok_or("snapshot source is not a binding")?;
        let [source_index] = source_elements.as_slice() else {
            return Err("snapshot source is not one element".into());
        };
        let initializer_node = reader.token()?;
        let materialization_count = reader.token()?;
        let mut materializations = Vec::new();
        for _ in 0..values[materialization_count].parse::<usize>()? {
            let span = reader.token()?;
            let type_start = reader.offset;
            let mut types = values[type_start..].iter();
            super::super::read_type(&mut types)?;
            let type_end = values.len() - types.len();
            reader.offset = type_end;
            let access_node = reader.token()?;
            materializations.push(SnapshotWireMaterialization {
                span,
                type_start,
                type_end,
                access_node,
            });
        }
        snapshots.push(SnapshotWire {
            destination,
            initializer,
            base,
            source_root,
            source_index: *source_index,
            initializer_node,
            materialization_count,
            materializations,
        });
    }
    Ok(SnapshotWireSection {
        permission_count,
        count,
        end: reader.offset,
        permissions,
        snapshots,
    })
}

/// Snapshot schema-5 rows fail closed when persisted nodes, places, spans or types are substituted. Every corruption
/// is projected into the real wire row and restored; no in-memory-only mutation is accepted as codec evidence.
#[test]
fn parameter_snapshot_restore_rejects_event_and_type_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "model Field:\n    owner: str\n    root: str\ndef keep(value: str) -> str:\n    return value\ndef collect(seed: Borrowed[int], rows: list[list[str]], other: list[list[str]], owned: Owned[list[list[str]]]) -> list[Field]:\n    header = rows[0]\n    mut mutable_header = other[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=keep(header[1]), root=header[2]))\n    return fields\n";
    let (program, checker, owner_span) = fixture(source, "collect")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let (canonical_owner, live) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .next()
        .ok_or("missing snapshot flow")?;
    let InputFlow::Checked { graph, .. } = live else {
        return Err(format!("snapshot fixture is not checked: {live:?}").into());
    };
    let valid_row = projected_schema_five_snapshot_row(&positions, graph)?;
    let section = snapshot_wire_section(&valid_row)?;
    let [snapshot] = section.snapshots.as_slice() else {
        return Err("schema-5 row does not have one snapshot".into());
    };
    let [first, second] = snapshot.materializations.as_slice() else {
        return Err("schema-5 row does not have two materializations".into());
    };
    assert_eq!(valid_row[section.count], "1");
    assert!(section.end < valid_row.len());

    let restore_rejects = |row: &[String]| {
        let mut restored = BodyLocal {
            owner: owner_span,
            ..BodyLocal::default()
        };
        Ok::<_, Box<dyn std::error::Error>>(restore(&positions, &mut restored, &row).is_err())
    };
    let checked_body_rejects = |row: &[String]| {
        let mut restored = BodyLocal {
            owner: owner_span,
            ..BodyLocal::default()
        };
        restore(&positions, &mut restored, &row)?;
        let mut facts = checker.type_info.clone();
        facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
        Ok::<_, Box<dyn std::error::Error>>(facts.checked_input_body(canonical_owner).is_err())
    };
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "collect" => Some(function),
            _ => None,
        })
        .ok_or("missing collect declaration")?;
    let full_admission_reason = |row: &[String]| {
        let mut restored = BodyLocal {
            owner: owner_span,
            ..BodyLocal::default()
        };
        restore(&positions, &mut restored, &row)?;
        let mut facts = checker.type_info.clone();
        facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
        let error = facts
            .executable_function_input(canonical_owner, function)
            .err()
            .ok_or("altered snapshot unexpectedly passed full source admission")?;
        Ok::<_, Box<dyn std::error::Error>>(error)
    };

    let mut initializer_oob = valid_row.clone();
    initializer_oob[snapshot.initializer_node] = graph.nodes.len().to_string();
    assert!(restore_rejects(&initializer_oob)?);

    let mut materialization_oob = valid_row.clone();
    materialization_oob[first.access_node] = graph.nodes.len().to_string();
    assert!(restore_rejects(&materialization_oob)?);

    let mut initializer_substitution = valid_row.clone();
    initializer_substitution[snapshot.initializer_node] = valid_row[first.access_node].clone();
    assert!(restore_rejects(&initializer_substitution)?);

    let mut reused_node = valid_row.clone();
    reused_node[first.access_node] = valid_row[snapshot.initializer_node].clone();
    assert!(restore_rejects(&reused_node)?);

    let mut materialization_substitution = valid_row.clone();
    materialization_substitution[first.access_node] = valid_row[second.access_node].clone();
    assert!(restore_rejects(&materialization_substitution)?);

    let destination = valid_row[snapshot.destination].clone();
    let source_root = valid_row[snapshot.source_root].clone();
    let materialization_events = valid_row
        .windows(8)
        .enumerate()
        .filter_map(|(index, values)| {
            (values[0] == "access"
                && values[1] == "binding"
                && values[2] == destination
                && values[3] == "1"
                && values[4] == "element"
                && values[6] == "read"
                && values[7] == "none")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(materialization_events.len(), 2);
    let event = materialization_events[0];

    let mut wrong_event = valid_row.clone();
    wrong_event[event + 6] = "write".into();
    assert!(restore_rejects(&wrong_event)?);

    let mut wrong_source_root = valid_row.clone();
    wrong_source_root[snapshot.source_root] = destination.clone();
    assert!(restore_rejects(&wrong_source_root)?);

    let other = graph
        .permissions
        .keys()
        .find_map(|root| match root {
            crate::typechecker::ownership_contracts::flow::Root::Binding(identity)
                if identity.declaration_name == "other" => Some(identity),
            _ => None,
        })
        .ok_or("missing same-typed alternate parameter")?;
    let other_identity = positions.identity(other)?;
    let source_identity = valid_row[snapshot.source_root].clone();
    let source_index = valid_row[snapshot.source_index].clone();
    let initializer_events = valid_row
        .windows(8)
        .enumerate()
        .filter_map(|(index, values)| {
            (values[0] == "access"
                && values[1] == "binding"
                && values[2] == source_identity
                && values[3] == "1"
                && values[4] == "element"
                && values[5] == source_index
                && values[6] == "read"
                && values[7] == "none")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let [initializer_event] = initializer_events.as_slice() else {
        return Err(format!("snapshot initializer event has {} matches", initializer_events.len()).into());
    };
    let mut coordinated_source_substitution = valid_row.clone();
    coordinated_source_substitution[snapshot.source_root] = other_identity.clone();
    coordinated_source_substitution[*initializer_event + 2] = other_identity;
    assert_eq!(coordinated_source_substitution[snapshot.base], valid_row[snapshot.base]);
    assert!(checked_body_rejects(&coordinated_source_substitution)?);

    // Deleting only the helper-argument authority leaves the Read event structurally valid. Full replay must still
    // refuse because the selected helper slot no longer has its exact retained snapshot materialization.
    let mut missing_helper_materialization = valid_row.clone();
    missing_helper_materialization[snapshot.materialization_count] = "1".into();
    missing_helper_materialization.drain(first.span..=first.access_node);
    assert!(!full_admission_reason(&missing_helper_materialization)?.is_empty());

    let mut wrong_source_index = valid_row.clone();
    wrong_source_index[snapshot.source_index] = valid_row[first.span].clone();
    assert!(restore_rejects(&wrong_source_index)?);

    let mut wrong_materialization_root = valid_row.clone();
    wrong_materialization_root[event + 2] = source_root;
    assert!(restore_rejects(&wrong_materialization_root)?);

    let mut wrong_materialization_index = valid_row.clone();
    wrong_materialization_index[event + 5] = valid_row[snapshot.source_index].clone();
    assert!(restore_rejects(&wrong_materialization_index)?);

    assert_eq!(first.type_end - first.type_start, 1);
    assert_eq!(valid_row[first.type_start], "str");
    let mut wrong_type = valid_row.clone();
    wrong_type[first.type_start] = "int".into();
    assert!(checked_body_rejects(&wrong_type)?);

    let mut wrong_span = valid_row.clone();
    wrong_span[first.span] = valid_row[snapshot.initializer].clone();
    assert!(restore_rejects(&wrong_span)?);
    Ok(())
}

/// Schema-5 permission bits cannot turn a current mutable declaration into the immutable destination that snapshot
/// authority requires. This is a full source-admission contract because declaration kind belongs to the AST layer.
#[test]
fn parameter_snapshot_restore_rejects_forged_mutable_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "model Field:\n    owner: str\n    root: str\ndef collect(seed: Borrowed[int], rows: list[list[str]], other: list[list[str]]) -> list[Field]:\n    header = rows[0]\n    mut mutable_header = other[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=header[1], root=header[2]))\n    return fields\n";
    let (program, checker, owner_span) = fixture(source, "collect")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let (owner, flow) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "collect")
        .ok_or("missing snapshot flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err("mutable-destination fixture has no checked graph".into());
    };
    let valid_row = projected_schema_five_snapshot_row(&positions, graph)?;
    let section = snapshot_wire_section(&valid_row)?;
    let [snapshot] = section.snapshots.as_slice() else {
        return Err("mutable-destination fixture has no snapshot row".into());
    };
    let destination = valid_row[snapshot.destination].clone();
    let mutable = graph
        .permissions
        .keys()
        .find_map(|root| match root {
            crate::typechecker::ownership_contracts::flow::Root::Binding(identity)
                if identity.declaration_name == "mutable_header" => Some(identity),
            _ => None,
        })
        .ok_or("missing mutable snapshot-shaped local")?;
    let mutable_identity = positions.identity(mutable)?;
    let permission = section
        .permissions
        .iter()
        .find(|permission| permission.root.is_some_and(|root| valid_row[root] == mutable_identity))
        .ok_or("missing mutable destination permission")?;
    let materialization_events = valid_row
        .windows(8)
        .enumerate()
        .filter_map(|(index, values)| {
            (values[0] == "access"
                && values[1] == "binding"
                && values[2] == destination
                && values[3] == "1"
                && values[4] == "element"
                && values[6] == "read"
                && values[7] == "none")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(materialization_events.len(), 2);
    let mut altered = valid_row.clone();
    altered[snapshot.destination] = mutable_identity.clone();
    altered[permission.exclusive] = "false".into();
    altered[permission.writable] = "false".into();
    altered[permission.transferable] = "true".into();
    for event in materialization_events {
        altered[event + 2] = mutable_identity.clone();
    }
    let mut restored = BodyLocal {
        owner: owner_span,
        ..BodyLocal::default()
    };
    restore(&positions, &mut restored, &altered)?;
    let mut facts = checker.type_info.clone();
    facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow.clone();
    assert!(facts.checked_input_body(owner)?.is_some());
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "collect" => Some(function),
            _ => None,
        })
        .ok_or("missing collect declaration")?;
    assert_eq!(
        facts
            .executable_function_input(owner, function)
            .err()
            .ok_or("forged mutable snapshot unexpectedly passed")?,
        "parameter snapshot destination is not one immutable inferred declaration"
    );
    Ok(())
}

/// A restored graph cannot replay a non-place `Try(Call)` constructor field merely because evaluating the inner call
/// has no stored place. The exact selected ordinary Result invocation remains mandatory at the constructor boundary.
#[test]
fn closed_result_try_call_constructor_requires_restored_invocation()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef text_value() -> Result[str, Fault]:\n    return Ok(\"value\")\ndef decode(seed: Borrowed[int]) -> Result[Record, Fault]:\n    return Ok(Record(text=text_value()?))\n";
    let (program, checker, owner_span) = fixture(source, "decode")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let (owner, flow) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "decode")
        .ok_or("missing constructor Try(Call) flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err("constructor Try(Call) fixture has no checked graph".into());
    };
    let call_span = checker
        .type_info
        .protocols
        .explicit_input_calls
        .iter()
        .find_map(|(span, call)| (call.callee.declaration_name == "text_value").then_some(*span))
        .ok_or("missing selected text_value call")?;
    let mut missing = graph.clone();
    assert!(missing.invocations.remove(&call_span).is_some());
    let row = projected_schema_five_snapshot_row(&positions, &missing)?;
    let mut restored = BodyLocal {
        owner: owner_span,
        ..BodyLocal::default()
    };
    restore(&positions, &mut restored, &row)?;
    let mut facts = checker.type_info.clone();
    facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
    assert!(facts.checked_input_body(owner)?.is_some());
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "decode" => Some(function),
            _ => None,
        })
        .ok_or("missing decode declaration")?;
    assert!(
        !facts
            .executable_function_input(owner, function)
            .err()
            .ok_or("restored graph without closed Result invocation unexpectedly passed")?
            .is_empty()
    );
    Ok(())
}

/// The persisted closed-Enum observer marker is a claim, not authority. Flipping an unrelated ordinary argument's
/// false bit to true survives the wire format but full source replay must reject it against the actual int argument.
#[test]
fn closed_enum_parameter_observer_requires_current_source_after_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Family:\n    Local\ndef family_name(value: Family) -> str:\n    return \"local\"\ndef keep(value: int) -> int:\n    return value\ndef inspect(seed: Borrowed[int], family: Family, count: int) -> str:\n    name = family_name(family)\n    observed = keep(count)\n    return name\n";
    let (program, checker, owner_span) = fixture(source, "inspect")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let (owner, flow) = checker.type_info.protocols.explicit_input_flow.iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .ok_or("missing observer-forgery flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err("observer-forgery fixture is not checked".into());
    };
    let keep_span = checker.type_info.protocols.explicit_input_calls.iter()
        .find_map(|(span, call)| (call.callee.declaration_name == "keep").then_some(*span))
        .ok_or("missing keep invocation")?;
    let mut forged = graph.clone();
    let argument = forged.invocations.get_mut(&keep_span)
        .and_then(|invocation| invocation.arguments.first_mut())
        .ok_or("missing keep argument")?;
    assert!(!argument.closed_enum_parameter_observer);
    argument.closed_enum_parameter_observer = true;
    let row = projected_schema_five_snapshot_row(&positions, &forged)?;
    let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
    restore(&positions, &mut restored, &row)?;
    let mut facts = checker.type_info.clone();
    facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
    let function = program.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "inspect" => Some(function),
        _ => None,
    }).ok_or("missing inspect declaration")?;
    assert!(
        facts.executable_function_input(owner, function).is_err(),
        "restored false-to-true observer forgery passed full source replay"
    );
    Ok(())
}

/// A restored loan-free scrutinee flag cannot replace the actual local/dependent-loan maps. Deleting a real loan or
/// substituting its destination with the matched row survives structural decoding but full source replay rejects it.
#[test]
fn match_assignment_restore_rejects_loan_deletion_and_subject_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef copy_cells(values: Borrowed[list[Cell]]) -> Owned[list[Cell]] from values:\n    return values.clone()\ndef decode(seed: Borrowed[int], text: Borrowed[str], cells: Borrowed[list[Cell]]) -> Result[list[Cell], Fault]:\n    alias: Borrowed[str] = text\n    dependent = copy_cells(cells)\n    held = dependent\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n";
    let (program, checker, owner_span) = fixture(source, "decode")?;
    let positions = Positions::new(&checker, &program, owner_span, false)?;
    let (owner, flow) = checker.type_info.protocols.explicit_input_flow.iter()
        .find(|(owner, _)| owner.declaration_name == "decode")
        .ok_or("missing match-loan restore flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("match-loan fixture is not checked: {flow:?}").into());
    };
    let identity = |name: &str| graph.permissions.keys().find_map(|root| match root {
        crate::typechecker::ownership_contracts::flow::Root::Binding(identity)
            if identity.declaration_name == name => Some(identity.clone()),
        _ => None,
    }).ok_or_else(|| format!("missing {name} identity"));
    let alias = graph.local_loans.keys().find(|identity| identity.declaration_name == "alias")
        .cloned().ok_or("missing alias identity")?;
    let dependent = graph.dependent_locals.keys().find(|identity| identity.declaration_name == "dependent")
        .cloned().ok_or("missing dependent identity")?;
    let row_identity = identity("row")?;
    let scrutinee = checker.type_info.protocols.loan_free_scrutinees.iter().copied()
        .find(|span| source.get(span.start..span.end) == Some("row.value"))
        .ok_or("missing retained row.value loan-free fact")?;

    let function = program.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "decode" => Some(function),
        _ => None,
    }).ok_or("missing decode declaration")?;
    checker.type_info.executable_function_input(owner, function)
        .map_err(|reason| format!("baseline match-loan fixture failed full admission: {reason}"))?;
    let valid_row = projected_schema_five_snapshot_row(&positions, graph)?;
    let refusal_layer = |row: &[String]| -> Result<&'static str, Box<dyn std::error::Error>> {
        let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
        if restore(&positions, &mut restored, row).is_err() {
            return Ok("restore");
        }
        let mut facts = checker.type_info.clone();
        facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
        facts.protocols.loan_free_scrutinees.insert(scrutinee);
        if facts.executable_function_input(owner, function).is_err() {
            Ok("admission")
        } else {
            Err("restored loan mutation passed under a true loan-free scrutinee flag".into())
        }
    };

    let wire = loan_maps_wire(&valid_row)?;
    let alias_token = positions.identity(&alias)?;
    let dependent_token = positions.identity(&dependent)?;
    let row_token = positions.identity(&row_identity)?;
    let alias_entry = wire.locals.iter().find(|entry| valid_row[entry.identity] == alias_token)
        .ok_or("missing alias row entry")?;
    let dependent_entry = wire.dependents.iter().find(|entry| valid_row[entry.identity] == dependent_token)
        .ok_or("missing dependent row entry")?;

    let mut missing_local = valid_row.clone();
    missing_local[wire.local_count] = (wire.locals.len() - 1).to_string();
    missing_local.drain(alias_entry.start..alias_entry.end);
    assert_eq!(refusal_layer(&missing_local)?, "admission");

    let mut substituted_local = valid_row.clone();
    substituted_local[alias_entry.identity] = row_token.clone();
    assert_eq!(refusal_layer(&substituted_local)?, "restore");

    let mut missing_dependent = valid_row.clone();
    missing_dependent[wire.dependent_count] = (wire.dependents.len() - 1).to_string();
    missing_dependent.drain(dependent_entry.start..dependent_entry.end);
    assert_eq!(refusal_layer(&missing_dependent)?, "admission");

    let mut substituted_dependent = valid_row;
    substituted_dependent[dependent_entry.identity] = row_token;
    assert_eq!(refusal_layer(&substituted_dependent)?, "admission");
    Ok(())
}

/// A capture-free selected builtin Result.map_err over an imported ordinary Result call has a complete schema-5
/// graph. Projecting and restoring that graph must preserve both structural analysis and current full admission.
#[test]
fn result_map_err_capture_free_graph_roundtrips_schema_five()
-> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub def decode(value: int) -> Result[int, str]:\n    return Ok(value)\n";
    let consumer_source = "from views import decode\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    decoded = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(decoded)\n";
    let provider_tokens = crate::lexer::lex(provider_source).map_err(|errors| format!("{errors:?}"))?;
    let provider = crate::parser::parse(&provider_tokens).map_err(|errors| format!("{errors:?}"))?;
    let consumer_tokens = crate::lexer::lex(consumer_source).map_err(|errors| format!("{errors:?}"))?;
    let consumer = crate::parser::parse(&consumer_tokens).map_err(|errors| format!("{errors:?}"))?;
    let dependencies = [("views", &provider)];
    let mut checker = crate::typechecker::TypeChecker::default();
    checker.set_current_package_identity(Some("fixture_records".into()));
    checker.register_dependency_module_path_segments("views", vec!["views".into()]);
    if let Err(errors) = checker.check_with_imports(&consumer, &dependencies) {
        checker.errors = errors;
    }
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let owner_span = consumer.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "use" => Some(declaration.span),
        _ => None,
    }).ok_or("missing baseline map_err owner span")?;
    let positions = Positions::new(&checker, &consumer, owner_span, false)
        .map_err(|reason| format!("baseline map_err positions: {reason:?}"))?;
    let (owner, flow) = checker.type_info.protocols.explicit_input_flow.iter()
        .find(|(owner, _)| owner.declaration_name == "use")
        .ok_or("missing baseline map_err flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("baseline map_err flow is not checked: {flow:?}").into());
    };
    let function = consumer.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
        _ => None,
    }).ok_or("missing baseline map_err declaration")?;
    checker.type_info.executable_function_input(owner, function)
        .map_err(|reason| format!("live baseline map_err admission: {reason}"))?;
    let row = projected_schema_five_snapshot_row(&positions, graph)
        .map_err(|reason| format!("baseline map_err projection: {reason}"))?;
    snapshot_wire_section(&row)
        .map_err(|reason| format!("baseline map_err through snapshot section: {reason}"))?;
    invocation_wire(&row)
        .map_err(|reason| format!("baseline map_err through invocation section: {reason}"))?;
    let mut reader = Reader {
        positions: &positions,
        values: row[5..].iter(),
    };
    let decoded = match reader.graph(5) {
        Ok(graph) => graph,
        Err(reason) => {
            let offset = row.len() - reader.values.len();
            let start = offset.saturating_sub(4);
            let end = (offset + 4).min(row.len());
            return Err(format!(
                "baseline map_err graph reader at token {offset}: {reason:?}; nearby={:?}",
                &row[start..end]
            ).into());
        }
    };
    reader.finish()
        .map_err(|reason| format!("baseline map_err trailing token: {reason:?}"))?;
    crate::typechecker::ownership_contracts::flow::analyze(&decoded)
        .map_err(|fault| format!("baseline map_err decoded graph: {}", fault.0))?;
    let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
    restore(&positions, &mut restored, &row)
        .map_err(|reason| format!("baseline map_err local restore: {reason:?}"))?;
    let mut facts = checker.type_info.clone();
    facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow.clone();
    facts.checked_input_body(owner)
        .map_err(|reason| format!("baseline map_err checked body: {reason}"))?
        .ok_or("baseline map_err restored no checked body")?;
    facts.executable_function_input(owner, function)
        .map_err(|reason| format!("baseline map_err restored admission: {reason}"))?;
    let mut roundtrip = Vec::new();
    super::project(&positions, &restored, &mut roundtrip)?;
    assert_eq!(roundtrip, vec![row.clone()]);

    let mut local_record = checker.produced_local_record(owner_span)
        .cloned().ok_or("missing baseline map_err local record")?;
    local_record.first_mut().ok_or("missing baseline map_err local header")?.insert(3, "local".into());
    let mut final_record = checker.produced_final_record(owner_span)
        .cloned().ok_or("missing baseline map_err final record")?;
    final_record.first_mut().ok_or("missing baseline map_err final header")?.insert(3, "final".into());
    let identity = local_record.first().ok_or("missing baseline map_err local header")?[1].clone();
    let records = crate::checker_records::GraphBodyRecords::new(vec![crate::checker_records::GraphBodyRecord {
        identity: identity.clone(),
        local: local_record.clone(),
        final_value: final_record.clone(),
        body_ir: None,
    }])?;
    let mut selected_checker = crate::typechecker::TypeChecker::default();
    selected_checker.set_current_package_identity(Some("fixture_records".into()));
    selected_checker.register_dependency_module_path_segments("views", vec!["views".into()]);
    selected_checker.set_graph_body_records(records);
    selected_checker.check_with_imports(&consumer, &dependencies)
        .map_err(|errors| format!("capture-free map_err record reuse: {errors:?}"))?;
    // A fresh selector has no checked body facts from which to bind the builtin member identity. It must safely
    // reject this record and check the capture-free body afresh; the direct schema roundtrip above is the codec proof.
    assert_eq!(selected_checker.check_work().body_view_reuses, 0);
    assert_eq!(selected_checker.check_work().function_bodies, 1);

    let builtin = positions.identity(&crate::symbols::canonical_builtin_member_identity("Result", "map_err"))?;
    let invocations = invocation_wire(&row)?;
    let selected = invocations.iter().find(|invocation| row[invocation.callee] == builtin)
        .ok_or("missing selected builtin map_err row")?;
    let mut unknown = row;
    unknown[selected.callee] = "member/method:builtin::Result.unseeded".into();
    let mut refused = BodyLocal { owner: owner_span, ..BodyLocal::default() };
    assert!(restore(&positions, &mut refused, &unknown).is_err());
    assert!(refused.protocols.explicit_input_flow.is_empty());
    Ok(())
}

/// A current closure Parameter is rebound through the same structural owner/name/ordinal identity as ordinary local
/// declarations. Persisted kind substitution is refused before an otherwise reusable record can publish.
#[test]
fn closure_parameter_binding_roundtrips_with_exact_kind()
-> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub def apply(callback: Callable[int, int], value: int) -> int:\n    return callback(value)\n";
    let source = "from views import apply\ndef use(value: int) -> int:\n    decoded = apply((item) => item, value)\n    return decoded\n";
    let provider = crate::parser::parse(&crate::lexer::lex(provider_source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let dependencies = [("views", &provider)];
    let mut checker = crate::typechecker::TypeChecker::default();
    checker.set_current_package_identity(Some("fixture_records".into()));
    checker.register_dependency_module_path_segments("views", vec!["views".into()]);
    checker.check_with_imports(&program, &dependencies).map_err(|errors| format!("{errors:?}"))?;
    let owner = program.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "use" => Some(declaration.span),
        _ => None,
    }).ok_or("missing closure owner")?;
    let mut local = checker.produced_local_record(owner)
        .cloned().ok_or_else(|| format!("missing closure local record: {:?}", checker.local_record_refusal(owner)))?;
    local.first_mut().ok_or("missing closure local header")?.insert(3, "local".into());
    let mut final_value = checker.produced_final_record(owner)
        .cloned().ok_or("missing closure final record")?;
    final_value.first_mut().ok_or("missing closure final header")?.insert(3, "final".into());
    let identity = local.first().ok_or("missing closure record identity")?[1].clone();
    assert!(local.iter().any(|row| {
        row.len() == 4 && row[0] == "binding" && row[2] == "declaration" && row[3].starts_with("local:")
    }));
    assert!(local.iter().any(|row| {
        row.len() == 4 && row[0] == "binding" && row[2] == "declaration" && row[3].starts_with("parameter:")
    }));

    let select = |local: Vec<Vec<String>>, label: &str|
     -> Result<crate::typechecker::CheckWork, Box<dyn std::error::Error>> {
        let records = crate::checker_records::GraphBodyRecords::new(vec![
            crate::checker_records::GraphBodyRecord {
                identity: identity.clone(),
                local,
                final_value: final_value.clone(),
                body_ir: None,
            },
        ])?;
        let mut selected = crate::typechecker::TypeChecker::default();
        selected.set_current_package_identity(Some("fixture_records".into()));
        selected.register_dependency_module_path_segments("views", vec!["views".into()]);
        selected.set_graph_body_records(records);
        selected.check_with_imports(&program, &dependencies).map_err(|errors| format!("{label}: {errors:?}"))?;
        Ok(selected.check_work().clone())
    };
    let baseline = select(local.clone(), "closure record baseline")?;
    assert_eq!(baseline.body_view_reuses, 1);
    assert_eq!(baseline.function_bodies, 0);

    for (from, to, label) in [
        ("local:", "parameter:", "Local-to-Parameter substitution"),
        ("parameter:", "local:", "Parameter-to-Local substitution"),
    ] {
        let mut forged = local.clone();
        let binding = forged.iter_mut().find(|row| {
            row.len() == 4
                && row[0] == "binding"
                && row[2] == "declaration"
                && row[3].starts_with(from)
        }).ok_or_else(|| format!("missing {label} binding row"))?;
        binding[3] = binding[3].replacen(from, to, 1);
        let refused = select(forged, label)?;
        assert_eq!(refused.body_view_reuses, 0, "{label}");
        assert_eq!(refused.function_bodies, 1, "{label}");
    }
    Ok(())
}

/// Restored source-free callback and receiver-invocation rows cannot authorize Result.map_err. Full replay
/// recomputes the closure's current free locals and requires the exact direct Package Result-call invocation.
#[test]
fn result_map_err_restore_rejects_capture_and_receiver_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    let provider_source = "pub def decode(value: int) -> Result[int, str]:\n    return Ok(value)\npub def passthrough(value: int) -> int:\n    return value\n";
    let consumer_source = "from views import decode, passthrough\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    decoded = decode(value).map_err((_) => Fault.Malformed)?\n    ignored = passthrough(value)\n    return Ok(decoded)\n";
    let provider_tokens = crate::lexer::lex(provider_source).map_err(|errors| format!("{errors:?}"))?;
    let provider = crate::parser::parse(&provider_tokens).map_err(|errors| format!("{errors:?}"))?;
    let consumer_tokens = crate::lexer::lex(consumer_source).map_err(|errors| format!("{errors:?}"))?;
    let consumer = crate::parser::parse(&consumer_tokens).map_err(|errors| format!("{errors:?}"))?;
    let dependencies = [("views", &provider)];
    let mut checker = crate::typechecker::TypeChecker::default();
    checker.set_current_package_identity(Some("fixture_records".into()));
    checker.register_dependency_module_path_segments("views", vec!["views".into()]);
    if let Err(errors) = checker.check_with_imports(&consumer, &dependencies) {
        checker.errors = errors;
    }
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let owner_span = consumer.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "use" => Some(declaration.span),
        _ => None,
    }).ok_or("missing use owner span")?;
    let positions = Positions::new(&checker, &consumer, owner_span, false)?;
    let (owner, flow) = checker.type_info.protocols.explicit_input_flow.iter()
        .find(|(owner, _)| owner.declaration_name == "use")
        .ok_or("missing restored map_err flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("map_err fixture is not checked: {flow:?}").into());
    };
    let decode_span = checker.type_info.protocols.explicit_input_calls.iter()
        .find_map(|(span, call)| (call.callee.declaration_name == "decode").then_some(*span))
        .ok_or("missing decode call")?;
    let passthrough_span = checker.type_info.protocols.explicit_input_calls.iter()
        .find_map(|(span, call)| (call.callee.declaration_name == "passthrough").then_some(*span))
        .ok_or("missing passthrough call")?;
    let function = consumer.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
        _ => None,
    }).ok_or("missing use declaration")?;
    checker.type_info.executable_function_input(owner, function)
        .map_err(|reason| format!("baseline map_err fixture failed full admission: {reason}"))?;
    let valid_row = projected_schema_five_snapshot_row(&positions, graph)?;
    let refusal_layer = |row: &[String]| -> Result<&'static str, Box<dyn std::error::Error>> {
        let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
        if restore(&positions, &mut restored, row).is_err() {
            return Ok("restore");
        }
        let mut facts = checker.type_info.clone();
        facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
        if facts.executable_function_input(owner, function).is_err() {
            Ok("admission")
        } else {
            Err("restored map_err mutation unexpectedly passed full admission".into())
        }
    };
    let mut baseline = BodyLocal { owner: owner_span, ..BodyLocal::default() };
    restore(&positions, &mut baseline, &valid_row)
        .map_err(|reason| format!("baseline map_err row restore: {reason:?}"))?;
    let mut baseline_facts = checker.type_info.clone();
    baseline_facts.protocols.explicit_input_flow = baseline.protocols.explicit_input_flow;
    baseline_facts.executable_function_input(owner, function)
        .map_err(|reason| format!("restored baseline map_err fixture failed: {reason}"))?;

    // Build an adversarial graph from the exact authentic captureful source by temporarily clearing only the
    // already-checked capture vector. Restore that source fact before replay; no AST, identity or type fact changes.
    let capturing_source = "from views import decode, passthrough\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    fault = Fault.Malformed\n    decoded = decode(value).map_err((_) => fault)?\n    ignored = passthrough(value)\n    return Ok(decoded)\n";
    let capturing_tokens = crate::lexer::lex(capturing_source).map_err(|errors| format!("{errors:?}"))?;
    let capturing_program = crate::parser::parse(&capturing_tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut capturing_checker = crate::typechecker::TypeChecker::default();
    capturing_checker.set_current_package_identity(Some("fixture_records".into()));
    capturing_checker.register_dependency_module_path_segments("views", vec!["views".into()]);
    if let Err(errors) = capturing_checker.check_with_imports(&capturing_program, &dependencies) {
        capturing_checker.errors = errors;
    }
    let (capturing_declaration_span, capturing_function) = capturing_program.declarations.iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" =>
                Some((declaration.span, function)),
            _ => None,
        }).ok_or("missing capturing map_err declaration")?;
    let capturing_owner = capturing_checker.type_info.protocols.explicit_input_flow.keys()
        .find(|owner| owner.declaration_name == "use").cloned()
        .ok_or("missing capturing map_err owner")?;
    let callback_span = capturing_checker.type_info.expressions.expr_types.keys().copied()
        .find(|span| capturing_source.get(span.start..span.end) == Some("(_) => fault"))
        .ok_or("missing authentic capturing callback span")?;
    let captures = capturing_checker.take_closure_captures_for_test(callback_span)
        .ok_or("missing authentic checked callback captures")?;
    assert!(!captures.is_empty(), "captureful callback unexpectedly has no current captures");
    let forged_graph = capturing_checker.rebuild_function_input_flow_for_test(
        capturing_function,
        capturing_declaration_span,
        &capturing_owner,
    ).map_err(|reason| format!("same-source map_err graph forgery failed: {}", reason.reason))?;
    capturing_checker.restore_closure_captures_for_test(callback_span, captures);
    let forged_analysis = crate::typechecker::ownership_contracts::flow::analyze(&forged_graph)
        .map_err(|fault| format!("same-source capture graph is invalid: {}", fault.0))?;
    let mut capture_facts = capturing_checker.type_info.clone();
    capture_facts.protocols.explicit_input_flow.insert(
        capturing_owner.clone(),
        InputFlow::Checked { graph: forged_graph.clone(), analysis: forged_analysis },
    );
    assert_eq!(
        capture_facts.executable_function_input(&capturing_owner, capturing_function).err()
            .ok_or("authentic direct capture replay unexpectedly passed")?,
        "builtin Result.map_err differs from its capture-free closed Package-call surface"
    );

    let capturing_positions = Positions::new(
        &capturing_checker,
        &capturing_program,
        capturing_declaration_span,
        false,
    )?;
    let capture_row = projected_schema_five_snapshot_row(&capturing_positions, &forged_graph)?;
    let capture_refusal = |row: &[String]| -> Result<&'static str, Box<dyn std::error::Error>> {
        let mut restored = BodyLocal { owner: capturing_declaration_span, ..BodyLocal::default() };
        if restore(&capturing_positions, &mut restored, row).is_err() {
            return Ok("restore");
        }
        let mut facts = capturing_checker.type_info.clone();
        facts.protocols.explicit_input_flow = restored.protocols.explicit_input_flow;
        if facts.executable_function_input(&capturing_owner, capturing_function).is_err() {
            Ok("admission")
        } else {
            Err("capture permission mutation passed current-source replay".into())
        }
    };
    let capture_identity = forged_graph.permissions.keys().find_map(|root| match root {
        crate::typechecker::ownership_contracts::flow::Root::Binding(identity)
            if identity.declaration_name == "fault" => Some(identity),
        _ => None,
    }).ok_or("missing captured fault permission")?;
    let replacement_identity = forged_graph.permissions.keys().find_map(|root| match root {
        crate::typechecker::ownership_contracts::flow::Root::Binding(identity)
            if identity.declaration_name == "ignored" => Some(identity),
        _ => None,
    }).ok_or("missing valid replacement permission")?;
    let capture_token = capturing_positions.identity(capture_identity)?;
    let replacement_token = capturing_positions.identity(replacement_identity)?;
    let wire = snapshot_wire_section(&capture_row)?;
    let capture_permission = wire.permissions.iter()
        .find(|permission| permission.root.is_some_and(|root| capture_row[root] == capture_token))
        .ok_or("missing captured permission row")?;
    let replacement_permission = wire.permissions.iter()
        .find(|permission| permission.root.is_some_and(|root| capture_row[root] == replacement_token))
        .ok_or("missing replacement permission row")?;
    let mut deleted = capture_row.clone();
    deleted[wire.permission_count] = (wire.permissions.len() - 1).to_string();
    deleted.drain(capture_permission.start..capture_permission.end);
    assert_eq!(capture_refusal(&deleted)?, "admission");
    let mut substituted_permission = capture_row.clone();
    substituted_permission[capture_permission.root.ok_or("captured permission has no binding root")?] =
        replacement_token;
    substituted_permission[wire.permission_count] = (wire.permissions.len() - 1).to_string();
    substituted_permission.drain(replacement_permission.start..replacement_permission.end);
    assert!(matches!(capture_refusal(&substituted_permission)?, "restore" | "admission"));
    let capturing_view = capturing_checker.checked_body(capturing_declaration_span)
        .ok_or("missing capturing map_err checked body")?;
    let mut local_record = super::super::produce_local(
        &capturing_checker,
        &capturing_program,
        capturing_declaration_span,
        &capturing_view.local,
    )?;
    local_record.first_mut().ok_or("missing capturing local header")?.insert(3, "local".into());
    let final_record = super::super::produce_final(
        &capturing_checker,
        &capturing_program,
        capturing_declaration_span,
        capturing_view,
    )?;
    for (label, forged_row) in [("deleted", deleted), ("substituted", substituted_permission)] {
        let mut forged_final = final_record.clone();
        let flow_row = forged_final.iter_mut()
            .find(|row| row.first().is_some_and(|family| family == "input-flow"))
            .ok_or("missing capturing map_err final flow row")?;
        *flow_row = forged_row;
        let identity = local_record.first().ok_or("missing capturing local header")?[1].clone();
        let records = crate::checker_records::GraphBodyRecords::new(vec![
            crate::checker_records::GraphBodyRecord {
                identity,
                local: local_record.clone(),
                final_value: forged_final,
                body_ir: None,
            },
        ])?;
        let mut selected = crate::typechecker::TypeChecker::default();
        selected.set_current_package_identity(Some("fixture_records".into()));
        selected.register_dependency_module_path_segments("views", vec!["views".into()]);
        selected.set_graph_body_records(records);
        assert!(
            selected.check_with_imports(&capturing_program, &dependencies).is_err(),
            "{label} captured-permission record unexpectedly became a clean program"
        );
        assert_eq!(selected.check_work().body_view_reuses, 0, "{label}");
        assert_eq!(selected.check_work().function_bodies, 1, "{label}");
    }

    let mut receiver_graph = graph.clone();
    let replacement = receiver_graph.invocations.get(&passthrough_span)
        .cloned().ok_or("missing passthrough graph invocation")?;
    receiver_graph.invocations.insert(decode_span, replacement);
    let analysis = crate::typechecker::ownership_contracts::flow::analyze(&receiver_graph)
        .map_err(|fault| format!("receiver replay graph is invalid: {}", fault.0))?;
    let mut receiver_facts = checker.type_info.clone();
    receiver_facts.protocols.explicit_input_flow.insert(
        owner.clone(),
        InputFlow::Checked { graph: receiver_graph, analysis },
    );
    assert_eq!(
        receiver_facts.executable_function_input(owner, function).err()
            .ok_or("direct receiver replay unexpectedly passed")?,
        "invocation differs from its checked caller or signature"
    );

    let decode_callee = positions.identity(&graph.invocations.get(&decode_span)
        .ok_or("missing decode graph invocation")?.callee)?;
    let passthrough_callee = positions.identity(&graph.invocations.get(&passthrough_span)
        .ok_or("missing passthrough graph invocation")?.callee)?;
    let invocations = invocation_wire(&valid_row)
        .map_err(|reason| format!("invocation wire: {reason}"))?;
    let decode = invocations.iter().find(|invocation| valid_row[invocation.callee] == decode_callee)
        .ok_or("missing decode invocation row")?;
    let mut substituted = valid_row;
    substituted[decode.callee] = passthrough_callee;
    assert_eq!(refusal_layer(&substituted)?, "admission");
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
    let graph = reader.graph(5)?;
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
    let graph = reader.graph(5)?;
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

/// Checked Copy, Clone and stored-layout choices survive independently of graph access tags; empty and duplicate
/// owner maps remain distinct.
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
    let restored_int = restored
        .protocols
        .explicit_input_copy
        .values()
        .find_map(|decisions| decisions.get(&crate::symbols::ResolvedType::Int))
        .ok_or("missing checked int value decisions")?;
    assert!(restored_int.copy && restored_int.clone && restored_int.closed);
    let mut roundtrip = Vec::new();
    super::project(&positions, &restored, &mut roundtrip)?;
    assert_eq!(roundtrip, vec![row.clone()]);
    assert!(restore(&positions, &mut restored, &row).is_err());
    let mut after_duplicate = Vec::new();
    super::project(&positions, &restored, &mut after_duplicate)?;
    assert_eq!(after_duplicate, roundtrip);

    let prefix = row[..5].to_vec();
    for entries in [
        vec!["2", "str", "false", "true", "true", "int", "true", "true", "true"],
        vec!["2", "int", "true", "true", "true", "int", "false", "true", "true"],
        vec!["1", "int", "maybe", "true", "true"],
        vec!["1", "callable", "00", "int", "false", "false", "false"],
        vec!["2", "int", "true", "true"],
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
    callable.extend(
        ["1", "callable", "0", "int", "false", "false", "false"]
            .into_iter()
            .map(str::to_owned),
    );
    let mut retained_callable = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut retained_callable, &callable)?;
    let mut callable_fields = Vec::new();
    super::project(&positions, &retained_callable, &mut callable_fields)?;
    assert_eq!(callable_fields, vec![callable]);

    // Legacy schema 1 retains Copy but cannot authorize either of the new layout-sensitive replay routes.
    let mut legacy = vec![row[0].clone(), row[1].clone(), row[2].clone(), "1".into(), row[4].clone(), "1".into()];
    super::super::type_fields(&crate::symbols::ResolvedType::Int, &mut legacy)?;
    legacy.push("true".into());
    let mut retained_legacy = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut retained_legacy, &legacy)?;
    let legacy_int = retained_legacy
        .protocols
        .explicit_input_copy
        .values()
        .find_map(|decisions| decisions.get(&crate::symbols::ResolvedType::Int))
        .ok_or("missing legacy int decision")?;
    assert!(legacy_int.copy);
    assert!(!legacy_int.clone && !legacy_int.closed);
    let mut upgraded_legacy = Vec::new();
    super::project(&positions, &retained_legacy, &mut upgraded_legacy)?;
    assert_eq!(upgraded_legacy.len(), 1);
    assert_eq!(upgraded_legacy[0].get(3).map(String::as_str), Some("2"));
    assert!(upgraded_legacy[0].ends_with(&["true".into(), "false".into(), "false".into()]));

    // A substituted exact type cannot lend its positive decisions to the actual checked int expression.
    let mut substituted = legacy.clone();
    let int = substituted.iter().position(|token| token == "int").ok_or("missing legacy int token")?;
    substituted[int] = "str".into();
    let mut retained_substitution = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    restore(&positions, &mut retained_substitution, &substituted)?;
    assert!(retained_substitution
        .protocols
        .explicit_input_copy
        .values()
        .all(|decisions| !decisions.contains_key(&crate::symbols::ResolvedType::Int)));

    // An owner-token substitution is rejected against the row's anchored declaration before any decision exists.
    let mut substituted_owner = legacy;
    substituted_owner[4] = "missing-owner".into();
    let mut refused_owner = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    assert!(restore(&positions, &mut refused_owner, &substituted_owner).is_err());
    assert!(refused_owner.protocols.explicit_input_copy.is_empty());

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

/// Complete source layout decisions include stored generic arguments and native owners while their lexical and
/// imported declarations are available; replay never turns a nominal display name into positive authority.
#[test]
fn checked_value_layout_retains_nested_negative_decisions() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::std::thread import JoinHandle\n@derive(Clone)\nmodel Carrier[T]:\n    value: T\ndef hold[T](value: Borrowed[T]) -> None:\n    pass\ndef inspect(seed: Borrowed[int], borrowed: Carrier[Borrowed[str]], native: Result[BorrowedMut[int], str], uncloneable: Carrier[JoinHandle[int]]) -> None:\n    hold(seed)\n    hold(borrowed)\n    hold(native)\n    hold(uncloneable)\n";
    let (program, checker, owner_span) = fixture(source, "inspect")?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let body = checker.checked_body(owner_span).ok_or("missing layout owner body")?;
    let owner = body.local.protocols.explicit_input_copy.keys().find(|owner| owner.declaration_name == "inspect")
        .ok_or("missing retained layout owner")?;
    let decisions = body.local.protocols.explicit_input_copy.get(owner).ok_or("missing retained layout decisions")?;
    let checked_type = |name: &str| -> Result<crate::symbols::ResolvedType, Box<dyn std::error::Error>> {
        body.local.expressions.expr_types.iter().find_map(|(span, ty)| {
            (source.get(span.start..span.end) == Some(name)).then_some(ty)
        }).cloned().ok_or_else(|| format!("missing checked {name} expression type").into())
    };
    let borrowed_type = checked_type("borrowed")?;
    let native_type = checked_type("native")?;
    let uncloneable_type = checked_type("uncloneable")?;
    let borrowed = decisions.get(&borrowed_type).copied().ok_or("missing retained borrowed decision")?;
    assert!(!borrowed.closed, "generic nominal with Borrowed storage became closed");
    let native = decisions.get(&native_type).copied().ok_or("missing retained native decision")?;
    assert!(!native.closed, "Result payload with an exclusive reference became closed");
    let uncloneable = decisions.get(&uncloneable_type).copied().ok_or("missing retained uncloneable decision")?;
    assert!(!uncloneable.clone, "generic nominal with non-Clone native storage became Clone");

    let positions = Positions::for_local(&checker, &program, owner_span, &body.local)?;
    let mut fields = Vec::new();
    super::project(&positions, &body.local, &mut fields)?;
    let row = fields.iter().find(|row| row.first().is_some_and(|family| family == "input-copy"))
        .ok_or("missing nested-layout schema-2 row")?;
    let restore_row = |row: &[String]| -> Result<BodyLocal, Box<dyn std::error::Error>> {
        let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
        restore(&positions, &mut restored, row)?;
        restored.protocols.explicit_input_flow = body.local.protocols.explicit_input_flow.clone();
        Ok(restored)
    };
    let baseline = restore_row(row)?;
    checker.validate_restored_input_value_decisions(&baseline.protocols)?;
    for (label, target, offset) in [
        ("Carrier[Borrowed[str]] closed", borrowed_type, 2usize),
        ("Result[BorrowedMut[int], str] closed", native_type, 2usize),
        ("Carrier[JoinHandle[int]] Clone", uncloneable_type, 1usize),
    ] {
        let mut forged = row.clone();
        let count: usize = forged.get(5).ok_or("missing input-layout count")?.parse()?;
        let mut values = forged[6..].iter();
        let mut decision_index = None;
        for _ in 0..count {
            let ty = super::super::read_type(&mut values)?;
            let first_decision = forged.len() - values.len();
            if ty == target {
                decision_index = Some(first_decision + offset);
            }
            values.next().ok_or("missing Copy decision")?;
            values.next().ok_or("missing Clone decision")?;
            values.next().ok_or("missing closed-layout decision")?;
        }
        let decision_index = decision_index.ok_or_else(|| format!("missing {label} wire decision"))?;
        assert_eq!(forged[decision_index], "false", "{label} was not a negative source decision");
        forged[decision_index] = "true".into();
        let restored = restore_row(&forged)?;
        assert!(
            checker.validate_restored_input_value_decisions(&restored.protocols).is_err(),
            "schema-2 {label} false-to-true forgery passed live current-source validation"
        );
    }
    Ok(())
}

/// Schema-2 false decisions remain body/type scoped after restore. They close narrow layout/Clone replay routes even
/// when the graph and current AST/call facts are otherwise authentic.
#[test]
fn restored_value_layout_decisions_guard_narrow_replay_routes() -> Result<(), Box<dyn std::error::Error>> {
    let refuses = |label: &str,
                   source: &str,
                   function_name: &str,
                   target: fn(&crate::symbols::ResolvedType) -> bool,
                   clone_decision: bool|
     -> Result<(), Box<dyn std::error::Error>> {
        let (program, checker, owner_span) = fixture(source, function_name)?;
        assert!(checker.errors.is_empty(), "{label}: {:?}", checker.errors);
        let function = program.declarations.iter().find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == function_name => Some(function),
            _ => None,
        }).ok_or_else(|| format!("missing {label} declaration"))?;
        let owner = checker.type_info.protocols.explicit_input_flow.keys()
            .find(|owner| owner.declaration_name == function_name).cloned()
            .ok_or_else(|| format!("missing {label} flow owner"))?;
        checker.type_info.executable_function_input(&owner, function)
            .map_err(|reason| format!("{label} baseline full admission: {reason}"))?;
        let mut local = checker.checked_body(owner_span).ok_or_else(|| format!("missing {label} body"))?
            .local.as_ref().clone();
        let retained = local.protocols.explicit_input_copy.get_mut(&owner)
            .and_then(|decisions| decisions.iter_mut().find_map(|(ty, retained)| target(ty).then_some(retained)))
            .ok_or_else(|| format!("missing {label} target decision"))?;
        if clone_decision { retained.clone = false } else { retained.closed = false }
        let positions = Positions::for_local(&checker, &program, owner_span, &local)?;
        let mut fields = Vec::new();
        super::project(&positions, &local, &mut fields)?;
        let row = fields.iter().find(|row| row.first().is_some_and(|family| family == "input-copy"))
            .ok_or_else(|| format!("missing {label} schema-2 row"))?;
        assert_eq!(row.get(3).map(String::as_str), Some("2"), "{label}");
        let mut restored = BodyLocal { owner: owner_span, ..BodyLocal::default() };
        restore(&positions, &mut restored, row)?;
        let mut facts = checker.type_info.clone();
        facts.protocols.explicit_input_copy = restored.protocols.explicit_input_copy;
        assert!(facts.executable_function_input(&owner, function).is_err(),
            "{label} borrowed a positive decision from another type or graph fact");
        Ok(())
    };

    fn string(ty: &crate::symbols::ResolvedType) -> bool { *ty == crate::symbols::ResolvedType::Str }
    fn named_family(ty: &crate::symbols::ResolvedType) -> bool {
        matches!(ty, crate::symbols::ResolvedType::Named(name) if name == "Family")
    }
    fn named_payload(ty: &crate::symbols::ResolvedType) -> bool {
        matches!(ty, crate::symbols::ResolvedType::Named(name) if name == "Payload")
    }
    fn field_list(ty: &crate::symbols::ResolvedType) -> bool {
        matches!(ty, crate::symbols::ResolvedType::Generic(name, arguments)
            if incan_lang::lang::types::collections::from_str(name)
                == Some(incan_lang::lang::types::collections::CollectionTypeId::List)
                && matches!(arguments.as_slice(), [crate::symbols::ResolvedType::Named(element)] if element == "Field"))
    }

    refuses(
        "Try(Call) constructor",
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef text_value() -> Result[str, Fault]:\n    return Ok(\"value\")\ndef decode(seed: Borrowed[int]) -> Result[Record, Fault]:\n    return Ok(Record(text=text_value()?))\n",
        "decode", string, false,
    )?;
    refuses(
        "Element+Field helper",
        "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Borrowed[int]) -> str:\n    values = [Cell(value=\"a\")]\n    return keep(values[0].value)\n",
        "decode", string, true,
    )?;
    refuses(
        "Enum Parameter constructor",
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    return Field(family=family)\n",
        "collect", named_family, false,
    )?;
    refuses(
        "whole Local list constructor",
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    fields = [Field(value=\"a\")]\n    return Record(fields=fields)\n",
        "collect", field_list, false,
    )?;
    refuses(
        "match payload extraction",
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
        "decode", named_payload, false,
    )?;
    refuses(
        "Enum Parameter observer",
        "enum Family:\n    Local\ndef family_name(value: Family) -> str:\n    return \"local\"\ndef inspect(seed: Borrowed[int], family: Family) -> str:\n    return family_name(family)\n",
        "inspect", named_family, false,
    )?;
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

/// Record types retain nominal versus generic identity.
#[test]
fn result_requirement_types_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    use crate::symbols::ResolvedType;
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
    Ok(())
}

/// A generic free function is its own checked record, and so is a caller that instantiates it at two types: each
/// declaration and final record is position-free and reconstructs natively to its independent cold check.
#[test]
fn generic_owners_and_their_callers_are_records() -> Result<(), Box<dyn std::error::Error>> {
    let pick = "def pick[T](items: list[T]) -> T:\n    return items[2]\n\ndef main() -> None:\n    println(pick([1, 2, 3]))\n    println(pick([\"a\", \"b\", \"c\"]))\n";
    let pair = "def second[A, B](left: A, right: B) -> B:\n    return right\n";
    for (source, name) in [(pick, "pick"), (pick, "main"), (pair, "second")] {
        for declaration in [true, false] {
            let fields = super::super::fixture_record(source, name, declaration)?;
            let moved = format!("# moved owner\n{source}");
            assert_eq!(fields, super::super::fixture_record(&moved, name, declaration)?);
            super::super::verify_fixture(&moved, name, fields)?;
        }
    }
    Ok(())
}
