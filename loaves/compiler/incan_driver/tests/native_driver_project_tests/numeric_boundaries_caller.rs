//! Exact numeric defaults and static-guard admission at the existing native adapter boundary (#1337).

use super::{TestResult, checked_source, lower_module, plan, statics, validation};

#[path = "numeric_boundaries_sources.rs"]
mod sources;

/// Validate genuine source plans, then probe the static accessor's owned-layout contract independently of bodies.
pub(super) fn check() -> TestResult<()> {
    // ---- Source lowering: exact defaults and scalar returns ----
    let module = checked_source(sources::DEFAULTS, "numeric_defaults")?;
    let defaults = lower_module(
        &module,
        sources::DEFAULTS.to_owned(),
        "numeric_defaults.incn".to_owned(),
    )?;
    validation::validate(&defaults)?;
    require_typed_defaults(&defaults)?;

    let module = checked_source(sources::STATIC_RESULTS, "numeric_static_results")?;
    let baseline = lower_module(
        &module,
        sources::STATIC_RESULTS.to_owned(),
        "numeric_static_results.incn".to_owned(),
    )?;
    validation::validate(&baseline)?;

    // ---- Accessor signatures: all numeric carriers and nested owned layouts ----
    let (slot, method) = guarded_method(&baseline)?;
    let name = baseline.functions[method].name.clone();
    for ty in numeric_types() {
        let mut candidate = baseline.clone();
        candidate.functions[method].return_type = ty;
        statics::method_target(&candidate, slot, &name)?;
    }

    // Only the signature is varied here: this checks escape admission, not execution of deliberately changed bodies.
    let mut nested = baseline.clone();
    let mut result = nested.models.first().ok_or("missing static owner model")?.clone();
    result.name = "OwnedNumericResult".to_owned();
    result.source_name = result.name.clone();
    result.fields.truncate(1);
    result.field_public.truncate(1);
    result.fields.first_mut().ok_or("missing static owner field")?.ty = plan::PlanType::F32;
    let result_index = nested.models.len();
    let result_type = plan::PlanType::Model(i64::try_from(result_index)?, result.name.clone());
    nested.models.push(result);
    nested.functions[method].return_type = result_type.clone();
    statics::method_target(&nested, slot, &name)?;
    let mut list = nested.clone();
    list.functions[method].return_type = plan::PlanType::List(
        plan::ListLeaf::Model(i64::try_from(result_index)?, nested.models[result_index].name.clone()),
        1,
    );
    statics::method_target(&list, slot, &name)?;

    // ---- Guard lifetime: borrowed, opaque and recursive results still cannot escape ----
    let mut borrowed = baseline.clone();
    borrowed.functions[method].return_type = plan::PlanType::ScalarRef(vec![plan::PlanType::F32]);
    require_escape_refusal(&borrowed, slot, &name)?;
    let mut borrowed_field = nested.clone();
    borrowed_field.models[result_index].fields[0].ty = plan::PlanType::ScalarRef(vec![plan::PlanType::F32]);
    require_escape_refusal(&borrowed_field, slot, &name)?;
    let mut opaque = nested.clone();
    opaque.models[result_index].library_item = "provider::Opaque".to_owned();
    opaque.models[result_index].fields.clear();
    require_escape_refusal(&opaque, slot, &name)?;
    let mut recursive = nested;
    recursive.models[result_index].fields[0].ty = result_type;
    require_escape_refusal(&recursive, slot, &name)?;
    println!(
        "exact numeric boundaries: typed defaults, 15 scalar returns, 2 nested owned returns, 4 escape refusals passed"
    );
    Ok(())
}

/// Enumerate the source fixture's independent expectations; production classification remains owned by Incan numerics.
fn numeric_types() -> [plan::PlanType; 15] {
    use plan::PlanType;
    [
        PlanType::I8,
        PlanType::I16,
        PlanType::I32,
        PlanType::I128,
        PlanType::ISize,
        PlanType::U8,
        PlanType::U16,
        PlanType::U32,
        PlanType::U64,
        PlanType::U128,
        PlanType::USize,
        PlanType::F32,
        PlanType::F64,
        PlanType::Int,
        PlanType::Float,
    ]
}

/// Require literal zeros to retain every field's exact carrier, including IEEE positive-zero bit payloads.
fn require_typed_defaults(value: &plan::Plan) -> TestResult<()> {
    let owner = value
        .models
        .iter()
        .position(|model| model.source_name == "ExactDefaults")
        .ok_or("missing exact-default model")?;
    let fields = value
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.statements)
        .find_map(|statement| match &statement.kind {
            plan::StatementKind::Assign(_, rvalue) => match &rvalue.kind {
                plan::RvalueKind::Model(index, fields) if usize::try_from(*index).ok() == Some(owner) => Some(fields),
                _ => None,
            },
            _ => None,
        })
        .ok_or("missing derived default construction")?;
    if fields.len() != 17 {
        return Err(format!("expected 17 defaulted fields, got {}", fields.len()).into());
    }
    for (index, ty) in numeric_types().into_iter().enumerate() {
        // Source f64 is float under RFC 009; keep the separate raw F64 case in static accessor admission above.
        let ty = if ty == plan::PlanType::F64 {
            plan::PlanType::Float
        } else {
            ty
        };
        let field = value.models[owner]
            .fields
            .get(index)
            .ok_or("missing canonical default field")?;
        if field.ty != ty {
            return Err(format!("default field {index} lost its canonical source carrier").into());
        }
        let expected = match ty {
            plan::PlanType::Int => plan::Constant::Int(0),
            plan::PlanType::Float => plan::Constant::Float(0.0),
            other => plan::Constant::Numeric("0".to_owned(), other),
        };
        let exact_zero = match (&fields[index].kind, expected) {
            (plan::OperandKind::Literal(plan::Constant::Float(actual)), plan::Constant::Float(expected)) => {
                actual.to_bits() == expected.to_bits()
            }
            (actual, expected) => actual == &plan::OperandKind::Literal(expected),
        };
        if !exact_zero {
            return Err(format!("default field {index} lost its typed zero").into());
        }
    }
    Ok(())
}

/// Select the accessor actually used by the genuine checked source, not a fabricated receiver signature.
fn guarded_method(value: &plan::Plan) -> TestResult<(i64, usize)> {
    for function in &value.functions {
        for block in &function.blocks {
            if let plan::TerminatorKind::Call(callee, ..) = &block.terminator.kind
                && let plan::CalleeKind::StaticMethod(slot, name) = &callee.kind
            {
                let method = value
                    .functions
                    .iter()
                    .position(|function| function.name == *name)
                    .ok_or("missing planned static method")?;
                return Ok((*slot, method));
            }
        }
    }
    Err("missing guarded static call".into())
}

/// Reject precisely an escaping-result contract, rather than accepting any unrelated malformed-plan failure.
fn require_escape_refusal(value: &plan::Plan, slot: i64, name: &str) -> TestResult<()> {
    match statics::method_target(value, slot, name) {
        Err(error)
            if error
                .to_string()
                .contains("unsupported static method carrier or escaping result") =>
        {
            Ok(())
        }
        Err(error) => Err(format!("unexpected static result refusal: {error}").into()),
        Ok(_) => Err("escaping static result was admitted".into()),
    }
}
