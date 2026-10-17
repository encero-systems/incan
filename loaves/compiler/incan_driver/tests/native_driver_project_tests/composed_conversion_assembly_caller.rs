//! Actual-driver assembly controls for compilation-scoped conversion authority.

mod error;
#[path = "frontend.rs"]
mod frontend;
mod source_headers;

use incan_mir_lowering::caller::incan::{Plan, PlanType, lower_program};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{ExecutableDeclaration, SurfaceReader, build_surface};
use incan_semantics_core::{CompilerNodeId, SymbolOrigin, canonical_module_identity};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
type Checked = (Vec<BodyIrModule>, Vec<String>, Vec<String>, usize);

/// A source display and its published entry may coexist; duplicate bodies, duplicate entries, and invalid native
/// targets may not.
fn check_selected_display_refusals(checked: &Checked) -> TestResult<()> {
    let Some((index, identity)) = checked.0.iter().enumerate().find_map(|(index, module)| {
        module.nominal_declarations.iter().find_map(|owner| {
            (owner.canonical.declaration_name == "HashError")
                .then_some(owner.display_method.as_ref())
                .flatten()
                .map(|identity| (index, identity))
        })
    }) else {
        return Ok(());
    };
    let body = checked.0[index]
        .bodies
        .iter()
        .find(|body| body.canonical.as_ref() == Some(identity))
        .ok_or("selected HashError source display missing")?;
    let slot = checked.0[index]
        .stdlib_delegations
        .iter()
        .position(|entry| &entry.canonical == identity)
        .ok_or("selected HashError native display missing")?;
    let mut duplicate_body = checked.clone();
    duplicate_body.0[index].bodies.push(body.clone());
    assert!(lower(&duplicate_body).is_err());
    let mut duplicate_entry = checked.clone();
    duplicate_entry.0[index]
        .stdlib_delegations
        .push(checked.0[index].stdlib_delegations[slot].clone());
    assert!(lower(&duplicate_entry).is_err());
    let mut invalid_target = checked.clone();
    invalid_target.0[index].stdlib_delegations[slot].target.origin = SymbolOrigin::Builtin;
    assert!(lower(&invalid_target).is_err());
    Ok(())
}

/// Missing executable bodies and foreign owners remain errors when directly presented to the unchanged planner.
fn check_unselected_published_implementation_refusals(checked: &Checked) -> TestResult<()> {
    use incan_semantics_core::body_ir::TraitImplementation;
    let Some((index, owner)) = checked.0.iter().enumerate().find_map(|(index, module)| {
        module
            .nominal_declarations
            .iter()
            .find(|owner| owner.canonical.declaration_name == "CompressionError")
            .map(|owner| (index, owner))
    }) else {
        return Ok(());
    };
    let adoption = owner
        .trait_adoptions
        .first()
        .ok_or("published Error adoption missing")?;
    let method = adoption
        .methods
        .as_ref()
        .and_then(|methods| methods.iter().find(|method| method.declaration_name == "message"))
        .ok_or("published Error.message slot missing")?;
    let implementation = owner
        .display_method
        .as_ref()
        .ok_or("published display identity missing")?;
    let record = TraitImplementation {
        owner: owner.canonical.clone(),
        method: method.clone(),
        implementation: implementation.clone(),
        type_arguments: Vec::new(),
    };
    if checked
        .0
        .iter()
        .flat_map(|module| &module.bodies)
        .any(|body| body.canonical.as_ref() == Some(implementation))
    {
        assert!(
            checked
                .0
                .iter()
                .flat_map(|module| &module.trait_implementations)
                .any(|selected| selected == &record)
        );
        return Ok(());
    }
    let mut missing = checked.clone();
    missing.0[index].trait_implementations.push(record.clone());
    assert!(lower(&missing).is_err());
    let mut foreign = checked.clone();
    let mut record = record;
    record.owner.declaration_name.push_str("_foreign");
    foreign.0[index].trait_implementations.push(record);
    assert!(lower(&foreign).is_err());
    Ok(())
}

/// Execute the actual session and frontend assembly for each independent import context.
fn main() -> TestResult<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let [mode, source, snapshot] = arguments.as_slice()
        && (mode == "--source-plan-probe" || mode == "--source-plan-check")
    {
        let checked = frontend::collect_for_test(Path::new(source))?;
        std::fs::write(snapshot, serde_json::to_vec_pretty(&checked.0)?)?;
        let plan = lower(&checked)?;
        if mode == "--source-plan-probe" {
            assert!(plan.models.iter().any(|model| model.source_name == "ColumnRefExpr"));
            assert!(plan.models.iter().any(|model| model.source_name == "AggregateMeasure"));
        } else {
            check_unselected_published_implementation_refusals(&checked)?;
            check_selected_display_refusals(&checked)?;
        }
        println!("actual source session and Incan planner passed");
        return Ok(());
    }
    let root = arguments.first().ok_or("source directory missing")?;
    let root = Path::new(&root);
    let original = frontend::collect_for_test(&root.join("original/main.incn"))?;
    check_selected(&original)?;
    for name in ["direct", "qualified", "dependency", "dependency_owners"] {
        let checked = frontend::collect_for_test(&root.join(name).join("main.incn"))?;
        check_selected(&checked)?;
    }
    let foreign = frontend::collect_for_test(&root.join("foreign_chain/main.incn"))?;
    check_foreign_chain(&foreign)?;
    let inactive = frontend::collect_for_test(&root.join("inactive/main.incn"))?;
    check_invocation(&inactive, "Inactive")?;
    lower(&inactive)?;
    check_corruptions(&original)?;
    check_imported_owner(&original)?;
    source_headers::check(&lower(&original)?)?;
    println!(
        "conversion assembly: 5 selected, 1 foreign chain, 1 inactive, 2 missing-authority controls, 11 authority refusals, and 4 source-header refusals passed"
    );
    Ok(())
}

/// The genuine source graph keeps every storage owner while only its independently eligible leaf gets a header.
fn check_foreign_chain(checked: &Checked) -> TestResult<()> {
    check_invocation(checked, "Selected")?;
    let plan = lower(checked)?;
    for name in ["Port", "Wrapper", "Envelope"] {
        assert!(
            plan.models.iter().any(|owner| owner.source_name == name),
            "ordinary {name} layout absent"
        );
    }
    let port_index = plan
        .models
        .iter()
        .position(|owner| owner.source_name == "Port")
        .ok_or("Port layout absent")?;
    let port = &plan.models[port_index];
    let mut roots = plan
        .functions
        .iter()
        .filter(|function| function.name.starts_with("__incan_string_try_from_"));
    let root = roots.next().ok_or("Port conversion root absent")?;
    assert!(roots.next().is_none());
    let PlanType::Enum(index, name) = &root.return_type else {
        return Err("Port conversion result is not an enum".into());
    };
    let result = plan
        .enums
        .get(usize::try_from(*index)?)
        .ok_or("Port result index absent")?;
    assert_eq!(result.name, *name);
    assert_eq!(result.carrier, "Result");
    assert_eq!(result.variants.len(), 2);
    assert_eq!(result.variants[0].name, "Ok");
    assert_eq!(
        result.variants[0].fields,
        [PlanType::Model(port_index.try_into()?, port.name.clone())]
    );
    assert_eq!(result.variants[1].name, "Err");
    assert_eq!(result.variants[1].fields, [PlanType::String]);
    assert_eq!(root.parameters.len(), 1);
    assert_eq!(root.parameters[0].ty, PlanType::String);
    assert!(root.locals.len() >= 2);
    assert_eq!(root.locals[0].ty, root.return_type);
    assert_eq!(root.locals[1].ty, PlanType::String);
    source_headers::check_foreign(&plan)?;
    Ok(())
}

/// An unrelated genuine published raw owner cannot acquire a local root or veto an activated source chain.
fn check_imported_owner(checked: &Checked) -> TestResult<()> {
    let source = "pub type ExternalId = newtype int\n";
    let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = incan_frontend::typechecker::TypeChecker::new();
    checker.set_current_package_identity(Some("conversion_owner".to_owned()));
    checker.set_current_module_path(Some(vec!["lib".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut module =
        incan_frontend::body_ir::build_body_ir_module_v0(&program, &["lib".to_owned()], checker.type_info());
    let identity = module
        .nominal_declarations
        .first()
        .ok_or("package owner absent")?
        .canonical
        .clone();
    let bytes = build_surface(
        &[module.clone()],
        "conversion_owner",
        "1.0.0",
        &BTreeSet::from([identity.clone()]),
        &BTreeSet::new(),
    )?;
    let ExecutableDeclaration::Nominal(owner) = SurfaceReader::open(&bytes)?.fragment(&identity)? else {
        return Err("published raw owner is not nominal".into());
    };
    module.module_id = CompilerNodeId::module(canonical_module_identity(&identity).ok_or("package module absent")?);
    module.nominal_declarations = vec![owner];
    for state in ["Unselected", "Automatic"] {
        let mut encoded = serde_json::to_value(&module)?;
        encoded["nominal_declarations"][0]["newtype_string_conversion"] = Value::from(state);
        let imported = serde_json::from_value(encoded)?;
        let mut inputs = checked.clone();
        inputs.0.push(imported);
        inputs.1.push(source.to_owned());
        inputs.2.push("conversion_owner/lib.incn".to_owned());
        let plan = lower(&inputs)?;
        check_roots(&plan)?;
        assert!(plan.models.iter().any(|owner| owner.source_name == "ExternalId"));
    }
    Ok(())
}

/// Only the actual entry may carry one fresh invocation selection; all package and dependency inputs stay unset.
fn check_invocation(checked: &Checked, tag: &str) -> TestResult<()> {
    for (index, module) in checked.0.iter().enumerate() {
        let encoded = serde_json::to_value(module)?;
        let state = &encoded["string_conversion_contract"];
        if index == checked.3 {
            if tag == "Selected" {
                assert!(state.get("Selected").is_some(), "{state}");
            } else {
                assert_eq!(state, tag);
            }
        } else {
            assert_eq!(state, "Unselected");
        }
    }
    Ok(())
}

/// Real provider identity and method source spans must survive collection before any negative control is built.
fn check_selected(checked: &Checked) -> TestResult<()> {
    check_invocation(checked, "Selected")?;
    let encoded = serde_json::to_value(&checked.0[checked.3])?;
    let selected = &encoded["string_conversion_contract"]["Selected"];
    assert_eq!(selected["trait_identity"]["kind"], "trait");
    assert_eq!(selected["trait_identity"]["declaration_name"], "TryFrom");
    assert_eq!(selected["method_identity"]["kind"], "method");
    assert_eq!(selected["method_identity"]["declaration_name"], "try_from");
    let plan = lower(checked)?;
    assert!(plan.models.iter().any(|owner| owner.source_name == "WrappedPort"));
    check_roots(&plan)?;
    Ok(())
}

/// Preserve actual identities and layout while removing or corrupting one selected authority axis at a time.
fn check_corruptions(checked: &Checked) -> TestResult<()> {
    for cause in [
        "missing",
        "inactive",
        "duplicate",
        "misplaced",
        "method",
        "trait",
        "trait-kind",
        "trait-namespace",
        "method-namespace",
        "method-origin",
        "method-span",
        "owner-defaults",
        "inner",
    ] {
        let mut damaged = checked.clone();
        let mut encoded = serde_json::to_value(&damaged.0[damaged.3])?;
        match cause {
            "missing" => {
                encoded
                    .as_object_mut()
                    .ok_or("entry is not an object")?
                    .remove("string_conversion_contract");
            }
            "inactive" => encoded["string_conversion_contract"] = Value::from("Inactive"),
            "duplicate" => {
                let nonentry = damaged
                    .0
                    .iter()
                    .position(|module| module.module_id != damaged.0[damaged.3].module_id)
                    .ok_or("original has no SDK/package input")?;
                let mut fragment = serde_json::to_value(&damaged.0[nonentry])?;
                fragment["string_conversion_contract"] = encoded["string_conversion_contract"].clone();
                damaged.0[nonentry] = serde_json::from_value(fragment)?;
            }
            "misplaced" => {
                let fragment_index = damaged
                    .0
                    .iter()
                    .position(|module| {
                        module
                            .nominal_declarations
                            .iter()
                            .any(|owner| matches!(owner.canonical.origin, SymbolOrigin::Package { .. }))
                            || module
                                .enum_declarations
                                .iter()
                                .any(|owner| matches!(owner.canonical.origin, SymbolOrigin::Package { .. }))
                    })
                    .ok_or("original has no genuine Package declaration fragment")?;
                assert_ne!(fragment_index, damaged.3);
                let mut fragment = serde_json::to_value(&damaged.0[fragment_index])?;
                fragment["string_conversion_contract"] = encoded["string_conversion_contract"].clone();
                damaged.0[fragment_index] = serde_json::from_value(fragment)?;
                encoded["string_conversion_contract"] = Value::from("Unselected");
            }
            "method" => {
                encoded["string_conversion_contract"]["Selected"]["method_identity"]["declaration_name"] =
                    Value::from("other")
            }
            "trait" => {
                encoded["string_conversion_contract"]["Selected"]["trait_identity"]["declaration_name"] =
                    Value::from("Other")
            }
            "trait-kind" => {
                let method_kind = encoded["string_conversion_contract"]["Selected"]["method_identity"]["kind"].clone();
                encoded["string_conversion_contract"]["Selected"]["trait_identity"]["kind"] = method_kind;
            }
            "trait-namespace" => {
                let namespace =
                    encoded["string_conversion_contract"]["Selected"]["method_identity"]["namespace"].clone();
                encoded["string_conversion_contract"]["Selected"]["trait_identity"]["namespace"] = namespace;
            }
            "method-namespace" => {
                let namespace =
                    encoded["string_conversion_contract"]["Selected"]["trait_identity"]["namespace"].clone();
                encoded["string_conversion_contract"]["Selected"]["method_identity"]["namespace"] = namespace;
            }
            "method-origin" => {
                encoded["string_conversion_contract"]["Selected"]["method_identity"]["origin"] =
                    serde_json::to_value(SymbolOrigin::Builtin)?;
            }
            "method-span" => {
                let owner_end = encoded["string_conversion_contract"]["Selected"]["trait_identity"]["declaration_span"]
                    ["end"]
                    .as_u64()
                    .ok_or("missing trait span end")?;
                let outside = owner_end.checked_add(1).ok_or("trait span overflow")?;
                encoded["string_conversion_contract"]["Selected"]["method_identity"]["declaration_span"]["end"] =
                    Value::from(outside);
            }
            "owner-defaults" => {
                let declarations = encoded["nominal_declarations"]
                    .as_array_mut()
                    .ok_or("nominals missing")?;
                let owner = declarations
                    .iter_mut()
                    .find(|owner| owner["name"] == "Port")
                    .ok_or("raw owner missing")?;
                owner["has_field_defaults"] = Value::from(true);
            }
            "inner" => {
                let declarations = encoded["nominal_declarations"]
                    .as_array_mut()
                    .ok_or("nominals missing")?;
                let inner = declarations
                    .iter_mut()
                    .find(|owner| owner["name"] == "Port")
                    .ok_or("inner owner missing")?;
                inner["newtype_string_conversion"] = Value::from("Unselected");
            }
            _ => return Err("unknown authority damage".into()),
        }
        damaged.0[damaged.3] = serde_json::from_value(encoded)?;
        if cause == "owner-defaults" {
            // Defaults are forbidden by the canonical newtype layout itself, before conversion admission.
            // Prove the one changed flag is the reason, not a foreign identity or another damaged owner.
            let module = &damaged.0[damaged.3];
            let invalid = module
                .nominal_declarations
                .iter()
                .filter(|owner| !module.is_well_formed_nominal_declaration(owner))
                .map(|owner| owner.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(invalid, ["Port"]);
            let mut restored = module.clone();
            restored
                .nominal_declarations
                .iter_mut()
                .find(|owner| owner.name == "Port")
                .ok_or("raw owner missing")?
                .has_field_defaults = false;
            assert_eq!(restored, checked.0[checked.3]);
            assert!(restored
                .nominal_declarations
                .iter()
                .all(|owner| restored.is_well_formed_nominal_declaration(owner)));
        }
        if matches!(cause, "missing" | "inactive") {
            let plan = lower(&damaged)?;
            assert!(
                plan.functions
                    .iter()
                    .all(|function| !function.name.starts_with("__incan_string_try_from_")),
                "{cause}: unauthorized conversion root"
            );
            continue;
        }
        let expected = match cause {
            "duplicate" | "misplaced" => "unsupported Body IR misplaced string conversion contract",
            "method" | "trait" | "trait-kind" | "trait-namespace" | "method-namespace" | "method-origin"
            | "method-span" => "unsupported Body IR string conversion contract identity",
            "owner-defaults" => "invalid Body IR nominal declaration identity",
            "inner" => "unsupported Body IR automatic string conversion underlying selection",
            _ => return Err("unknown refusal contract".into()),
        };
        match lower(&damaged) {
            Ok(_) => return Err(format!("corrupt conversion authority admitted: {cause}").into()),
            Err(error) if cause == "owner-defaults" => assert_eq!(error.to_string(), expected),
            Err(error) => assert!(error.to_string().contains(expected), "{cause}: {error}"),
        }
    }
    Ok(())
}

/// The selected record must cause exactly one owned-string, standard-Result root for each actual chain owner.
fn check_roots(plan: &Plan) -> TestResult<()> {
    let roots = plan
        .functions
        .iter()
        .filter(|function| function.name.starts_with("__incan_string_try_from_"))
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    for name in ["Port", "WrappedPort"] {
        let owner = plan
            .models
            .iter()
            .position(|model| model.source_name == name)
            .ok_or("planned owner missing")?;
        let mut matching = 0;
        for root in &roots {
            let PlanType::Enum(index, result_name) = &root.return_type else {
                return Err("conversion result is not an enum".into());
            };
            let result = plan
                .enums
                .get(usize::try_from(*index)?)
                .ok_or("conversion result index missing")?;
            assert_eq!(result.name, *result_name);
            assert_eq!(result.carrier, "Result");
            assert_eq!(result.variants.len(), 2);
            assert_eq!(result.variants[0].name, "Ok");
            assert_eq!(result.variants[1].name, "Err");
            assert_eq!(result.variants[1].fields, [PlanType::String]);
            if result.variants[0].fields != [PlanType::Model(owner.try_into()?, plan.models[owner].name.clone())] {
                continue;
            }
            matching += 1;
            assert_eq!(root.parameters.len(), 1);
            assert_eq!(root.parameters[0].ty, PlanType::String);
            assert!(root.locals.len() >= 2);
            assert_eq!(root.locals[0].ty, root.return_type);
            assert_eq!(root.locals[1].ty, PlanType::String);
        }
        assert_eq!(matching, 1, "owner {name} root is not unique");
    }
    Ok(())
}

/// Lower the exact collected modules through the production Incan project entry point.
fn lower(checked: &Checked) -> TestResult<incan_mir_lowering::caller::incan::Plan> {
    Ok(lower_program(
        &checked.0,
        checked.1.clone(),
        checked.2.clone(),
        checked.3.try_into()?,
    )?)
}
