//! Native restoration independently authenticates the retained invalid-graph fault.

use std::collections::BTreeMap;

use super::super::{BodyLocal, Positions, fixture};
use super::{InputFlow, Reader, restore};

/// Rebind a Rust field through the current provider and the record's exact checked field occurrence.
#[test]
fn restored_rust_field_requires_an_admitted_field_and_occurrence() -> Result<(), Box<dyn std::error::Error>> {
    use crate::symbols::ResolvedType;
    use crate::typechecker::type_info::RustCheckedFieldValue;
    use incan_lang::interop::RustTypeShape;
    use incan_lang::interop::metadata::{
        RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeMetadataCompleteness, RustVisibility,
    };
    use incan_semantics_core::SemanticSourceTargetKind;

    let (program, mut checker, owner) = fixture("def view() -> int:\n    return 1\n", "view")?;
    let path = "std::process::Output";
    let nominal = incan_semantics_core::body_ir::rust_item_identity(path).ok_or("missing Rust owner")?;
    let selected = nominal.generated_member("stderr", SemanticSourceTargetKind::Field);
    let metadata = RustItemMetadata {
        canonical_path: path.into(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo {
            metadata_completeness: RustTypeMetadataCompleteness::CompleteWithPrivateFields(vec!["private".into()]),
            fields: ["stderr", "private"]
                .into_iter()
                .map(|name| RustFieldInfo {
                    name: name.into(),
                    type_display: "Vec<u8>".into(),
                    type_shape: RustTypeShape::Bytes,
                })
                .collect(),
            ..RustTypeInfo::default()
        }),
    };
    let mut loaves = crate::loaf_index::LoafIndex::default();
    loaves.add_rust_facts(&Default::default(), BTreeMap::from([(path.into(), metadata)]));
    checker.set_loaves(std::sync::Arc::new(loaves));
    let positions = Positions::new(&checker, &program, owner, false)?;
    let mut local = BodyLocal {
        owner,
        ..Default::default()
    };
    local.references.resolved_identities.insert(owner, selected.clone());
    local.rust.field_values.insert(
        owner,
        RustCheckedFieldValue {
            native_type: ResolvedType::Bytes,
            item_path: None,
        },
    );
    let mut reference = Vec::new();
    super::super::projections::project_references(&positions, &local, &mut reference)?;
    let row = reference.first().ok_or("missing selected reference")?;
    assert_eq!(reference.len(), 1);

    let mut restored = BodyLocal {
        owner,
        ..Default::default()
    };
    let mut identities = BTreeMap::new();
    assert!(super::super::restore_local(&checker, &positions, &mut restored, &mut identities, row).is_err());
    let mut native = Vec::new();
    super::super::rust_artifacts_generated::project(&positions, &local.rust, &mut native)?;
    for field in &native {
        super::super::restore_local(&checker, &positions, &mut restored, &mut identities, field)?;
    }
    let mut forged = row.clone();
    forged[3] = forged[3].replace("::stderr", "::private");
    assert!(super::super::restore_local(&checker, &positions, &mut restored, &mut identities, &forged).is_err());
    super::super::restore_local(&checker, &positions, &mut restored, &mut identities, row)?;
    assert_eq!(restored.references.resolved_identities.get(&owner), Some(&selected));
    Ok(())
}

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

/// Only the native result selected at this expression can supply a missing nominal type identity.
#[test]
fn restored_native_result_requires_the_same_checked_call() -> Result<(), Box<dyn std::error::Error>> {
    let (program, checker, owner) = fixture("def entry() -> int:\n    return 1\n", "entry")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let path = positions.nodes.anchor(owner)?.path;
    let mut local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    let canonical =
        incan_semantics_core::body_ir::rust_item_identity("std::env::Args").ok_or("missing native identity")?;
    let stable = incan_semantics_core::stable_identity::StableDeclarationId::from_canonical(&canonical, None, None)
        .ok_or("missing stable native identity")?
        .render_compact();
    let row = vec!["type-identity".into(), path, "root".into(), stable];
    let mut identities = BTreeMap::new();
    assert!(super::super::restore_local(&checker, &positions, &mut local, &mut identities, &row).is_err());
    local.rust.checked_calls.insert(
        owner,
        crate::typechecker::type_info::RustCheckedCall {
            storage: None,
            source_type_parameters: Vec::new(),
            signature_loans: Vec::new(),
            receiver_adjustments: Vec::new(),
            path: "std::env::args".into(),
            type_arguments: Vec::new(),
            parameters: Vec::new(),
            return_type: crate::symbols::ResolvedType::RustPath("std::env::Args".into()),
            result_item_path: Some("std::env::Args".into()),
            has_receiver: false,
        },
    );
    let mut wrong = row.clone();
    wrong[3] = wrong[3].replace("Args", "Other");
    assert!(super::super::restore_local(&checker, &positions, &mut local, &mut identities, &wrong).is_err());
    super::super::restore_local(&checker, &positions, &mut local, &mut identities, &row)?;
    assert_eq!(
        local
            .expressions
            .expression_type_identities
            .get(&owner)
            .and_then(|identities| identities.get(&Vec::new())),
        Some(&canonical)
    );
    Ok(())
}

/// A nested Rust result component must match both the checked expression type and the current ABI item.
#[test]
fn restored_native_result_component_keeps_its_checked_abi_identity() -> Result<(), Box<dyn std::error::Error>> {
    use crate::loaf_index::LoafIndex;
    use crate::symbols::ResolvedType;
    use incan_lang::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustVisibility};

    let (program, mut checker, owner) = fixture("def entry() -> int:\n    return 1\n", "entry")?;
    let native = "std::fs::File";
    let mut loaves = LoafIndex::default();
    loaves.add_rust_facts(
        &Default::default(),
        BTreeMap::from([(
            native.into(),
            RustItemMetadata {
                canonical_path: native.into(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo::default()),
            },
        )]),
    );
    checker.set_loaves(std::sync::Arc::new(loaves));
    let positions = Positions::new(&checker, &program, owner, false)?;
    let path = positions.nodes.anchor(owner)?.path;
    let mut local = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    local.expressions.expr_types.insert(
        owner,
        ResolvedType::Generic(
            "Result".into(),
            vec![ResolvedType::RustPath(native.into()), ResolvedType::Str],
        ),
    );
    let canonical = incan_semantics_core::body_ir::rust_item_identity(native).ok_or("missing native identity")?;
    let stable = incan_semantics_core::stable_identity::StableDeclarationId::from_canonical(&canonical, None, None)
        .ok_or("missing stable native identity")?
        .render_compact();
    let row =
        |selector: &str, identity: &str| vec!["type-identity".into(), path.clone(), selector.into(), identity.into()];
    let mut identities = BTreeMap::new();
    assert!(
        super::super::restore_local(&checker, &positions, &mut local, &mut identities, &row("1", &stable)).is_err()
    );
    assert!(
        super::super::restore_local(
            &checker,
            &positions,
            &mut local,
            &mut identities,
            &row("0", &stable.replace("File", "Other"))
        )
        .is_err()
    );
    super::super::restore_local(&checker, &positions, &mut local, &mut identities, &row("0", &stable))?;
    assert_eq!(
        local
            .expressions
            .expression_type_identities
            .get(&owner)
            .and_then(|owners| owners.get(&vec![0])),
        Some(&canonical)
    );
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
        super::super::restore_local(&checker, &positions, &mut restored, &mut BTreeMap::new(), row)?;
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
    super::super::restore_local(&checker, &positions, &mut restored, &mut BTreeMap::new(), row)?;
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

/// Only the current ownership schemas can restore authority; legacy rows remain a safe fresh-check fallback.
#[test]
fn legacy_input_schemas_are_not_restored() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(value: Owned[int]) -> int:\n    return value\n";
    let (program, checker, owner) = fixture(source, "keep")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let view = checker.checked_body(owner).ok_or("missing input owner body")?;
    let mut fields = Vec::new();
    super::project(&positions, &view.local, &mut fields)?;
    let flow = fields
        .iter()
        .find(|row| row.first().is_some_and(|family| family == "input-flow"))
        .ok_or("missing current input-flow row")?;
    let copy = fields
        .iter()
        .find(|row| row.first().is_some_and(|family| family == "input-copy"))
        .ok_or("missing current input-copy row")?;

    for version in ["1", "2", "3", "4"] {
        let mut legacy = flow.clone();
        legacy[3] = version.into();
        let mut restored = BodyLocal {
            owner,
            ..BodyLocal::default()
        };
        assert!(restore(&positions, &mut restored, &legacy).is_err());
        assert!(restored.protocols.explicit_input_flow.is_empty());
    }
    let mut legacy_copy = copy.clone();
    legacy_copy[3] = "1".into();
    let mut restored = BodyLocal {
        owner,
        ..BodyLocal::default()
    };
    assert!(restore(&positions, &mut restored, &legacy_copy).is_err());
    assert!(restored.protocols.explicit_input_copy.is_empty());
    Ok(())
}

/// A body admitted by current-source value classification publishes no reusable local or final record until that
/// classification has a wire representation. The next activation therefore checks the body afresh.
#[test]
fn current_source_value_uses_are_not_projected_as_record_authority() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n";
    let (program, checker, owner) = fixture(source, "collect")?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let view = checker.checked_body(owner).ok_or("missing classified owner body")?;
    assert!(
        view.local
            .protocols
            .explicit_input_flow
            .values()
            .any(|flow| matches!(flow, InputFlow::Checked { graph, .. } if !graph.value_uses.is_empty()))
    );
    let mut fields = Vec::new();
    assert_eq!(
        super::project(&positions, &view.local, &mut fields),
        Err(super::NodeBindingError::Unsupported(
            "current-source value classification"
        ))
    );
    assert!(checker.produced_local_record(owner).is_none());
    assert!(checker.produced_final_record(owner).is_none());
    assert!(checker.local_record_refusal(owner).is_some());
    assert_eq!(
        checker.final_record_refusal(owner),
        Some(super::NodeBindingError::Unsupported(
            "current-source value classification"
        ))
    );

    let mut fresh = crate::typechecker::TypeChecker::new();
    fresh.set_graph_body_records(crate::checker_records::GraphBodyRecords::new(Vec::new())?);
    fresh.check_program(&program).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(fresh.check_work().function_bodies, 2);
    assert_eq!(fresh.check_work().body_view_reuses, 0);
    assert!(!fresh.restored_body_record(owner));
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
        super::super::restore_local(&checker, &positions, &mut local, &mut BTreeMap::new(), &row)?;
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
        assert!(super::super::restore_local(&checker, &positions, &mut local, &mut BTreeMap::new(), &row).is_err());
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
    assert!(super::super::restore_local(&checker, &positions, &mut local, &mut BTreeMap::new(), &wrong).is_err());
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
