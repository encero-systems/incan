//! Graph body record tests: published method records, per-owner call reads, restored scopes and the physical inputs and
//! frames a selected graph body requires.

use super::*;

fn lower_graph_test_module(
    parsed: &crate::ast::Program,
    checker: &TypeChecker,
) -> incan_semantics_core::body_ir::BodyIrModule {
    let providers = crate::body_ir::ProviderOperationCatalog::new();
    crate::body_ir::build_body_ir_module(
        parsed,
        &["main".into()],
        checker.type_info(),
        &crate::body_ir::BodyIrContext {
            modules: crate::body_ir::ContextModules::default(),
            provider_operations: &providers,
            registry_package: None,
            library_package: None,
            include_inline_tests: false,
        },
    )
}

/// Method records come from the ordinary checked nominal and keep owners with the same member name distinct.
#[test]
fn ordinary_nominal_methods_publish_their_own_checked_records() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model Left:\n    value: int\n",
        "    def read(self) -> int:\n        return self.value\n\n",
        "model Right:\n    value: int\n",
        "    def read(self) -> int:\n        return self.value + 1\n"
    );
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(checker.check_work().method_bodies, 2);
    let lowered = lower_graph_test_module(&parsed, &checker);
    let outputs = super::checked_graph_record_outputs(&parsed, &checker, &["main".into()])?;
    assert_eq!(outputs.len(), 2);
    let mut selected = Vec::new();
    for (declaration, output) in parsed.declarations.iter().zip(outputs) {
        let crate::ast::Declaration::Model(model) = &declaration.node else {
            return Err("missing model".into());
        };
        let stable = format!("member/method:main::read#in(model:main::{})[0]", model.name);
        let CheckedOwnerRecordOutput::Value(owner) = output else {
            return Err(format!("{}: {output:?}", model.name).into());
        };
        assert_eq!(owner.identity, stable);
        for fields in [&owner.declaration, &owner.local, &owner.final_value] {
            assert_eq!(fields[0][1], stable);
        }
        assert!(
            owner
                .dependency_reads
                .iter()
                .any(|read| read.declaration_name == "value"),
            "{}: {:?}",
            model.name,
            owner.dependency_reads
        );
        let body = lowered
            .bodies
            .iter()
            .find(|body| body.span == owner.span)
            .ok_or("missing nominal frame")?;
        let witness = checker
            .graph_body_physical_inputs(&parsed, owner.span)
            .ok_or("missing nominal witness")?;
        let wire = incan_semantics_core::executable_representation::encode_graph_owner(
            &lowered.module_id,
            "src/main.incn",
            &owner.identity,
            body,
            Some(witness),
        )?;
        let mut record = selected_graph_record(owner)?;
        record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&wire)?);
        selected.push(record);
    }
    for (changed, expected_checks) in [
        (source.to_owned(), 0),
        (source.replace("self.value + 1", "self.value + 2"), 1),
    ] {
        let current = program(&changed)?;
        let mut warm = TypeChecker::new();
        warm.set_current_module_path(Some(vec!["main".into()]));
        warm.set_graph_body_records(super::GraphBodyRecords::new(selected.clone())?);
        warm.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
        assert_eq!(warm.check_work().method_bodies, expected_checks);
        let mut fresh = TypeChecker::new();
        fresh.set_current_module_path(Some(vec!["main".into()]));
        fresh.check_program(&current).map_err(|errors| format!("{errors:?}"))?;
        let restored_frames = current
            .declarations
            .iter()
            .flat_map(|declaration| crate::typechecker::canonical_nodes::nominal_methods(&declaration.node))
            .filter(|method| warm.type_info().selected_callable_body(method.span).is_some())
            .count();
        assert_eq!(restored_frames, 2 - expected_checks);
        assert_eq!(
            lower_graph_test_module(&current, &warm).bodies,
            lower_graph_test_module(&current, &fresh).bodies
        );
        assert_eq!(
            warm.type_info().references.resolved_identities,
            fresh.type_info().references.resolved_identities
        );
    }
    Ok(())
}

#[test]
fn per_owner_call_reads_survive_without_a_namespace_and_refuse_another_reader() -> Result<(), Box<dyn std::error::Error>>
{
    let module = vec!["main".to_owned()];
    let parsed = program("def callee() -> int:\n    return 1\n\ndef caller() -> int:\n    return callee()\n")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module.clone()));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let mut lowered = lower_graph_test_module(&parsed, &checker);
    let access = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&lowered))?;
    let reads = access.reads(&mut lowered);
    let [read] = reads.as_slice() else {
        return Err("expected one project call".into());
    };
    assert!(access.matches(read));
    let mut forged = read.clone();
    forged.digest = format!("sha256:{}", "0".repeat(64));
    assert!(!access.matches(&forged));
    let owner = checked_owner_record_outputs(&parsed, &checker, &module)?
        .into_iter()
        .find_map(|output| match output {
            CheckedOwnerRecordOutput::Value(owner) if owner.name == "caller" => Some(owner),
            _ => None,
        })
        .ok_or("missing caller")?;
    let body = lowered
        .bodies
        .iter()
        .find(|body| body.name == "caller")
        .ok_or("missing kernel caller")?;
    let source = "src/main.incn".to_owned();
    let wire = incan_semantics_core::executable_representation::encode_graph_owner(
        &lowered.module_id,
        &source,
        &owner.identity,
        body,
        Some([1; 32]),
    )?;
    let mut record = selected_graph_record(owner.clone())?;
    let select = |record: super::GraphBodyRecord| super::GraphBodyRecords::new(vec![record]);
    record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&wire)?);
    assert!(
        select(record.clone())?
            .get(&owner.identity)?
            .ok_or("missing old record")?
            .body_ir
            .is_none()
    );
    let encoded_reads = vec![internal_models::SettledCallRead {
        reader: read.reader.clone(),
        target: read.target.clone(),
        digest: read.digest.clone(),
    }];
    for (identity, allowed) in [(&owner.identity, true), (&"other".to_owned(), false)] {
        let witness = internal_models::encode_owner_call_reads(identity, &read.reader, &source, &encoded_reads)?;
        let mut guarded = wire.clone();
        guarded.extend_from_slice(&witness);
        record.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&guarded)?);
        let mut selected = select(record.clone())?;
        if allowed {
            let mut retry = super::GraphBodyRecords::default();
            retry.retain_current(vec![record.clone()]);
            assert!(retry.checked_here(&owner.identity));
            assert!(
                retry
                    .get(&owner.identity)?
                    .ok_or("missing current provider")?
                    .body_ir
                    .is_some()
            );
            assert_eq!(retry.selected_call_reads(&source)?, reads);
            assert!(
                selected
                    .get(&owner.identity)?
                    .ok_or("missing current frame")?
                    .body_ir
                    .is_some()
            );
            assert_eq!(selected.selected_call_reads(&source)?, reads);
            selected.reopen_callers(
                [(source.clone(), [read.reader.clone()].into_iter().collect())]
                    .into_iter()
                    .collect(),
            )?;
            assert!(
                selected
                    .get(&owner.identity)?
                    .ok_or("missing checked pair")?
                    .body_ir
                    .is_none()
            );
            assert!(selected.selected_call_reads(&source)?.is_empty());
        } else {
            assert!(selected.get(&owner.identity).is_err());
        }
    }
    Ok(())
}

/// A graph peer restores all body-internal scopes in source order, including empty branches, before a later owner.
#[test]
fn restored_nested_scopes_match_fresh_bindings_in_later_owners() -> Result<(), Box<dyn std::error::Error>> {
    let source = program(concat!(
        "def count(items: list[int]) -> int:\n",
        "    mut total = 0\n",
        "    for item in items:\n",
        "        if item > 0:\n",
        "            kept = item\n",
        "            total += kept\n",
        "        else:\n",
        "            pass\n",
        "    return total\n\n",
        "def next_value(number: int) -> int:\n    return number + 1\n",
    ))?;
    let module = vec!["main".to_owned()];
    let mut cold = TypeChecker::new();
    cold.set_current_module_path(Some(module.clone()));
    cold.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
    let checked = checked_owner_record_outputs(&source, &cold, &module)?;
    let mut warm = TypeChecker::new();
    warm.set_current_module_path(Some(module));
    let Some(CheckedOwnerRecordOutput::Value(owner)) = checked
        .iter()
        .find(|v| matches!(v,CheckedOwnerRecordOutput::Value(o) if o.name=="count"))
    else {
        return Err(format!("scope owner produced no checked rows: {checked:?}").into());
    };
    let selected = selected_graph_record(owner.clone())?;
    let allocation = selected
        .local
        .iter()
        .find(|row| row[0] == "scope")
        .ok_or("no lexical scope tree")?;
    assert!(allocation[3].parse::<usize>()? >= 3);
    warm.set_graph_body_records(super::GraphBodyRecords::new(vec![selected.clone()])?);
    warm.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(warm.check_work().function_bodies, 1);
    assert_eq!(warm.symbols.next_scope_index(), cold.symbols.next_scope_index());
    assert_eq!(
        warm.type_info().references.resolved_identities,
        cold.type_info().references.resolved_identities
    );
    assert_eq!(
        warm.type_info().references.resolved_write_identities,
        cold.type_info().references.resolved_write_identities
    );

    let mut bad = selected;
    let layout = std::sync::Arc::make_mut(&mut bad.local)
        .iter_mut()
        .find(|row| row[0] == "scope")
        .ok_or("missing scope tree")?;
    layout[4] = "99".into();
    let mut refused = TypeChecker::new();
    refused.set_current_module_path(Some(vec!["main".into()]));
    refused.set_graph_body_records(super::GraphBodyRecords::new(vec![bad])?);
    refused.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(refused.check_work().function_bodies, 2);
    assert_eq!(refused.symbols.next_scope_index(), cold.symbols.next_scope_index());
    Ok(())
}

/// A retained native body needs its current positions as well as the reusable position-independent check result.
#[test]
fn selected_graph_body_requires_current_physical_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def twice(value: int) -> int:\n    return  value + value\n";
    let module = vec!["main".to_string()];
    let original = program(source)?;
    let check = |parsed: &crate::ast::Program, records: Vec<super::GraphBodyRecord>| -> Result<TypeChecker, String> {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
        checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
        Ok(checker)
    };
    let cold = check(&original, Vec::new())?;
    let checked = checked_owner_record_outputs(&original, &cold, &module)?;
    let [CheckedOwnerRecordOutput::Value(owner)] = checked.as_slice() else {
        return Err("missing checked native owner".into());
    };
    let lowered = lower_graph_test_module(&original, &cold);
    let body = lowered.bodies.first().ok_or("missing lowered body")?;
    let witness = cold
        .graph_body_physical_inputs(&original, owner.span)
        .ok_or("missing positioned producer witness")?;
    for (input, guarded, expected) in [
        (source.to_string(), true, true),
        (source.to_string(), false, false),
        (
            source.replace("return  value + value", "return value  + value"),
            true,
            false,
        ),
        (format!("# moved\n{source}"), true, false),
    ] {
        let encoded = incan_semantics_core::executable_representation::encode_graph_owner(
            &lowered.module_id,
            "src/main.incn",
            &owner.identity,
            body,
            guarded.then_some(witness),
        )?;
        let frame = incan_semantics_core::executable_representation::decode_owner(&encoded)?;
        let mut record = selected_graph_record(owner.clone())?;
        record.body_ir = Some(frame);
        let parsed = program(&input)?;
        let warm = check(&parsed, vec![record])?;
        let fresh = check(&parsed, vec![])?;
        let span = parsed.declarations[0].span;
        assert_eq!(warm.check_work().function_bodies, 0);
        assert_eq!(
            warm.type_info().selected_callable_body(span).is_some(),
            expected,
            "{input:?}, guarded={guarded}"
        );
        assert_eq!(
            lower_graph_test_module(&parsed, &warm).bodies,
            lower_graph_test_module(&parsed, &fresh).bodies
        );
    }
    Ok(())
}

/// Rebinding source positions must reproduce the exact fresh native body for both the edited owner and its caller.
#[test]
fn selected_scalar_frames_rebind_comments_and_documentation() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{append_owner_coordinates, decode_owner, encode_graph_owner};
    let src = "def greeting() -> str:\n    return \"hello\"\n\n\ndef main() -> None:\n    println(greeting())\n";
    let module = vec!["main".to_owned()];
    let check = |parsed: &crate::ast::Program, records| -> Result<TypeChecker, String> {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
        checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
        Ok(checker)
    };
    let parsed = program(src)?;
    let first = check(&parsed, Vec::new())?;
    let mut native = lower_graph_test_module(&parsed, &first);
    let access = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&native))?;
    let reads = access.reads(&mut native);
    let mut records = Vec::new();
    for output in checked_owner_record_outputs(&parsed, &first, &module)? {
        let CheckedOwnerRecordOutput::Value(owner) = output else {
            return Err("required scalar owner missing".into());
        };
        let body = native
            .bodies
            .iter()
            .find(|body| body.name == owner.name)
            .ok_or("missing native body")?;
        let (proof, coordinates) = first
            .graph_body_inputs(&parsed, owner.span, true)
            .ok_or("missing checked placement")?;
        assert!(!coordinates.is_empty());
        let mut bytes = encode_graph_owner(&native.module_id, "main.incn", &owner.identity, body, Some(proof))?;
        append_owner_coordinates(&mut bytes, &coordinates);
        let reader = body.direct_call_id.path();
        let calls = reads
            .iter()
            .filter(|read| read.reader == reader)
            .map(|read| internal_models::SettledCallRead {
                reader: read.reader.clone(),
                target: read.target.clone(),
                digest: read.digest.clone(),
            })
            .collect::<Vec<_>>();
        bytes.extend_from_slice(&internal_models::encode_owner_call_reads(
            &owner.identity,
            reader,
            "main.incn",
            &calls,
        )?);
        let frame = decode_owner(&bytes)?;
        assert_eq!(frame.coordinates, coordinates);
        let mut record = selected_graph_record(owner)?;
        record.body_ir = Some(frame);
        records.push(record);
    }
    assert_eq!(records.len(), 2);
    for changed in [
        src.replace("def main()", "# a comment before main\ndef main()"),
        src.replace("    return", "    \"\"\"New documentation.\"\"\"\n    return"),
    ] {
        let after = program(&changed)?;
        let warm = check(&after, records.clone())?;
        let fresh = check(&after, Vec::new())?;
        for decl in &after.declarations {
            assert!(
                warm.type_info().selected_callable_body(decl.span).is_some(),
                "decl {:?} in {changed}",
                decl.span
            );
            assert_eq!(
                warm.completed_graph_body_digest(decl.span),
                warm.graph_body_physical_inputs(&after, decl.span)
            );
        }
        assert_eq!(
            lower_graph_test_module(&after, &warm).bodies,
            lower_graph_test_module(&after, &fresh).bodies
        );
        let mut corrupted = records.clone();
        for record in &mut corrupted {
            let frame = record.body_ir.as_mut().ok_or("no selected body")?;
            frame.physical_inputs = Some([0x42; 32]);
        }
        let refused = check(&after, corrupted)?;
        assert!(
            after
                .declarations
                .iter()
                .all(|decl| refused.type_info().selected_callable_body(decl.span).is_none())
        );
    }
    Ok(())
}

/// Scalar products and graph pages can select the same untouched callable during an ordinary edit.
/// Only its current checker bindings can authenticate the graph frame that will be published again.
#[test]
fn graph_body_republication_with_a_competing_scalar_product() -> Result<(), Box<dyn std::error::Error>> {
    use crate::declaration_interface::{CallableBodyInputs, ValidatedCallableBodies};
    use incan_semantics_core::executable_representation::{decode_owner, encode_graph_owner};

    let source = "def greeting() -> int:\n    return 1\n\ndef twice(value: int) -> int:\n    return  value + value\n";
    let module = vec!["main".to_owned()];
    let check = |parsed: &crate::ast::Program, records, scalar: Option<ValidatedCallableBodies>| {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
        if let Some(scalar) = scalar {
            checker.set_callable_body_reuse(scalar);
        }
        checker.check_program(parsed).map_err(|errors| format!("{errors:?}"))?;
        Ok::<_, String>(checker)
    };
    let original = program(source)?;
    let baseline = check(&original, vec![], None)?;
    let lowered = lower_graph_test_module(&original, &baseline);
    let products =
        CallableBodyInputs::from_checked_for_unit(&original, source, &baseline, "fixture")?.finish(&lowered)?;
    let owner = checked_owner_record_outputs(&original, &baseline, &module)?
        .into_iter()
        .find_map(|output| match output {
            CheckedOwnerRecordOutput::Value(owner) if owner.name == "twice" => Some(owner),
            _ => None,
        })
        .ok_or("missing checked peer")?;
    let publish =
        |parsed: &crate::ast::Program, checker: &TypeChecker| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            let lowered = lower_graph_test_module(parsed, checker);
            let body = lowered
                .bodies
                .iter()
                .find(|body| body.name == "twice")
                .ok_or("missing lowered peer")?;
            let physical = checker
                .graph_body_physical_inputs(parsed, body.span)
                .ok_or("missing current physical inputs")?;
            Ok(encode_graph_owner(
                &lowered.module_id,
                "main.incn",
                &owner.identity,
                body,
                Some(physical),
            )?)
        };
    let frame = decode_owner(&publish(&original, &baseline)?)?;
    let valid = frame.physical_inputs.ok_or("missing baseline proof")?;
    let mut corrupt = valid;
    corrupt[0] ^= 1;
    let edited = source.replace("return 1", "return 2");
    let scalar_only_program = program(&edited)?;
    let scalar_only = check(
        &scalar_only_program,
        vec![],
        Some(products.select(&scalar_only_program, &edited, "fixture", &module)?),
    )?;
    let peer = scalar_only_program.declarations[1].span;
    assert!(scalar_only.type_info().selected_callable_body(peer).is_some());
    assert_eq!(scalar_only.check_work().function_bodies, 1);

    for (input, proof, corrupt_rows, expected_scalar, expected_frame) in [
        (edited.clone(), Some(valid), false, 1, true),
        (edited.clone(), None, false, 1, false),
        (edited.clone(), Some(corrupt), false, 1, false),
        (edited.clone(), Some(valid), true, 1, false),
        (
            edited.replace("return  value + value", "return value  + value"),
            Some(valid),
            false,
            0,
            false,
        ),
    ] {
        let parsed = program(&input)?;
        let selection = products.select(&parsed, &input, "fixture", &module)?;
        assert_eq!(selection.len(), expected_scalar);
        let mut record = selected_graph_record(owner.clone())?;
        let mut candidate = frame.clone();
        candidate.physical_inputs = proof;
        record.body_ir = Some(candidate);
        if corrupt_rows {
            Arc::make_mut(&mut record.final_value).retain(|row| !row.first().is_some_and(|field| field == "scope"));
        }
        let warm = check(&parsed, vec![record], Some(selection))?;
        let fresh = check(&parsed, vec![], None)?;
        assert_eq!(warm.check_work().function_bodies, if corrupt_rows { 2 } else { 1 });
        assert_eq!(
            warm.type_info()
                .selected_callable_body(parsed.declarations[1].span)
                .is_some(),
            expected_frame
        );
        assert_eq!(publish(&parsed, &warm)?, publish(&parsed, &fresh)?);
    }
    Ok(())
}

/// Moving a callee invalidates its current physical call target. An earlier owner with no read of that
/// declaration keeps its already lowered frame, even when both owners received valid checker records.
#[test]
fn physical_owner_frames_follow_selected_callable_handles() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def unrelated() -> int:\n    return 1\n\ndef caller() -> int:\n    return provider()\n\ndef caller_through_value() -> int:\n    callback = provider\n    return callback()\n\ndef provider() -> int:\n    return 20\n";
    let module = vec!["main".to_owned()];
    let check = |program: &crate::ast::Program, records| -> Result<TypeChecker, String> {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|err| err.to_string())?);
        checker.check_program(program).map_err(|errors| format!("{errors:?}"))?;
        Ok(checker)
    };
    let original = program(source)?;
    let cold = check(&original, vec![])?;
    let lowered = lower_graph_test_module(&original, &cold);
    let mut inputs = Vec::new();
    for output in checked_owner_record_outputs(&original, &cold, &module)? {
        let CheckedOwnerRecordOutput::Value(owner) = output else {
            return Err("missing checked callable in handle fixture".into());
        };
        if owner.name == "provider" {
            continue;
        }
        let body = lowered
            .bodies
            .iter()
            .find(|body| body.name == owner.name)
            .ok_or("missing lowered owner")?;
        let witness = cold
            .graph_body_physical_inputs(&original, owner.span)
            .ok_or("missing physical inputs")?;
        let bytes = incan_semantics_core::executable_representation::encode_graph_owner(
            &lowered.module_id,
            "src/main.incn",
            &owner.identity,
            body,
            Some(witness),
        )?;
        let mut input = selected_graph_record(owner)?;
        input.body_ir = Some(incan_semantics_core::executable_representation::decode_owner(&bytes)?);
        inputs.push(input);
    }
    let edited = program(&source.replace("return 20", "return 2000"))?;
    let warm = check(&edited, inputs)?;
    let fresh = check(&edited, vec![])?;
    assert!(
        warm.type_info()
            .selected_callable_body(edited.declarations[0].span)
            .is_some()
    );
    assert!(
        warm.type_info()
            .selected_callable_body(edited.declarations[1].span)
            .is_none()
    );
    assert!(
        warm.type_info()
            .selected_callable_body(edited.declarations[2].span)
            .is_none()
    );
    assert_eq!(
        lower_graph_test_module(&edited, &warm).bodies,
        lower_graph_test_module(&edited, &fresh).bodies
    );
    Ok(())
}

/// Frame admission tracks a named type the owner uses and ignores a physically moved ambient type.
#[test]
fn physical_owner_frames_follow_selected_nominal_reads() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = program(concat!(
        "model Selected:\n    value: int\n\n",
        "model Unused:\n    value: int\n\n",
        "def take(item: Selected) -> int:\n    return item.value\n",
    ))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_owned()]));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let owner = parsed.declarations[2].span;
    let before = checker
        .graph_body_physical_inputs(&parsed, owner)
        .ok_or("missing initial proof")?;
    for (name, same) in [("Unused", true), ("Selected", false)] {
        let binding = checker
            .type_info
            .declarations
            .named_type_identities
            .get_mut(name)
            .ok_or("missing declared type binding")?;
        binding.declaration_span.start += 1;
        let current = checker
            .graph_body_physical_inputs(&parsed, owner)
            .ok_or("missing updated proof")?;
        assert_eq!(current == before, same, "{name}");
    }
    Ok(())
}

/// Moving an imported function changes the executable handles for its direct and first-class users.
/// A separate function in the consumer keeps the same authenticated executable inputs.
#[test]
fn physical_owner_frames_follow_foreign_callable_reads() -> Result<(), Box<dyn std::error::Error>> {
    let consumer = program(concat!(
        "from provider import value\n\n",
        "def unrelated() -> int:\n    return 1\n\n",
        "def direct() -> int:\n    return value()\n\n",
        "def indirect() -> int:\n    callback = value\n    return callback()\n",
    ))?;
    let fingerprints = |provider: &crate::ast::Program| -> Result<BTreeMap<String, [u8; 32]>, String> {
        let path = vec!["provider".to_owned()];
        let mut declarations = TypeChecker::new();
        declarations.set_current_module_path(Some(path.clone()));
        declarations.register_dependency_module_path_segments("provider", path.clone());
        let interface = declarations
            .build_dependency_interface_component(
                &[(
                    "provider",
                    provider,
                    Arc::from(crate::module::exported_symbols(provider)),
                )],
                &[],
                true,
            )
            .pop()
            .ok_or("provider interface missing")?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".to_owned()]));
        checker.register_dependency_module_path_segments("provider", path);
        checker.select_dependency_interfaces(vec![interface]);
        checker
            .check_with_imports(&consumer, &[("provider", provider)])
            .map_err(|errors| format!("{errors:?}"))?;
        consumer
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                crate::ast::Declaration::Function(function) => Some((function.name.clone(), decl.span)),
                _ => None,
            })
            .map(|(name, span)| {
                checker
                    .graph_body_physical_inputs(&consumer, span)
                    .map(|fingerprint| (name.clone(), fingerprint))
                    .ok_or_else(|| format!("missing physical proof for {name}"))
            })
            .collect()
    };
    let provider = "pub def value() -> int:\n    return 20\n";
    let original = fingerprints(&program(provider)?)?;
    let moved = fingerprints(&program(&format!("# moved\n{provider}"))?)?;
    assert_eq!(original.len(), 3);
    assert_ne!(original["direct"], moved["direct"]);
    assert_ne!(original["indirect"], moved["indirect"]);
    assert_eq!(original["unrelated"], moved["unrelated"]);
    Ok(())
}

/// A loan-free match scrutinee is retained as a reusable owner fact instead of forcing a source fallback.
#[test]
fn loan_free_match_owner_publishes_reusable_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def main() -> int:\n    match 1:\n        1 => return 2\n        _ => return 3\n";
    let program = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;

    let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
    assert_eq!(outputs.len(), 1, "a checked owner was silently omitted");
    let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
        return Err("the match owner fell back".into());
    };
    assert_eq!(owner.name, "main");
    assert!(
        owner
            .final_value
            .iter()
            .any(|row| row[0] == "artifact-protocol" && row[2] == "loan_free_scrutinees" && row[3] == "true")
    );
    Ok(())
}

/// A checked owner whose signature cannot produce a stable identity remains an explicit source fallback. The Rust
/// carrier `Rc` has no source identity, so the signature publishes no portable key.
#[test]
fn identityless_checked_owner_publishes_fallback_reason() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::std::rc import Rc\n\nmodel Guard:\n    held: int\n\ndef required_pin(pin: Option[Rc[Guard]]) -> Result[Rc[Guard], str]:\n    return Err(\"missing\")\n";
    let program = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;

    let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
    assert_eq!(outputs.len(), 1, "a checked owner was silently omitted");
    let CheckedOwnerRecordOutput::Fallback(fallback) = &outputs[0] else {
        return Err("the identity-less owner unexpectedly published a reusable value".into());
    };
    assert_eq!(fallback.name, "required_pin");
    assert_eq!(fallback.identity, None);
    assert_eq!(fallback.reason, "checked owner has no stable declaration identity");
    Ok(())
}
