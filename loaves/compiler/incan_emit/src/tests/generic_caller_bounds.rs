//! Bound inference carries what a called source method requires into the generic caller that calls it (#1779,
//! #1280): the method's own bounds through its arguments, and its impl header's bounds through the receiver.

use incan_frontend::ast::Program;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_ir::IrProgram;
use incan_ir::decl::IrDeclKind;
use incan_ir::lower::AstLowering;
use incan_lang::lang::trait_bounds::rust as rust_trait_bounds;

use crate::trait_bound_inference::{infer_trait_bounds, propagate_trait_bounds_from_programs};

/// Parse one source module.
fn parse(source: &str) -> Result<Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))
}

/// Check, lower and infer the bounds of one source module the checker must accept.
fn inferred_program(source: &str) -> Result<IrProgram, String> {
    inferred_module(source, None)
}

/// Check, lower and infer the bounds of one source module under an optional module path.
fn inferred_module(source: &str, module_path: Option<&str>) -> Result<IrProgram, String> {
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(module_path.map(|path| vec![path.to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("check failed: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let mut ir = lowering
        .lower_program(&program)
        .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    infer_trait_bounds(&mut ir);
    Ok(ir)
}

/// Return the trait paths bounding one type parameter of the named free function.
fn function_bounds(ir: &IrProgram, function: &str, type_param: &str) -> Result<Vec<String>, String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(candidate) if candidate.name == function => Some(candidate),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{function}`"))?;
    let param = function
        .type_params
        .iter()
        .find(|param| param.name == type_param)
        .ok_or_else(|| format!("`{}` has no type parameter `{type_param}`", function.name))?;
    Ok(param.bounds.iter().map(|bound| bound.trait_path.clone()).collect())
}

/// Fail unless `bounds` names every trait in `expected`.
fn require_bounds(context: &str, bounds: &[String], expected: &[&str]) -> Result<(), String> {
    match expected
        .iter()
        .find(|trait_path| !bounds.iter().any(|bound| bound == **trait_path))
    {
        Some(missing) => Err(format!("{context}: missing `{missing}`, got {bounds:?}")),
        None => Ok(()),
    }
}

/// #1779: a generic caller forwarding its `T` to a method that formats it, or compares two of its values, needs the
/// method's inferred `Display` or `PartialEq` bound.
#[test]
fn method_body_bounds_reach_a_forwarding_caller_issue1779() -> Result<(), String> {
    let ir = inferred_program(
        r#"
model Client:
    def send[T](self, value: T) -> None:
        println(f"sent {value}")

    def same[T](self, a: T, b: T) -> bool:
        return a == b


def send_once[T](client: Client, value: T) -> None:
    client.send(value)


def check[T](client: Client, a: T, b: T) -> bool:
    return client.same(a, b)
"#,
    )?;
    require_bounds(
        "send_once",
        &function_bounds(&ir, "send_once", "T")?,
        &[rust_trait_bounds::DISPLAY],
    )?;
    require_bounds(
        "check",
        &function_bounds(&ir, "check", "T")?,
        &[rust_trait_bounds::PARTIAL_EQ],
    )
}

/// #1779: a caller of any method of a generic model needs the whole impl header, which carries `Display` from the
/// formatting method and `Clone` from the method returning its field, even when it calls only the second.
#[test]
fn impl_header_bounds_reach_a_caller_of_a_sibling_method_issue1779() -> Result<(), String> {
    let ir = inferred_program(
        r#"
model Holder[V]:
    value: V

    def show(self) -> str:
        return f"{self.value}"

    def get(self) -> V:
        return self.value


def first[U](held: Holder[U]) -> U:
    return held.get()
"#,
    )?;
    require_bounds(
        "first",
        &function_bounds(&ir, "first", "U")?,
        &[rust_trait_bounds::DISPLAY, rust_trait_bounds::CLONE],
    )
}

/// #1280: a caller holding `Stream[T]` that calls the trait method formatting the stream's item needs `Display`
/// beside the `Clone` it declares.
#[test]
fn trait_method_bounds_reach_a_caller_through_the_receiver_issue1280() -> Result<(), String> {
    let ir = inferred_program(
        r#"
trait Walk:
    def walk(self) -> str: ...


model Stream[R with Clone] with Walk:
    item: R

    def walk(self) -> str:
        return f"walked {self.item}"


def consume[T with Clone](stream: Stream[T]) -> str:
    return stream.walk()
"#,
    )?;
    require_bounds(
        "consume",
        &function_bounds(&ir, "consume", "T")?,
        &[rust_trait_bounds::CLONE, rust_trait_bounds::DISPLAY],
    )
}

/// #1280: an impl declared in another module carries its inferred bound into a generic caller of this module.
#[test]
fn external_impl_bounds_reach_a_generic_caller_issue1280() -> Result<(), String> {
    let shapes_source = r#"
pub model Holder[V]:
    pub value: V

    def show(self) -> str:
        return f"held {self.value}"
"#;
    let consumer_source = r#"
from shapes import Holder


def describe[U](held: Holder[U]) -> str:
    return held.show()
"#;
    let shapes_ir = inferred_module(shapes_source, Some("shapes"))?;
    let shapes = parse(shapes_source)?;
    let consumer = parse(consumer_source)?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("shapes", &shapes)])
        .map_err(|errors| format!("consumer check failed: {errors:?}"))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let mut consumer_ir = lowering
        .lower_program(&consumer)
        .map_err(|errors| format!("consumer lowering failed: {errors:?}"))?;
    infer_trait_bounds(&mut consumer_ir);
    propagate_trait_bounds_from_programs(&mut consumer_ir, &[&shapes_ir]);
    require_bounds(
        "describe",
        &function_bounds(&consumer_ir, "describe", "U")?,
        &[rust_trait_bounds::DISPLAY],
    )
}
