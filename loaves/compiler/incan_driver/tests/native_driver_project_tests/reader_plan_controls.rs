//! Test-only mutations of an actual source-lowered reader plan for the native metadata boundary.

use std::path::Path;

use crate::plan::{CalleeKind, Plan, PlanType, TerminatorKind};

/// Read a source-owned test case marker so completed-source reuse keys include the exact mutation mode.
pub(crate) fn apply(plan: &mut Plan, source: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(source).map_err(|error| error.to_string())?;
    let mode = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# reader-plan-control: "))
        .ok_or("reader native control marker absent")?;
    let index = plan
        .models
        .iter()
        .position(|model| model.library_item == "incan_stdlib_system::io::ReaderChunks")
        .ok_or("genuine source plan has no SDK ReaderChunks alias")?;
    if plan.models[index].library_type_arguments.len() != 1 || plan.models[index].fields.len() != 2 {
        return Err("genuine source plan lost its closed ReaderChunks argument or public fields".into());
    }
    mutate(plan, index, mode)
}

/// Change one native contract at a time after genuine lowering, without replacing the adapter or its policies.
fn mutate(plan: &mut Plan, index: usize, mode: &str) -> Result<(), String> {
    match mode {
        "genuine" => {}
        "missing-arity" => plan.models[index].library_type_arguments.clear(),
        "extra-arity" => plan.models[index].library_type_arguments.push(PlanType::Int),
        "model-bound" => plan.models[index].library_type_arguments = vec![PlanType::Int],
        "parameter-kind" => {
            // This existing public standard struct has a type parameter followed by a const parameter.
            // Supplying two types must be rejected from metadata before Rust can interpret the alias AST.
            plan.models[index].library_item = "std::array::IntoIter".into();
            plan.models[index].library_type_arguments = vec![PlanType::Int, PlanType::Int];
        }
        "argument-cycle" => {
            let owner = i64::try_from(index).map_err(|error| error.to_string())?;
            let name = plan.models[index].name.clone();
            plan.models[index].library_type_arguments = vec![PlanType::Model(owner, name)];
        }
        "borrowed-argument" => {
            plan.models[index].library_item = "std::cell::Cell".into();
            plan.models[index].library_type_arguments = vec![PlanType::StringMutRef];
        }
        "field-type" => {
            let field = plan.models[index].fields.first_mut().ok_or("reader field absent")?;
            field.ty = PlanType::Int;
        }
        "trait-bound" => return invalid_trait_self(plan),
        _ => return Err(format!("unknown reader native metadata control {mode}")),
    }
    Ok(())
}

/// Keep the call and declaration's instantiation key consistent so the wrong Self reaches native bounds validation.
fn invalid_trait_self(plan: &mut Plan) -> Result<(), String> {
    let path = "incan_stdlib_system::io::BinaryReader::chunks";
    let external = plan
        .externals
        .iter_mut()
        .find(|external| external.path == path)
        .ok_or("genuine SDK reader default external absent")?;
    external.type_arguments = vec![PlanType::Int];
    let mut changed = 0;
    for block in plan.functions.iter_mut().flat_map(|function| &mut function.blocks) {
        let TerminatorKind::Call(callee, ..) = &mut block.terminator.kind else {
            continue;
        };
        let CalleeKind::Instantiated(target, arguments) = &mut callee.kind else {
            continue;
        };
        if target.as_str() == path {
            *arguments = vec![PlanType::Int];
            changed += 1;
        }
    }
    if changed != 1 {
        return Err(format!("genuine SDK reader call count changed: {changed}"));
    }
    Ok(())
}
