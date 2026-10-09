//! Actual checked Body IR and native plan admission controls for nested carriers (#1337, #1698).
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_middle;
extern crate rustc_span;
extern crate thin_vec;

// DRIVER_MODULES

use incan_frontend::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::{
    IncanPrimitiveType, IncanType,
    body_ir::{Block, BodyIrModule, Operand, Place, PlaceElem, Rvalue, StatementKind},
};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const SOURCE: &str = r#"@derive(Clone)
type Kind = newtype str
type Choice = Kind | str

def describe(value: Option[Choice]) -> str:
    if value is not None:
        if isinstance(value, Kind):
            return value.0
        if isinstance(value, str):
            return value
    return "missing"

def other(value: Option[str]) -> None:
    pass

def reversed(value: Option[Kind] | str) -> str:
    if isinstance(value, str):
        return value
    return "optional"

def main() -> None:
    describe(Some(Kind("seven")))
"#;

/// Obtain projection and declaration facts from the real frontend before mutating any retained input.
fn checked_module() -> TestResult<BodyIrModule> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["dev7_carrier_admission".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Locate the actual optional/union/newtype return path, traversing checked branch bodies without changing them.
fn nested_place(block: &mut Block) -> Option<&mut Place> {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            StatementKind::Return {
                value: Some(Operand::Place(read)),
            } if read.place.projection.len() >= 3
                && matches!(read.place.projection[0], PlaceElem::OptionPayload { .. }) =>
            {
                return Some(&mut read.place);
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                if let Some(place) = nested_place(then_block) {
                    return Some(place);
                }
                if let Some(block) = else_block {
                    if let Some(place) = nested_place(block) {
                        return Some(place);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Require the baseline to expose the complete path so a missing frontend projection cannot hide behind negatives.
fn projection(module: &mut BodyIrModule) -> TestResult<&mut Place> {
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "describe")
        .ok_or("missing describe")?;
    nested_place(&mut body.block).ok_or_else(|| "missing nested checked carrier projection".into())
}

/// Require one exact lowering error family; preparation failures and unrelated refusals cannot satisfy a control.
fn rejected_body(module: &BodyIrModule, label: &str, family: &str) -> TestResult<()> {
    match lower_module(module, SOURCE.to_owned(), "dev7_carrier_admission.incn".to_owned()) {
        Err(reason) if reason.starts_with(family) => Ok(()),
        Err(reason) => Err(format!("{label}: expected {family}, got {reason}").into()),
        Ok(_) => Err(format!("{label}: malformed Body IR was admitted").into()),
    }
}

/// Independently corrupt the container, payload, step order, member, and field identity on checked inputs.
fn body_controls(baseline: &BodyIrModule) -> TestResult<()> {
    let optional_text = IncanType::Generic {
        base: "Option".into(),
        args: vec![IncanType::Primitive(IncanPrimitiveType::Str)],
    };
    let mut module = baseline.clone();
    let PlaceElem::OptionPayload { option_type, .. } = &mut projection(&mut module)?.projection[0] else {
        return Err("expected Option payload projection".into());
    };
    *option_type = optional_text;
    rejected_body(
        &module,
        "foreign optional container",
        "invalid Body IR optional projection owner",
    )?;

    let mut module = baseline.clone();
    let PlaceElem::OptionPayload { payload_type, .. } = &mut projection(&mut module)?.projection[0] else {
        return Err("expected Option payload projection".into());
    };
    *payload_type = IncanType::Primitive(IncanPrimitiveType::Int);
    rejected_body(
        &module,
        "wrong optional payload",
        "invalid Body IR optional projection payload",
    )?;

    let mut module = baseline.clone();
    projection(&mut module)?.projection.remove(0);
    rejected_body(
        &module,
        "missing optional step",
        "unsupported Body IR nonunion enum isinstance",
    )?;

    let mut module = baseline.clone();
    projection(&mut module)?.projection.swap(0, 1);
    rejected_body(
        &module,
        "reordered payload steps",
        "unsupported Body IR nonunion enum isinstance",
    )?;

    let mut module = baseline.clone();
    let step = projection(&mut module)?.projection[0].clone();
    projection(&mut module)?.projection.insert(1, step);
    rejected_body(
        &module,
        "duplicated optional step",
        "invalid Body IR optional projection owner",
    )?;

    let mut module = baseline.clone();
    let PlaceElem::UnionMember { ty } = &mut projection(&mut module)?.projection[1] else {
        return Err("expected union payload projection".into());
    };
    *ty = IncanType::Primitive(IncanPrimitiveType::Float);
    rejected_body(
        &module,
        "foreign union member",
        "unsupported Body IR union member conversion",
    )?;

    let mut module = baseline.clone();
    let PlaceElem::Field { canonical, .. } = &mut projection(&mut module)?.projection[2] else {
        return Err("expected newtype field projection".into());
    };
    *canonical = None;
    rejected_body(
        &module,
        "missing newtype field authority",
        "unsupported Body IR unproven model field",
    )?;
    Ok(())
}

/// Find a real native borrow carrying the nested path, so negative controls exercise the driver's actual validator.
fn native_projection(plan: &mut plan::Plan) -> TestResult<&mut plan::Place> {
    for function in &mut plan.functions {
        if function.name != "describe" {
            continue;
        }
        for block in &mut function.blocks {
            for statement in &mut block.statements {
                if let plan::StatementKind::Assign(_, rvalue) = &mut statement.kind {
                    if let plan::RvalueKind::Borrow(place) = &mut rvalue.kind {
                        if matches!(&place.projection, plan::Projection::Fields(fields) if fields.len() >= 3 && fields[0].variant == 1)
                        {
                            return Ok(place);
                        }
                    }
                }
            }
        }
    }
    Err("missing native nested payload borrow".into())
}

/// Check a native refusal through the source driver validator, never a test-local mirror.
fn rejected_plan(plan: &plan::Plan, label: &str, family: &str) -> TestResult<()> {
    match validation::validate(plan) {
        Err(error::PlanError::Invalid { reason, .. }) if reason.starts_with(family) => Ok(()),
        Err(reason) => Err(format!("{label}: expected {family}, got {reason}").into()),
        Ok(_) => Err(format!("{label}: malformed native plan was admitted").into()),
    }
}

/// Corrupt exact field indices and variant/type facts separately, preserving all unrelated plan structure.
fn plan_controls(baseline: &plan::Plan) -> TestResult<()> {
    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    fields[0].variant = 99;
    rejected_plan(&value, "unknown active variant", "unknown enum payload field")?;

    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    fields[1].slot = 99;
    rejected_plan(&value, "unknown union payload slot", "unknown enum payload field")?;

    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    fields[0].ty = plan::PlanType::Float;
    rejected_plan(&value, "wrong payload field type", "enum payload projection:")?;

    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    fields[0].variant = -2;
    rejected_plan(&value, "invalid ordinary-field sentinel", "invalid field path variant")?;

    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    let plan::PlanType::Enum(_, name) = &mut fields[0].ty else {
        return Err("expected retained enum payload type".into());
    };
    *name = "WrongOwner".to_owned();
    rejected_plan(
        &value,
        "wrong payload owner name",
        "enum type differs from its declaration",
    )?;

    let mut value = baseline.clone();
    let plan::Projection::Fields(fields) = &mut native_projection(&mut value)?.projection else {
        return Err("expected native field path".into());
    };
    let plan::PlanType::Enum(owner, _) = &mut fields[0].ty else {
        return Err("expected retained enum payload type".into());
    };
    *owner = -1;
    rejected_plan(
        &value,
        "negative payload owner index",
        "enum type differs from its declaration",
    )?;

    let mut value = baseline.clone();
    let function = value.functions.first_mut().ok_or("missing native function")?;
    let mut local = function.locals.first().ok_or("missing native local")?.clone();
    local.ty = plan::PlanType::List(plan::ListLeaf::Enum(-1, "MissingCarrier".into()), 1);
    function.locals.push(local);
    rejected_plan(
        &value,
        "negative collection leaf owner",
        "enum type differs from its declaration",
    )?;

    let mut value = baseline.clone();
    let (index, declaration) = value
        .enums
        .iter_mut()
        .enumerate()
        .find(|(_, declaration)| declaration.carrier == "Option")
        .ok_or("missing Option declaration")?;
    declaration.variants[1].fields[0] = plan::PlanType::Enum(i64::try_from(index)?, declaration.name.clone());
    rejected_plan(&value, "recursive by-value carrier", "recursive by-value layout")?;
    Ok(())
}

/// Check nested payloads and reversed carrier ordering before independent malformed-input controls.
fn main() -> TestResult<()> {
    let baseline = checked_module()?;
    let mut checked = baseline.clone();
    let _ = projection(&mut checked)?;
    let plan = lower_module(&baseline, SOURCE.to_owned(), "dev7_carrier_admission.incn".to_owned())?;
    validation::validate(&plan)?;
    let mut native = plan.clone();
    let _ = native_projection(&mut native)?;
    // An unused recursive enum behind Vec is a finite heap-backed layout and must retain admission.
    let mut heap = plan.clone();
    let index = i64::try_from(heap.enums.len())?;
    let mut declaration = heap.enums.first().ok_or("missing enum layout")?.clone();
    declaration.name = "HeapRecursive".into();
    declaration.carrier.clear();
    declaration.source_type.clear();
    declaration.variants = vec![plan::EnumVariant {
        name: "Children".into(),
        fields: vec![plan::PlanType::List(
            plan::ListLeaf::Enum(index, declaration.name.clone()),
            1,
        )],
    }];
    heap.enums.push(declaration);
    validation::validate(&heap)?;
    body_controls(&baseline)?;
    plan_controls(&plan)?;
    println!("nested carrier admission: 2 positive, 7 Body IR negative, and 8 native plan negative controls passed");
    Ok(())
}
