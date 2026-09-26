//! Method-decorator receivers (#1790): the declarations of a `self` method's decorator chain take the receiver the way
//! the method's wrapper passes it, a direct call of a planned replacement passes it the same way, and a `mut`-marked
//! callable-type parameter is passed so the caller sees the changes.

use std::collections::HashMap;

use super::*;
use crate::visit::{self, Visitor};
use incan_frontend::typechecker::{MethodDecoratorReceiverRole, MethodDecoratorReceiverSlot};

/// A parsed program with its function declarations' spans keyed by function name.
type ProgramWithFunctionSpans = (ast::Program, HashMap<String, (usize, usize)>);

/// Parse `source` and return its function declarations' spans keyed by function name.
fn parse_with_function_spans(source: &str) -> Result<ProgramWithFunctionSpans, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let spans = program
        .declarations
        .iter()
        .filter_map(|decl| match &decl.node {
            ast::Declaration::Function(function) => Some((function.name.clone(), (decl.span.start, decl.span.end))),
            _ => None,
        })
        .collect();
    Ok((program, spans))
}

/// Return the function declaration named `name` in `program`.
fn function_named<'p>(program: &'p ast::Program, name: &str) -> Result<&'p ast::FunctionDecl, String> {
    program
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            ast::Declaration::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the lowered function named `name`.
fn lowered_function<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a IrFunction, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing lowered function `{name}`"))
}

/// A slot planned for a `self` method in `role`.
fn shared(role: MethodDecoratorReceiverRole) -> MethodDecoratorReceiverSlot {
    MethodDecoratorReceiverSlot { mutable: false, role }
}

/// Issue #1790: a `self` method's decorator shapes and replacement are planned to the form the method's wrapper
/// passes, and nothing else in the program changes.
#[test]
fn shared_receiver_slots_are_planned_to_the_wrapper_form() -> Result<(), String> {
    let (program, spans) = parse_with_function_spans(
        r#"
class Box:
    value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

def parse(box: Box, value: int) -> int:
    return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return parse

def unrelated(box: Box, func: (Box, int) -> int) -> int:
    return func(box, 1)
"#,
    )?;
    let span_of = |name: &str| {
        spans
            .get(name)
            .copied()
            .ok_or_else(|| format!("missing span for `{name}`"))
    };
    let slots = HashMap::from([
        (span_of("as_int")?, shared(MethodDecoratorReceiverRole::Decorator)),
        (span_of("parse")?, shared(MethodDecoratorReceiverRole::Replacement)),
    ]);
    let planned = crate::lower::receiver_plan::plan_shared_method_decorator_receivers(&program, &slots)
        .ok_or("expected a planned program")?;

    let as_int = function_named(&planned, "as_int")?;
    assert_eq!(as_int.params[0].node.ty.node.to_string(), "(&Box, int) -> str");
    assert_eq!(as_int.return_type.node.to_string(), "(&Box, int) -> int");
    let parse = function_named(&planned, "parse")?;
    assert_eq!(parse.params[0].node.ty.node.to_string(), "&Box");
    assert_eq!(parse.params[1].node.ty.node.to_string(), "int");
    let unrelated = function_named(&planned, "unrelated")?;
    assert_eq!(unrelated.params[0].node.ty.node.to_string(), "Box");
    assert_eq!(unrelated.params[1].node.ty.node.to_string(), "(Box, int) -> int");
    Ok(())
}

/// Issue #1790: a factory's slot plans the decorator shape it returns: the receiver of the shape that shape accepts
/// and of the shape it returns.
#[test]
fn factory_slot_plans_the_decorator_shape_it_returns() -> Result<(), String> {
    let (program, spans) = parse_with_function_spans(
        r#"
class Box:
    value: int

def labeled(name: str) -> ((Box, int) -> str) -> (Box, int) -> int:
    return as_int

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return func
"#,
    )?;
    let labeled_span = spans.get("labeled").copied().ok_or("missing span for `labeled`")?;
    let slots = HashMap::from([(labeled_span, shared(MethodDecoratorReceiverRole::Factory))]);
    let planned = crate::lower::receiver_plan::plan_shared_method_decorator_receivers(&program, &slots)
        .ok_or("expected a planned program")?;
    let labeled = function_named(&planned, "labeled")?;
    assert_eq!(labeled.params[0].node.ty.node.to_string(), "str");
    assert_eq!(
        labeled.return_type.node.to_string(),
        "((&Box, int) -> str) -> (&Box, int) -> int"
    );
    Ok(())
}

/// Issue #1790: a `mut self` method's shapes already say how the receiver is passed, so a program whose slots are all
/// mutable is not copied.
#[test]
fn mutable_receiver_slots_leave_the_program_as_written() -> Result<(), String> {
    let (program, spans) = parse_with_function_spans(
        r#"
class Counter:
    value: int

def keep(func: (mut Counter, int) -> int) -> (mut Counter, int) -> int:
    return func
"#,
    )?;
    let keep = spans.get("keep").copied().ok_or("missing span for `keep`")?;
    let slots = HashMap::from([(
        keep,
        MethodDecoratorReceiverSlot {
            mutable: true,
            role: MethodDecoratorReceiverRole::Decorator,
        },
    )]);
    assert!(crate::lower::receiver_plan::plan_shared_method_decorator_receivers(&program, &slots).is_none());
    Ok(())
}

/// Issue #1790: a direct call of a planned replacement in its module passes the receiver the way the planned
/// signature takes it, through the call's own signature.
#[test]
fn direct_call_of_a_planned_replacement_uses_the_planned_signature() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
class Box:
    pub value: int

    @as_int
    def label(self, value: int) -> str:
        return "value"

def parse(box: Box, value: int) -> int:
    return box.value + value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
    return parse

def direct(box: Box) -> int:
    return parse(box, 3)
"#,
    )?;
    let receiver = IrType::Ref(Box::new(IrType::Struct("Box".to_string())));
    assert_eq!(lowered_function(&ir, "parse")?.params[0].ty, receiver);
    let direct = lowered_function(&ir, "direct")?;
    let Some(IrStmt {
        kind: IrStmtKind::Return(Some(call)),
        ..
    }) = direct.body.last()
    else {
        return Err(format!("`direct` must end in a return, got {:?}", direct.body));
    };
    let IrExprKind::Call {
        callable_signature: Some(signature),
        ..
    } = &call.kind
    else {
        return Err(format!("`direct` must return a call with its signature, got {call:?}"));
    };
    assert_eq!(signature.params[0].ty, receiver);
    Ok(())
}

/// Collects the parameter types of every closure in a function body.
#[derive(Default)]
struct ClosureParams {
    types: Vec<Vec<IrType>>,
}

impl Visitor for ClosureParams {
    /// Record one closure's parameter types, then visit its body.
    fn expr(&mut self, expr: &mut crate::IrExpr) {
        if let IrExprKind::Closure { params, .. } = &expr.kind {
            self.types.push(params.iter().map(|(_, ty)| ty.clone()).collect());
        }
        visit::walk_expr(expr, self);
    }
}

/// Issue #1790: a closure checked against a `mut`-marked shape takes that argument so the caller sees its changes.
#[test]
fn closure_checked_against_a_mut_marked_shape_takes_the_argument_mut() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
class Counter:
    pub value: int

def apply(step: (mut Counter, int) -> int, mut counter: Counter) -> int:
    return step(counter, 2)

def main() -> None:
    mut counter = Counter(value=1)
    println(apply((c, by) => c.value + by, counter))
"#,
    )?;
    let mut main = lowered_function(&ir, "main")?.clone();
    let mut closures = ClosureParams::default();
    for stmt in &mut main.body {
        closures.stmt(stmt);
    }
    let [params] = closures.types.as_slice() else {
        return Err(format!("expected one closure in `main`, got {:?}", closures.types));
    };
    assert_eq!(
        params.first(),
        Some(&IrType::RefMut(Box::new(IrType::Struct("Counter".to_string()))))
    );
    assert_eq!(params.get(1), Some(&IrType::Int));
    Ok(())
}
