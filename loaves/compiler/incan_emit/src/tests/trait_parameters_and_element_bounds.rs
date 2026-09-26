//! What a generic element read and a trait method's `mut` parameter need from the generated Rust: an index read of a
//! type-parameter element states the `Clone` capability its copy needs (#1756), and a `mut` aggregate parameter keeps
//! one Rust shape across the trait slot, every implementation and the recoverable wrapper (#1773).

use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{ast, lexer, parser};
use incan_ir::IrProgram;
use incan_ir::decl::{IrDeclKind, IrFunction, IrTraitBound};
use incan_ir::lower::AstLowering;
use incan_lang::lang::trait_bounds::rust as tb;

use crate::IrCodegen;

/// Parse one source module, keeping fixture failures as ordinary test errors.
fn parse(source: &str) -> Result<ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))
}

/// Check, lower and run trait-bound inference over one module, as codegen does before emission.
fn lower_with_inferred_bounds(source: &str) -> Result<IrProgram, String> {
    let program = parse(source)?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
    let mut ir = AstLowering::new_with_type_info(checker.type_info().clone())
        .lower_program(&program)
        .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    crate::trait_bound_inference::infer_trait_bounds(&mut ir);
    Ok(ir)
}

/// Return the lowered free function `name`.
fn function<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a IrFunction, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return whether the named type parameter of a function carries a bound on `trait_path`.
fn type_param_has_bound(function: &IrFunction, param: &str, trait_path: &str) -> Result<bool, String> {
    let type_param = function
        .type_params
        .iter()
        .find(|type_param| type_param.name == param)
        .ok_or_else(|| format!("`{}` has no type parameter `{param}`", function.name))?;
    Ok(type_param
        .bounds
        .iter()
        .any(|bound: &IrTraitBound| bound.trait_path == trait_path))
}

/// #1756: `items[0]` on a `list[K]` and `table[key]` on a `dict[str, V]` copy the element out, so the generic function
/// states `Clone` on the element's type parameter; a `Copy` element and a parameter the read does not mention get no
/// such bound, and a caller forwarding its own parameter inherits the requirement.
#[test]
fn index_read_of_a_type_parameter_element_states_clone_issue1756() -> Result<(), String> {
    let ir = lower_with_inferred_bounds(
        r#"
def first[K](items: list[K]) -> K:
    return items[0]

def lookup[V](table: dict[str, V], key: str) -> V:
    return table[key]

def first_count[K](items: list[int], marker: K) -> int:
    return items[0]

def first_of_first[K](items: list[K]) -> K:
    return first(items)
"#,
    )?;
    assert!(type_param_has_bound(function(&ir, "first")?, "K", tb::CLONE)?);
    assert!(type_param_has_bound(function(&ir, "lookup")?, "V", tb::CLONE)?);
    assert!(
        !type_param_has_bound(function(&ir, "first_count")?, "K", tb::CLONE)?,
        "an `int` element is copied without a bound, and `K` is not the element"
    );
    assert!(
        type_param_has_bound(function(&ir, "first_of_first")?, "K", tb::CLONE)?,
        "a caller that passes its own `K` to `first` needs the bound `first` states"
    );
    Ok(())
}

/// Emit one source module to Rust and return it with all whitespace removed, so assertions do not depend on layout.
fn compact_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let program = parse(source)?;
    let code = IrCodegen::new().try_generate(&program)?;
    Ok(code.chars().filter(|ch| !ch.is_whitespace()).collect())
}

/// #1773: a trait method's `mut` list parameter is `&mut` in the trait slot, in an expanded default, in an adopter's
/// own implementation and in the recoverable wrapper a concrete call targets, and the call passes the caller's list
/// the same way, so the method's change reaches the caller. A `mut` scalar stays a value in the slot and becomes a
/// mutable binding in a body that uses it, as on a free function.
#[test]
fn trait_method_mut_parameter_keeps_one_shape_across_slot_impl_and_wrapper_issue1773()
-> Result<(), Box<dyn std::error::Error>> {
    let rust = compact_rust(
        r#"
trait Replacer:
    def replace(self, mut items: list[int]) -> int:
        items.append(9)
        return len(items)

    def extend(self, mut items: list[int]) -> None: ...

    def bump(self, mut n: int) -> int:
        return n + 1


model Widget with Replacer:
    id: int

    def extend(self, mut items: list[int]) -> None:
        items.append(self.id)


def main() -> None:
    mut items: list[int] = [1, 2]
    println(Widget(id=1).replace(items))
    Widget(id=4).extend(items)
    println(len(items))
    println(Widget(id=1).bump(5))
"#,
    )?;
    assert!(
        rust.contains("fnreplace(&self,items:&mutVec<i64>)->i64;"),
        "the trait slot takes the list as `&mut`: {rust}"
    );
    assert!(
        rust.contains("fnreplace(&self,items:&mutVec<i64>)->i64{"),
        "the expanded default takes the list as `&mut`: {rust}"
    );
    assert!(
        rust.contains("fnextend(&self,items:&mutVec<i64>);") && rust.contains("fnextend(&self,items:&mutVec<i64>){"),
        "a required method and the adopter's implementation take the list as `&mut`: {rust}"
    );
    assert!(
        rust.contains("<SelfasReplacer>::replace(self,items)"),
        "the recoverable wrapper forwards its parameter to the slot: {rust}"
    );
    assert!(
        !rust.contains("&self,items:Vec<i64>"),
        "no trait-method signature takes the `mut` list by value: {rust}"
    );
    assert!(
        rust.matches("(&mutitems").count() == 2,
        "both concrete calls pass the caller's list as `&mut`: {rust}"
    );
    assert!(
        rust.contains("fnbump(&self,n:i64)->i64;") && rust.contains("fnbump(&self,mutn:i64)->i64{"),
        "a `mut` scalar is a value in the slot and a mutable binding in the body that uses it: {rust}"
    );
    Ok(())
}
