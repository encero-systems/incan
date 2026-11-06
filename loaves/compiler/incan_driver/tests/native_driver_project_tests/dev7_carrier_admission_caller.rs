//! Actual checked Body IR and native plan admission controls for nested carriers (#1337, #1698).
#![feature(rustc_private)]
extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_infer;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;
extern crate rustc_trait_selection;
extern crate thin_vec;

// DRIVER_MODULES

#[path = "instantiated_owned_results_caller.rs"]
mod instantiated_owned_results;

#[path = "checked_json_admission_caller.rs"]
mod checked_json_admission;

#[path = "numeric_boundaries_caller.rs"]
mod numeric_boundaries;

use incan_frontend::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::{
    CompilerNodeId, IncanPrimitiveType, IncanType, SymbolOrigin,
    body_ir::{
        Block, BodyIrModule, CallableTarget, Callee, NamedCallableTarget, Operand, OwnershipFact, Place, PlaceElem,
        PlaceOperand, Rvalue, StatementKind,
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
    checked_source_in_package(source, module, None)
}

/// Mint actual package identities before checking a publication control; local source controls keep module origins.
fn checked_source_in_package(source: &str, module: &str, package: Option<&str>) -> TestResult<BodyIrModule> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec![module.to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(package.map(str::to_owned));
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
fn published_calls() -> TestResult<BodyIrModule> {
    let source = checked_source_in_package(CALL_SOURCE, "dev7_calls", Some("dev7_calls"))?;
    let public: BTreeSet<_> = source
        .bodies
        .iter()
        .map(|body| body.canonical.clone().ok_or("missing checked package body identity"))
        .collect::<Result<_, &str>>()?;
    let bytes = build_surface(
        std::slice::from_ref(&source),
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
    let mut published = published_calls()?;
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

const PROJECTED_BORROW_SOURCE: &str = r#"@derive(Clone)
type Kind = newtype str
type Choice = Kind | str

model Holder:
    value: Option[Choice]

model Envelope:
    holder: Holder

def take(value: Option[Choice]) -> Option[Choice]:
    return value

def payload(value: Option[Choice]) -> Choice:
    if value is not None:
        return value
    return "missing"

def main() -> None:
    mut holder = Holder(value=Some(Kind("retained")))
    first = take(holder.value)
    second = take(holder.value)
    holder.value = None
    take(holder.value)
    payload(first)
    payload(second)
    mut nested = Envelope(holder=Holder(value=Some("nested")))
    take(nested.holder.value)
    nested.holder.value = Some("replacement")
    take(nested.holder.value)
"#;

/// Require an actual borrowed projected call argument, rather than manufacture the positive ownership fact.
fn projected_enum_read(module: &mut BodyIrModule, depth: usize) -> TestResult<&mut PlaceOperand> {
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("missing projected carrier caller")?;
    for statement in &mut body.block.stmts {
        if let StatementKind::Call { args, .. } = &mut statement.kind {
            for argument in args {
                if let incan_semantics_core::body_ir::ArgumentElement::One(Operand::Place(read)) = argument {
                    if read.fact == OwnershipFact::Borrow && read.place.projection.len() == depth {
                        return Ok(read);
                    }
                }
            }
        }
    }
    Err(format!("missing checked projected Borrow with {depth} steps").into())
}

/// Check genuine model-field enum clones, retained parents and independently malformed projection/ownership facts.
fn projected_enum_borrow_controls() -> TestResult<()> {
    let mut baseline = checked_source(PROJECTED_BORROW_SOURCE, "dev7_projected_carrier_borrow")?;
    let _ = projected_enum_read(&mut baseline, 1)?;
    let _ = projected_enum_read(&mut baseline, 2)?;
    let plan = lower_module(
        &baseline,
        PROJECTED_BORROW_SOURCE.to_owned(),
        "dev7_projected_carrier_borrow.incn".to_owned(),
    )?;
    validation::validate(&plan)?;
    let mut field = false;
    let mut fields = false;
    let mut clones = 0;
    for function in &plan.functions {
        for block in &function.blocks {
            if matches!(&block.terminator.kind, plan::TerminatorKind::Call(callee, ..) if matches!(&callee.kind, plan::CalleeKind::CloneEnum(..)))
            {
                clones += 1;
            }
            for statement in &block.statements {
                if let plan::StatementKind::Assign(_, rvalue) = &statement.kind {
                    if let plan::RvalueKind::Borrow(place) = &rvalue.kind {
                        match &place.projection {
                            plan::Projection::Field(_, plan::PlanType::Enum(..)) => field = true,
                            plan::Projection::Fields(path)
                                if matches!(path.last().map(|field| &field.ty), Some(plan::PlanType::Enum(..))) =>
                            {
                                fields = true
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    if !field || !fields || clones < 5 {
        return Err("projected enum reads did not retain actual field borrows and CloneEnum calls".into());
    }

    for (label, fact, family) in [
        (
            "mutable projected enum",
            OwnershipFact::MutBorrow,
            "unsupported Body IR ownership MutBorrow",
        ),
        (
            "unknown projected enum",
            OwnershipFact::Unknown,
            "unsupported Body IR ownership Unknown",
        ),
    ] {
        let mut module = baseline.clone();
        projected_enum_read(&mut module, 1)?.fact = fact;
        rejected_projected_borrow(&module, label, family)?;
    }
    let mut module = baseline.clone();
    let bare = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .and_then(|body| body.locals.iter().find(|local| local.name.as_deref() == Some("first")))
        .ok_or("missing bare enum carrier local")?
        .id;
    projected_enum_read(&mut module, 1)?.place = Place::from_local(bare);
    rejected_projected_borrow(
        &module,
        "bare borrowed carrier",
        "unsupported Body IR ownership Borrow outside formatting",
    )?;

    for corruption in ["missing authority", "foreign owner", "wrong member", "structural field"] {
        let mut module = baseline.clone();
        let read = projected_enum_read(&mut module, 1)?;
        let PlaceElem::Field {
            name,
            canonical,
            structural,
        } = &mut read.place.projection[0]
        else {
            return Err("expected canonical Holder.value projection".into());
        };
        match corruption {
            "missing authority" => *canonical = None,
            "foreign owner" => {
                canonical.as_mut().ok_or("missing field authority")?.origin =
                    SymbolOrigin::Module(vec!["foreign_owner".into()])
            }
            "wrong member" => *name = "unretained".into(),
            "structural field" => *structural = true,
            _ => return Err("unknown projected carrier corruption".into()),
        }
        let family = if corruption == "structural field" {
            "unsupported Body IR structural model projection"
        } else {
            "unsupported Body IR unproven model field"
        };
        rejected_projected_borrow(&module, corruption, family)?;
    }
    let mut module = baseline.clone();
    projected_enum_read(&mut module, 2)?.place.projection.swap(0, 1);
    rejected_projected_borrow(
        &module,
        "reordered canonical fields",
        "unsupported Body IR unproven model field",
    )?;
    Ok(())
}

/// Reject through the actual lowerer with stable families while retaining diagnostic source context.
fn rejected_projected_borrow(module: &BodyIrModule, label: &str, family: &str) -> TestResult<()> {
    match lower_module(
        module,
        PROJECTED_BORROW_SOURCE.to_owned(),
        "dev7_projected_carrier_borrow.incn".to_owned(),
    ) {
        Err(reason) if reason.starts_with(family) => {
            if family == "unsupported Body IR ownership Borrow outside formatting"
                && (!reason.contains("dev7_projected_carrier_borrow.incn:")
                    || !reason.contains(" in ")
                    || !reason.contains("projection ")
                    || !reason.contains("selected type "))
            {
                return Err(format!("{label}: missing retained refusal context: {reason}").into());
            }
            Ok(())
        }
        Err(reason) => Err(format!("{label}: expected {family}, got {reason}").into()),
        Ok(_) => Err(format!("{label}: malformed projected Borrow was admitted").into()),
    }
}

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
    let PlaceElem::OptionPayload {
        option_type,
        payload_type,
    } = &mut indexed_projection(&mut corrupted)?.projection[0]
    else {
        return Err("expected optional list payload".into());
    };
    // The list payload is already admitted, so this isolates container-owner validation rather than missing layout.
    *option_type = payload_type.clone();
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
fn nested_carrier_controls() -> TestResult<()> {
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
    declaration.member_debug = false;
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
    projected_enum_borrow_controls()?;
    println!("nested carrier admission: 6 positive, 23 Body IR negative, and 8 native plan negative controls passed");
    Ok(())
}

/// Select independent ownership controls so each regression fails only its dedicated host test.
fn main() -> TestResult<()> {
    match std::env::args().nth(1).as_deref() {
        Some("mutable-enum") => mutable_enum_reference_controls(),
        Some("mutable-tuple") => mutable_tuple_reference_controls(),
        Some("checked-json") => checked_json_admission::check(),
        Some("numeric-boundaries") => numeric_boundaries::check(),
        Some("collection-arity") => nested_collection_arity_controls(),
        Some("owned-result") => instantiated_owned_results::check(),
        Some("validation-format") => instantiated_owned_results::check_validation_format(),
        _ => nested_carrier_controls(),
    }
}

/// Check mutable-reference identity and move-only storage through the canonical native validator.
fn mutable_enum_reference_controls() -> TestResult<()> {
    // Reuse a genuine retained enum declaration; vary only the newly inserted unused local's type.
    let module = checked_module()?;
    let mut baseline = lower_module(&module, SOURCE.to_owned(), "mutable_enum_identity.incn".to_owned())?;
    validation::validate(&baseline)?;
    let name = baseline.enums.first().ok_or("missing source enum declaration")?.name.clone();
    let function = baseline.functions.iter_mut().find(|function| function.name == "main").ok_or("missing main")?;
    let slot = function.locals.len();
    let mut local = function.locals.first().ok_or("missing return local")?.clone();
    local.ty = plan::PlanType::EnumMutRef(0, name.clone());
    function.locals.push(local);
    validation::validate(&baseline)?;
    for ty in [
        plan::PlanType::EnumMutRef(-1, name.clone()),
        plan::PlanType::EnumMutRef(i64::try_from(baseline.enums.len())?, name.clone()),
        plan::PlanType::EnumMutRef(0, "NotTheDeclaredEnum".to_owned()),
    ] {
        let mut candidate = baseline.clone();
        let function = candidate.functions.iter_mut().find(|function| function.name == "main").ok_or("missing main")?;
        function.locals.get_mut(slot).ok_or("missing mutable enum local")?.ty = ty;
        rejected_plan(&candidate, "mutable enum identity", "enum type differs from its declaration")?;
    }

    println!("mutable enum identity: valid identity passed; negative/dangling/mismatched identities refused");
    Ok(())
}

/// Check that an initialized exclusive tuple parameter can move but cannot be implicitly copied.
fn mutable_tuple_reference_controls() -> TestResult<()> {
    // A checked `mut tuple` input is an initialized exclusive reference; moving it remains valid.
    let source = "def keep(mut pair: tuple[list[int], int]) -> None:\n    pass\n\ndef main() -> None:\n    mut pair: tuple[list[int], int] = ([1], 2)\n    keep(pair)\n";
    let module = checked_source(source, "mutable_tuple_transfer")?;
    let baseline = lower_module(&module, source.to_owned(), "mutable_tuple_transfer.incn".to_owned())?;
    validation::validate(&baseline)?;
    let mut moved = baseline.clone();
    add_tuple_transfer(&mut moved, true)?;
    validation::validate(&moved)?;
    let mut copied = baseline.clone();
    add_tuple_transfer(&mut copied, false)?;
    rejected_plan(&copied, "exclusive tuple reference copied", "owned formatting values cannot be copied")?;
    println!("mutable tuple admission: canonical borrow and explicit move passed; implicit Copy refused");
    Ok(())
}

/// Insert one transfer from a checked tuple parameter to otherwise identical local storage.
fn add_tuple_transfer(plan: &mut plan::Plan, moved: bool) -> TestResult<()> {
    let function = plan.functions.iter_mut().find(|function| function.name == "keep").ok_or("missing keep function")?;
    let parameter = function.locals.get(1).ok_or("missing tuple parameter local")?.clone();
    if !matches!(parameter.ty, plan::PlanType::TupleMutRef(_)) {
        return Err("checked mut tuple parameter did not retain exclusive tuple representation".into());
    }
    let output = i64::try_from(function.locals.len())?;
    let span = parameter.span.clone();
    function.locals.push(parameter);
    let source = plan::Place { local: 1, projection: plan::Projection::Whole, span: span.clone() };
    let operand = plan::Operand {
        kind: if moved { plan::OperandKind::Move(source) } else { plan::OperandKind::Copy(source) },
        span: span.clone(),
    };
    let destination = plan::Place { local: output, projection: plan::Projection::Whole, span: span.clone() };
    function.blocks.first_mut().ok_or("missing function entry")?.statements.push(plan::Statement {
        kind: plan::StatementKind::Assign(destination, plan::Rvalue { kind: plan::RvalueKind::Use(operand), span: span.clone() }),
        span,
    });
    Ok(())
}

/// Start with checked nested collection signatures, then refuse malformed leaf vectors in each native type position.
fn nested_collection_arity_controls() -> TestResult<()> {
    let source = r#"def lists(values: dict[str, list[int]]) -> dict[str, list[int]]:
    return values

def dictionaries(values: list[dict[str, int]]) -> list[dict[str, int]]:
    return values

def main() -> None:
    println(lists({"a": [1]})["a"][0])
    println(dictionaries([{"a": 2}])[0]["a"])
    pairs = zip([[3]], [[4]])
    for left, right in pairs:
        println(left[0] + right[0])
"#;
    let module = checked_source(source, "collection_arity")?;
    let baseline = lower_module(&module, source.to_owned(), "collection_arity.incn".to_owned())?;
    validation::validate(&baseline)?;
    let observed_parameters = || baseline.functions.iter().map(|function| {
        let carrier = function.parameters.first().map_or("no parameter", |parameter| collection_carrier_name(&parameter.ty));
        format!("{}: {carrier}", function.name)
    }).collect::<Vec<_>>().join(", ");
    let list_owner = baseline.functions.iter().position(|function| {
        matches!(function.parameters.first().map(|parameter| &parameter.ty),
            Some(plan::PlanType::Dict(_, plan::ListLeaf::List(_, _))
                | plan::PlanType::DictRef(_, plan::ListLeaf::List(_, _))
                | plan::PlanType::DictMutRef(_, plan::ListLeaf::List(_, _))))
    }).ok_or_else(|| format!("genuine dictionary-of-lists parameter missing; observed {}", observed_parameters()))?;
    let dict_owner = baseline.functions.iter().position(|function| {
        matches!(function.parameters.first().map(|parameter| &parameter.ty),
            Some(plan::PlanType::List(plan::ListLeaf::Dict(_), _)
                | plan::PlanType::ListRef(plan::ListLeaf::Dict(_), _)
                | plan::PlanType::ListMutRef(plan::ListLeaf::Dict(_), _)))
    }).ok_or_else(|| format!("genuine list-of-dictionaries parameter missing; observed {}", observed_parameters()))?;
    let cases = [
        ("list-empty", list_owner, plan::ListLeaf::List(Vec::new(), 1)),
        ("list-extra", list_owner, plan::ListLeaf::List(vec![plan::ListLeaf::Int, plan::ListLeaf::Int], 1)),
        ("dict-short", dict_owner, plan::ListLeaf::Dict(vec![plan::ListLeaf::Str])),
        ("dict-extra", dict_owner, plan::ListLeaf::Dict(vec![plan::ListLeaf::Str, plan::ListLeaf::Int, plan::ListLeaf::Int])),
    ];
    let zip = baseline.functions.iter().flat_map(|function| &function.locals).find_map(|local| {
        if let plan::PlanType::ZipIterator(left, right) = &local.ty {
            Some((left.clone(), right.clone()))
        } else {
            None
        }
    }).ok_or("genuine structured zip local missing")?;
    let mut failures = Vec::new();
    for (label, owner, leaf) in cases {
        for position in ["local", "parameter", "return", "external"] {
            let mut candidate = baseline.clone();
            let function = &mut candidate.functions[owner];
            let mut changed = match position {
                "local" => function.locals.get(1).ok_or("genuine parameter local missing")?.ty.clone(),
                "parameter" | "external" => function.parameters[0].ty.clone(),
                "return" => function.return_type.clone(),
                _ => return Err("invalid mutation position".into()),
            };
            match &mut changed {
                plan::PlanType::Dict(_, value)
                | plan::PlanType::DictRef(_, value)
                | plan::PlanType::DictMutRef(_, value)
                | plan::PlanType::List(value, _)
                | plan::PlanType::ListRef(value, _)
                | plan::PlanType::ListMutRef(value, _) => *value = leaf.clone(),
                _ => return Err(format!("{label}/{position}: expected existing collection carrier in {}, observed {}", function.name, collection_carrier_name(&changed)).into()),
            }
            match position {
                "local" => function.locals.push(plan::Local { ty: changed, span: function.span.clone() }),
                "parameter" => {
                    function.parameters[0].ty = changed.clone();
                    function.locals[1].ty = changed;
                }
                "return" => {
                    function.return_type = changed.clone();
                    function.locals[0].ty = changed;
                }
                "external" => {
                    let external = candidate.externals.iter_mut()
                        .find(|external| external.path == "incan_native_runtime::println_text")
                        .ok_or("genuine println_text external missing")?;
                    let parameter = external.parameters.first_mut().ok_or("genuine println_text parameter missing")?;
                    *parameter = changed;
                }
                _ => return Err("invalid mutation position".into()),
            }
            record_collection_arity_refusal(&candidate, &format!("{label}/{position}"), &mut failures);
        }
        // A valid outer dictionary must not hide a malformed collection one layer deeper.
        let mut nested = baseline.clone();
        let function = &mut nested.functions[owner];
        function.locals.push(plan::Local {
            ty: plan::PlanType::List(plan::ListLeaf::Dict(vec![plan::ListLeaf::Str, leaf.clone()]), 1),
            span: function.span.clone(),
        });
        record_collection_arity_refusal(&nested, &format!("{label}/nested"), &mut failures);
        for side in ["left", "right"] {
            for variant in ["owned", "shared", "exclusive"] {
                let (mut left, mut right) = zip.clone();
                let changed = match side {
                    "left" => &mut left,
                    "right" => &mut right,
                    _ => return Err("invalid zip side".into()),
                };
                let plan::TupleElement::List(element, _) = changed else {
                    return Err("genuine zip element did not retain its list shape".into());
                };
                *element = leaf.clone();
                let ty = match variant {
                    "owned" => plan::PlanType::ZipIterator(left, right),
                    "shared" => plan::PlanType::ZipIteratorRef(left, right),
                    "exclusive" => plan::PlanType::ZipIteratorMutRef(left, right),
                    _ => return Err("invalid zip variant".into()),
                };
                let mut candidate = baseline.clone();
                let function = &mut candidate.functions[owner];
                function.locals.push(plan::Local { ty, span: function.span.clone() });
                record_collection_arity_refusal(&candidate, &format!("{label}/zip-{variant}-{side}"), &mut failures);
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    println!("nested collection arity: genuine source plan and 44 typed native refusals passed");
    Ok(())
}

/// Identify the outer lowered collection carrier without assuming generated plan types implement Debug.
fn collection_carrier_name(ty: &plan::PlanType) -> &'static str {
    match ty {
        plan::PlanType::Dict(..) => "Dict",
        plan::PlanType::DictRef(..) => "DictRef",
        plan::PlanType::DictMutRef(..) => "DictMutRef",
        plan::PlanType::List(..) => "List",
        plan::PlanType::ListRef(..) => "ListRef",
        plan::PlanType::ListMutRef(..) => "ListMutRef",
        _ => "noncollection",
    }
}

/// Require the precise native arity refusal; panics and unrelated failures remain failed controls.
fn record_collection_arity_refusal(plan: &plan::Plan, label: &str, failures: &mut Vec<String>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| validation::validate(plan)));
    match outcome {
        Ok(Err(crate::error::PlanError::Invalid { function, reason }))
            if function == "collection leaf" && reason.contains("exactly one list child or two dictionary children") => {}
        Ok(other) => failures.push(format!("{label}: expected typed leaf-arity refusal, got {other:?}")),
        Err(_) => failures.push(format!("{label}: validator panicked before returning its typed error")),
    }
}
