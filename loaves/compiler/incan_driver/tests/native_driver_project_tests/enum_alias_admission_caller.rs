//! Test-only mutations reach the real Incan enum admission boundary, without a second verifier (#1337, #1698).

use incan_frontend::{body_ir::build_body_ir_module_v0, lexer, parser, typechecker::TypeChecker};
use incan_mir_lowering::caller::incan::lower_module;
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentBinding, ArgumentElement, BodyIrModule, EnumVariantTarget, Pattern, Rvalue, StatementKind,
};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

const SOURCE: &str = r#"enum Shape:
    Empty
    Pair(int)

enum Other:
    Empty
    Pair(int)

def payload(shape: Shape) -> int:
    match shape:
        Shape.Pair(value) => return value
        _ => return 0

def main() -> None:
    value = Shape.Pair(7)
    payload(value)
"#;

/// Obtain the baseline from ordinary checking; only subsequent target mutations bypass that authority.
fn checked_module() -> TestResult<BodyIrModule> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["enum_alias_admission".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Borrow the checked constructor and its actual operands, leaving declaration and local-type records intact.
fn constructor(module: &mut BodyIrModule) -> TestResult<(&mut EnumVariantTarget, &mut Vec<ArgumentElement>)> {
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "main")
        .ok_or("missing main body")?;
    for statement in &mut body.block.stmts {
        if let StatementKind::Assign {
            rvalue: Rvalue::Aggregate(AggregateKind::EnumVariant(target), arguments),
            ..
        } = &mut statement.kind
        {
            return Ok((target, arguments));
        }
    }
    Err("missing checked enum constructor".into())
}

/// Borrow the checked payload pattern so corrupted targets retain their real scrutinee and binding context.
fn pattern(module: &mut BodyIrModule) -> TestResult<&mut Pattern> {
    let body = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "payload")
        .ok_or("missing payload body")?;
    for statement in &mut body.block.stmts {
        if let StatementKind::Assign {
            rvalue: Rvalue::Match { arms, .. },
            ..
        } = &mut statement.kind
        {
            return Ok(&mut arms.first_mut().ok_or("missing match arm")?.pattern);
        }
    }
    Err("missing checked enum pattern".into())
}

/// Require the named admission error, so unrelated preparation or declaration failures cannot satisfy a negative.
fn rejected(module: &BodyIrModule, label: &str, family: &str) -> TestResult<()> {
    match lower_module(module, SOURCE.to_owned(), "enum_alias_admission.incn".to_owned()) {
        Err(reason) if reason.starts_with(family) => Ok(()),
        Err(reason) => Err(format!("{label}: expected {family}, got {reason}").into()),
        Ok(_) => Err(format!("{label}: malformed target was admitted").into()),
    }
}

/// Independently mutate constructor targets while preserving the exact checked declaration layouts.
fn constructor_controls(baseline: &BodyIrModule) -> TestResult<()> {
    let other = baseline
        .enum_declarations
        .iter()
        .find(|owner| owner.name == "Other")
        .ok_or("missing Other layout")?;
    let foreign_pair = other
        .variants
        .iter()
        .find(|variant| variant.name == "Pair")
        .ok_or("missing Other.Pair")?;
    let shape = baseline
        .enum_declarations
        .iter()
        .find(|owner| owner.name == "Shape")
        .ok_or("missing Shape layout")?;
    let empty = shape
        .variants
        .iter()
        .find(|variant| variant.name == "Empty")
        .ok_or("missing Shape.Empty")?;
    let identity_error = "invalid Body IR enum constructor identity";

    let mut module = baseline.clone();
    constructor(&mut module)?.0.enum_canonical = Some(other.canonical.clone());
    rejected(&module, "wrong constructor owner", identity_error)?;

    let mut module = baseline.clone();
    constructor(&mut module)?.0.variant_canonical = foreign_pair.canonical.clone();
    rejected(&module, "foreign same-spelled constructor variant", identity_error)?;

    let mut module = baseline.clone();
    constructor(&mut module)?.0.variant_canonical = empty.canonical.clone();
    rejected(&module, "wrong constructor variant", identity_error)?;

    let mut module = baseline.clone();
    constructor(&mut module)?.0.variant_name = "Foreign".to_owned();
    rejected(&module, "wrong constructor member spelling", identity_error)?;

    let mut module = baseline.clone();
    constructor(&mut module)?.0.enum_canonical = None;
    rejected(&module, "missing constructor owner authority", identity_error)?;

    let mut module = baseline.clone();
    constructor(&mut module)?.0.binding = ArgumentBinding::resolved_positional(0);
    rejected(
        &module,
        "wrong constructor binding arity",
        "unsupported Body IR incomplete enum payload binding",
    )?;

    let mut module = baseline.clone();
    let ArgumentBinding::Resolved { arguments, .. } = &mut constructor(&mut module)?.0.binding else {
        return Err("expected resolved constructor binding".into());
    };
    arguments.first_mut().ok_or("missing constructor binding slot")?.slot = 1;
    rejected(
        &module,
        "wrong constructor payload slot",
        "unsupported Body IR reordered enum payload binding",
    )?;

    let mut module = baseline.clone();
    constructor(&mut module)?.1.clear();
    rejected(
        &module,
        "missing actual constructor payload",
        "invalid Body IR enum constructor payload count",
    )?;
    Ok(())
}

/// Transplant or erase variant facts separately from member spelling and payload count.
fn pattern_controls(baseline: &BodyIrModule) -> TestResult<()> {
    let other = baseline
        .enum_declarations
        .iter()
        .find(|owner| owner.name == "Other")
        .ok_or("missing Other layout")?;
    let foreign_pair = other
        .variants
        .iter()
        .find(|variant| variant.name == "Pair")
        .ok_or("missing Other.Pair")?;
    let shape = baseline
        .enum_declarations
        .iter()
        .find(|owner| owner.name == "Shape")
        .ok_or("missing Shape layout")?;
    let empty = shape
        .variants
        .iter()
        .find(|variant| variant.name == "Empty")
        .ok_or("missing Shape.Empty")?;

    let mut module = baseline.clone();
    let Pattern::Enum { canonical, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    *canonical = Some(foreign_pair.canonical.clone());
    rejected(
        &module,
        "foreign same-spelled pattern variant",
        "unsupported Body IR enum pattern",
    )?;

    let mut module = baseline.clone();
    let Pattern::Enum { canonical, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    *canonical = Some(empty.canonical.clone());
    rejected(&module, "wrong pattern variant", "invalid Body IR enum pattern layout")?;

    let mut module = baseline.clone();
    let Pattern::Enum { variant, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    *variant = "Alias::Foreign".to_owned();
    rejected(
        &module,
        "wrong pattern member spelling",
        "invalid Body IR enum pattern layout",
    )?;

    let mut module = baseline.clone();
    let Pattern::Enum { canonical, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    *canonical = None;
    rejected(&module, "missing pattern authority", "unsupported Body IR enum pattern")?;

    let mut module = baseline.clone();
    let Pattern::Enum { fields, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    fields.clear();
    rejected(
        &module,
        "missing pattern payload",
        "invalid Body IR enum pattern layout",
    )?;

    let mut module = baseline.clone();
    let Pattern::Enum { fields, .. } = pattern(&mut module)? else {
        return Err("expected enum pattern".into());
    };
    fields.push(Pattern::Wildcard);
    rejected(&module, "extra pattern payload", "invalid Body IR enum pattern layout")?;
    Ok(())
}

/// Check the baseline and source aliases before negative controls; an always-refusing implementation must fail.
fn main() -> TestResult<()> {
    let baseline = checked_module()?;
    let _ = lower_module(&baseline, SOURCE.to_owned(), "enum_alias_admission.incn".to_owned())?;

    let mut constructor_alias = baseline.clone();
    constructor(&mut constructor_alias)?.0.enum_name = "LocalShape".to_owned();
    let _ = lower_module(
        &constructor_alias,
        SOURCE.to_owned(),
        "enum_alias_admission.incn".to_owned(),
    )?;

    let mut pattern_alias = baseline.clone();
    let Pattern::Enum { name, variant, .. } = pattern(&mut pattern_alias)? else {
        return Err("expected enum pattern".into());
    };
    *name = "LocalShape".to_owned();
    *variant = "LocalShape::Pair".to_owned();
    let _ = lower_module(
        &pattern_alias,
        SOURCE.to_owned(),
        "enum_alias_admission.incn".to_owned(),
    )?;

    constructor_controls(&baseline)?;
    pattern_controls(&baseline)?;
    println!("enum target admission: 3 positive and 14 negative controls passed");
    Ok(())
}
