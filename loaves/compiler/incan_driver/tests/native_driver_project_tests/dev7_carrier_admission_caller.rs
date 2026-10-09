//! Actual checked Body IR and native plan admission controls for nested carriers (#1337, #1698).
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;
extern crate thin_vec;

// DRIVER_MODULES

use incan_frontend::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::{
    CompilerNodeId, IncanPrimitiveType, IncanType, SymbolOrigin,
    body_ir::{
        Block, BodyIrModule, CallableTarget, Callee, NamedCallableTarget, Operand, Place, PlaceElem, Rvalue,
        StatementKind,
    },
    canonical_module_identity,
    executable_representation::{SurfaceReader, build_surface},
};
use std::collections::BTreeSet;

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
    checked_source(SOURCE, "dev7_carrier_admission")
}

/// Retain genuine frontend identities for each independent source control.
fn checked_source(source: &str, module: &str) -> TestResult<BodyIrModule> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec![module.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

const CALL_SOURCE: &str = r#"pub def echo(value: int) -> int:
    return value

pub def main() -> None:
    echo(7)
"#;

/// Decode actual package fragments whose publisher deliberately removed every call's source-local span id.
fn published_calls(source: &BodyIrModule) -> TestResult<BodyIrModule> {
    let public: BTreeSet<_> = source
        .bodies
        .iter()
        .map(|body| {
            let mut identity = body.canonical.clone().ok_or("missing source body identity")?;
            let SymbolOrigin::Module(module_path) = identity.origin else {
                return Err("expected checked source owner");
            };
            identity.origin = SymbolOrigin::Package {
                library: "dev7_calls".into(),
                module_path,
            };
            Ok(identity)
        })
        .collect::<Result<_, &str>>()?;
    let bytes = build_surface(
        std::slice::from_ref(source),
        "dev7_calls",
        "1.0.0",
        &public,
        &BTreeSet::new(),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    let mut published = source.clone();
    published.bodies.clear();
    for identity in &public {
        published.bodies.push(reader.declaration(identity)?);
    }
    let first = public.first().ok_or("missing published identity")?;
    published.module_id = CompilerNodeId::module(canonical_module_identity(first).ok_or("missing package owner")?);
    Ok(published)
}

/// Select the retained ordinary call by statement kind, with no name-based verifier.
fn named_call(module: &mut BodyIrModule) -> TestResult<&mut NamedCallableTarget> {
    for body in &mut module.bodies {
        for statement in &mut body.block.stmts {
            if let StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } = &mut statement.kind
            {
                return Ok(target);
            }
        }
    }
    Err("missing checked named call".into())
}

/// Require the real Incan lowerer to reject an independently corrupted callable identity.
fn rejected_call(module: &BodyIrModule, label: &str, family: &str) -> TestResult<()> {
    match lower_module(module, CALL_SOURCE.to_owned(), "dev7_calls.incn".to_owned()) {
        Err(reason) if reason.starts_with(family) => Ok(()),
        Err(reason) => Err(format!("{label}: expected {family}, got {reason}").into()),
        Ok(_) => Err(format!("{label}: malformed callable identity was admitted").into()),
    }
}

/// Preserve source-local span proof while admitting package calls exclusively through unique canonical bodies.
fn callable_controls() -> TestResult<()> {
    let source = checked_source(CALL_SOURCE, "dev7_calls")?;
    let plan = lower_module(&source, CALL_SOURCE.to_owned(), "dev7_calls.incn".to_owned())?;
    validation::validate(&plan)?;
    let mut published = published_calls(&source)?;
    if named_call(&mut published)?.direct_call_id.is_some() {
        return Err("publisher retained a source-local call id".into());
    }
    let plan = lower_module(&published, CALL_SOURCE.to_owned(), "dev7_calls.incn".to_owned())?;
    validation::validate(&plan)?;

    let mut module = source.clone();
    named_call(&mut module)?.direct_call_id = None;
    rejected_call(
        &module,
        "missing local span identity",
        "unsupported Body IR local NamedCallableTarget without direct identity",
    )?;

    let mut module = published.clone();
    named_call(&mut module)?.canonical = None;
    rejected_call(
        &module,
        "missing package authority",
        "unsupported Body IR builtin or unproven NamedCallableTarget",
    )?;

    let mut module = published.clone();
    let identity = named_call(&mut module)?
        .canonical
        .as_mut()
        .ok_or("missing published call identity")?;
    let SymbolOrigin::Package { library, .. } = &mut identity.origin else {
        return Err("expected published callable owner".into());
    };
    *library = "foreign_same_spelling".into();
    rejected_call(
        &module,
        "foreign package owner",
        "unsupported Body IR imported or unresolved canonical call target",
    )?;

    let mut module = published.clone();
    named_call(&mut module)?
        .canonical
        .as_mut()
        .ok_or("missing published call identity")?
        .declaration_span
        .end += 1;
    rejected_call(
        &module,
        "wrong package declaration span",
        "unsupported Body IR imported or unresolved canonical call target",
    )?;

    let mut module = published.clone();
    let wrong_span = CompilerNodeId::declaration_span(module.module_id.path(), 0, 1);
    named_call(&mut module)?.direct_call_id = Some(wrong_span);
    rejected_call(
        &module,
        "wrong present package span identity",
        "Body IR call identity disagrees with target body",
    )?;

    let mut module = published.clone();
    let target = named_call(&mut module)?
        .canonical
        .clone()
        .ok_or("missing published call identity")?;
    let duplicate = module
        .bodies
        .iter()
        .find(|body| body.canonical.as_ref() == Some(&target))
        .ok_or("missing published target body")?
        .clone();
    module.bodies.push(duplicate);
    rejected_call(
        &module,
        "duplicate canonical target bodies",
        "unsupported Body IR imported or unresolved canonical call target",
    )?;
    Ok(())
}

const INDEXED_SOURCE: &str = r#"def first(values: Option[List[Result[str, str]]]) -> str:
    if values is not None:
        match values[0]:
            Ok(inner) => return inner
            Err(error) => return error
    return "empty"

def main() -> None:
    first(Some([Ok("retained")]))
"#;

/// Locate the checked optional payload followed by a list index in the actual enum match scrutinee.
fn indexed_match(block: &mut Block) -> Option<&mut Place> {
    for statement in &mut block.stmts {
        match &mut statement.kind {
            StatementKind::Assign {
                rvalue:
                    Rvalue::Match {
                        scrutinee: Operand::Place(read),
                        ..
                    },
                ..
            } if read
                .place
                .projection
                .iter()
                .any(|step| matches!(step, PlaceElem::Index(_))) =>
            {
                return Some(&mut read.place);
            }
            StatementKind::If { then_block, .. } => {
                if let Some(place) = indexed_match(then_block) {
                    return Some(place);
                }
            }
            _ => {}
        }
    }
    None
}

/// Require a retained indexed path, so a frontend simplification cannot quietly remove this test's boundary.
fn indexed_projection(module: &mut BodyIrModule) -> TestResult<&mut Place> {
    for body in &mut module.bodies {
        if let Some(place) = indexed_match(&mut body.block) {
            return Ok(place);
        }
    }
    Err("missing checked indexed enum scrutinee".into())
}

/// Exercise an indexed enum match inside a checked optional list without bypassing the actual frontend.
fn indexed_carrier_control() -> TestResult<()> {
    let mut module = checked_source(INDEXED_SOURCE, "dev7_indexed_carrier")?;
    if !matches!(
        indexed_projection(&mut module)?.projection.first(),
        Some(PlaceElem::OptionPayload { .. })
    ) {
        return Err("missing optional list payload authority".into());
    }
    let plan = lower_module(
        &module,
        INDEXED_SOURCE.to_owned(),
        "dev7_indexed_carrier.incn".to_owned(),
    )?;
    validation::validate(&plan)?;
    let mut corrupted = module.clone();
    let PlaceElem::OptionPayload { option_type, .. } = &mut indexed_projection(&mut corrupted)?.projection[0] else {
        return Err("expected optional list payload".into());
    };
    *option_type = IncanType::Generic {
        base: "Option".into(),
        args: vec![IncanType::Primitive(IncanPrimitiveType::Str)],
    };
    for (label, corrupted, family) in [
        (
            "wrong indexed container",
            corrupted,
            "invalid Body IR optional projection owner",
        ),
        {
            let mut corrupted = module.clone();
            indexed_projection(&mut corrupted)?.projection.remove(0);
            (
                "missing indexed payload",
                corrupted,
                "unsupported Body IR model indexing without a field",
            )
        },
    ] {
        match lower_module(
            &corrupted,
            INDEXED_SOURCE.to_owned(),
            "dev7_indexed_carrier.incn".to_owned(),
        ) {
            Err(reason) if reason.starts_with(family) => {}
            Err(reason) => return Err(format!("{label}: expected {family}, got {reason}").into()),
            Ok(_) => return Err(format!("{label}: malformed indexed payload was admitted").into()),
        }
    }
    Ok(())
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
    callable_controls()?;
    indexed_carrier_control()?;
    println!("nested carrier admission: 5 positive, 15 Body IR negative, and 8 native plan negative controls passed");
    Ok(())
}
